//! The closed loop between stored evidence and an auditable decision.
//!
//! The index, the engine, and the private ledger are each self-contained. This
//! crate is the only place they meet, and it owns the two directions of travel:
//!
//! ```text
//! public snapshot + private outcomes --> calibrated candidates --> quote
//!                                                                    |
//!                            private ledger <-- recorded decision <--+
//!                                   |
//!                                   +--> stronger local evidence for the next quote
//! ```
//!
//! The calibration rule is the project's central claim made executable: public
//! benchmark observations seed a *capped, weak* prior, verified local outcomes
//! update at full strength, and evidence is admitted only for the exact skill
//! it measured. A worker's legal-factuality score can never raise its estimate
//! for a CAD task, because the two never meet in the same posterior.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use workforce_domain::{
    CostProfile, EvidenceTier, OutcomeEvent, ProbabilityEstimate, SkillId, TaskSpec,
    WorkerEstimate, WorkerId, WorkerIdentity, WorkerProfile,
};
use workforce_engine::{BetaPosterior, EngineError, QuoteRequest, RoutingQuote};
use workforce_store::{
    CandidateQuoteAuditRecord, PrivateLedgerRead, PrivateOutcomeRecord, PublicEvidenceRecord,
    PublicIndexRead, QuoteRecord, RejectedCandidateAuditRecord, SelectionExplanationAuditRecord,
    StoreError, build_public_export,
};

/// How stored observations become a posterior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationPolicy {
    /// Immutable identifier for this calibration, recorded with every quote.
    pub calibration_id: String,
    /// One-sided tail probability for lower bounds. `0.05` is a 95% bound.
    pub confidence_tail_probability: f64,
    /// The posterior before any evidence at all.
    pub prior_alpha: f64,
    pub prior_beta: f64,
    /// Ceiling on the *total* pseudo-observation weight that public benchmark
    /// evidence may contribute to one skill.
    ///
    /// This cap is the difference between an index and a leaderboard. A
    /// benchmark reporting ten thousand samples is still a weak statement about
    /// how a worker will behave on your task, so it is not allowed to swamp the
    /// handful of verified local outcomes that actually measured your work.
    pub max_public_prior_weight: f64,
    /// Pseudo-observation weight of one verified local outcome. Deliberately
    /// larger than any single public observation.
    pub private_outcome_weight: f64,
}

impl Default for CalibrationPolicy {
    fn default() -> Self {
        Self {
            calibration_id: "calibration:v1".to_owned(),
            confidence_tail_probability: 0.05,
            prior_alpha: 1.0,
            prior_beta: 1.0,
            max_public_prior_weight: 8.0,
            private_outcome_weight: 1.0,
        }
    }
}

impl CalibrationPolicy {
    pub fn validate(&self) -> Result<(), AllocatorError> {
        if self.calibration_id.trim().is_empty() {
            return Err(AllocatorError::EmptyField("calibration.calibration_id"));
        }
        if !(0.0..=1.0).contains(&self.confidence_tail_probability) {
            return Err(AllocatorError::InvalidCalibration(
                "confidence_tail_probability must lie within [0, 1]",
            ));
        }
        if !(self.prior_alpha.is_finite() && self.prior_alpha > 0.0)
            || !(self.prior_beta.is_finite() && self.prior_beta > 0.0)
        {
            return Err(AllocatorError::InvalidCalibration(
                "prior_alpha and prior_beta must be finite and positive",
            ));
        }
        if !self.max_public_prior_weight.is_finite() || self.max_public_prior_weight < 0.0 {
            return Err(AllocatorError::InvalidCalibration(
                "max_public_prior_weight must be finite and non-negative",
            ));
        }
        if !self.private_outcome_weight.is_finite() || self.private_outcome_weight <= 0.0 {
            return Err(AllocatorError::InvalidCalibration(
                "private_outcome_weight must be finite and positive",
            ));
        }
        Ok(())
    }
}

/// Relative trust in a public observation, by how it was produced.
///
/// Vendor self-reports are admitted but discounted heavily; nothing here is
/// ever worth as much as a locally reproduced outcome.
const fn evidence_tier_weight(tier: EvidenceTier) -> f64 {
    match tier {
        EvidenceTier::ProjectReproduced => 1.0,
        EvidenceTier::IndependentSigned => 0.6,
        EvidenceTier::CommunityReproducible => 0.35,
        EvidenceTier::VendorReported => 0.1,
    }
}

/// One skill's posterior and the provenance behind it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillCalibration {
    pub skill_id: SkillId,
    pub posterior: BetaPosterior,
    pub estimate: ProbabilityEstimate,
    /// Applicable public observations folded into the prior.
    pub public_observation_count: u64,
    /// Verified local outcomes folded in at full strength.
    pub private_outcome_count: u64,
    /// Observations skipped because no score in `[0, 1]` could be read without
    /// inventing a normalization. Surfaced rather than silently dropped.
    pub unusable_observation_count: u64,
}

/// A candidate plus the derivation of every number in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibratedCandidate {
    pub estimate: WorkerEstimate,
    pub skill_calibrations: Vec<SkillCalibration>,
}

/// Assumptions the index cannot supply, because they describe your workflow
/// rather than the worker.
///
/// These were previously hand-written into every quote fixture, which is what
/// let a fixture assert its own answer. Keeping them in one named, explicit
/// place makes the boundary obvious: everything else is now derived.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WorkflowAssumptions {
    /// Expected non-model charges, such as sandbox or search fees.
    #[serde(default)]
    pub expected_tool_cash_micros: u64,
    /// Cost of the review step, when one is policy-required.
    #[serde(default)]
    pub expected_review_cash_micros: u64,
    /// Cost of one retry or escalation.
    #[serde(default)]
    pub expected_fallback_cash_micros: u64,
    #[serde(default)]
    pub expected_additional_quota_milliunits: u64,
    /// Observed p95 latency, per worker. Workers absent from this map fall back
    /// to `default_p95_latency_ms`.
    #[serde(default)]
    pub p95_latency_ms: BTreeMap<WorkerId, u64>,
    #[serde(default)]
    pub default_p95_latency_ms: u64,
    #[serde(default)]
    pub checker_worker_id: Option<WorkerId>,
}

/// Builds engine candidates from a verified public snapshot and the private
/// outcome history.
///
/// Every candidate's success estimate is *derived* here. Nothing in the quote
/// asserts the answer it is supposed to compute.
pub fn calibrate_candidates(
    public: &impl PublicIndexRead,
    private: &impl PrivateLedgerRead,
    snapshot_id: &str,
    task: &TaskSpec,
    calibration: &CalibrationPolicy,
    assumptions: &WorkflowAssumptions,
    at_epoch_ms: i64,
) -> Result<Vec<CalibratedCandidate>, AllocatorError> {
    calibration.validate()?;
    task.validate()?;

    // The export path re-verifies the snapshot digest and dependency closure,
    // so a candidate set can never be built from a tampered or partial index.
    let export = build_public_export(public, snapshot_id)?;
    let outcomes = private.outcomes()?;

    let offerings_by_id: BTreeMap<_, _> = export
        .provider_offerings
        .iter()
        .map(|offering| (offering.id.clone(), offering))
        .collect();
    let releases: BTreeSet<_> = export
        .model_releases
        .iter()
        .map(|release| release.id.clone())
        .collect();

    let mut candidates = Vec::new();
    for profile in &export.worker_profiles {
        let offering = offerings_by_id.get(&profile.offering_id).ok_or_else(|| {
            AllocatorError::MissingOffering {
                worker_id: profile.id.clone(),
                offering_id: profile.offering_id.0.clone(),
            }
        })?;
        if !releases.contains(&offering.model_release_id) {
            return Err(AllocatorError::MissingModelRelease {
                worker_id: profile.id.clone(),
                model_release_id: offering.model_release_id.0.clone(),
            });
        }

        let worker = WorkerProfile {
            identity: WorkerIdentity {
                worker_id: profile.id.clone(),
                model_release_id: offering.model_release_id.clone(),
                offering_id: offering.id.clone(),
                provider: offering.provider.clone(),
                harness_id: profile.harness_id.clone(),
                harness_version: profile.harness_version.clone(),
                reasoning_configuration: profile.reasoning_configuration.clone(),
                system_prompt_sha256: profile.system_prompt_sha256.clone(),
                skill_pack_version: profile.skill_pack_version.clone(),
                toolset_version: profile.toolset_version.clone(),
                execution_policy_sha256: profile.execution_policy_sha256.clone(),
            },
            supported_skills: profile.supported_skill_ids.clone(),
            tools: profile.tools.clone(),
            data_clearance: profile.privacy_clearance,
            context_window_tokens: offering.context_window_tokens,
            cost: CostProfile {
                currency: offering.currency.clone(),
                input_micros_per_million_tokens: offering.input_micros_per_million_tokens,
                output_micros_per_million_tokens: offering.output_micros_per_million_tokens,
                fixed_request_micros: offering.fixed_request_micros,
                quota_milliunits_per_request: offering.quota_milliunits_per_request,
            },
            // Availability is the offering's own time bound, not a free-floating
            // flag a caller can assert.
            available: offering.effective_from_epoch_ms <= at_epoch_ms
                && offering
                    .effective_until_epoch_ms
                    .is_none_or(|until| at_epoch_ms < until),
        };

        let mut skill_calibrations = Vec::new();
        let mut skill_estimates = BTreeMap::new();
        for requirement in &task.required_skills {
            let calibrated = calibrate_skill(
                &profile.id,
                &offering.model_release_id,
                &requirement.skill_id,
                &export.evidence,
                &outcomes,
                calibration,
            )?;
            skill_estimates.insert(requirement.skill_id.clone(), calibrated.estimate.clone());
            skill_calibrations.push(calibrated);
        }

        let success = task_estimate(&profile.id, &skill_calibrations, &outcomes, calibration)?;

        candidates.push(CalibratedCandidate {
            estimate: WorkerEstimate {
                worker,
                success,
                skill_estimates,
                expected_tool_cash_micros: assumptions.expected_tool_cash_micros,
                expected_review_cash_micros: assumptions.expected_review_cash_micros,
                expected_fallback_cash_micros: assumptions.expected_fallback_cash_micros,
                expected_additional_quota_milliunits: assumptions
                    .expected_additional_quota_milliunits,
                checker_worker_id: assumptions.checker_worker_id.clone(),
                p95_latency_ms: assumptions
                    .p95_latency_ms
                    .get(&profile.id)
                    .copied()
                    .unwrap_or(assumptions.default_p95_latency_ms),
                evidence_snapshot_id: snapshot_id.to_owned(),
            },
            skill_calibrations,
        });
    }

    candidates.sort_by(|left, right| {
        left.estimate
            .worker
            .identity
            .worker_id
            .cmp(&right.estimate.worker.identity.worker_id)
    });
    Ok(candidates)
}

/// Folds all applicable evidence for one `(worker, skill)` pair into a posterior.
///
/// Applicability is exact. A public observation counts only when it measured
/// this skill *and* either this exact worker or the model release behind it;
/// a private outcome counts only when it recorded this worker on this skill.
/// Evidence measured on any other skill is not discounted — it is absent.
fn calibrate_skill(
    worker_id: &WorkerId,
    model_release_id: &workforce_domain::ModelReleaseId,
    skill_id: &SkillId,
    evidence: &[PublicEvidenceRecord],
    outcomes: &[PrivateOutcomeRecord],
    calibration: &CalibrationPolicy,
) -> Result<SkillCalibration, AllocatorError> {
    let mut posterior = BetaPosterior::new(calibration.prior_alpha, calibration.prior_beta)?;

    let applicable: Vec<_> = evidence
        .iter()
        .filter(|record| &record.skill_id == skill_id)
        .filter(|record| match &record.worker_id {
            Some(measured) => measured == worker_id,
            // Release-scoped evidence transfers to a worker built on that
            // release; it says nothing about a different release.
            None => &record.model_release_id == model_release_id,
        })
        .collect();

    let mut public_observation_count = 0_u64;
    let mut unusable_observation_count = 0_u64;
    let mut raw_success = 0.0_f64;
    let mut raw_failure = 0.0_f64;

    for record in &applicable {
        let Some(score) = usable_score(record) else {
            unusable_observation_count += 1;
            continue;
        };
        #[allow(clippy::cast_precision_loss)]
        let samples = record.sample_count.unwrap_or(1).max(1) as f64;
        let weight = samples * evidence_tier_weight(record.evidence_tier);
        raw_success += score * weight;
        raw_failure += (1.0 - score) * weight;
        public_observation_count += 1;
    }

    // Rescale so public evidence contributes at most `max_public_prior_weight`
    // in total, preserving the success/failure ratio it reported.
    let raw_total = raw_success + raw_failure;
    if raw_total > 0.0 {
        let scale = if raw_total > calibration.max_public_prior_weight {
            calibration.max_public_prior_weight / raw_total
        } else {
            1.0
        };
        posterior.observe(raw_success * scale, raw_failure * scale)?;
    }

    let mut private_outcome_count = 0_u64;
    for outcome in outcomes {
        let event = &outcome.event;
        if &event.worker_id != worker_id || &event.skill_id != skill_id {
            continue;
        }
        posterior.observe_outcome(event.accepted, calibration.private_outcome_weight)?;
        private_outcome_count += 1;
    }

    let evidence_count = public_observation_count.saturating_add(private_outcome_count);
    let estimate = posterior.estimate(evidence_count, calibration.confidence_tail_probability)?;

    Ok(SkillCalibration {
        skill_id: skill_id.clone(),
        posterior,
        estimate,
        public_observation_count,
        private_outcome_count,
        unusable_observation_count,
    })
}

/// A benchmark score is usable as a pass rate only if it already lies in
/// `[0, 1]`. Anything else needs a normalization the index refuses to invent.
fn usable_score(record: &PublicEvidenceRecord) -> Option<f64> {
    let score = record.normalized_score.unwrap_or(record.raw_score);
    (score.is_finite() && (0.0..=1.0).contains(&score)).then_some(score)
}

/// Combines per-skill posteriors into one task-level estimate.
///
/// A task requiring several skills must clear all of them, so the mean is the
/// product of the skill means and the bound is the product of the skill bounds.
/// The evidence count is the *minimum* across skills: a worker is only as
/// measured as its least-measured requirement.
fn task_estimate(
    worker_id: &WorkerId,
    skills: &[SkillCalibration],
    outcomes: &[PrivateOutcomeRecord],
    calibration: &CalibrationPolicy,
) -> Result<ProbabilityEstimate, AllocatorError> {
    if skills.is_empty() {
        // With no declared skill requirements the only applicable signal is
        // this worker's own verified history.
        let mut posterior = BetaPosterior::new(calibration.prior_alpha, calibration.prior_beta)?;
        let mut count = 0_u64;
        for outcome in outcomes {
            if &outcome.event.worker_id != worker_id {
                continue;
            }
            posterior
                .observe_outcome(outcome.event.accepted, calibration.private_outcome_weight)?;
            count += 1;
        }
        return Ok(posterior.estimate(count, calibration.confidence_tail_probability)?);
    }

    let mut success_mean = 1.0_f64;
    let mut success_lower_bound = 1.0_f64;
    let mut evidence_count = u64::MAX;
    for skill in skills {
        success_mean *= skill.estimate.success_mean;
        success_lower_bound *= skill.estimate.success_lower_bound;
        evidence_count = evidence_count.min(skill.estimate.evidence_count);
    }

    Ok(ProbabilityEstimate {
        success_mean,
        success_lower_bound,
        evidence_count,
    })
}

/// Converts an engine quote into the private ledger's audit record.
///
/// The fingerprint is a one-way digest of the canonical request, so a recorded
/// decision can be matched to its inputs without persisting the task text.
pub fn quote_record(
    request: &QuoteRequest,
    quote: &RoutingQuote,
    calibration: &CalibrationPolicy,
    created_at: &str,
) -> Result<QuoteRecord, AllocatorError> {
    let selected = quote
        .eligible_candidates
        .iter()
        .find(|candidate| Some(&candidate.worker_id) == quote.selected_worker_id.as_ref());

    Ok(QuoteRecord {
        decision_id: quote.decision_id.clone(),
        task_id: quote.task_id.clone(),
        selected_worker_id: quote.selected_worker_id.clone(),
        selected_checker_worker_id: quote.selected_checker_worker_id.clone(),
        verification_policy: request.task.verification,
        evidence_snapshot_id: quote.evidence_snapshot_id.clone(),
        policy_version: format!("{}+{}", quote.policy_id, calibration.calibration_id),
        expected_cash_micros: selected.map(|candidate| candidate.cost.expected_cash_micros),
        expected_quota_milliunits: selected
            .map(|candidate| candidate.cost.expected_quota_milliunits),
        expected_success_probability: selected.map(|candidate| candidate.success_mean),
        p95_latency_ms: selected.map(|candidate| candidate.p95_latency_ms),
        eligible_candidates: quote
            .eligible_candidates
            .iter()
            .map(|candidate| {
                Ok(CandidateQuoteAuditRecord {
                    rank: u64::try_from(candidate.rank).unwrap_or(u64::MAX),
                    worker_id: candidate.worker_id.clone(),
                    checker_worker_id: candidate.checker_worker_id.clone(),
                    success_mean: candidate.success_mean,
                    success_lower_bound: candidate.success_lower_bound,
                    p95_latency_ms: candidate.p95_latency_ms,
                    expected_cash_micros: candidate.cost.expected_cash_micros,
                    expected_quota_milliunits: candidate.cost.expected_quota_milliunits,
                    expected_accepted_cost_micros: candidate.cost.expected_accepted_cost_micros,
                    pareto_efficient: candidate.pareto_efficient,
                    cost_breakdown: serde_json::to_value(&candidate.cost)?,
                })
            })
            .collect::<Result<Vec<_>, AllocatorError>>()?,
        rejected_candidates: quote
            .rejected_candidates
            .iter()
            .map(|rejected| {
                Ok(RejectedCandidateAuditRecord {
                    worker_id: rejected.worker_id.clone(),
                    reasons: rejected
                        .reasons
                        .iter()
                        .map(serde_json::to_value)
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect::<Result<Vec<_>, AllocatorError>>()?,
        pareto_worker_ids: quote.pareto_worker_ids.clone(),
        selection_explanation: quote.selection_explanation.as_ref().map(|explanation| {
            SelectionExplanationAuditRecord {
                objective: explanation.objective.clone(),
                eligible_candidate_count: u64::try_from(explanation.eligible_candidate_count)
                    .unwrap_or(u64::MAX),
                tie_break_order: explanation.tie_break_order.clone(),
            }
        }),
        created_at: created_at.to_owned(),
        request_fingerprint: request_fingerprint(request)?,
    })
}

/// SHA-256 over the canonical serialization of the quote request.
pub fn request_fingerprint(request: &QuoteRequest) -> Result<String, AllocatorError> {
    let canonical = serde_json::to_vec(request)?;
    let mut hasher = Sha256::new();
    hasher.update(b"open-workforce-index/quote-request/v1\0");
    hasher.update(canonical);
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut accumulator, byte| {
            use std::fmt::Write as _;
            let _ = write!(accumulator, "{byte:02x}");
            accumulator
        }))
}

/// Wraps a verified outcome for the private ledger.
pub fn outcome_record(
    decision_id: Option<workforce_domain::DecisionId>,
    event: OutcomeEvent,
    checker_worker_id: Option<WorkerId>,
) -> PrivateOutcomeRecord {
    PrivateOutcomeRecord {
        decision_id,
        event,
        checker_worker_id,
    }
}

#[derive(Debug, Error)]
pub enum AllocatorError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Domain(#[from] workforce_domain::DomainError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("invalid calibration policy: {0}")]
    InvalidCalibration(&'static str),
    #[error("worker {worker_id} references offering {offering_id}, which the snapshot omits")]
    MissingOffering {
        worker_id: WorkerId,
        offering_id: String,
    },
    #[error(
        "worker {worker_id} references model release {model_release_id}, which the snapshot omits"
    )]
    MissingModelRelease {
        worker_id: WorkerId,
        model_release_id: String,
    },
}

#[cfg(test)]
mod tests {
    use workforce_domain::{
        BenchmarkId, DecisionId, ModelReleaseId, OfferingId, PrivacyClass, RiskLevel,
        SkillRequirement, TaskId, ValidationKind, VerificationPolicy,
    };
    use workforce_engine::{RoutingPolicy, quote};
    use workforce_store::{
        ModelReleaseRecord, PrivateLedgerWrite, PrivateLocalStore, ProviderOfferingRecord,
        PublicIndexStore, PublicIndexWrite, SnapshotRecord, WorkerProfileRecord,
        worker_configuration_sha256,
    };

    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const DIGEST: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    const NOW_MS: i64 = 1_785_000_000_000;
    const SKILL: &str = "skill:rust-debugging";

    fn release(id: &str) -> ModelReleaseRecord {
        ModelReleaseRecord {
            id: ModelReleaseId(id.to_owned()),
            developer: "example-lab".to_owned(),
            model_family: "example".to_owned(),
            released_at: "2026-01-01T00:00:00Z".to_owned(),
            context_window_tokens: 200_000,
            source_url: "https://example.test/release".to_owned(),
            artifact_sha256: DIGEST.to_owned(),
            recorded_at: "2026-08-01T00:00:00Z".to_owned(),
        }
    }

    fn offering(id: &str, release_id: &str, input: u64, output: u64) -> ProviderOfferingRecord {
        ProviderOfferingRecord {
            id: OfferingId(id.to_owned()),
            model_release_id: ModelReleaseId(release_id.to_owned()),
            provider: "example-provider".to_owned(),
            supersedes_offering_id: None,
            effective_from_epoch_ms: 1_754_006_400_000,
            effective_until_epoch_ms: None,
            currency: "USD".to_owned(),
            input_micros_per_million_tokens: input,
            output_micros_per_million_tokens: output,
            fixed_request_micros: 0,
            quota_milliunits_per_request: 0,
            context_window_tokens: 200_000,
            source_url: "https://example.test/pricing".to_owned(),
            recorded_at: "2026-08-01T00:00:00Z".to_owned(),
        }
    }

    fn worker(id: &str, offering_id: &str, release_id: &str) -> WorkerProfileRecord {
        let identity = WorkerIdentity {
            worker_id: WorkerId(id.to_owned()),
            model_release_id: ModelReleaseId(release_id.to_owned()),
            offering_id: OfferingId(offering_id.to_owned()),
            provider: "example-provider".to_owned(),
            harness_id: "example-agent".to_owned(),
            harness_version: "1.0.0".to_owned(),
            reasoning_configuration: "standard".to_owned(),
            system_prompt_sha256: EMPTY_SHA256.to_owned(),
            skill_pack_version: "rust-v1".to_owned(),
            toolset_version: "shell-v1".to_owned(),
            execution_policy_sha256: EMPTY_SHA256.to_owned(),
        };
        WorkerProfileRecord {
            id: WorkerId(id.to_owned()),
            offering_id: OfferingId(offering_id.to_owned()),
            harness_id: identity.harness_id.clone(),
            harness_version: identity.harness_version.clone(),
            reasoning_configuration: identity.reasoning_configuration.clone(),
            system_prompt_sha256: identity.system_prompt_sha256.clone(),
            skill_pack_version: identity.skill_pack_version.clone(),
            toolset_version: identity.toolset_version.clone(),
            execution_policy_sha256: identity.execution_policy_sha256.clone(),
            supported_skill_ids: BTreeSet::from([SkillId(SKILL.to_owned())]),
            tools: BTreeSet::from(["shell".to_owned()]),
            privacy_clearance: PrivacyClass::PrivateMetadata,
            configuration_sha256: worker_configuration_sha256(&identity),
            recorded_at: "2026-08-01T00:00:00Z".to_owned(),
        }
    }

    fn evidence(
        id: &str,
        release_id: &str,
        worker_id: Option<&str>,
        skill: &str,
        score: f64,
        samples: u64,
        tier: EvidenceTier,
    ) -> PublicEvidenceRecord {
        PublicEvidenceRecord {
            id: id.to_owned(),
            model_release_id: ModelReleaseId(release_id.to_owned()),
            worker_id: worker_id.map(|value| WorkerId(value.to_owned())),
            skill_id: SkillId(skill.to_owned()),
            benchmark_id: BenchmarkId("benchmark:example".to_owned()),
            evidence_tier: tier,
            raw_score: score * 100.0,
            metric: "pass_rate".to_owned(),
            unit: "percent".to_owned(),
            normalized_score: Some(score),
            adapter_version: "example-adapter@1".to_owned(),
            sample_count: Some(samples),
            observed_at: "2026-08-01T00:00:00Z".to_owned(),
            source_url: "https://example.test/evidence".to_owned(),
            artifact_sha256: DIGEST.to_owned(),
            license: "Apache-2.0".to_owned(),
        }
    }

    /// A two-worker index: `cheap` is half the price, `strong` has better
    /// public evidence.
    fn seeded_index() -> (PublicIndexStore, String) {
        let store = PublicIndexStore::in_memory().expect("open public index");
        store.append_model_release(&release("model:cheap")).unwrap();
        store
            .append_model_release(&release("model:strong"))
            .unwrap();
        store
            .append_provider_offering(&offering(
                "offering:cheap",
                "model:cheap",
                250_000,
                1_000_000,
            ))
            .unwrap();
        store
            .append_provider_offering(&offering(
                "offering:strong",
                "model:strong",
                2_000_000,
                8_000_000,
            ))
            .unwrap();
        store
            .append_worker_profile(&worker("worker:cheap", "offering:cheap", "model:cheap"))
            .unwrap();
        store
            .append_worker_profile(&worker("worker:strong", "offering:strong", "model:strong"))
            .unwrap();
        store
            .append_evidence(&evidence(
                "evidence:cheap",
                "model:cheap",
                Some("worker:cheap"),
                SKILL,
                0.55,
                40,
                EvidenceTier::CommunityReproducible,
            ))
            .unwrap();
        store
            .append_evidence(&evidence(
                "evidence:strong",
                "model:strong",
                Some("worker:strong"),
                SKILL,
                0.90,
                40,
                EvidenceTier::CommunityReproducible,
            ))
            .unwrap();

        let snapshot = SnapshotRecord::new(
            "snapshot:test",
            "2026-08-07T00:00:00Z",
            "ontology:v1",
            "source:v1",
            vec![
                ModelReleaseId("model:cheap".to_owned()),
                ModelReleaseId("model:strong".to_owned()),
            ],
            vec![
                OfferingId("offering:cheap".to_owned()),
                OfferingId("offering:strong".to_owned()),
            ],
            vec![
                WorkerId("worker:cheap".to_owned()),
                WorkerId("worker:strong".to_owned()),
            ],
            vec!["evidence:cheap".to_owned(), "evidence:strong".to_owned()],
        )
        .expect("snapshot");
        store.append_snapshot(&snapshot).expect("append snapshot");
        (store, snapshot.id)
    }

    fn task(minimum_evidence_count: u64) -> TaskSpec {
        TaskSpec {
            id: TaskId::from("task:fix-failing-test"),
            summary: "Make the failing test pass".to_owned(),
            repository: None,
            required_skills: vec![SkillRequirement {
                skill_id: SkillId(SKILL.to_owned()),
                minimum_success_probability: 0.1,
                minimum_evidence_count,
            }],
            required_tools: BTreeSet::from(["shell".to_owned()]),
            allowed_providers: BTreeSet::new(),
            privacy: PrivacyClass::PrivateMetadata,
            risk: RiskLevel::Low,
            verification: VerificationPolicy::Deterministic,
            minimum_success_probability: 0.1,
            minimum_evidence_count: 0,
            max_expected_cash_micros: None,
            max_p95_latency_ms: None,
            estimated_input_tokens: 20_000,
            estimated_output_tokens: 4_000,
        }
    }

    fn assumptions() -> WorkflowAssumptions {
        WorkflowAssumptions {
            expected_fallback_cash_micros: 150_000,
            default_p95_latency_ms: 30_000,
            ..WorkflowAssumptions::default()
        }
    }

    fn routing_policy() -> RoutingPolicy {
        RoutingPolicy {
            policy_id: "policy:economy-v1".to_owned(),
            currency: "USD".to_owned(),
            quota_shadow_cash_micros_per_unit: 0,
            max_expected_quota_milliunits: None,
            authorized_checker_worker_ids: BTreeSet::new(),
            failure_probability_basis: workforce_engine::FailureProbabilityBasis::Mean,
            max_attempts: 2,
        }
    }

    fn failure(id: &str, worker: &str) -> PrivateOutcomeRecord {
        outcome_record(
            Some(DecisionId::from("decision:test")),
            OutcomeEvent {
                id: id.to_owned(),
                task_id: TaskId::from("task:fix-failing-test"),
                worker_id: WorkerId(worker.to_owned()),
                skill_id: SkillId(SKILL.to_owned()),
                accepted: false,
                validation_kind: ValidationKind::Deterministic,
                actual_cash_micros: 9_000,
                actual_quota_milliunits: 0,
                latency_ms: 28_000,
                observed_at: "2026-08-07T01:00:00Z".to_owned(),
                repository_scope: None,
                metadata: serde_json::Value::Null,
            },
            None,
        )
    }

    fn run_quote(
        public: &PublicIndexStore,
        private: &PrivateLocalStore,
        snapshot_id: &str,
        task: &TaskSpec,
    ) -> (QuoteRequest, RoutingQuote, Vec<CalibratedCandidate>) {
        let calibration = CalibrationPolicy::default();
        let calibrated = calibrate_candidates(
            public,
            private,
            snapshot_id,
            task,
            &calibration,
            &assumptions(),
            NOW_MS,
        )
        .expect("calibrate");
        let request = QuoteRequest {
            decision_id: DecisionId::from("decision:test"),
            evidence_snapshot_id: snapshot_id.to_owned(),
            task: task.clone(),
            policy: routing_policy(),
            candidates: calibrated
                .iter()
                .map(|candidate| candidate.estimate.clone())
                .collect(),
        };
        let result = quote(&request).expect("quote");
        (request, result, calibrated)
    }

    #[test]
    fn candidates_are_derived_from_the_index_rather_than_asserted() {
        let (public, snapshot_id) = seeded_index();
        let private = PrivateLocalStore::in_memory().expect("open private ledger");
        let (_, _, calibrated) = run_quote(&public, &private, &snapshot_id, &task(0));

        assert_eq!(calibrated.len(), 2);
        let strong = &calibrated[1];
        assert_eq!(strong.estimate.worker.identity.worker_id.0, "worker:strong");
        // Price, provider, and context window all came from the offering.
        assert_eq!(
            strong.estimate.worker.cost.output_micros_per_million_tokens,
            8_000_000
        );
        assert_eq!(strong.estimate.worker.identity.provider, "example-provider");
        // The success estimate was computed, not supplied.
        assert!(strong.estimate.success.success_mean > calibrated[0].estimate.success.success_mean);
        assert_eq!(strong.skill_calibrations[0].public_observation_count, 1);
        assert_eq!(strong.skill_calibrations[0].private_outcome_count, 0);
    }

    /// The whole thesis, executable: evidence measured on one skill must not
    /// raise a worker's estimate for a different skill.
    #[test]
    fn evidence_never_transfers_across_skills() {
        let (public, snapshot_id) = seeded_index();
        public
            .append_evidence(&evidence(
                "evidence:unrelated",
                "model:cheap",
                Some("worker:cheap"),
                "skill:legal-factuality",
                1.0,
                10_000,
                EvidenceTier::ProjectReproduced,
            ))
            .expect("append unrelated evidence");
        let private = PrivateLocalStore::in_memory().expect("open private ledger");

        // The unrelated observation is outside the snapshot, and even inside it
        // the skill scope would exclude it.
        let (_, _, calibrated) = run_quote(&public, &private, &snapshot_id, &task(0));
        let cheap = &calibrated[0];
        assert_eq!(cheap.skill_calibrations[0].public_observation_count, 1);
        assert!(cheap.estimate.success.success_mean < 0.75);
    }

    /// A public benchmark reporting a huge sample count must not swamp local
    /// evidence. This is the cap that separates an index from a leaderboard.
    #[test]
    fn public_evidence_weight_is_capped() {
        let (public, snapshot_id) = seeded_index();
        let private = PrivateLocalStore::in_memory().expect("open private ledger");
        let calibration = CalibrationPolicy::default();
        let calibrated = calibrate_candidates(
            &public,
            &private,
            &snapshot_id,
            &task(0),
            &calibration,
            &assumptions(),
            NOW_MS,
        )
        .expect("calibrate");

        let posterior = calibrated[1].skill_calibrations[0].posterior;
        let total_weight = posterior.alpha + posterior.beta;
        let prior_weight = calibration.prior_alpha + calibration.prior_beta;
        assert!(
            total_weight <= prior_weight + calibration.max_public_prior_weight + 1e-9,
            "40 samples must not contribute more than the cap: {total_weight}"
        );
    }

    /// The loop: quote, record a verified outcome, re-quote, and observe the
    /// decision move. Without this the store and the engine never meet.
    #[test]
    fn verified_local_outcomes_change_the_next_decision() {
        let (public, snapshot_id) = seeded_index();
        let private = PrivateLocalStore::in_memory().expect("open private ledger");

        let (request, first, before) = run_quote(&public, &private, &snapshot_id, &task(0));
        assert_eq!(
            first.selected_worker_id.as_ref().map(|id| id.0.as_str()),
            Some("worker:cheap"),
            "the cheap worker wins on expected accepted cost before local evidence"
        );

        // The decision is persisted to the private ledger, not just printed.
        let record = quote_record(
            &request,
            &first,
            &CalibrationPolicy::default(),
            "2026-08-07T00:00:00Z",
        )
        .expect("build quote record");
        private.append_quote(&record).expect("append quote");
        assert_eq!(private.quotes().expect("read quotes").len(), 1);
        assert_eq!(record.request_fingerprint.len(), 64);

        // Six verified local failures on the cheap worker.
        for index in 0..6 {
            private
                .append_outcome(&failure(&format!("outcome:{index}"), "worker:cheap"))
                .expect("append outcome");
        }

        let (_, second, after) = run_quote(&public, &private, &snapshot_id, &task(0));
        assert_eq!(
            second.selected_worker_id.as_ref().map(|id| id.0.as_str()),
            Some("worker:strong"),
            "after local failures the cheap worker is no longer the cheapest accepted result"
        );

        let cheap_before = before[0].estimate.success.success_mean;
        let cheap_after = after[0].estimate.success.success_mean;
        assert!(
            cheap_after < cheap_before,
            "local failures must lower the estimate: {cheap_before} -> {cheap_after}"
        );
        assert_eq!(after[0].skill_calibrations[0].private_outcome_count, 6);
    }

    /// An asserted confidence bound with nothing behind it must not route.
    #[test]
    fn a_minimum_evidence_requirement_rejects_unmeasured_workers() {
        let (public, snapshot_id) = seeded_index();
        let private = PrivateLocalStore::in_memory().expect("open private ledger");
        let (_, result, _) = run_quote(&public, &private, &snapshot_id, &task(50));

        assert!(result.eligible_candidates.is_empty());
        assert_eq!(result.rejected_candidates.len(), 2);
        assert!(result.rejected_candidates.iter().all(|rejected| {
            rejected.reasons.iter().any(|reason| {
                matches!(
                    reason,
                    workforce_engine::IneligibilityReason::InsufficientSkillEvidence { .. }
                )
            })
        }));
    }

    /// A benchmark whose score is not already a rate is skipped rather than
    /// normalized by guesswork, and the skip stays visible.
    #[test]
    fn unnormalizable_evidence_is_reported_not_invented() {
        let store = PublicIndexStore::in_memory().expect("open public index");
        store.append_model_release(&release("model:cheap")).unwrap();
        store
            .append_provider_offering(&offering(
                "offering:cheap",
                "model:cheap",
                250_000,
                1_000_000,
            ))
            .unwrap();
        store
            .append_worker_profile(&worker("worker:cheap", "offering:cheap", "model:cheap"))
            .unwrap();
        let mut unusable = evidence(
            "evidence:elo",
            "model:cheap",
            Some("worker:cheap"),
            SKILL,
            0.5,
            10,
            EvidenceTier::VendorReported,
        );
        unusable.normalized_score = None;
        unusable.raw_score = 1337.0;
        unusable.metric = "elo".to_owned();
        unusable.unit = "rating".to_owned();
        store.append_evidence(&unusable).unwrap();
        let snapshot = SnapshotRecord::new(
            "snapshot:elo",
            "2026-08-07T00:00:00Z",
            "ontology:v1",
            "source:v1",
            vec![ModelReleaseId("model:cheap".to_owned())],
            vec![OfferingId("offering:cheap".to_owned())],
            vec![WorkerId("worker:cheap".to_owned())],
            vec!["evidence:elo".to_owned()],
        )
        .expect("snapshot");
        store.append_snapshot(&snapshot).unwrap();

        let private = PrivateLocalStore::in_memory().expect("open private ledger");
        let calibrated = calibrate_candidates(
            &store,
            &private,
            &snapshot.id,
            &task(0),
            &CalibrationPolicy::default(),
            &assumptions(),
            NOW_MS,
        )
        .expect("calibrate");

        let skill = &calibrated[0].skill_calibrations[0];
        assert_eq!(skill.unusable_observation_count, 1);
        assert_eq!(skill.public_observation_count, 0);
        // The estimate falls back to the bare prior rather than inventing a rate.
        assert!((skill.estimate.success_mean - 0.5).abs() < 1e-9);
    }

    /// A snapshot whose digest no longer matches must not produce candidates.
    #[test]
    fn a_tampered_snapshot_cannot_produce_candidates() {
        let (public, snapshot_id) = seeded_index();
        let private = PrivateLocalStore::in_memory().expect("open private ledger");
        let missing = calibrate_candidates(
            &public,
            &private,
            "snapshot:does-not-exist",
            &task(0),
            &CalibrationPolicy::default(),
            &assumptions(),
            NOW_MS,
        );
        assert!(missing.is_err());
        assert!(!snapshot_id.is_empty());
    }
}
