//! Deterministic workforce allocation under capability, privacy, budget, and
//! latency constraints.
//!
//! The engine does not call providers. It turns an immutable task, policy,
//! evidence snapshot, and candidate set into an auditable quote. Hard constraints
//! are applied first; eligible workers are then ranked by confidence-aware
//! expected accepted cost.

use std::{cmp::Ordering, collections::BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use workforce_domain::{
    DecisionId, DomainError, PrivacyClass, ProbabilityEstimate, SkillId, TaskSpec, WorkerEstimate,
    WorkerId,
};

const QUOTA_MILLIUNITS_PER_UNIT: u128 = 1_000;

/// A transparent conjugate posterior for Bernoulli success observations.
///
/// `alpha` is prior-plus-observed success weight and `beta` is
/// prior-plus-observed failure weight. Fractional weights allow evidence tiers to
/// contribute less than locally reproduced outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BetaPosterior {
    pub alpha: f64,
    pub beta: f64,
}

impl BetaPosterior {
    pub fn new(alpha: f64, beta: f64) -> Result<Self, EngineError> {
        let posterior = Self { alpha, beta };
        posterior.validate()?;
        Ok(posterior)
    }

    pub fn validate(self) -> Result<(), EngineError> {
        if positive_finite(self.alpha) && positive_finite(self.beta) {
            Ok(())
        } else {
            Err(EngineError::InvalidBetaParameters {
                alpha: self.alpha,
                beta: self.beta,
            })
        }
    }

    pub fn mean(self) -> f64 {
        self.alpha / (self.alpha + self.beta)
    }

    pub fn variance(self) -> f64 {
        let total = self.alpha + self.beta;
        self.alpha * self.beta / (total * total * (total + 1.0))
    }

    /// A conservative one-sided lower bound using the beta posterior's normal
    /// approximation: `mean - z * sqrt(variance)`, clipped to `[0, 1]`.
    ///
    /// Keeping `z` explicit makes the confidence policy inspectable. A value of
    /// `1.645` approximates a one-sided 95% lower bound. This approximation is
    /// intentionally dependency-free; consumers needing exact beta quantiles can
    /// calculate them upstream and still use [`ProbabilityEstimate`].
    pub fn lower_bound(self, z: f64) -> Result<f64, EngineError> {
        if !z.is_finite() || z < 0.0 {
            return Err(EngineError::InvalidConfidenceMultiplier(z));
        }
        Ok((self.mean() - z * self.variance().sqrt()).clamp(0.0, 1.0))
    }

    pub fn observe(&mut self, success_weight: f64, failure_weight: f64) -> Result<(), EngineError> {
        self.validate()?;
        if !non_negative_finite(success_weight) || !non_negative_finite(failure_weight) {
            return Err(EngineError::InvalidObservationWeights {
                success_weight,
                failure_weight,
            });
        }
        let updated = Self {
            alpha: self.alpha + success_weight,
            beta: self.beta + failure_weight,
        };
        updated.validate()?;
        *self = updated;
        Ok(())
    }

    pub fn observe_outcome(&mut self, accepted: bool, weight: f64) -> Result<(), EngineError> {
        if accepted {
            self.observe(weight, 0.0)
        } else {
            self.observe(0.0, weight)
        }
    }

    pub fn estimate(
        self,
        evidence_count: u64,
        confidence_z: f64,
    ) -> Result<ProbabilityEstimate, EngineError> {
        self.validate()?;
        Ok(ProbabilityEstimate {
            success_mean: self.mean(),
            success_lower_bound: self.lower_bound(confidence_z)?,
            evidence_count,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingPolicy {
    /// Immutable policy version or content-addressed identifier.
    pub policy_id: String,
    pub currency: String,
    /// Local value assigned to one unit of scarce provider quota.
    pub quota_shadow_cash_micros_per_unit: u64,
    /// An optional hard quota ceiling, separate from the cash budget.
    #[serde(default)]
    pub max_expected_quota_milliunits: Option<u64>,
}

impl RoutingPolicy {
    pub fn validate(&self) -> Result<(), EngineError> {
        if self.policy_id.trim().is_empty() {
            return Err(EngineError::EmptyField("policy.policy_id"));
        }
        if self.currency.trim().is_empty() {
            return Err(EngineError::EmptyField("policy.currency"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteRequest {
    pub decision_id: DecisionId,
    /// All eligible candidates must have been computed from this exact snapshot.
    pub evidence_snapshot_id: String,
    pub task: TaskSpec,
    pub policy: RoutingPolicy,
    pub candidates: Vec<WorkerEstimate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingQuote {
    pub decision_id: DecisionId,
    pub task_id: workforce_domain::TaskId,
    pub evidence_snapshot_id: String,
    pub policy_id: String,
    pub selected_worker_id: Option<WorkerId>,
    pub eligible_candidates: Vec<CandidateQuote>,
    pub rejected_candidates: Vec<RejectedCandidate>,
    pub pareto_worker_ids: Vec<WorkerId>,
    pub selection_explanation: Option<SelectionExplanation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateQuote {
    pub rank: usize,
    pub worker_id: WorkerId,
    pub success_mean: f64,
    pub success_lower_bound: f64,
    pub p95_latency_ms: u64,
    pub cost: CostBreakdown,
    pub pareto_efficient: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub run_cash_micros: u64,
    pub review_cash_micros: u64,
    pub expected_failure_cash_micros: u64,
    pub expected_cash_micros: u64,
    pub expected_quota_milliunits: u64,
    pub quota_shadow_cash_micros: u64,
    /// Cash plus the local shadow value of scarce quota.
    pub expected_accepted_cost_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RejectedCandidate {
    pub worker_id: WorkerId,
    /// Every failed hard constraint, in a stable order.
    pub reasons: Vec<IneligibilityReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum IneligibilityReason {
    Unavailable,
    EvidenceSnapshotMismatch {
        expected: String,
        actual: String,
    },
    ProviderNotAllowed {
        provider: String,
    },
    InsufficientPrivacyClearance {
        available: PrivacyClass,
        required: PrivacyClass,
    },
    ContextWindowTooSmall {
        available_tokens: u64,
        required_tokens: u64,
    },
    MissingSkill {
        skill_id: SkillId,
    },
    MissingSkillEstimate {
        skill_id: SkillId,
    },
    SkillConfidenceBelowMinimum {
        skill_id: SkillId,
        lower_bound: f64,
        minimum: f64,
    },
    MissingTool {
        tool: String,
    },
    TaskConfidenceBelowMinimum {
        lower_bound: f64,
        minimum: f64,
    },
    CurrencyMismatch {
        expected: String,
        actual: String,
    },
    CashBudgetExceeded {
        expected_cash_micros: u64,
        budget_cash_micros: u64,
    },
    QuotaBudgetExceeded {
        expected_quota_milliunits: u64,
        budget_quota_milliunits: u64,
    },
    LatencyLimitExceeded {
        p95_latency_ms: u64,
        maximum_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectionExplanation {
    pub objective: String,
    pub eligible_candidate_count: usize,
    pub selected_expected_accepted_cost_micros: u64,
    pub selected_success_lower_bound: f64,
    pub selected_p95_latency_ms: u64,
    /// The lexicographic rules actually used, in order.
    pub tie_break_order: Vec<String>,
}

/// Produce a deterministic, side-effect-free allocation quote.
pub fn quote(request: &QuoteRequest) -> Result<RoutingQuote, EngineError> {
    validate_request(request)?;
    let required_context_tokens = request.task.required_context_tokens()?;

    let mut eligible_candidates = Vec::new();
    let mut rejected_candidates = Vec::new();

    for estimate in &request.candidates {
        let cost = cost_breakdown(estimate, &request.policy);
        let reasons = ineligibility_reasons(
            estimate,
            &request.task,
            &request.policy,
            &request.evidence_snapshot_id,
            required_context_tokens,
            &cost,
        );

        if reasons.is_empty() {
            eligible_candidates.push(CandidateQuote {
                rank: 0,
                worker_id: estimate.worker.identity.worker_id.clone(),
                success_mean: estimate.success.success_mean,
                success_lower_bound: estimate.success.success_lower_bound,
                p95_latency_ms: estimate.p95_latency_ms,
                cost,
                pareto_efficient: false,
            });
        } else {
            rejected_candidates.push(RejectedCandidate {
                worker_id: estimate.worker.identity.worker_id.clone(),
                reasons,
            });
        }
    }

    mark_pareto_candidates(&mut eligible_candidates);
    eligible_candidates.sort_by(compare_candidates);
    for (index, candidate) in eligible_candidates.iter_mut().enumerate() {
        candidate.rank = index + 1;
    }
    rejected_candidates.sort_by(|left, right| left.worker_id.cmp(&right.worker_id));

    let pareto_worker_ids = eligible_candidates
        .iter()
        .filter(|candidate| candidate.pareto_efficient)
        .map(|candidate| candidate.worker_id.clone())
        .collect();
    let selected_worker_id = eligible_candidates
        .first()
        .map(|candidate| candidate.worker_id.clone());
    let selection_explanation = eligible_candidates
        .first()
        .map(|selected| SelectionExplanation {
            objective: "minimum confidence-gated expected accepted cost".to_owned(),
            eligible_candidate_count: eligible_candidates.len(),
            selected_expected_accepted_cost_micros: selected.cost.expected_accepted_cost_micros,
            selected_success_lower_bound: selected.success_lower_bound,
            selected_p95_latency_ms: selected.p95_latency_ms,
            tie_break_order: vec![
                "expected_accepted_cost_micros ascending".to_owned(),
                "success_lower_bound descending".to_owned(),
                "p95_latency_ms ascending".to_owned(),
                "worker_id ascending".to_owned(),
            ],
        });

    Ok(RoutingQuote {
        decision_id: request.decision_id.clone(),
        task_id: request.task.id.clone(),
        evidence_snapshot_id: request.evidence_snapshot_id.clone(),
        policy_id: request.policy.policy_id.clone(),
        selected_worker_id,
        eligible_candidates,
        rejected_candidates,
        pareto_worker_ids,
        selection_explanation,
    })
}

fn validate_request(request: &QuoteRequest) -> Result<(), EngineError> {
    if request.decision_id.is_empty() {
        return Err(EngineError::EmptyField("decision_id"));
    }
    if request.evidence_snapshot_id.trim().is_empty() {
        return Err(EngineError::EmptyField("evidence_snapshot_id"));
    }
    request.task.validate()?;
    request.policy.validate()?;

    let mut worker_ids = BTreeSet::new();
    for candidate in &request.candidates {
        candidate.validate()?;
        let worker_id = candidate.worker.identity.worker_id.clone();
        if !worker_ids.insert(worker_id.clone()) {
            return Err(EngineError::DuplicateCandidate(worker_id));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn ineligibility_reasons(
    estimate: &WorkerEstimate,
    task: &TaskSpec,
    policy: &RoutingPolicy,
    evidence_snapshot_id: &str,
    required_context_tokens: u64,
    cost: &CostBreakdown,
) -> Vec<IneligibilityReason> {
    let worker = &estimate.worker;
    let mut reasons = Vec::new();

    if !worker.available {
        reasons.push(IneligibilityReason::Unavailable);
    }
    if estimate.evidence_snapshot_id != evidence_snapshot_id {
        reasons.push(IneligibilityReason::EvidenceSnapshotMismatch {
            expected: evidence_snapshot_id.to_owned(),
            actual: estimate.evidence_snapshot_id.clone(),
        });
    }
    if !task.allowed_providers.is_empty()
        && !task.allowed_providers.contains(&worker.identity.provider)
    {
        reasons.push(IneligibilityReason::ProviderNotAllowed {
            provider: worker.identity.provider.clone(),
        });
    }
    if !worker.data_clearance.permits(task.privacy) {
        reasons.push(IneligibilityReason::InsufficientPrivacyClearance {
            available: worker.data_clearance,
            required: task.privacy,
        });
    }
    if worker.context_window_tokens < required_context_tokens {
        reasons.push(IneligibilityReason::ContextWindowTooSmall {
            available_tokens: worker.context_window_tokens,
            required_tokens: required_context_tokens,
        });
    }

    let mut required_skills: Vec<_> = task.required_skills.iter().collect();
    required_skills.sort_by(|left, right| left.skill_id.cmp(&right.skill_id));
    for requirement in required_skills {
        if !worker.supported_skills.contains(&requirement.skill_id) {
            reasons.push(IneligibilityReason::MissingSkill {
                skill_id: requirement.skill_id.clone(),
            });
            continue;
        }
        match estimate.skill_estimates.get(&requirement.skill_id) {
            Some(skill_estimate)
                if skill_estimate.success_lower_bound < requirement.minimum_success_probability =>
            {
                reasons.push(IneligibilityReason::SkillConfidenceBelowMinimum {
                    skill_id: requirement.skill_id.clone(),
                    lower_bound: skill_estimate.success_lower_bound,
                    minimum: requirement.minimum_success_probability,
                });
            }
            Some(_) => {}
            None => reasons.push(IneligibilityReason::MissingSkillEstimate {
                skill_id: requirement.skill_id.clone(),
            }),
        }
    }

    for tool in &task.required_tools {
        if !worker.tools.contains(tool) {
            reasons.push(IneligibilityReason::MissingTool { tool: tool.clone() });
        }
    }
    if estimate.success.success_lower_bound < task.minimum_success_probability {
        reasons.push(IneligibilityReason::TaskConfidenceBelowMinimum {
            lower_bound: estimate.success.success_lower_bound,
            minimum: task.minimum_success_probability,
        });
    }
    if worker.cost.currency != policy.currency {
        reasons.push(IneligibilityReason::CurrencyMismatch {
            expected: policy.currency.clone(),
            actual: worker.cost.currency.clone(),
        });
    }
    if let Some(budget) = task.max_expected_cash_micros {
        if cost.expected_cash_micros > budget {
            reasons.push(IneligibilityReason::CashBudgetExceeded {
                expected_cash_micros: cost.expected_cash_micros,
                budget_cash_micros: budget,
            });
        }
    }
    if let Some(budget) = policy.max_expected_quota_milliunits {
        if cost.expected_quota_milliunits > budget {
            reasons.push(IneligibilityReason::QuotaBudgetExceeded {
                expected_quota_milliunits: cost.expected_quota_milliunits,
                budget_quota_milliunits: budget,
            });
        }
    }
    if let Some(maximum_ms) = task.max_p95_latency_ms {
        if estimate.p95_latency_ms > maximum_ms {
            reasons.push(IneligibilityReason::LatencyLimitExceeded {
                p95_latency_ms: estimate.p95_latency_ms,
                maximum_ms,
            });
        }
    }
    reasons
}

fn cost_breakdown(estimate: &WorkerEstimate, policy: &RoutingPolicy) -> CostBreakdown {
    let failure_probability = 1.0 - estimate.success.success_mean;
    let expected_failure_cash_micros =
        probability_weighted_cost(estimate.expected_fallback_cash_micros, failure_probability);
    let expected_cash_micros = estimate
        .expected_run_cash_micros
        .saturating_add(estimate.expected_review_cash_micros)
        .saturating_add(expected_failure_cash_micros);
    let quota_shadow_cash_micros = quota_shadow_cost(
        estimate.expected_quota_milliunits,
        policy.quota_shadow_cash_micros_per_unit,
    );

    CostBreakdown {
        run_cash_micros: estimate.expected_run_cash_micros,
        review_cash_micros: estimate.expected_review_cash_micros,
        expected_failure_cash_micros,
        expected_cash_micros,
        expected_quota_milliunits: estimate.expected_quota_milliunits,
        quota_shadow_cash_micros,
        expected_accepted_cost_micros: expected_cash_micros
            .saturating_add(quota_shadow_cash_micros),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn probability_weighted_cost(cost_micros: u64, probability: f64) -> u64 {
    if cost_micros == 0 || probability <= 0.0 {
        return 0;
    }
    if probability >= 1.0 {
        return cost_micros;
    }
    let weighted = (cost_micros as f64 * probability).ceil();
    if weighted >= u64::MAX as f64 {
        u64::MAX
    } else {
        weighted as u64
    }
}

fn quota_shadow_cost(quota_milliunits: u64, cash_micros_per_unit: u64) -> u64 {
    let product = u128::from(quota_milliunits) * u128::from(cash_micros_per_unit);
    let rounded_up = product.div_ceil(QUOTA_MILLIUNITS_PER_UNIT);
    u64::try_from(rounded_up).unwrap_or(u64::MAX)
}

fn mark_pareto_candidates(candidates: &mut [CandidateQuote]) {
    let efficient: Vec<bool> = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            !candidates
                .iter()
                .enumerate()
                .any(|(other_index, other)| other_index != index && dominates(other, candidate))
        })
        .collect();
    for (candidate, is_efficient) in candidates.iter_mut().zip(efficient) {
        candidate.pareto_efficient = is_efficient;
    }
}

fn dominates(left: &CandidateQuote, right: &CandidateQuote) -> bool {
    let no_worse = left.cost.expected_accepted_cost_micros
        <= right.cost.expected_accepted_cost_micros
        && left.p95_latency_ms <= right.p95_latency_ms
        && left.success_lower_bound >= right.success_lower_bound;
    let strictly_better = left.cost.expected_accepted_cost_micros
        < right.cost.expected_accepted_cost_micros
        || left.p95_latency_ms < right.p95_latency_ms
        || left.success_lower_bound > right.success_lower_bound;
    no_worse && strictly_better
}

fn compare_candidates(left: &CandidateQuote, right: &CandidateQuote) -> Ordering {
    left.cost
        .expected_accepted_cost_micros
        .cmp(&right.cost.expected_accepted_cost_micros)
        .then_with(|| {
            right
                .success_lower_bound
                .total_cmp(&left.success_lower_bound)
        })
        .then_with(|| left.p95_latency_ms.cmp(&right.p95_latency_ms))
        .then_with(|| left.worker_id.cmp(&right.worker_id))
}

fn positive_finite(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn non_negative_finite(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

#[derive(Debug, Error, PartialEq)]
pub enum EngineError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("candidate {0} appears more than once")]
    DuplicateCandidate(WorkerId),
    #[error("beta parameters must be finite and positive, got alpha={alpha}, beta={beta}")]
    InvalidBetaParameters { alpha: f64, beta: f64 },
    #[error(
        "observation weights must be finite and non-negative, got success={success_weight}, failure={failure_weight}"
    )]
    InvalidObservationWeights {
        success_weight: f64,
        failure_weight: f64,
    },
    #[error("confidence multiplier must be finite and non-negative, got {0}")]
    InvalidConfidenceMultiplier(f64),
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use workforce_domain::{
        CostProfile, ModelReleaseId, OfferingId, PrivacyClass, ProbabilityEstimate, RiskLevel,
        SkillRequirement, TaskId, VerificationPolicy, WorkerIdentity, WorkerProfile,
    };

    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn beta_posterior_updates_are_transparent() {
        let mut posterior = BetaPosterior::new(1.0, 1.0).expect("valid prior");
        let initial_lower = posterior.lower_bound(1.645).expect("valid z");
        posterior.observe(8.0, 2.0).expect("valid observation");

        assert_eq!(
            posterior,
            BetaPosterior {
                alpha: 9.0,
                beta: 3.0
            }
        );
        assert!((posterior.mean() - 0.75).abs() < 1e-12);
        assert!(posterior.lower_bound(1.645).expect("valid z") > initial_lower);
    }

    #[test]
    fn a_local_accepted_outcome_improves_posterior() {
        let mut posterior = BetaPosterior::new(2.0, 2.0).expect("valid prior");
        let before = posterior.mean();
        posterior
            .observe_outcome(true, 1.0)
            .expect("valid outcome weight");
        assert!(posterior.mean() > before);
        assert_eq!(posterior.alpha, 3.0);
        assert_eq!(posterior.beta, 2.0);
    }

    #[test]
    fn cheapest_eligible_worker_wins() {
        let cheap = candidate("cheap", 100, 0.85, 0.75);
        let expensive = candidate("expensive", 300, 0.95, 0.85);

        let result = quote(&request(vec![expensive, cheap])).expect("valid quote");

        assert_eq!(result.selected_worker_id, Some("worker:cheap".into()));
        assert_eq!(result.eligible_candidates[0].rank, 1);
        assert_eq!(
            result.eligible_candidates[0].worker_id,
            "worker:cheap".into()
        );
    }

    #[test]
    fn low_confidence_cheap_worker_is_rejected() {
        let cheap = candidate("cheap", 10, 0.8, 0.59);
        let safe = candidate("safe", 500, 0.9, 0.8);

        let result = quote(&request(vec![cheap, safe])).expect("valid quote");

        assert_eq!(result.selected_worker_id, Some("worker:safe".into()));
        assert!(result.rejected_candidates[0].reasons.iter().any(|reason| {
            matches!(
                reason,
                IneligibilityReason::TaskConfidenceBelowMinimum { .. }
            )
        }));
    }

    #[test]
    fn privacy_clearance_is_a_hard_constraint() {
        let public = candidate("public", 10, 0.9, 0.8);
        let mut local = candidate("local", 100, 0.9, 0.8);
        local.worker.data_clearance = PrivacyClass::Secret;
        let mut input = request(vec![public, local]);
        input.task.privacy = PrivacyClass::ConfidentialContent;

        let result = quote(&input).expect("valid quote");

        assert_eq!(result.selected_worker_id, Some("worker:local".into()));
        assert!(matches!(
            result.rejected_candidates[0].reasons.as_slice(),
            [IneligibilityReason::InsufficientPrivacyClearance { .. }]
        ));
    }

    #[test]
    fn expected_cost_contains_run_review_failure_and_quota_shadow() {
        let mut worker = candidate("costed", 100, 0.75, 0.7);
        worker.expected_review_cash_micros = 10;
        worker.expected_fallback_cash_micros = 100;
        worker.expected_quota_milliunits = 500;

        let result = quote(&request(vec![worker])).expect("valid quote");
        let cost = &result.eligible_candidates[0].cost;

        assert_eq!(cost.expected_failure_cash_micros, 25);
        assert_eq!(cost.expected_cash_micros, 135);
        assert_eq!(cost.quota_shadow_cash_micros, 10);
        assert_eq!(cost.expected_accepted_cost_micros, 145);
    }

    #[test]
    fn pareto_frontier_preserves_real_tradeoffs() {
        let mut cheap = candidate("cheap", 100, 0.8, 0.7);
        cheap.p95_latency_ms = 200;
        let mut capable = candidate("capable", 200, 0.95, 0.9);
        capable.p95_latency_ms = 150;
        let mut dominated = candidate("dominated", 300, 0.8, 0.7);
        dominated.p95_latency_ms = 250;

        let result = quote(&request(vec![dominated, capable, cheap])).expect("valid quote");

        assert_eq!(
            result.pareto_worker_ids,
            vec![
                WorkerId::from("worker:cheap"),
                WorkerId::from("worker:capable")
            ]
        );
        assert!(
            !result
                .eligible_candidates
                .iter()
                .find(|candidate| candidate.worker_id == WorkerId::from("worker:dominated"))
                .expect("dominated candidate")
                .pareto_efficient
        );
    }

    #[test]
    fn quote_is_independent_of_candidate_input_order() {
        let first = candidate("a", 100, 0.9, 0.8);
        let second = candidate("b", 100, 0.9, 0.8);

        let forward = quote(&request(vec![first.clone(), second.clone()])).expect("valid quote");
        let reverse = quote(&request(vec![second, first])).expect("valid quote");

        assert_eq!(forward, reverse);
        assert_eq!(forward.selected_worker_id, Some("worker:a".into()));
    }

    #[test]
    fn every_failed_hard_constraint_is_explained() {
        let mut worker = candidate("bad", 500, 0.5, 0.4);
        worker.worker.available = false;
        worker.worker.data_clearance = PrivacyClass::Public;
        worker.worker.context_window_tokens = 100;
        worker.worker.tools.clear();
        worker.skill_estimates.clear();
        worker.evidence_snapshot_id = "snapshot:old".to_owned();
        worker.p95_latency_ms = 500;
        let mut input = request(vec![worker]);
        input.task.privacy = PrivacyClass::ConfidentialContent;
        input.task.max_expected_cash_micros = Some(100);
        input.task.max_p95_latency_ms = Some(200);
        input.task.estimated_input_tokens = 200;

        let result = quote(&input).expect("valid quote");
        let reasons = &result.rejected_candidates[0].reasons;

        assert!(reasons.len() >= 7);
        assert!(
            reasons.iter().any(|reason| matches!(
                reason,
                IneligibilityReason::EvidenceSnapshotMismatch { .. }
            ))
        );
        assert!(
            reasons
                .iter()
                .any(|reason| matches!(reason, IneligibilityReason::MissingSkillEstimate { .. }))
        );
        assert!(
            reasons
                .iter()
                .any(|reason| matches!(reason, IneligibilityReason::CashBudgetExceeded { .. }))
        );
    }

    fn request(candidates: Vec<WorkerEstimate>) -> QuoteRequest {
        QuoteRequest {
            decision_id: "decision:test".into(),
            evidence_snapshot_id: "snapshot:1".to_owned(),
            task: TaskSpec {
                id: TaskId::from("task:test"),
                summary: "Implement a Rust change".to_owned(),
                repository: None,
                required_skills: vec![SkillRequirement {
                    skill_id: "skill:rust".into(),
                    minimum_success_probability: 0.6,
                }],
                required_tools: BTreeSet::from(["shell".to_owned()]),
                allowed_providers: BTreeSet::new(),
                privacy: PrivacyClass::Public,
                risk: RiskLevel::Low,
                verification: VerificationPolicy::Deterministic,
                minimum_success_probability: 0.6,
                max_expected_cash_micros: None,
                max_p95_latency_ms: None,
                estimated_input_tokens: 100,
                estimated_output_tokens: 100,
            },
            policy: RoutingPolicy {
                policy_id: "policy:test-v1".to_owned(),
                currency: "USD".to_owned(),
                quota_shadow_cash_micros_per_unit: 20,
                max_expected_quota_milliunits: None,
            },
            candidates,
        }
    }

    fn candidate(
        name: &str,
        run_cash_micros: u64,
        success_mean: f64,
        success_lower_bound: f64,
    ) -> WorkerEstimate {
        let worker_id = format!("worker:{name}");
        let skill_id = SkillId::from("skill:rust");
        WorkerEstimate {
            worker: WorkerProfile {
                identity: WorkerIdentity {
                    worker_id: WorkerId::new(worker_id),
                    model_release_id: ModelReleaseId::new(format!("model:{name}-2026-01-01")),
                    offering_id: OfferingId::new(format!("offering:{name}")),
                    provider: "provider-a".to_owned(),
                    harness_id: "raw-api".to_owned(),
                    harness_version: "1".to_owned(),
                    reasoning_configuration: "standard".to_owned(),
                    system_prompt_sha256: EMPTY_SHA256.to_owned(),
                    skill_pack_version: "1".to_owned(),
                    toolset_version: "1".to_owned(),
                    execution_policy_sha256: EMPTY_SHA256.to_owned(),
                },
                supported_skills: BTreeSet::from([skill_id.clone()]),
                tools: BTreeSet::from(["shell".to_owned()]),
                data_clearance: PrivacyClass::Public,
                context_window_tokens: 10_000,
                cost: CostProfile {
                    currency: "USD".to_owned(),
                    input_micros_per_million_tokens: 0,
                    output_micros_per_million_tokens: 0,
                    fixed_request_micros: 0,
                    quota_milliunits_per_request: 0,
                },
                available: true,
            },
            success: ProbabilityEstimate {
                success_mean,
                success_lower_bound,
                evidence_count: 20,
            },
            skill_estimates: BTreeMap::from([(
                skill_id,
                ProbabilityEstimate {
                    success_mean,
                    success_lower_bound,
                    evidence_count: 20,
                },
            )]),
            expected_run_cash_micros: run_cash_micros,
            expected_review_cash_micros: 0,
            expected_fallback_cash_micros: 0,
            expected_quota_milliunits: 0,
            p95_latency_ms: 100,
            evidence_snapshot_id: "snapshot:1".to_owned(),
        }
    }
}
