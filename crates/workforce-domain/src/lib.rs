//! Stable, provider-neutral contracts shared by the index and allocator.
//!
//! A worker is deliberately more specific than a model. It identifies the exact
//! release, commercial offering, harness, prompt, reasoning configuration, skill
//! pack, toolset, and execution policy that produced the measured behaviour.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn is_empty(&self) -> bool {
                self.0.trim().is_empty()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

string_id!(ModelReleaseId);
string_id!(OfferingId);
string_id!(WorkerId);
string_id!(SkillId);
string_id!(TaskId);
string_id!(BenchmarkId);
string_id!(DecisionId);

/// Sensitivity of data supplied to a worker.
///
/// The ordering is intentional: a worker clearance permits a task only when it
/// is greater than or equal to the task's classification. `Secret` should be
/// reserved for local, explicitly approved execution environments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyClass {
    Public,
    PrivateMetadata,
    ConfidentialContent,
    Secret,
}

impl PrivacyClass {
    pub fn permits(self, required: Self) -> bool {
        self >= required
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Consequential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTier {
    ProjectReproduced,
    IndependentSigned,
    CommunityReproducible,
    VendorReported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationKind {
    Deterministic,
    Human,
    IndependentModel,
    SelfReported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationPolicy {
    Deterministic,
    MakerChecker,
    HumanApproval,
}

impl RiskLevel {
    /// The weakest verification policy that is acceptable for this risk level.
    pub const fn minimum_verification(self) -> VerificationPolicy {
        match self {
            Self::Low | Self::Medium => VerificationPolicy::Deterministic,
            Self::High => VerificationPolicy::MakerChecker,
            Self::Consequential => VerificationPolicy::HumanApproval,
        }
    }
}

impl VerificationPolicy {
    /// Whether this policy meets the minimum control required for `risk`.
    pub const fn permits(self, risk: RiskLevel) -> bool {
        match risk {
            RiskLevel::Low | RiskLevel::Medium => true,
            RiskLevel::High => matches!(self, Self::MakerChecker | Self::HumanApproval),
            RiskLevel::Consequential => matches!(self, Self::HumanApproval),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillRequirement {
    pub skill_id: SkillId,
    pub minimum_success_probability: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub id: TaskId,
    /// A local description. Public index exports must never include this value.
    pub summary: String,
    /// A local opaque repository reference. It is not public-index data.
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub required_skills: Vec<SkillRequirement>,
    #[serde(default)]
    pub required_tools: BTreeSet<String>,
    /// Exact, case-sensitive canonical provider identifiers. Empty means every
    /// provider is eligible; blank or whitespace-padded identifiers are invalid.
    #[serde(default)]
    pub allowed_providers: BTreeSet<String>,
    pub privacy: PrivacyClass,
    pub risk: RiskLevel,
    pub verification: VerificationPolicy,
    pub minimum_success_probability: f64,
    #[serde(default)]
    pub max_expected_cash_micros: Option<u64>,
    #[serde(default)]
    pub max_p95_latency_ms: Option<u64>,
    #[serde(default)]
    pub estimated_input_tokens: u64,
    #[serde(default)]
    pub estimated_output_tokens: u64,
}

impl TaskSpec {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.id.is_empty() {
            return Err(DomainError::EmptyField("task.id"));
        }
        if self.summary.trim().is_empty() {
            return Err(DomainError::EmptyField("task.summary"));
        }
        validate_probability(
            "task.minimum_success_probability",
            self.minimum_success_probability,
        )?;
        if !self.verification.permits(self.risk) {
            return Err(DomainError::InsufficientVerification {
                risk: self.risk,
                required: self.risk.minimum_verification(),
                actual: self.verification,
            });
        }

        let mut skills = BTreeSet::new();
        for requirement in &self.required_skills {
            if requirement.skill_id.is_empty() {
                return Err(DomainError::EmptyField("task.required_skills.skill_id"));
            }
            validate_probability(
                "task.required_skills.minimum_success_probability",
                requirement.minimum_success_probability,
            )?;
            if !skills.insert(requirement.skill_id.clone()) {
                return Err(DomainError::DuplicateSkill(requirement.skill_id.clone()));
            }
        }

        for provider in &self.allowed_providers {
            validate_provider_identifier("task.allowed_providers", provider)?;
        }

        self.required_context_tokens()?;
        Ok(())
    }

    pub fn required_context_tokens(&self) -> Result<u64, DomainError> {
        self.estimated_input_tokens
            .checked_add(self.estimated_output_tokens)
            .ok_or(DomainError::ContextTokenOverflow)
    }
}

/// The complete, immutable identity of one executable AI worker configuration.
///
/// Model aliases such as `latest` are not valid release identifiers. The two
/// SHA-256 fields make prompt and execution-policy changes identity changes even
/// if all human-readable labels stay the same. `worker_id` is a stable external
/// identifier; [`Self::configuration_key`] is the canonical material from which
/// callers can derive a content-addressed ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerIdentity {
    pub worker_id: WorkerId,
    pub model_release_id: ModelReleaseId,
    pub offering_id: OfferingId,
    /// Canonical, case-sensitive routing identity for the provider that serves
    /// this offering. It is execution-relevant because provider allow-lists are
    /// evaluated against it, so it is also bound into [`Self::configuration_key`].
    /// A store may additionally verify it against the offering record.
    pub provider: String,
    pub harness_id: String,
    pub harness_version: String,
    pub reasoning_configuration: String,
    pub system_prompt_sha256: String,
    pub skill_pack_version: String,
    pub toolset_version: String,
    pub execution_policy_sha256: String,
}

impl WorkerIdentity {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.worker_id.is_empty() {
            return Err(DomainError::EmptyField("worker.worker_id"));
        }
        if self.model_release_id.is_empty() {
            return Err(DomainError::EmptyField("worker.model_release_id"));
        }
        if self.offering_id.is_empty() {
            return Err(DomainError::EmptyField("worker.offering_id"));
        }

        validate_provider_identifier("worker.provider", &self.provider)?;

        for (field, value) in [
            ("worker.harness_id", self.harness_id.as_str()),
            ("worker.harness_version", self.harness_version.as_str()),
            (
                "worker.reasoning_configuration",
                self.reasoning_configuration.as_str(),
            ),
            (
                "worker.skill_pack_version",
                self.skill_pack_version.as_str(),
            ),
            ("worker.toolset_version", self.toolset_version.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(DomainError::EmptyField(field));
            }
        }

        validate_sha256("worker.system_prompt_sha256", &self.system_prompt_sha256)?;
        validate_sha256(
            "worker.execution_policy_sha256",
            &self.execution_policy_sha256,
        )?;
        Ok(())
    }

    /// Length-prefixed canonical identity material, excluding `worker_id`.
    ///
    /// Length prefixes prevent ambiguous concatenations and preserve exact
    /// version strings. Hash this UTF-8 value to create a content-addressed ID.
    /// `provider` is included even though `offering_id` should be globally
    /// unique: the routing engine evaluates provider allow-lists directly, and
    /// changing that execution-relevant label must change canonical identity.
    pub fn configuration_key(&self) -> String {
        let components = [
            self.model_release_id.0.as_str(),
            self.offering_id.0.as_str(),
            self.provider.as_str(),
            self.harness_id.as_str(),
            self.harness_version.as_str(),
            self.reasoning_configuration.as_str(),
            self.system_prompt_sha256.as_str(),
            self.skill_pack_version.as_str(),
            self.toolset_version.as_str(),
            self.execution_policy_sha256.as_str(),
        ];
        let mut key = String::new();
        for component in components {
            key.push_str(&component.len().to_string());
            key.push(':');
            key.push_str(component);
        }
        key
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostProfile {
    pub currency: String,
    pub input_micros_per_million_tokens: u64,
    pub output_micros_per_million_tokens: u64,
    #[serde(default)]
    pub fixed_request_micros: u64,
    /// Provider quota is kept separate from money, in thousandths of one unit.
    #[serde(default)]
    pub quota_milliunits_per_request: u64,
}

impl CostProfile {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.currency.trim().is_empty() {
            Err(DomainError::EmptyField("worker.cost.currency"))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerProfile {
    pub identity: WorkerIdentity,
    #[serde(default)]
    pub supported_skills: BTreeSet<SkillId>,
    #[serde(default)]
    pub tools: BTreeSet<String>,
    pub data_clearance: PrivacyClass,
    pub context_window_tokens: u64,
    pub cost: CostProfile,
    pub available: bool,
}

impl WorkerProfile {
    pub fn validate(&self) -> Result<(), DomainError> {
        self.identity.validate()?;
        self.cost.validate()?;
        if self.context_window_tokens == 0 {
            return Err(DomainError::ZeroContextWindow);
        }
        if self.supported_skills.iter().any(SkillId::is_empty) {
            return Err(DomainError::EmptyField("worker.supported_skills"));
        }
        if self.tools.iter().any(|tool| tool.trim().is_empty()) {
            return Err(DomainError::EmptyField("worker.tools"));
        }
        Ok(())
    }
}

/// A probability estimate with its conservative confidence bound and provenance
/// weight. The engine's beta-posterior utility can produce these fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbabilityEstimate {
    pub success_mean: f64,
    pub success_lower_bound: f64,
    pub evidence_count: u64,
}

impl ProbabilityEstimate {
    pub fn validate(&self, prefix: &'static str) -> Result<(), DomainError> {
        validate_probability(prefix, self.success_mean)?;
        validate_probability(prefix, self.success_lower_bound)?;
        if self.success_lower_bound > self.success_mean {
            return Err(DomainError::InvalidConfidenceBounds);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerEstimate {
    pub worker: WorkerProfile,
    /// Task-level probability, conditioned on the supplied task class.
    pub success: ProbabilityEstimate,
    /// Per-skill estimates used for hard requirement checks.
    #[serde(default)]
    pub skill_estimates: BTreeMap<SkillId, ProbabilityEstimate>,
    /// Expected non-model charges, such as search or sandbox fees. Model-token
    /// and request charges are always derived from [`CostProfile`].
    #[serde(default)]
    pub expected_tool_cash_micros: u64,
    #[serde(default)]
    pub expected_review_cash_micros: u64,
    #[serde(default)]
    pub expected_fallback_cash_micros: u64,
    /// Quota consumed in addition to the offering's per-request base quota.
    #[serde(default)]
    pub expected_additional_quota_milliunits: u64,
    /// Required for maker-checker routing. The checker is part of the quoted
    /// execution plan and must not be the maker itself.
    #[serde(default)]
    pub checker_worker_id: Option<WorkerId>,
    pub p95_latency_ms: u64,
    pub evidence_snapshot_id: String,
}

impl WorkerEstimate {
    pub fn validate(&self) -> Result<(), DomainError> {
        self.worker.validate()?;
        self.success.validate("estimate.success")?;
        if self.evidence_snapshot_id.trim().is_empty() {
            return Err(DomainError::EmptyField("estimate.evidence_snapshot_id"));
        }
        if self
            .checker_worker_id
            .as_ref()
            .is_some_and(WorkerId::is_empty)
        {
            return Err(DomainError::EmptyField("estimate.checker_worker_id"));
        }
        for (skill_id, estimate) in &self.skill_estimates {
            if skill_id.is_empty() {
                return Err(DomainError::EmptyField("estimate.skill_estimates"));
            }
            estimate.validate("estimate.skill_estimates")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceObservation {
    pub id: String,
    pub model_release_id: ModelReleaseId,
    /// Present only when the source discloses the complete worker
    /// configuration. The store validates that such a worker uses
    /// `model_release_id`; release-level evidence leaves this unset.
    #[serde(default)]
    pub worker_id: Option<WorkerId>,
    pub skill_id: SkillId,
    pub benchmark_id: BenchmarkId,
    pub evidence_tier: EvidenceTier,
    /// Source-native value. Benchmarks with different metrics are never
    /// compared directly or silently collapsed into one global score.
    pub raw_score: f64,
    pub metric: String,
    pub unit: String,
    /// Optional adapter-produced value used only by a named, versioned model.
    #[serde(default)]
    pub normalized_score: Option<f64>,
    pub adapter_version: String,
    /// `None` means that the source did not publish its sample size.
    #[serde(default)]
    pub sample_count: Option<u64>,
    pub observed_at: String,
    pub source_url: String,
    pub artifact_sha256: String,
    pub license: String,
}

impl EvidenceObservation {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.model_release_id.is_empty() {
            return Err(DomainError::EmptyField("evidence.model_release_id"));
        }
        if self.worker_id.as_ref().is_some_and(WorkerId::is_empty) {
            return Err(DomainError::EmptyField("evidence.worker_id"));
        }
        if self.skill_id.is_empty() {
            return Err(DomainError::EmptyField("evidence.skill_id"));
        }
        if self.benchmark_id.is_empty() {
            return Err(DomainError::EmptyField("evidence.benchmark_id"));
        }
        for (field, value) in [
            ("evidence.id", self.id.as_str()),
            ("evidence.metric", self.metric.as_str()),
            ("evidence.unit", self.unit.as_str()),
            ("evidence.adapter_version", self.adapter_version.as_str()),
            ("evidence.observed_at", self.observed_at.as_str()),
            ("evidence.source_url", self.source_url.as_str()),
            ("evidence.license", self.license.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(DomainError::EmptyField(field));
            }
        }
        if !self.raw_score.is_finite() {
            return Err(DomainError::NonFiniteMetric("evidence.raw_score"));
        }
        if let Some(score) = self.normalized_score {
            validate_probability("evidence.normalized_score", score)?;
        }
        if self.sample_count == Some(0) {
            return Err(DomainError::ZeroSampleCount);
        }
        validate_sha256("evidence.artifact_sha256", &self.artifact_sha256)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutcomeEvent {
    pub id: String,
    pub task_id: TaskId,
    pub worker_id: WorkerId,
    pub skill_id: SkillId,
    pub accepted: bool,
    pub validation_kind: ValidationKind,
    pub actual_cash_micros: u64,
    #[serde(default)]
    pub actual_quota_milliunits: u64,
    pub latency_ms: u64,
    pub observed_at: String,
    /// Hash or local opaque identifier; never a public repository payload.
    #[serde(default)]
    pub repository_scope: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Error, PartialEq)]
pub enum DomainError {
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("{field} must be between 0 and 1, got {value}")]
    InvalidProbability { field: &'static str, value: f64 },
    #[error("the success lower bound cannot exceed the mean")]
    InvalidConfidenceBounds,
    #[error("task skill {0} is declared more than once")]
    DuplicateSkill(SkillId),
    #[error("input and output token estimates overflow u64")]
    ContextTokenOverflow,
    #[error(
        "{risk:?} risk requires at least {required:?} verification, but the task requested {actual:?}"
    )]
    InsufficientVerification {
        risk: RiskLevel,
        required: VerificationPolicy,
        actual: VerificationPolicy,
    },
    #[error("worker context window must be greater than zero")]
    ZeroContextWindow,
    #[error("evidence sample count must be greater than zero")]
    ZeroSampleCount,
    #[error("{0} must be finite")]
    NonFiniteMetric(&'static str),
    #[error("{0} must be a lowercase 64-character SHA-256 digest")]
    InvalidSha256(&'static str),
    #[error("{0} must not have leading or trailing whitespace")]
    NonCanonicalProviderIdentifier(&'static str),
}

fn validate_probability(field: &'static str, value: f64) -> Result<(), DomainError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(DomainError::InvalidProbability { field, value })
    }
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), DomainError> {
    let valid = value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if valid {
        Ok(())
    } else {
        Err(DomainError::InvalidSha256(field))
    }
}

fn validate_provider_identifier(field: &'static str, value: &str) -> Result<(), DomainError> {
    if value.trim().is_empty() {
        Err(DomainError::EmptyField(field))
    } else if value.trim() != value {
        Err(DomainError::NonCanonicalProviderIdentifier(field))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn privacy_clearance_is_monotonic() {
        assert!(PrivacyClass::Secret.permits(PrivacyClass::ConfidentialContent));
        assert!(PrivacyClass::ConfidentialContent.permits(PrivacyClass::PrivateMetadata));
        assert!(!PrivacyClass::Public.permits(PrivacyClass::PrivateMetadata));
    }

    #[test]
    fn exact_worker_identity_changes_with_configuration() {
        let identity = sample_identity();
        let mut changed = identity.clone();
        changed.reasoning_configuration = "high".to_owned();

        assert_ne!(identity, changed);
        assert_ne!(identity.configuration_key(), changed.configuration_key());
    }

    #[test]
    fn configuration_key_binds_provider_routing_identity() {
        let identity = sample_identity();
        let mut changed = identity.clone();
        changed.provider = "another-provider".to_owned();

        assert_ne!(identity.configuration_key(), changed.configuration_key());
    }

    #[test]
    fn provider_identifiers_must_be_nonblank_and_canonical() {
        let mut identity = sample_identity();
        identity.provider = " test".to_owned();
        assert_eq!(
            identity.validate(),
            Err(DomainError::NonCanonicalProviderIdentifier(
                "worker.provider"
            ))
        );

        let mut task = sample_task();
        task.allowed_providers = BTreeSet::from(["".to_owned()]);
        assert_eq!(
            task.validate(),
            Err(DomainError::EmptyField("task.allowed_providers"))
        );

        task.allowed_providers = BTreeSet::from(["test ".to_owned()]);
        assert_eq!(
            task.validate(),
            Err(DomainError::NonCanonicalProviderIdentifier(
                "task.allowed_providers"
            ))
        );
    }

    #[test]
    fn identity_requires_content_digests() {
        let mut identity = sample_identity();
        identity.system_prompt_sha256 = "unknown".to_owned();
        assert_eq!(
            identity.validate(),
            Err(DomainError::InvalidSha256("worker.system_prompt_sha256"))
        );
    }

    #[test]
    fn task_rejects_duplicate_skills() {
        let task = TaskSpec {
            id: "task:test".into(),
            summary: "test".to_owned(),
            repository: None,
            required_skills: vec![
                SkillRequirement {
                    skill_id: "skill:rust".into(),
                    minimum_success_probability: 0.7,
                },
                SkillRequirement {
                    skill_id: "skill:rust".into(),
                    minimum_success_probability: 0.8,
                },
            ],
            required_tools: BTreeSet::new(),
            allowed_providers: BTreeSet::new(),
            privacy: PrivacyClass::Public,
            risk: RiskLevel::Low,
            verification: VerificationPolicy::Deterministic,
            minimum_success_probability: 0.5,
            max_expected_cash_micros: None,
            max_p95_latency_ms: None,
            estimated_input_tokens: 100,
            estimated_output_tokens: 100,
        };

        assert_eq!(
            task.validate(),
            Err(DomainError::DuplicateSkill("skill:rust".into()))
        );
    }

    #[test]
    fn risk_sets_a_minimum_verification_policy() {
        let mut task = sample_task();
        task.risk = RiskLevel::High;
        task.verification = VerificationPolicy::Deterministic;
        assert_eq!(
            task.validate(),
            Err(DomainError::InsufficientVerification {
                risk: RiskLevel::High,
                required: VerificationPolicy::MakerChecker,
                actual: VerificationPolicy::Deterministic,
            })
        );

        task.risk = RiskLevel::Consequential;
        task.verification = VerificationPolicy::MakerChecker;
        assert_eq!(
            task.validate(),
            Err(DomainError::InsufficientVerification {
                risk: RiskLevel::Consequential,
                required: VerificationPolicy::HumanApproval,
                actual: VerificationPolicy::MakerChecker,
            })
        );

        task.verification = VerificationPolicy::HumanApproval;
        assert_eq!(task.validate(), Ok(()));
    }

    #[test]
    fn estimate_rejects_inverted_confidence_bounds() {
        let mut estimate = sample_estimate();
        estimate.success = ProbabilityEstimate {
            success_mean: 0.8,
            success_lower_bound: 0.9,
            evidence_count: 10,
        };
        assert_eq!(
            estimate.validate(),
            Err(DomainError::InvalidConfidenceBounds)
        );
    }

    #[test]
    fn evidence_keeps_source_native_scores() {
        let mut observation = EvidenceObservation {
            id: "evidence:test".to_owned(),
            model_release_id: "model:test-2026-01-01".into(),
            worker_id: Some("worker:test".into()),
            skill_id: "skill:rust".into(),
            benchmark_id: "benchmark:test".into(),
            evidence_tier: EvidenceTier::CommunityReproducible,
            raw_score: 1_247.5,
            metric: "elo".to_owned(),
            unit: "rating_points".to_owned(),
            normalized_score: None,
            adapter_version: "adapter:test-v1".to_owned(),
            sample_count: Some(100),
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            source_url: "https://example.invalid/evidence".to_owned(),
            artifact_sha256: EMPTY_SHA256.to_owned(),
            license: "CC-BY-4.0".to_owned(),
        };
        assert_eq!(observation.validate(), Ok(()));

        observation.normalized_score = Some(1.1);
        assert!(matches!(
            observation.validate(),
            Err(DomainError::InvalidProbability { .. })
        ));
    }

    #[test]
    fn release_level_evidence_can_omit_worker_and_sample_count() {
        let observation = EvidenceObservation {
            id: "evidence:release-level".to_owned(),
            model_release_id: "model:test-2026-01-01".into(),
            worker_id: None,
            skill_id: "skill:rust".into(),
            benchmark_id: "benchmark:test".into(),
            evidence_tier: EvidenceTier::VendorReported,
            raw_score: 0.75,
            metric: "pass_rate".to_owned(),
            unit: "ratio".to_owned(),
            normalized_score: None,
            adapter_version: "adapter:test-v1".to_owned(),
            sample_count: None,
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            source_url: "https://example.invalid/evidence".to_owned(),
            artifact_sha256: EMPTY_SHA256.to_owned(),
            license: "CC-BY-4.0".to_owned(),
        };

        assert_eq!(observation.validate(), Ok(()));
        let mut encoded = serde_json::to_value(&observation).expect("serialize evidence");
        let object = encoded.as_object_mut().expect("evidence JSON object");
        object.remove("worker_id");
        object.remove("sample_count");
        assert_eq!(
            serde_json::from_value::<EvidenceObservation>(encoded).expect("deserialize evidence"),
            observation
        );
    }

    #[test]
    fn evidence_rejects_missing_required_identity_and_zero_known_sample_count() {
        let mut observation = EvidenceObservation {
            id: "evidence:test".to_owned(),
            model_release_id: ModelReleaseId::from(""),
            worker_id: None,
            skill_id: "skill:rust".into(),
            benchmark_id: "benchmark:test".into(),
            evidence_tier: EvidenceTier::CommunityReproducible,
            raw_score: 0.75,
            metric: "pass_rate".to_owned(),
            unit: "ratio".to_owned(),
            normalized_score: None,
            adapter_version: "adapter:test-v1".to_owned(),
            sample_count: None,
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            source_url: "https://example.invalid/evidence".to_owned(),
            artifact_sha256: EMPTY_SHA256.to_owned(),
            license: "CC-BY-4.0".to_owned(),
        };

        assert_eq!(
            observation.validate(),
            Err(DomainError::EmptyField("evidence.model_release_id"))
        );
        observation.model_release_id = "model:test-2026-01-01".into();
        observation.benchmark_id = BenchmarkId::from("");
        assert_eq!(
            observation.validate(),
            Err(DomainError::EmptyField("evidence.benchmark_id"))
        );
        observation.benchmark_id = "benchmark:test".into();
        observation.worker_id = Some(WorkerId::from(""));
        assert_eq!(
            observation.validate(),
            Err(DomainError::EmptyField("evidence.worker_id"))
        );
        observation.worker_id = None;
        observation.sample_count = Some(0);
        assert_eq!(observation.validate(), Err(DomainError::ZeroSampleCount));
    }

    fn sample_identity() -> WorkerIdentity {
        WorkerIdentity {
            worker_id: "worker:test".into(),
            model_release_id: "model:test-2026-01-01".into(),
            offering_id: "offering:test".into(),
            provider: "test".to_owned(),
            harness_id: "raw-api".to_owned(),
            harness_version: "1".to_owned(),
            reasoning_configuration: "standard".to_owned(),
            system_prompt_sha256: EMPTY_SHA256.to_owned(),
            skill_pack_version: "1".to_owned(),
            toolset_version: "1".to_owned(),
            execution_policy_sha256: EMPTY_SHA256.to_owned(),
        }
    }

    fn sample_task() -> TaskSpec {
        TaskSpec {
            id: "task:test".into(),
            summary: "test".to_owned(),
            repository: None,
            required_skills: Vec::new(),
            required_tools: BTreeSet::new(),
            allowed_providers: BTreeSet::new(),
            privacy: PrivacyClass::Public,
            risk: RiskLevel::Low,
            verification: VerificationPolicy::Deterministic,
            minimum_success_probability: 0.5,
            max_expected_cash_micros: None,
            max_p95_latency_ms: None,
            estimated_input_tokens: 100,
            estimated_output_tokens: 100,
        }
    }

    fn sample_estimate() -> WorkerEstimate {
        WorkerEstimate {
            worker: WorkerProfile {
                identity: sample_identity(),
                supported_skills: BTreeSet::new(),
                tools: BTreeSet::new(),
                data_clearance: PrivacyClass::Public,
                context_window_tokens: 1_000,
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
                success_mean: 0.8,
                success_lower_bound: 0.7,
                evidence_count: 10,
            },
            skill_estimates: BTreeMap::new(),
            expected_tool_cash_micros: 0,
            expected_review_cash_micros: 0,
            expected_fallback_cash_micros: 0,
            expected_additional_quota_milliunits: 0,
            checker_worker_id: None,
            p95_latency_ms: 0,
            evidence_snapshot_id: "snapshot:test".to_owned(),
        }
    }
}
