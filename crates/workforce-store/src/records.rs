//! The record types: what the index stores, exactly as stored.

use std::collections::BTreeSet;

use crate::StoreError;
use crate::hash_component;
use crate::hash_id_list;
use crate::lower_hex;
use crate::validate_canonical_identifier;
use crate::validate_manifest_list;
use crate::validate_sha256;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use workforce_domain::{
    BenchmarkId, DecisionId, EvidenceTier, ModelReleaseId, OfferingId, OutcomeEvent, PrivacyClass,
    SkillId, TaskId, VerificationPolicy, WorkerId,
};

/// A public, immutable description of a concrete model release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelReleaseRecord {
    pub id: ModelReleaseId,
    pub developer: String,
    pub model_family: String,
    pub released_at: String,
    pub context_window_tokens: u64,
    pub source_url: String,
    pub artifact_sha256: String,
    pub recorded_at: String,
}

/// A time-bounded, provider-specific price and context-window offering.
///
/// Mutable aliases such as `latest` are intentionally not accepted as release
/// identities. A price change is represented by appending a new offering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderOfferingRecord {
    pub id: OfferingId,
    pub model_release_id: ModelReleaseId,
    pub provider: String,
    /// The immediately preceding immutable revision, when this is a revision.
    #[serde(default)]
    pub supersedes_offering_id: Option<OfferingId>,
    /// Inclusive UTC Unix epoch boundary in milliseconds.
    pub effective_from_epoch_ms: i64,
    /// Exclusive UTC Unix epoch boundary in milliseconds.
    #[serde(default)]
    pub effective_until_epoch_ms: Option<i64>,
    pub currency: String,
    pub input_micros_per_million_tokens: u64,
    pub output_micros_per_million_tokens: u64,
    pub fixed_request_micros: u64,
    /// Provider subscription/rate-limit consumption, separate from cash.
    #[serde(default)]
    pub quota_milliunits_per_request: u64,
    pub context_window_tokens: u64,
    pub source_url: String,
    pub recorded_at: String,
}

/// The exact configuration that turns an offering into a measurable worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerProfileRecord {
    pub id: WorkerId,
    pub offering_id: OfferingId,
    pub harness_id: String,
    pub harness_version: String,
    pub reasoning_configuration: String,
    pub system_prompt_sha256: String,
    pub skill_pack_version: String,
    pub toolset_version: String,
    pub execution_policy_sha256: String,
    /// Capability assertion used for routing; identity binds the skill-pack
    /// version rather than serializing this mutable authorization view.
    #[serde(default)]
    pub supported_skill_ids: BTreeSet<SkillId>,
    /// Capability assertion used for routing; identity binds `toolset_version`.
    #[serde(default)]
    pub tools: BTreeSet<String>,
    /// Authorization assertion used for eligibility; execution permissions are
    /// bound through `execution_policy_sha256`.
    pub privacy_clearance: PrivacyClass,
    /// SHA-256 over the domain worker identity's canonical configuration key.
    pub configuration_sha256: String,
    pub recorded_at: String,
}

/// Public evidence tied to a concrete release and, when known, an exact worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicEvidenceRecord {
    pub id: String,
    pub model_release_id: ModelReleaseId,
    /// Exact measured worker when the source identifies its full configuration.
    #[serde(default)]
    pub worker_id: Option<WorkerId>,
    pub skill_id: SkillId,
    pub benchmark_id: BenchmarkId,
    pub evidence_tier: EvidenceTier,
    /// Score exactly as reported by the benchmark source.
    pub raw_score: f64,
    pub metric: String,
    pub unit: String,
    /// Optional explicit normalization; never inferred by the store.
    #[serde(default)]
    pub normalized_score: Option<f64>,
    /// Version of the importer/normalizer that produced this observation.
    pub adapter_version: String,
    /// Number of benchmark samples, when the source reports it.
    #[serde(default)]
    pub sample_count: Option<u64>,
    pub observed_at: String,
    pub source_url: String,
    pub artifact_sha256: String,
    pub license: String,
}

/// Manifest for a reproducible public-index snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRecord {
    pub id: String,
    pub created_at: String,
    pub ontology_version: String,
    pub source_revision: String,
    pub content_sha256: String,
    /// Sorted, duplicate-free identifiers included in this immutable snapshot.
    #[serde(default)]
    pub model_release_ids: Vec<ModelReleaseId>,
    /// Sorted, duplicate-free identifiers included in this immutable snapshot.
    #[serde(default)]
    pub provider_offering_ids: Vec<OfferingId>,
    /// Sorted, duplicate-free identifiers included in this immutable snapshot.
    #[serde(default)]
    pub worker_profile_ids: Vec<WorkerId>,
    /// Sorted, duplicate-free identifiers included in this immutable snapshot.
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    pub model_release_count: u64,
    pub provider_offering_count: u64,
    pub worker_profile_count: u64,
    pub evidence_count: u64,
}

impl SnapshotRecord {
    /// Constructs a canonical manifest and calculates its deterministic digest.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        created_at: impl Into<String>,
        ontology_version: impl Into<String>,
        source_revision: impl Into<String>,
        mut model_release_ids: Vec<ModelReleaseId>,
        mut provider_offering_ids: Vec<OfferingId>,
        mut worker_profile_ids: Vec<WorkerId>,
        mut evidence_ids: Vec<String>,
    ) -> Result<Self, StoreError> {
        model_release_ids.sort();
        provider_offering_ids.sort();
        worker_profile_ids.sort();
        evidence_ids.sort();

        let mut snapshot = Self {
            id: id.into(),
            created_at: created_at.into(),
            ontology_version: ontology_version.into(),
            source_revision: source_revision.into(),
            content_sha256: String::new(),
            model_release_count: u64::try_from(model_release_ids.len()).map_err(|_| {
                StoreError::IntegerOutOfRange {
                    field: "model_release_count",
                }
            })?,
            provider_offering_count: u64::try_from(provider_offering_ids.len()).map_err(|_| {
                StoreError::IntegerOutOfRange {
                    field: "provider_offering_count",
                }
            })?,
            worker_profile_count: u64::try_from(worker_profile_ids.len()).map_err(|_| {
                StoreError::IntegerOutOfRange {
                    field: "worker_profile_count",
                }
            })?,
            evidence_count: u64::try_from(evidence_ids.len()).map_err(|_| {
                StoreError::IntegerOutOfRange {
                    field: "evidence_count",
                }
            })?,
            model_release_ids,
            provider_offering_ids,
            worker_profile_ids,
            evidence_ids,
        };
        snapshot.validate_manifest_shape()?;
        snapshot.content_sha256 = snapshot.calculate_content_sha256()?;
        Ok(snapshot)
    }

    /// Recomputes the digest over versioned, length-prefixed manifest material.
    pub fn calculate_content_sha256(&self) -> Result<String, StoreError> {
        self.validate_manifest_shape()?;
        let mut hasher = Sha256::new();
        hasher.update(b"open-workforce-index/snapshot-manifest/v1\0");
        hash_component(&mut hasher, &self.ontology_version);
        hash_component(&mut hasher, &self.source_revision);
        hash_id_list(
            &mut hasher,
            "model_releases",
            self.model_release_count,
            self.model_release_ids.iter().map(|id| id.0.as_str()),
        );
        hash_id_list(
            &mut hasher,
            "provider_offerings",
            self.provider_offering_count,
            self.provider_offering_ids.iter().map(|id| id.0.as_str()),
        );
        hash_id_list(
            &mut hasher,
            "worker_profiles",
            self.worker_profile_count,
            self.worker_profile_ids.iter().map(|id| id.0.as_str()),
        );
        hash_id_list(
            &mut hasher,
            "evidence",
            self.evidence_count,
            self.evidence_ids.iter().map(String::as_str),
        );
        Ok(lower_hex(&hasher.finalize()))
    }

    /// Validates canonical ordering, uniqueness, counts, and digest.
    pub fn validate(&self) -> Result<(), StoreError> {
        self.validate_manifest_shape()?;
        validate_sha256("snapshot.content_sha256", &self.content_sha256)?;
        let calculated = self.calculate_content_sha256()?;
        if calculated == self.content_sha256 {
            Ok(())
        } else {
            Err(StoreError::SnapshotDigestMismatch {
                snapshot_id: self.id.clone(),
                expected: calculated,
                actual: self.content_sha256.clone(),
            })
        }
    }

    fn validate_manifest_shape(&self) -> Result<(), StoreError> {
        validate_canonical_identifier("snapshot.id", &self.id)?;
        if self.ontology_version.trim().is_empty() || self.source_revision.trim().is_empty() {
            return Err(StoreError::InvalidSnapshotManifest {
                snapshot_id: self.id.clone(),
                reason: "ontology version and source revision must be non-empty".to_owned(),
            });
        }
        validate_manifest_list(
            &self.id,
            "model_release_ids",
            self.model_release_count,
            &self.model_release_ids,
        )?;
        validate_manifest_list(
            &self.id,
            "provider_offering_ids",
            self.provider_offering_count,
            &self.provider_offering_ids,
        )?;
        validate_manifest_list(
            &self.id,
            "worker_profile_ids",
            self.worker_profile_count,
            &self.worker_profile_ids,
        )?;
        validate_manifest_list(
            &self.id,
            "evidence_ids",
            self.evidence_count,
            &self.evidence_ids,
        )
    }
}

/// One eligible candidate preserved in a private routing-decision audit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateQuoteAuditRecord {
    pub rank: u64,
    pub worker_id: WorkerId,
    #[serde(default)]
    pub checker_worker_id: Option<WorkerId>,
    pub success_mean: f64,
    pub success_lower_bound: f64,
    pub p95_latency_ms: u64,
    pub expected_cash_micros: u64,
    pub expected_quota_milliunits: u64,
    pub expected_accepted_cost_micros: u64,
    pub pareto_efficient: bool,
    /// Full versioned cost decomposition produced by the engine.
    pub cost_breakdown: serde_json::Value,
}

/// One ineligible candidate and all failed hard constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RejectedCandidateAuditRecord {
    pub worker_id: WorkerId,
    /// Structured, internally tagged reason values produced by the engine.
    pub reasons: Vec<serde_json::Value>,
}

/// Objective and tie-break facts that explain the winning candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionExplanationAuditRecord {
    pub objective: String,
    pub eligible_candidate_count: u64,
    pub tie_break_order: Vec<String>,
}

/// A private allocator quote. No prompt or repository content is persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteRecord {
    pub decision_id: DecisionId,
    pub task_id: TaskId,
    #[serde(default)]
    pub selected_worker_id: Option<WorkerId>,
    #[serde(default)]
    pub selected_checker_worker_id: Option<WorkerId>,
    pub verification_policy: VerificationPolicy,
    pub evidence_snapshot_id: String,
    pub policy_version: String,
    #[serde(default)]
    pub expected_cash_micros: Option<u64>,
    #[serde(default)]
    pub expected_quota_milliunits: Option<u64>,
    #[serde(default)]
    pub expected_success_probability: Option<f64>,
    #[serde(default)]
    pub p95_latency_ms: Option<u64>,
    pub eligible_candidates: Vec<CandidateQuoteAuditRecord>,
    #[serde(default)]
    pub rejected_candidates: Vec<RejectedCandidateAuditRecord>,
    #[serde(default)]
    pub pareto_worker_ids: Vec<WorkerId>,
    #[serde(default)]
    pub selection_explanation: Option<SelectionExplanationAuditRecord>,
    pub created_at: String,
    /// A one-way digest of the request fields used for the quote.
    pub request_fingerprint: String,
}

/// A private outcome linked to its quote when a quote was recorded locally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrivateOutcomeRecord {
    #[serde(default)]
    pub decision_id: Option<DecisionId>,
    pub event: OutcomeEvent,
    /// The checker must differ from the worker that produced the result.
    #[serde(default)]
    pub checker_worker_id: Option<WorkerId>,
}
