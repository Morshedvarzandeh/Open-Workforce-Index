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
    DecisionId, DomainError, PrivacyClass, ProbabilityEstimate, SkillId, TaskSpec,
    VerificationPolicy, WorkerEstimate, WorkerId,
};

const TOKENS_PER_MILLION: u128 = 1_000_000;
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

    /// The exact one-sided lower credible bound: the `tail_probability`
    /// quantile of `Beta(alpha, beta)`.
    ///
    /// `tail_probability = 0.05` gives a 95% lower bound. The quantile is
    /// computed from the regularized incomplete beta function rather than a
    /// normal approximation, because the approximation is *anti-conservative*
    /// exactly where this gate matters most — few observations and a high
    /// success rate. At `Beta(6, 1)` (five successes) the normal form reports
    /// 0.654 against a true bound of 0.607, and the error only becomes
    /// negligible past roughly a hundred observations. A gate whose purpose is
    /// conservatism must not overstate the floor for a barely-measured worker.
    ///
    /// [`Self::normal_approximation_lower_bound`] preserves the old behaviour
    /// for comparison and regression testing.
    pub fn lower_bound(self, tail_probability: f64) -> Result<f64, EngineError> {
        self.validate()?;
        if !(0.0..=1.0).contains(&tail_probability) || !tail_probability.is_finite() {
            return Err(EngineError::InvalidTailProbability(tail_probability));
        }
        Ok(beta::quantile(self.alpha, self.beta, tail_probability))
    }

    /// The superseded `mean - z * sqrt(variance)` bound, clipped to `[0, 1]`.
    ///
    /// Retained so the divergence from [`Self::lower_bound`] stays measurable
    /// in tests. It must not be used as an eligibility gate.
    pub fn normal_approximation_lower_bound(self, z: f64) -> Result<f64, EngineError> {
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
        tail_probability: f64,
    ) -> Result<ProbabilityEstimate, EngineError> {
        self.validate()?;
        Ok(ProbabilityEstimate {
            success_mean: self.mean(),
            success_lower_bound: self.lower_bound(tail_probability)?,
            evidence_count,
        })
    }
}

/// Dependency-free regularized incomplete beta function and its inverse.
///
/// The engine deliberately carries this rather than a statistics dependency:
/// the quantile sits directly in the safety gate, so its implementation should
/// be readable and auditable in-tree.
mod beta {
    use std::f64::consts::PI;

    const LANCZOS_G: f64 = 7.0;
    const LANCZOS_COEFFICIENTS: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];

    /// Lanczos approximation of `ln|Gamma(x)|` for `x > 0`.
    fn ln_gamma(x: f64) -> f64 {
        if x < 0.5 {
            // Reflection formula: Gamma(x)Gamma(1-x) = pi / sin(pi x).
            (PI / (PI * x).sin().abs()).ln() - ln_gamma(1.0 - x)
        } else {
            let shifted = x - 1.0;
            let mut series = LANCZOS_COEFFICIENTS[0];
            for (index, coefficient) in LANCZOS_COEFFICIENTS.iter().enumerate().skip(1) {
                #[allow(clippy::cast_precision_loss)]
                let denominator = shifted + index as f64;
                series += coefficient / denominator;
            }
            let t = shifted + LANCZOS_G + 0.5;
            0.5 * (2.0 * PI).ln() + (shifted + 0.5) * t.ln() - t + series.ln()
        }
    }

    fn ln_beta(a: f64, b: f64) -> f64 {
        ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b)
    }

    /// Modified Lentz evaluation of the continued fraction for `I_x(a, b)`.
    fn continued_fraction(a: f64, b: f64, x: f64) -> f64 {
        const MAX_ITERATIONS: usize = 400;
        const EPSILON: f64 = 3e-16;
        const TINY: f64 = 1e-300;

        let qab = a + b;
        let qap = a + 1.0;
        let qam = a - 1.0;
        let mut c = 1.0_f64;
        let mut d = 1.0 - qab * x / qap;
        if d.abs() < TINY {
            d = TINY;
        }
        d = 1.0 / d;
        let mut h = d;

        for iteration in 1..=MAX_ITERATIONS {
            #[allow(clippy::cast_precision_loss)]
            let m = iteration as f64;
            let m2 = 2.0 * m;

            let even = m * (b - m) * x / ((qam + m2) * (a + m2));
            d = 1.0 + even * d;
            if d.abs() < TINY {
                d = TINY;
            }
            c = 1.0 + even / c;
            if c.abs() < TINY {
                c = TINY;
            }
            d = 1.0 / d;
            h *= d * c;

            let odd = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
            d = 1.0 + odd * d;
            if d.abs() < TINY {
                d = TINY;
            }
            c = 1.0 + odd / c;
            if c.abs() < TINY {
                c = TINY;
            }
            d = 1.0 / d;
            let delta = d * c;
            h *= delta;

            if (delta - 1.0).abs() < EPSILON {
                break;
            }
        }
        h
    }

    /// The regularized incomplete beta function `I_x(a, b)`, i.e. the CDF of
    /// `Beta(a, b)` evaluated at `x`.
    pub fn cdf(a: f64, b: f64, x: f64) -> f64 {
        if !x.is_finite() || x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        let front = (a * x.ln() + b * (-x).ln_1p() - ln_beta(a, b)).exp();
        // The continued fraction converges quickly only on its own side of the
        // distribution's mode, so mirror the arguments past the crossover.
        if x < (a + 1.0) / (a + b + 2.0) {
            (front * continued_fraction(a, b, x) / a).clamp(0.0, 1.0)
        } else {
            (1.0 - front * continued_fraction(b, a, 1.0 - x) / b).clamp(0.0, 1.0)
        }
    }

    /// The `probability` quantile of `Beta(a, b)`, found by bisection on the
    /// monotone CDF. Bisection is used in preference to Newton steps because it
    /// cannot overshoot near `0` or `1`, where the gate operates.
    pub fn quantile(a: f64, b: f64, probability: f64) -> f64 {
        if probability <= 0.0 {
            return 0.0;
        }
        if probability >= 1.0 {
            return 1.0;
        }
        let mut low = 0.0_f64;
        let mut high = 1.0_f64;
        for _ in 0..128 {
            let mid = 0.5 * (low + high);
            if mid <= low || mid >= high {
                break;
            }
            if cdf(a, b, mid) < probability {
                low = mid;
            } else {
                high = mid;
            }
        }
        0.5 * (low + high)
    }
}

/// Which point of the success posterior prices the retry term.
///
/// The eligibility gate is always conservative — it uses the lower bound. The
/// objective is a separate choice, because an expectation is ordinarily taken
/// at the mean. Making it explicit matters: under [`Self::Mean`], `Beta(2, 1)`
/// and `Beta(60, 30)` share a mean and therefore rank identically, even though
/// one is a guess and the other is measured. [`Self::LowerBound`] prices that
/// uncertainty into the retry term instead of discarding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureProbabilityBasis {
    /// Price retries at `1 - success_mean`. A true expected value.
    #[default]
    Mean,
    /// Price retries at `1 - success_lower_bound`, so a wide posterior is
    /// charged for its own uncertainty.
    LowerBound,
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
    /// Checkers approved by the local policy owner. A maker cannot authorize
    /// an arbitrary worker merely by naming it in its estimate.
    #[serde(default)]
    pub authorized_checker_worker_ids: BTreeSet<WorkerId>,
    /// Which point of the posterior prices the retry term.
    #[serde(default)]
    pub failure_probability_basis: FailureProbabilityBasis,
    /// Total attempts allowed, including the first. `2` means one retry, which
    /// is the historical single-retry behaviour and remains the default.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
}

const fn default_max_attempts() -> u32 {
    2
}

impl RoutingPolicy {
    pub fn validate(&self) -> Result<(), EngineError> {
        if self.policy_id.trim().is_empty() {
            return Err(EngineError::EmptyField("policy.policy_id"));
        }
        if self.currency.trim().is_empty() {
            return Err(EngineError::EmptyField("policy.currency"));
        }
        if self
            .authorized_checker_worker_ids
            .iter()
            .any(WorkerId::is_empty)
        {
            return Err(EngineError::EmptyField(
                "policy.authorized_checker_worker_ids",
            ));
        }
        if self.max_attempts == 0 {
            return Err(EngineError::InvalidMaxAttempts(self.max_attempts));
        }
        Ok(())
    }

    fn failure_probability(&self, estimate: &ProbabilityEstimate) -> f64 {
        let success = match self.failure_probability_basis {
            FailureProbabilityBasis::Mean => estimate.success_mean,
            FailureProbabilityBasis::LowerBound => estimate.success_lower_bound,
        };
        (1.0 - success).clamp(0.0, 1.0)
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
    pub selected_checker_worker_id: Option<WorkerId>,
    pub eligible_candidates: Vec<CandidateQuote>,
    pub rejected_candidates: Vec<RejectedCandidate>,
    pub pareto_worker_ids: Vec<WorkerId>,
    pub selection_explanation: Option<SelectionExplanation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateQuote {
    pub rank: usize,
    pub worker_id: WorkerId,
    pub checker_worker_id: Option<WorkerId>,
    pub success_mean: f64,
    pub success_lower_bound: f64,
    pub p95_latency_ms: u64,
    pub cost: CostBreakdown,
    pub pareto_efficient: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub currency: String,
    pub estimated_input_tokens: u64,
    pub estimated_output_tokens: u64,
    pub input_micros_per_million_tokens: u64,
    pub output_micros_per_million_tokens: u64,
    pub input_token_cash_micros: u64,
    pub output_token_cash_micros: u64,
    pub fixed_request_cash_micros: u64,
    pub tool_cash_micros: u64,
    pub run_cash_micros: u64,
    pub review_cash_micros: u64,
    pub expected_failure_cash_micros: u64,
    pub expected_cash_micros: u64,
    pub base_quota_milliunits: u64,
    pub additional_quota_milliunits: u64,
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
    MissingCheckerWorker,
    CheckerMatchesMaker {
        checker_worker_id: WorkerId,
    },
    UnauthorizedCheckerWorker {
        checker_worker_id: WorkerId,
    },
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
    InsufficientTaskEvidence {
        evidence_count: u64,
        minimum: u64,
    },
    InsufficientSkillEvidence {
        skill_id: SkillId,
        evidence_count: u64,
        minimum: u64,
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
        let cost = cost_breakdown(estimate, &request.task, &request.policy)?;
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
                checker_worker_id: estimate.checker_worker_id.clone(),
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
    let selected_checker_worker_id = eligible_candidates
        .first()
        .and_then(|candidate| candidate.checker_worker_id.clone());
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
        selected_checker_worker_id,
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
    // v0.1 has no independently verified locality/deployment attribute. A
    // clearance label alone cannot prove that a candidate is safe for secrets,
    // so fail closed until a local-execution boundary can be enforced.
    if request.task.privacy == PrivacyClass::Secret {
        return Err(EngineError::SecretRoutingUnsupported);
    }
    request.policy.validate()?;
    if request.candidates.is_empty() {
        return Err(EngineError::EmptyCandidates);
    }

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
    if task.verification == VerificationPolicy::MakerChecker {
        match &estimate.checker_worker_id {
            None => reasons.push(IneligibilityReason::MissingCheckerWorker),
            Some(checker_worker_id) if checker_worker_id == &worker.identity.worker_id => {
                reasons.push(IneligibilityReason::CheckerMatchesMaker {
                    checker_worker_id: checker_worker_id.clone(),
                });
            }
            Some(checker_worker_id)
                if !policy
                    .authorized_checker_worker_ids
                    .contains(checker_worker_id) =>
            {
                reasons.push(IneligibilityReason::UnauthorizedCheckerWorker {
                    checker_worker_id: checker_worker_id.clone(),
                });
            }
            Some(_) => {}
        }
    }
    if estimate.evidence_snapshot_id != evidence_snapshot_id {
        reasons.push(IneligibilityReason::EvidenceSnapshotMismatch {
            expected: evidence_snapshot_id.to_owned(),
            actual: estimate.evidence_snapshot_id.clone(),
        });
    }
    // Provider identifiers are validated as nonblank, whitespace-canonical
    // values before this point. Deliberately use exact, case-sensitive matching
    // so the allow-list cannot silently broaden through normalization.
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
            Some(skill_estimate) => {
                if skill_estimate.success_lower_bound < requirement.minimum_success_probability {
                    reasons.push(IneligibilityReason::SkillConfidenceBelowMinimum {
                        skill_id: requirement.skill_id.clone(),
                        lower_bound: skill_estimate.success_lower_bound,
                        minimum: requirement.minimum_success_probability,
                    });
                }
                // A bound without observations behind it is an assertion, not
                // evidence. Check it separately so the quote says which of the
                // two failed.
                if skill_estimate.evidence_count < requirement.minimum_evidence_count {
                    reasons.push(IneligibilityReason::InsufficientSkillEvidence {
                        skill_id: requirement.skill_id.clone(),
                        evidence_count: skill_estimate.evidence_count,
                        minimum: requirement.minimum_evidence_count,
                    });
                }
            }
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
    if estimate.success.evidence_count < task.minimum_evidence_count {
        reasons.push(IneligibilityReason::InsufficientTaskEvidence {
            evidence_count: estimate.success.evidence_count,
            minimum: task.minimum_evidence_count,
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

fn cost_breakdown(
    estimate: &WorkerEstimate,
    task: &TaskSpec,
    policy: &RoutingPolicy,
) -> Result<CostBreakdown, EngineError> {
    let worker_id = &estimate.worker.identity.worker_id;
    let profile = &estimate.worker.cost;
    let input_token_cash_micros = token_cash_cost(
        task.estimated_input_tokens,
        profile.input_micros_per_million_tokens,
        worker_id,
        "input_token_cash_micros",
    )?;
    let output_token_cash_micros = token_cash_cost(
        task.estimated_output_tokens,
        profile.output_micros_per_million_tokens,
        worker_id,
        "output_token_cash_micros",
    )?;
    let model_cash_micros = checked_add_cost(
        input_token_cash_micros,
        output_token_cash_micros,
        worker_id,
        "model_token_cash_micros",
    )?;
    let model_and_request_cash_micros = checked_add_cost(
        model_cash_micros,
        profile.fixed_request_micros,
        worker_id,
        "model_and_request_cash_micros",
    )?;
    let run_cash_micros = checked_add_cost(
        model_and_request_cash_micros,
        estimate.expected_tool_cash_micros,
        worker_id,
        "run_cash_micros",
    )?;
    let failure_probability = policy.failure_probability(&estimate.success);
    let expected_failure_cash_micros = expected_retry_cost(
        estimate.expected_fallback_cash_micros,
        failure_probability,
        policy.max_attempts,
    );
    let run_and_review_cash_micros = checked_add_cost(
        run_cash_micros,
        estimate.expected_review_cash_micros,
        worker_id,
        "run_and_review_cash_micros",
    )?;
    let expected_cash_micros = checked_add_cost(
        run_and_review_cash_micros,
        expected_failure_cash_micros,
        worker_id,
        "expected_cash_micros",
    )?;
    let expected_quota_milliunits = profile
        .quota_milliunits_per_request
        .checked_add(estimate.expected_additional_quota_milliunits)
        .ok_or_else(|| EngineError::CostOverflow {
            worker_id: worker_id.clone(),
            component: "expected_quota_milliunits",
        })?;
    let quota_shadow_cash_micros = quota_shadow_cost(
        expected_quota_milliunits,
        policy.quota_shadow_cash_micros_per_unit,
        worker_id,
    )?;
    let expected_accepted_cost_micros = checked_add_cost(
        expected_cash_micros,
        quota_shadow_cash_micros,
        worker_id,
        "expected_accepted_cost_micros",
    )?;

    Ok(CostBreakdown {
        currency: profile.currency.clone(),
        estimated_input_tokens: task.estimated_input_tokens,
        estimated_output_tokens: task.estimated_output_tokens,
        input_micros_per_million_tokens: profile.input_micros_per_million_tokens,
        output_micros_per_million_tokens: profile.output_micros_per_million_tokens,
        input_token_cash_micros,
        output_token_cash_micros,
        fixed_request_cash_micros: profile.fixed_request_micros,
        tool_cash_micros: estimate.expected_tool_cash_micros,
        run_cash_micros,
        review_cash_micros: estimate.expected_review_cash_micros,
        expected_failure_cash_micros,
        expected_cash_micros,
        base_quota_milliunits: profile.quota_milliunits_per_request,
        additional_quota_milliunits: estimate.expected_additional_quota_milliunits,
        expected_quota_milliunits,
        quota_shadow_cash_micros,
        expected_accepted_cost_micros,
    })
}

fn token_cash_cost(
    tokens: u64,
    cash_micros_per_million_tokens: u64,
    worker_id: &WorkerId,
    component: &'static str,
) -> Result<u64, EngineError> {
    let product = u128::from(tokens) * u128::from(cash_micros_per_million_tokens);
    u64::try_from(product.div_ceil(TOKENS_PER_MILLION)).map_err(|_| EngineError::CostOverflow {
        worker_id: worker_id.clone(),
        component,
    })
}

fn checked_add_cost(
    left: u64,
    right: u64,
    worker_id: &WorkerId,
    component: &'static str,
) -> Result<u64, EngineError> {
    left.checked_add(right)
        .ok_or_else(|| EngineError::CostOverflow {
            worker_id: worker_id.clone(),
            component,
        })
}

/// Expected retry spend over `max_attempts` total attempts.
///
/// Each attempt fails independently with `failure_probability`, so attempt
/// `k + 1` is reached with probability `failure_probability^k` and the expected
/// extra spend is `fallback * sum_{k=1}^{max_attempts-1} failure_probability^k`.
///
/// With the default `max_attempts = 2` this reduces to the original
/// `(1 - p) * fallback`. The general form matters because the single-retry
/// model assumes the fallback always succeeds, which systematically flatters
/// cheap unreliable workers — the precise error this engine exists to avoid.
fn expected_retry_cost(
    fallback_cash_micros: u64,
    failure_probability: f64,
    max_attempts: u32,
) -> u64 {
    if fallback_cash_micros == 0 || failure_probability <= 0.0 || max_attempts <= 1 {
        return 0;
    }
    let mut weight = 0.0_f64;
    let mut term = 1.0_f64;
    for _ in 1..max_attempts {
        term *= failure_probability;
        weight += term;
    }
    probability_weighted_cost(fallback_cash_micros, weight)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn probability_weighted_cost(cost_micros: u64, weight: f64) -> u64 {
    if cost_micros == 0 || weight <= 0.0 {
        return 0;
    }
    let weighted = (cost_micros as f64 * weight).ceil();
    if weighted >= u64::MAX as f64 {
        u64::MAX
    } else {
        weighted as u64
    }
}

fn quota_shadow_cost(
    quota_milliunits: u64,
    cash_micros_per_unit: u64,
    worker_id: &WorkerId,
) -> Result<u64, EngineError> {
    let product = u128::from(quota_milliunits) * u128::from(cash_micros_per_unit);
    let rounded_up = product.div_ceil(QUOTA_MILLIUNITS_PER_UNIT);
    u64::try_from(rounded_up).map_err(|_| EngineError::CostOverflow {
        worker_id: worker_id.clone(),
        component: "quota_shadow_cash_micros",
    })
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
    #[error("quote request must contain at least one candidate")]
    EmptyCandidates,
    #[error("secret tasks cannot be routed until local execution can be verified")]
    SecretRoutingUnsupported,
    #[error("candidate {0} appears more than once")]
    DuplicateCandidate(WorkerId),
    #[error("cost component {component} overflows u64 for candidate {worker_id}")]
    CostOverflow {
        worker_id: WorkerId,
        component: &'static str,
    },
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
    #[error("tail probability must be finite and within [0, 1], got {0}")]
    InvalidTailProbability(f64),
    #[error("policy.max_attempts must be at least 1, got {0}")]
    InvalidMaxAttempts(u32),
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

    /// Reference 5th percentiles of `Beta(alpha, beta)`, computed independently
    /// by numerical quadrature of the density. These pin the quantile against a
    /// source outside its own implementation.
    const BETA_FIFTH_PERCENTILE: [(f64, f64, f64); 8] = [
        (1.0, 1.0, 0.050),
        (5.0, 0.5, 0.668),
        (6.0, 1.0, 0.607),
        (9.0, 1.0, 0.717),
        (19.0, 1.0, 0.854),
        (8.0, 2.0, 0.571),
        (45.0, 5.0, 0.823),
        (90.0, 10.0, 0.847),
    ];

    #[test]
    fn exact_lower_bound_matches_independently_computed_quantiles() {
        for (alpha, beta, expected) in BETA_FIFTH_PERCENTILE {
            let bound = BetaPosterior::new(alpha, beta)
                .expect("valid posterior")
                .lower_bound(0.05)
                .expect("valid tail probability");
            assert!(
                (bound - expected).abs() < 1e-3,
                "Beta({alpha}, {beta}): got {bound}, expected {expected}"
            );
        }
    }

    /// The reason the normal approximation was replaced: it overstates the
    /// floor precisely when a worker is barely measured, which is the default
    /// state of every newly discovered model.
    #[test]
    fn normal_approximation_is_anti_conservative_for_sparse_evidence() {
        for (alpha, beta, exact) in BETA_FIFTH_PERCENTILE {
            if alpha + beta > 20.0 {
                continue;
            }
            let posterior = BetaPosterior::new(alpha, beta).expect("valid posterior");
            let approximate = posterior
                .normal_approximation_lower_bound(1.645)
                .expect("valid z");
            if (alpha, beta) == (1.0, 1.0) {
                // The uniform prior is the one case the approximation is safe.
                assert!(approximate <= exact + 1e-9);
                continue;
            }
            assert!(
                approximate > exact + 1e-3,
                "Beta({alpha}, {beta}): approximation {approximate} should overstate {exact}"
            );
        }
    }

    #[test]
    fn beta_cdf_and_quantile_are_mutually_consistent() {
        for (alpha, beta) in [(2.0, 5.0), (7.0, 3.0), (1.5, 1.5), (60.0, 30.0)] {
            for probability in [0.01, 0.05, 0.25, 0.5, 0.75, 0.99] {
                let x = beta::quantile(alpha, beta, probability);
                let round_trip = beta::cdf(alpha, beta, x);
                assert!(
                    (round_trip - probability).abs() < 1e-6,
                    "Beta({alpha}, {beta}) at p={probability}: cdf(quantile(p)) = {round_trip}"
                );
            }
        }
    }

    #[test]
    fn lower_bound_rejects_a_tail_probability_outside_the_unit_interval() {
        let posterior = BetaPosterior::new(2.0, 2.0).expect("valid posterior");
        assert_eq!(
            posterior.lower_bound(1.645),
            Err(EngineError::InvalidTailProbability(1.645))
        );
        assert_eq!(
            posterior.lower_bound(-0.1),
            Err(EngineError::InvalidTailProbability(-0.1))
        );
    }

    #[test]
    fn beta_posterior_updates_are_transparent() {
        let mut posterior = BetaPosterior::new(1.0, 1.0).expect("valid prior");
        let initial_lower = posterior.lower_bound(0.05).expect("valid tail probability");
        posterior.observe(8.0, 2.0).expect("valid observation");

        assert_eq!(
            posterior,
            BetaPosterior {
                alpha: 9.0,
                beta: 3.0
            }
        );
        assert!((posterior.mean() - 0.75).abs() < 1e-12);
        assert!(posterior.lower_bound(0.05).expect("valid tail probability") > initial_lower);
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
    fn empty_candidate_set_is_rejected() {
        assert_eq!(
            quote(&request(Vec::new())),
            Err(EngineError::EmptyCandidates)
        );
    }

    #[test]
    fn a_nonempty_no_selection_quote_contains_candidate_rejections() {
        let mut unavailable = candidate("unavailable", 100, 0.9, 0.8);
        unavailable.worker.available = false;

        let result = quote(&request(vec![unavailable])).expect("valid no-selection quote");

        assert_eq!(result.selected_worker_id, None);
        assert!(result.eligible_candidates.is_empty());
        assert_eq!(result.rejected_candidates.len(), 1);
        assert!(matches!(
            result.rejected_candidates[0].reasons.as_slice(),
            [IneligibilityReason::Unavailable]
        ));
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
    fn secret_tasks_fail_closed_without_verified_local_execution() {
        let mut local_label_only = candidate("secret-clearance", 10, 0.9, 0.8);
        local_label_only.worker.data_clearance = PrivacyClass::Secret;
        let mut input = request(vec![local_label_only]);
        input.task.privacy = PrivacyClass::Secret;

        assert_eq!(quote(&input), Err(EngineError::SecretRoutingUnsupported));
    }

    #[test]
    fn provider_allow_list_uses_exact_canonical_identity() {
        let mut exact = candidate("exact-provider", 10, 0.9, 0.8);
        exact.worker.identity.provider = "provider-a".to_owned();
        let mut different_case = candidate("different-case-provider", 5, 0.9, 0.8);
        different_case.worker.identity.provider = "Provider-A".to_owned();
        let mut input = request(vec![different_case, exact]);
        input.task.allowed_providers = BTreeSet::from(["provider-a".to_owned()]);

        let result = quote(&input).expect("valid provider-filtered quote");

        assert_eq!(
            result.selected_worker_id,
            Some(WorkerId::from("worker:exact-provider"))
        );
        assert!(matches!(
            result.rejected_candidates[0].reasons.as_slice(),
            [IneligibilityReason::ProviderNotAllowed { provider }]
                if provider == "Provider-A"
        ));
    }

    #[test]
    fn quote_rejects_noncanonical_provider_values_before_filtering() {
        let mut worker = candidate("padded-provider", 10, 0.9, 0.8);
        worker.worker.identity.provider = "provider-a ".to_owned();

        assert_eq!(
            quote(&request(vec![worker])),
            Err(EngineError::Domain(
                DomainError::NonCanonicalProviderIdentifier("worker.provider")
            ))
        );
    }

    #[test]
    fn high_and_consequential_risk_require_stronger_verification() {
        let mut high = request(vec![candidate("maker", 100, 0.9, 0.8)]);
        high.task.risk = RiskLevel::High;
        assert_eq!(
            quote(&high),
            Err(EngineError::Domain(DomainError::InsufficientVerification {
                risk: RiskLevel::High,
                required: VerificationPolicy::MakerChecker,
                actual: VerificationPolicy::Deterministic,
            }))
        );

        let mut consequential = high;
        consequential.task.risk = RiskLevel::Consequential;
        consequential.task.verification = VerificationPolicy::MakerChecker;
        assert_eq!(
            quote(&consequential),
            Err(EngineError::Domain(DomainError::InsufficientVerification {
                risk: RiskLevel::Consequential,
                required: VerificationPolicy::HumanApproval,
                actual: VerificationPolicy::MakerChecker,
            }))
        );

        consequential.task.verification = VerificationPolicy::HumanApproval;
        assert!(quote(&consequential).is_ok());
    }

    #[test]
    fn maker_checker_requires_a_named_independent_checker() {
        let missing = candidate("missing", 10, 0.9, 0.8);
        let mut self_checked = candidate("self-checked", 20, 0.9, 0.8);
        self_checked.checker_worker_id = Some(self_checked.worker.identity.worker_id.clone());
        let mut unauthorized = candidate("unauthorized", 25, 0.9, 0.8);
        unauthorized.checker_worker_id = Some("worker:invented".into());
        let mut independent = candidate("independent", 30, 0.9, 0.8);
        independent.checker_worker_id = Some("worker:reviewer".into());
        let mut input = request(vec![missing, self_checked, unauthorized, independent]);
        input.task.risk = RiskLevel::High;
        input.task.verification = VerificationPolicy::MakerChecker;
        input.policy.authorized_checker_worker_ids =
            BTreeSet::from([WorkerId::from("worker:reviewer")]);

        let result = quote(&input).expect("valid maker-checker quote");

        assert_eq!(
            result.selected_worker_id,
            Some(WorkerId::from("worker:independent"))
        );
        assert_eq!(
            result.selected_checker_worker_id,
            Some(WorkerId::from("worker:reviewer"))
        );
        let missing = result
            .rejected_candidates
            .iter()
            .find(|candidate| candidate.worker_id == WorkerId::from("worker:missing"))
            .expect("missing-checker rejection");
        assert!(matches!(
            missing.reasons.as_slice(),
            [IneligibilityReason::MissingCheckerWorker]
        ));
        let self_checked = result
            .rejected_candidates
            .iter()
            .find(|candidate| candidate.worker_id == WorkerId::from("worker:self-checked"))
            .expect("self-check rejection");
        assert!(matches!(
            self_checked.reasons.as_slice(),
            [IneligibilityReason::CheckerMatchesMaker { checker_worker_id }]
                if checker_worker_id == &WorkerId::from("worker:self-checked")
        ));
        let unauthorized = result
            .rejected_candidates
            .iter()
            .find(|candidate| candidate.worker_id == WorkerId::from("worker:unauthorized"))
            .expect("unauthorized-checker rejection");
        assert!(matches!(
            unauthorized.reasons.as_slice(),
            [IneligibilityReason::UnauthorizedCheckerWorker { checker_worker_id }]
                if checker_worker_id == &WorkerId::from("worker:invented")
        ));
    }

    #[test]
    fn expected_cost_contains_run_review_failure_and_quota_shadow() {
        let mut worker = candidate("costed", 100, 0.75, 0.7);
        worker.expected_tool_cash_micros = 7;
        worker.expected_review_cash_micros = 10;
        worker.expected_fallback_cash_micros = 100;
        worker.worker.cost.quota_milliunits_per_request = 250;
        worker.expected_additional_quota_milliunits = 500;

        let result = quote(&request(vec![worker])).expect("valid quote");
        let cost = &result.eligible_candidates[0].cost;

        assert_eq!(cost.fixed_request_cash_micros, 100);
        assert_eq!(cost.tool_cash_micros, 7);
        assert_eq!(cost.run_cash_micros, 107);
        assert_eq!(cost.expected_failure_cash_micros, 25);
        assert_eq!(cost.expected_cash_micros, 142);
        assert_eq!(cost.base_quota_milliunits, 250);
        assert_eq!(cost.additional_quota_milliunits, 500);
        assert_eq!(cost.expected_quota_milliunits, 750);
        assert_eq!(cost.quota_shadow_cash_micros, 15);
        assert_eq!(cost.expected_accepted_cost_micros, 157);
    }

    #[test]
    fn token_tariffs_are_derived_with_per_component_ceiling() {
        let mut worker = candidate("metered", 3, 0.9, 0.8);
        worker.worker.cost.input_micros_per_million_tokens = 1;
        worker.worker.cost.output_micros_per_million_tokens = 1;
        let mut input = request(vec![worker]);
        input.task.estimated_input_tokens = 1;
        input.task.estimated_output_tokens = 1;

        let result = quote(&input).expect("valid quote");
        let cost = &result.eligible_candidates[0].cost;

        assert_eq!(cost.estimated_input_tokens, 1);
        assert_eq!(cost.estimated_output_tokens, 1);
        assert_eq!(cost.input_micros_per_million_tokens, 1);
        assert_eq!(cost.output_micros_per_million_tokens, 1);
        assert_eq!(cost.input_token_cash_micros, 1);
        assert_eq!(cost.output_token_cash_micros, 1);
        assert_eq!(cost.fixed_request_cash_micros, 3);
        assert_eq!(cost.run_cash_micros, 5);
    }

    #[test]
    fn token_cost_overflow_is_a_structured_error() {
        let mut worker = candidate("overflow", 0, 0.9, 0.8);
        worker.worker.context_window_tokens = u64::MAX;
        worker.worker.cost.input_micros_per_million_tokens = u64::MAX;
        let mut input = request(vec![worker]);
        input.task.estimated_input_tokens = u64::MAX;
        input.task.estimated_output_tokens = 0;

        assert_eq!(
            quote(&input),
            Err(EngineError::CostOverflow {
                worker_id: "worker:overflow".into(),
                component: "input_token_cash_micros",
            })
        );
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
                    minimum_evidence_count: 0,
                }],
                required_tools: BTreeSet::from(["shell".to_owned()]),
                allowed_providers: BTreeSet::new(),
                privacy: PrivacyClass::Public,
                risk: RiskLevel::Low,
                verification: VerificationPolicy::Deterministic,
                minimum_success_probability: 0.6,
                minimum_evidence_count: 0,
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
                authorized_checker_worker_ids: BTreeSet::new(),
                failure_probability_basis: FailureProbabilityBasis::Mean,
                max_attempts: 2,
            },
            candidates,
        }
    }

    fn candidate(
        name: &str,
        fixed_request_cash_micros: u64,
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
                    fixed_request_micros: fixed_request_cash_micros,
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
            expected_tool_cash_micros: 0,
            expected_review_cash_micros: 0,
            expected_fallback_cash_micros: 0,
            expected_additional_quota_milliunits: 0,
            checker_worker_id: None,
            p95_latency_ms: 100,
            evidence_snapshot_id: "snapshot:1".to_owned(),
        }
    }
}
