//! Persistence boundaries for the public index and the private local allocator.
//!
//! The two store types deliberately have unrelated read/write traits and reject
//! opening a database initialized for the other trust domain. Public export
//! functions accept only [`PublicIndexRead`], so private ledger values cannot
//! accidentally enter a public snapshot through this API.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use workforce_domain::{
    BenchmarkId, DecisionId, EvidenceTier, ModelReleaseId, OfferingId, OutcomeEvent, PrivacyClass,
    SkillId, TaskId, ValidationKind, VerificationPolicy, WorkerId, WorkerIdentity,
};

const PUBLIC_STORE_KIND: &str = "public_index";
const PRIVATE_STORE_KIND: &str = "private_local";
const PUBLIC_SCHEMA_VERSION: i64 = 2;
const PRIVATE_SCHEMA_VERSION: i64 = 2;

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

/// The only aggregate accepted by the public export boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicIndexExport {
    pub model_releases: Vec<ModelReleaseRecord>,
    pub provider_offerings: Vec<ProviderOfferingRecord>,
    pub worker_profiles: Vec<WorkerProfileRecord>,
    pub evidence: Vec<PublicEvidenceRecord>,
    pub snapshot: SnapshotRecord,
}

/// Narrow read surface for public, publishable data only.
pub trait PublicIndexRead {
    fn model_releases(&self) -> Result<Vec<ModelReleaseRecord>, StoreError>;
    fn provider_offerings(&self) -> Result<Vec<ProviderOfferingRecord>, StoreError>;
    fn worker_profiles(&self) -> Result<Vec<WorkerProfileRecord>, StoreError>;
    fn evidence(&self) -> Result<Vec<PublicEvidenceRecord>, StoreError>;
    fn snapshots(&self) -> Result<Vec<SnapshotRecord>, StoreError>;
    fn snapshot(&self, id: &str) -> Result<Option<SnapshotRecord>, StoreError>;
    fn model_release(&self, id: &ModelReleaseId) -> Result<Option<ModelReleaseRecord>, StoreError>;
    fn provider_offering(
        &self,
        id: &OfferingId,
    ) -> Result<Option<ProviderOfferingRecord>, StoreError>;
    fn worker_profile(&self, id: &WorkerId) -> Result<Option<WorkerProfileRecord>, StoreError>;
    fn evidence_observation(&self, id: &str) -> Result<Option<PublicEvidenceRecord>, StoreError>;
    /// Returns only revisions active at `at_epoch_ms`, excluding any revision
    /// superseded by another revision already effective at that instant.
    fn current_provider_offerings(
        &self,
        at_epoch_ms: i64,
    ) -> Result<Vec<ProviderOfferingRecord>, StoreError>;
}

/// Append-only mutation surface for curating the public index.
pub trait PublicIndexWrite {
    fn append_model_release(&self, record: &ModelReleaseRecord) -> Result<(), StoreError>;
    fn append_provider_offering(&self, record: &ProviderOfferingRecord) -> Result<(), StoreError>;
    fn append_worker_profile(&self, record: &WorkerProfileRecord) -> Result<(), StoreError>;
    fn append_evidence(&self, record: &PublicEvidenceRecord) -> Result<(), StoreError>;
    fn append_snapshot(&self, record: &SnapshotRecord) -> Result<(), StoreError>;
}

/// Narrow read surface for local, non-publishable allocator history.
pub trait PrivateLedgerRead {
    fn quotes(&self) -> Result<Vec<QuoteRecord>, StoreError>;
    fn outcomes(&self) -> Result<Vec<PrivateOutcomeRecord>, StoreError>;
}

/// Append-only mutation surface for the private allocator ledger.
pub trait PrivateLedgerWrite {
    fn append_quote(&self, record: &QuoteRecord) -> Result<(), StoreError>;
    fn append_outcome(&self, record: &PrivateOutcomeRecord) -> Result<(), StoreError>;
}

/// Builds an export without accepting a private-ledger capability.
pub fn build_public_export(
    source: &impl PublicIndexRead,
    snapshot_id: &str,
) -> Result<PublicIndexExport, StoreError> {
    let snapshot = source
        .snapshot(snapshot_id)?
        .ok_or_else(|| StoreError::SnapshotNotFound(snapshot_id.to_owned()))?;
    snapshot.validate()?;

    let model_releases = snapshot
        .model_release_ids
        .iter()
        .map(|id| required_snapshot_member("model release", &id.0, source.model_release(id)?))
        .collect::<Result<Vec<_>, _>>()?;
    let provider_offerings = snapshot
        .provider_offering_ids
        .iter()
        .map(|id| {
            required_snapshot_member("provider offering", &id.0, source.provider_offering(id)?)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let worker_profiles = snapshot
        .worker_profile_ids
        .iter()
        .map(|id| required_snapshot_member("worker profile", &id.0, source.worker_profile(id)?))
        .collect::<Result<Vec<_>, _>>()?;
    let evidence = snapshot
        .evidence_ids
        .iter()
        .map(|id| {
            required_snapshot_member("evidence observation", id, source.evidence_observation(id)?)
        })
        .collect::<Result<Vec<_>, _>>()?;

    validate_export_dependency_closure(
        &snapshot,
        &model_releases,
        &provider_offerings,
        &worker_profiles,
        &evidence,
    )?;

    Ok(PublicIndexExport {
        model_releases,
        provider_offerings,
        worker_profiles,
        evidence,
        snapshot,
    })
}

/// Rebuildable public catalog and evidence store.
pub struct PublicIndexStore {
    connection: Connection,
}

impl PublicIndexStore {
    /// Opens or initializes a file-backed public index in WAL mode.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        configure_file_connection(&connection)?;
        initialize_or_validate_store(
            &connection,
            PUBLIC_STORE_KIND,
            PUBLIC_SCHEMA_VERSION,
            PUBLIC_SCHEMA,
        )?;
        Ok(Self { connection })
    }

    /// Creates an isolated in-memory public index, primarily for tests/tools.
    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        configure_memory_connection(&connection)?;
        initialize_or_validate_store(
            &connection,
            PUBLIC_STORE_KIND,
            PUBLIC_SCHEMA_VERSION,
            PUBLIC_SCHEMA,
        )?;
        Ok(Self { connection })
    }
}

/// Read-only public-index handle. It does not implement [`PublicIndexWrite`].
pub struct ReadOnlyPublicIndexStore {
    connection: Connection,
}

impl ReadOnlyPublicIndexStore {
    /// Opens an existing public index with SQLite's read-only flag.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        validate_schema_version(&connection, PUBLIC_SCHEMA_VERSION)?;
        validate_identity(&connection, PUBLIC_STORE_KIND)?;
        Ok(Self { connection })
    }
}

struct ConnectionPublicReader<'connection>(&'connection Connection);

impl PublicIndexRead for ConnectionPublicReader<'_> {
    fn model_releases(&self) -> Result<Vec<ModelReleaseRecord>, StoreError> {
        let mut statement = self.0.prepare(
            "SELECT id, developer, model_family, released_at, context_window_tokens, \
             source_url, artifact_sha256, recorded_at \
             FROM model_releases ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;

        rows.map(|row| {
            let (id, developer, family, released_at, context, source, digest, recorded_at) = row?;
            Ok(ModelReleaseRecord {
                id: ModelReleaseId(id),
                developer,
                model_family: family,
                released_at,
                context_window_tokens: from_i64("context_window_tokens", context)?,
                source_url: source,
                artifact_sha256: digest,
                recorded_at,
            })
        })
        .collect()
    }

    fn provider_offerings(&self) -> Result<Vec<ProviderOfferingRecord>, StoreError> {
        let mut statement = self.0.prepare(
            "SELECT id, model_release_id, provider, supersedes_offering_id,
                    effective_from_epoch_ms, effective_until_epoch_ms,
                    currency, input_micros_per_million_tokens,
                    output_micros_per_million_tokens, fixed_request_micros,
                    quota_milliunits_per_request, context_window_tokens, source_url, recorded_at
             FROM provider_offerings ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
            ))
        })?;

        rows.map(|row| {
            let (
                id,
                model,
                provider,
                supersedes,
                from,
                until,
                currency,
                input,
                output,
                fixed,
                quota,
                context,
                url,
                at,
            ) = row?;
            Ok(ProviderOfferingRecord {
                id: OfferingId(id),
                model_release_id: ModelReleaseId(model),
                provider,
                supersedes_offering_id: supersedes.map(OfferingId),
                effective_from_epoch_ms: from,
                effective_until_epoch_ms: until,
                currency,
                input_micros_per_million_tokens: from_i64(
                    "input_micros_per_million_tokens",
                    input,
                )?,
                output_micros_per_million_tokens: from_i64(
                    "output_micros_per_million_tokens",
                    output,
                )?,
                fixed_request_micros: from_i64("fixed_request_micros", fixed)?,
                quota_milliunits_per_request: from_i64("quota_milliunits_per_request", quota)?,
                context_window_tokens: from_i64("context_window_tokens", context)?,
                source_url: url,
                recorded_at: at,
            })
        })
        .collect()
    }

    fn worker_profiles(&self) -> Result<Vec<WorkerProfileRecord>, StoreError> {
        let mut statement = self.0.prepare(
            "SELECT id, offering_id, harness_id, harness_version, reasoning_configuration,
                    system_prompt_sha256, skill_pack_version, toolset_version,
                    execution_policy_sha256, supported_skill_ids_json, tools_json,
                    privacy_clearance, configuration_sha256, recorded_at
             FROM worker_profiles ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
            ))
        })?;
        rows.map(|row| {
            let (
                id,
                offering_id,
                harness_id,
                harness_version,
                reasoning_configuration,
                system_prompt_sha256,
                skill_pack_version,
                toolset_version,
                execution_policy_sha256,
                skills_json,
                tools_json,
                privacy_clearance,
                configuration_sha256,
                recorded_at,
            ) = row?;
            Ok(WorkerProfileRecord {
                id: WorkerId(id),
                offering_id: OfferingId(offering_id),
                harness_id,
                harness_version,
                reasoning_configuration,
                system_prompt_sha256,
                skill_pack_version,
                toolset_version,
                execution_policy_sha256,
                supported_skill_ids: serde_json::from_str(&skills_json)?,
                tools: serde_json::from_str(&tools_json)?,
                privacy_clearance: decode_privacy_class(&privacy_clearance)?,
                configuration_sha256,
                recorded_at,
            })
        })
        .collect()
    }

    fn evidence(&self) -> Result<Vec<PublicEvidenceRecord>, StoreError> {
        let mut statement = self.0.prepare(
            "SELECT id, model_release_id, worker_id, skill_id, benchmark_id, evidence_tier, \
             raw_score, metric, unit, normalized_score, adapter_version, sample_count, \
             observed_at, source_url, artifact_sha256, license \
             FROM evidence_observations ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<f64>>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, Option<i64>>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, String>(14)?,
                row.get::<_, String>(15)?,
            ))
        })?;

        rows.map(|row| {
            let (
                id,
                model_release_id,
                worker_id,
                skill_id,
                benchmark_id,
                tier,
                raw_score,
                metric,
                unit,
                normalized_score,
                adapter_version,
                sample_count,
                observed_at,
                source_url,
                digest,
                license,
            ) = row?;
            Ok(PublicEvidenceRecord {
                id,
                model_release_id: ModelReleaseId(model_release_id),
                worker_id: worker_id.map(WorkerId),
                skill_id: SkillId(skill_id),
                benchmark_id: BenchmarkId(benchmark_id),
                evidence_tier: decode_evidence_tier(&tier)?,
                raw_score,
                metric,
                unit,
                normalized_score,
                adapter_version,
                sample_count: sample_count
                    .map(|value| from_i64("sample_count", value))
                    .transpose()?,
                observed_at,
                source_url,
                artifact_sha256: digest,
                license,
            })
        })
        .collect()
    }

    fn snapshots(&self) -> Result<Vec<SnapshotRecord>, StoreError> {
        let mut statement = self.0.prepare(
            "SELECT id, created_at, ontology_version, source_revision, content_sha256, \
             model_release_ids_json, provider_offering_ids_json, worker_profile_ids_json,
             evidence_ids_json, model_release_count, provider_offering_count,
             worker_profile_count, evidence_count \
             FROM snapshots ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], snapshot_row)?;
        rows.map(|row| row?.try_into()).collect()
    }

    fn snapshot(&self, id: &str) -> Result<Option<SnapshotRecord>, StoreError> {
        let raw = self
            .0
            .query_row(
                "SELECT id, created_at, ontology_version, source_revision, content_sha256, \
                 model_release_ids_json, provider_offering_ids_json, worker_profile_ids_json,
                 evidence_ids_json, model_release_count, provider_offering_count,
                 worker_profile_count, evidence_count \
                 FROM snapshots WHERE id = ?1",
                [id],
                snapshot_row,
            )
            .optional()?;
        raw.map(SnapshotRecord::try_from).transpose()
    }

    fn model_release(&self, id: &ModelReleaseId) -> Result<Option<ModelReleaseRecord>, StoreError> {
        let raw = self
            .0
            .query_row(
                "SELECT id, developer, model_family, released_at, context_window_tokens,
                        source_url, artifact_sha256, recorded_at
                 FROM model_releases WHERE id = ?1",
                [&id.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?;
        raw.map(
            |(id, developer, family, released, context, source, digest, recorded)| {
                Ok(ModelReleaseRecord {
                    id: ModelReleaseId(id),
                    developer,
                    model_family: family,
                    released_at: released,
                    context_window_tokens: from_i64("context_window_tokens", context)?,
                    source_url: source,
                    artifact_sha256: digest,
                    recorded_at: recorded,
                })
            },
        )
        .transpose()
    }

    fn provider_offering(
        &self,
        id: &OfferingId,
    ) -> Result<Option<ProviderOfferingRecord>, StoreError> {
        read_provider_offering(self.0, &id.0)
    }

    fn worker_profile(&self, id: &WorkerId) -> Result<Option<WorkerProfileRecord>, StoreError> {
        let raw = self
            .0
            .query_row(
                "SELECT id, offering_id, harness_id, harness_version,
                        reasoning_configuration, system_prompt_sha256, skill_pack_version,
                        toolset_version, execution_policy_sha256, supported_skill_ids_json,
                        tools_json, privacy_clearance, configuration_sha256, recorded_at
                 FROM worker_profiles WHERE id = ?1",
                [&id.0],
                worker_profile_row,
            )
            .optional()?;
        raw.map(WorkerProfileRecord::try_from).transpose()
    }

    fn evidence_observation(&self, id: &str) -> Result<Option<PublicEvidenceRecord>, StoreError> {
        let raw = self
            .0
            .query_row(
                "SELECT id, model_release_id, worker_id, skill_id, benchmark_id,
                        evidence_tier, raw_score, metric, unit, normalized_score,
                        adapter_version, sample_count, observed_at, source_url,
                        artifact_sha256, license
                 FROM evidence_observations WHERE id = ?1",
                [id],
                public_evidence_row,
            )
            .optional()?;
        raw.map(PublicEvidenceRecord::try_from).transpose()
    }

    fn current_provider_offerings(
        &self,
        at_epoch_ms: i64,
    ) -> Result<Vec<ProviderOfferingRecord>, StoreError> {
        let mut statement = self.0.prepare(
            "SELECT current.id
             FROM provider_offerings AS current
             WHERE current.effective_from_epoch_ms <= ?1
               AND (current.effective_until_epoch_ms IS NULL
                    OR current.effective_until_epoch_ms > ?1)
               AND NOT EXISTS (
                   SELECT 1 FROM provider_offerings AS successor
                   WHERE successor.supersedes_offering_id = current.id
                     AND successor.effective_from_epoch_ms <= ?1
               )
             ORDER BY current.id",
        )?;
        let ids = statement
            .query_map([at_epoch_ms], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                read_provider_offering(self.0, &id)?.ok_or_else(|| {
                    StoreError::SnapshotMemberMissing {
                        kind: "provider offering",
                        id,
                    }
                })
            })
            .collect()
    }
}

macro_rules! delegate_public_reads {
    ($store:ty) => {
        impl PublicIndexRead for $store {
            fn model_releases(&self) -> Result<Vec<ModelReleaseRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).model_releases()
            }

            fn provider_offerings(&self) -> Result<Vec<ProviderOfferingRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).provider_offerings()
            }

            fn worker_profiles(&self) -> Result<Vec<WorkerProfileRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).worker_profiles()
            }

            fn evidence(&self) -> Result<Vec<PublicEvidenceRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).evidence()
            }

            fn snapshots(&self) -> Result<Vec<SnapshotRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).snapshots()
            }

            fn snapshot(&self, id: &str) -> Result<Option<SnapshotRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).snapshot(id)
            }

            fn model_release(
                &self,
                id: &ModelReleaseId,
            ) -> Result<Option<ModelReleaseRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).model_release(id)
            }

            fn provider_offering(
                &self,
                id: &OfferingId,
            ) -> Result<Option<ProviderOfferingRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).provider_offering(id)
            }

            fn worker_profile(
                &self,
                id: &WorkerId,
            ) -> Result<Option<WorkerProfileRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).worker_profile(id)
            }

            fn evidence_observation(
                &self,
                id: &str,
            ) -> Result<Option<PublicEvidenceRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).evidence_observation(id)
            }

            fn current_provider_offerings(
                &self,
                at_epoch_ms: i64,
            ) -> Result<Vec<ProviderOfferingRecord>, StoreError> {
                ConnectionPublicReader(&self.connection).current_provider_offerings(at_epoch_ms)
            }
        }
    };
}

delegate_public_reads!(PublicIndexStore);
delegate_public_reads!(ReadOnlyPublicIndexStore);

impl PublicIndexWrite for PublicIndexStore {
    fn append_model_release(&self, record: &ModelReleaseRecord) -> Result<(), StoreError> {
        validate_canonical_identifier("model_release.id", &record.id.0)?;
        validate_sha256("model_release.artifact_sha256", &record.artifact_sha256)?;
        self.connection.execute(
            "INSERT INTO model_releases (
                id, developer, model_family, released_at, context_window_tokens,
                source_url, artifact_sha256, recorded_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.id.0,
                record.developer,
                record.model_family,
                record.released_at,
                to_i64("context_window_tokens", record.context_window_tokens)?,
                record.source_url,
                record.artifact_sha256,
                record.recorded_at,
            ],
        )?;
        Ok(())
    }

    fn append_provider_offering(&self, record: &ProviderOfferingRecord) -> Result<(), StoreError> {
        validate_canonical_identifier("provider_offering.id", &record.id.0)?;
        validate_canonical_identifier(
            "provider_offering.model_release_id",
            &record.model_release_id.0,
        )?;
        if let Some(predecessor_id) = &record.supersedes_offering_id {
            validate_canonical_identifier(
                "provider_offering.supersedes_offering_id",
                &predecessor_id.0,
            )?;
        }
        validate_canonical_identifier("provider_offering.provider", &record.provider)?;
        self.connection.execute(
            "INSERT INTO provider_offerings (
                id, model_release_id, provider, supersedes_offering_id,
                effective_from_epoch_ms, effective_until_epoch_ms,
                currency, input_micros_per_million_tokens,
                output_micros_per_million_tokens, fixed_request_micros,
                quota_milliunits_per_request, context_window_tokens, source_url, recorded_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                record.id.0,
                record.model_release_id.0,
                record.provider,
                record
                    .supersedes_offering_id
                    .as_ref()
                    .map(|id| id.0.as_str()),
                record.effective_from_epoch_ms,
                record.effective_until_epoch_ms,
                record.currency,
                to_i64(
                    "input_micros_per_million_tokens",
                    record.input_micros_per_million_tokens,
                )?,
                to_i64(
                    "output_micros_per_million_tokens",
                    record.output_micros_per_million_tokens,
                )?,
                to_i64("fixed_request_micros", record.fixed_request_micros)?,
                to_i64(
                    "quota_milliunits_per_request",
                    record.quota_milliunits_per_request,
                )?,
                to_i64("context_window_tokens", record.context_window_tokens)?,
                record.source_url,
                record.recorded_at,
            ],
        )?;
        Ok(())
    }

    fn append_worker_profile(&self, record: &WorkerProfileRecord) -> Result<(), StoreError> {
        validate_canonical_identifier("worker_profile.id", &record.id.0)?;
        validate_canonical_identifier("worker_profile.offering_id", &record.offering_id.0)?;
        validate_sha256(
            "worker_profile.system_prompt_sha256",
            &record.system_prompt_sha256,
        )?;
        validate_sha256(
            "worker_profile.execution_policy_sha256",
            &record.execution_policy_sha256,
        )?;
        validate_sha256(
            "worker_profile.configuration_sha256",
            &record.configuration_sha256,
        )?;
        for skill_id in &record.supported_skill_ids {
            validate_canonical_identifier("worker_profile.supported_skill_ids", &skill_id.0)?;
        }
        for tool in &record.tools {
            validate_canonical_identifier("worker_profile.tools", tool)?;
        }

        let (model_release_id, provider) = self
            .connection
            .query_row(
                "SELECT model_release_id, provider
                 FROM provider_offerings WHERE id = ?1",
                [&record.offering_id.0],
                |row| {
                    Ok((
                        ModelReleaseId(row.get::<_, String>(0)?),
                        row.get::<_, String>(1)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                StoreError::InvalidWorkerProfile(format!(
                    "provider offering `{}` does not exist",
                    record.offering_id
                ))
            })?;
        let identity = worker_identity(record, model_release_id, provider);
        identity.validate().map_err(|error| {
            StoreError::InvalidWorkerProfile(format!("invalid execution identity: {error}"))
        })?;
        let expected_configuration_sha256 = worker_configuration_sha256(&identity);
        if record.configuration_sha256 != expected_configuration_sha256 {
            return Err(StoreError::WorkerConfigurationDigestMismatch {
                worker_id: record.id.clone(),
                expected: expected_configuration_sha256,
                actual: record.configuration_sha256.clone(),
            });
        }

        let supported_skill_ids_json = serde_json::to_string(&record.supported_skill_ids)?;
        let tools_json = serde_json::to_string(&record.tools)?;
        self.connection.execute(
            "INSERT INTO worker_profiles (
                id, offering_id, harness_id, harness_version, reasoning_configuration,
                system_prompt_sha256, skill_pack_version, toolset_version,
                execution_policy_sha256, supported_skill_ids_json, tools_json,
                privacy_clearance, configuration_sha256, recorded_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                record.id.0,
                record.offering_id.0,
                record.harness_id,
                record.harness_version,
                record.reasoning_configuration,
                record.system_prompt_sha256,
                record.skill_pack_version,
                record.toolset_version,
                record.execution_policy_sha256,
                supported_skill_ids_json,
                tools_json,
                encode_privacy_class(record.privacy_clearance),
                record.configuration_sha256,
                record.recorded_at,
            ],
        )?;
        Ok(())
    }

    fn append_evidence(&self, record: &PublicEvidenceRecord) -> Result<(), StoreError> {
        validate_canonical_identifier("evidence.id", &record.id)?;
        validate_canonical_identifier("evidence.model_release_id", &record.model_release_id.0)?;
        if let Some(worker_id) = &record.worker_id {
            validate_canonical_identifier("evidence.worker_id", &worker_id.0)?;
        }
        validate_canonical_identifier("evidence.skill_id", &record.skill_id.0)?;
        validate_canonical_identifier("evidence.benchmark_id", &record.benchmark_id.0)?;
        for (field, value) in [
            ("evidence.metric", record.metric.as_str()),
            ("evidence.unit", record.unit.as_str()),
            ("evidence.adapter_version", record.adapter_version.as_str()),
            ("evidence.observed_at", record.observed_at.as_str()),
            ("evidence.source_url", record.source_url.as_str()),
            ("evidence.license", record.license.as_str()),
        ] {
            validate_required_text(field, value)?;
        }
        validate_sha256("evidence.artifact_sha256", &record.artifact_sha256)?;
        validate_finite("raw_score", record.raw_score)?;
        if let Some(score) = record.normalized_score {
            validate_probability("normalized_score", score)?;
        }
        self.connection.execute(
            "INSERT INTO evidence_observations (
                id, model_release_id, worker_id, skill_id, benchmark_id, evidence_tier,
                raw_score, metric, unit, normalized_score, adapter_version, sample_count,
                observed_at, source_url, artifact_sha256, license
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                record.id,
                record.model_release_id.0,
                record.worker_id.as_ref().map(|id| id.0.as_str()),
                record.skill_id.0,
                record.benchmark_id.0,
                encode_evidence_tier(record.evidence_tier),
                record.raw_score,
                record.metric,
                record.unit,
                record.normalized_score,
                record.adapter_version,
                record
                    .sample_count
                    .map(|value| to_i64("sample_count", value))
                    .transpose()?,
                record.observed_at,
                record.source_url,
                record.artifact_sha256,
                record.license,
            ],
        )?;
        Ok(())
    }

    fn append_snapshot(&self, record: &SnapshotRecord) -> Result<(), StoreError> {
        validate_snapshot_dependencies(&self.connection, record)?;
        let model_release_ids_json = serde_json::to_string(&record.model_release_ids)?;
        let provider_offering_ids_json = serde_json::to_string(&record.provider_offering_ids)?;
        let worker_profile_ids_json = serde_json::to_string(&record.worker_profile_ids)?;
        let evidence_ids_json = serde_json::to_string(&record.evidence_ids)?;
        self.connection.execute(
            "INSERT INTO snapshots (
                id, created_at, ontology_version, source_revision, content_sha256,
                model_release_ids_json, provider_offering_ids_json,
                worker_profile_ids_json, evidence_ids_json,
                model_release_count, provider_offering_count, worker_profile_count,
                evidence_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                record.id,
                record.created_at,
                record.ontology_version,
                record.source_revision,
                record.content_sha256,
                model_release_ids_json,
                provider_offering_ids_json,
                worker_profile_ids_json,
                evidence_ids_json,
                to_i64("model_release_count", record.model_release_count)?,
                to_i64("provider_offering_count", record.provider_offering_count,)?,
                to_i64("worker_profile_count", record.worker_profile_count)?,
                to_i64("evidence_count", record.evidence_count)?,
            ],
        )?;
        Ok(())
    }
}

/// Private local quotes and verified outcomes. This type never implements a
/// public export trait.
pub struct PrivateLocalStore {
    connection: Connection,
    path: Option<PathBuf>,
}

impl PrivateLocalStore {
    /// Opens or initializes a private file store and restricts SQLite files to
    /// the current user on Unix.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        prepare_private_database_file(&path)?;
        let connection = Connection::open(&path)?;
        configure_file_connection(&connection)?;
        initialize_or_validate_store(
            &connection,
            PRIVATE_STORE_KIND,
            PRIVATE_SCHEMA_VERSION,
            PRIVATE_SCHEMA,
        )?;
        let store = Self {
            connection,
            path: Some(path),
        };
        store.secure_files()?;
        Ok(store)
    }

    /// Creates an isolated in-memory private ledger, primarily for tests/tools.
    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        configure_memory_connection(&connection)?;
        initialize_or_validate_store(
            &connection,
            PRIVATE_STORE_KIND,
            PRIVATE_SCHEMA_VERSION,
            PRIVATE_SCHEMA,
        )?;
        Ok(Self {
            connection,
            path: None,
        })
    }

    fn secure_files(&self) -> Result<(), StoreError> {
        if let Some(path) = &self.path {
            secure_private_sqlite_files(path)?;
        }
        Ok(())
    }
}

impl PrivateLedgerRead for PrivateLocalStore {
    fn quotes(&self) -> Result<Vec<QuoteRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT decision_id, task_id, selected_worker_id, selected_checker_worker_id,
                    verification_policy, evidence_snapshot_id, policy_version,
                    expected_cash_micros, expected_quota_milliunits,
                    expected_success_probability, p95_latency_ms,
                    eligible_candidates_json, rejected_candidates_json,
                    pareto_worker_ids_json, selection_explanation_json,
                    created_at, request_fingerprint
             FROM routing_quotes ORDER BY created_at, decision_id",
        )?;
        statement
            .query_map([], quote_row)?
            .map(|row| row?.try_into())
            .collect()
    }

    fn outcomes(&self) -> Result<Vec<PrivateOutcomeRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, decision_id, task_id, worker_id, skill_id, accepted,
                    validation_kind, actual_cash_micros, actual_quota_milliunits, latency_ms,
                    observed_at, repository_scope, metadata_json, checker_worker_id
             FROM outcome_events ORDER BY observed_at, id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, Option<String>>(13)?,
            ))
        })?;

        rows.map(|row| {
            let (
                id,
                decision,
                task,
                worker,
                skill,
                accepted,
                validation,
                cash,
                quota,
                latency,
                observed_at,
                repository_scope,
                metadata,
                checker,
            ) = row?;
            Ok(PrivateOutcomeRecord {
                decision_id: decision.map(DecisionId),
                event: OutcomeEvent {
                    id,
                    task_id: TaskId(task),
                    worker_id: WorkerId(worker),
                    skill_id: SkillId(skill),
                    accepted: decode_bool(accepted)?,
                    validation_kind: decode_validation_kind(&validation)?,
                    actual_cash_micros: from_i64("actual_cash_micros", cash)?,
                    actual_quota_milliunits: from_i64("actual_quota_milliunits", quota)?,
                    latency_ms: from_i64("latency_ms", latency)?,
                    observed_at,
                    repository_scope,
                    metadata: serde_json::from_str(&metadata)?,
                },
                checker_worker_id: checker.map(WorkerId),
            })
        })
        .collect()
    }
}

impl PrivateLedgerWrite for PrivateLocalStore {
    fn append_quote(&self, record: &QuoteRecord) -> Result<(), StoreError> {
        validate_sha256("quote.request_fingerprint", &record.request_fingerprint)?;
        if let Some(probability) = record.expected_success_probability {
            validate_probability("expected_success_probability", probability)?;
        }
        validate_quote_audit(record)?;
        let eligible_candidates_json = serde_json::to_string(&record.eligible_candidates)?;
        let rejected_candidates_json = serde_json::to_string(&record.rejected_candidates)?;
        let pareto_worker_ids_json = serde_json::to_string(&record.pareto_worker_ids)?;
        let selection_explanation_json = record
            .selection_explanation
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        self.connection.execute(
            "INSERT INTO routing_quotes (
                decision_id, task_id, selected_worker_id, selected_checker_worker_id,
                verification_policy, evidence_snapshot_id, policy_version,
                expected_cash_micros, expected_quota_milliunits,
                expected_success_probability, p95_latency_ms,
                eligible_candidates_json, rejected_candidates_json,
                pareto_worker_ids_json, selection_explanation_json,
                created_at, request_fingerprint
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17
             )",
            params![
                record.decision_id.0,
                record.task_id.0,
                record.selected_worker_id.as_ref().map(|id| id.0.as_str()),
                record
                    .selected_checker_worker_id
                    .as_ref()
                    .map(|id| id.0.as_str()),
                encode_verification_policy(record.verification_policy),
                record.evidence_snapshot_id,
                record.policy_version,
                record
                    .expected_cash_micros
                    .map(|amount| to_i64("expected_cash_micros", amount))
                    .transpose()?,
                record
                    .expected_quota_milliunits
                    .map(|amount| to_i64("expected_quota_milliunits", amount))
                    .transpose()?,
                record.expected_success_probability,
                record
                    .p95_latency_ms
                    .map(|latency| to_i64("p95_latency_ms", latency))
                    .transpose()?,
                eligible_candidates_json,
                rejected_candidates_json,
                pareto_worker_ids_json,
                selection_explanation_json,
                record.created_at,
                record.request_fingerprint,
            ],
        )?;
        self.secure_files()?;
        Ok(())
    }

    fn append_outcome(&self, record: &PrivateOutcomeRecord) -> Result<(), StoreError> {
        validate_canonical_identifier("outcome.id", &record.event.id)?;
        validate_canonical_identifier("outcome.task_id", &record.event.task_id.0)?;
        validate_canonical_identifier("outcome.worker_id", &record.event.worker_id.0)?;
        validate_canonical_identifier("outcome.skill_id", &record.event.skill_id.0)?;
        if let Some(decision_id) = &record.decision_id {
            validate_canonical_identifier("outcome.decision_id", &decision_id.0)?;
        }
        if let Some(checker_worker_id) = &record.checker_worker_id {
            validate_canonical_identifier("outcome.checker_worker_id", &checker_worker_id.0)?;
        }
        validate_required_text("outcome.observed_at", &record.event.observed_at)?;
        validate_outcome_quote_link(&self.connection, record)?;
        let metadata = serde_json::to_string(&record.event.metadata)?;
        self.connection.execute(
            "INSERT INTO outcome_events (
                id, decision_id, task_id, worker_id, skill_id, accepted,
                validation_kind, actual_cash_micros, actual_quota_milliunits, latency_ms,
                observed_at, repository_scope, metadata_json, checker_worker_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                record.event.id,
                record.decision_id.as_ref().map(|id| id.0.as_str()),
                record.event.task_id.0,
                record.event.worker_id.0,
                record.event.skill_id.0,
                i64::from(record.event.accepted),
                encode_validation_kind(record.event.validation_kind),
                to_i64("actual_cash_micros", record.event.actual_cash_micros)?,
                to_i64(
                    "actual_quota_milliunits",
                    record.event.actual_quota_milliunits,
                )?,
                to_i64("latency_ms", record.event.latency_ms)?,
                record.event.observed_at,
                record.event.repository_scope,
                metadata,
                record.checker_worker_id.as_ref().map(|id| id.0.as_str()),
            ],
        )?;
        self.secure_files()?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database belongs to `{actual}`, not expected `{expected}` trust domain")]
    StoreKindMismatch {
        expected: &'static str,
        actual: String,
    },
    #[error("unsupported database schema version {actual}; expected {expected}")]
    UnsupportedSchemaVersion { expected: i64, actual: i64 },
    #[error("refusing to initialize a non-empty, unversioned database")]
    UnversionedDatabaseNotEmpty,
    #[error("{field} is outside SQLite's non-negative 64-bit integer range")]
    IntegerOutOfRange { field: &'static str },
    #[error("unknown {kind} value `{value}` in database")]
    UnknownEnum { kind: &'static str, value: String },
    #[error("invalid SQLite boolean value {0}")]
    InvalidBoolean(i64),
    #[error("{field} must be finite, got {value}")]
    InvalidReal { field: &'static str, value: f64 },
    #[error("{field} must be a finite value between 0 and 1, got {value}")]
    InvalidProbability { field: &'static str, value: f64 },
    #[error("{field} must be exactly 64 lowercase hexadecimal characters")]
    InvalidSha256 { field: &'static str },
    #[error("{field} must be non-empty and have no leading or trailing whitespace")]
    NonCanonicalIdentifier { field: &'static str },
    #[error("{field} must not be blank")]
    EmptyRequiredField { field: &'static str },
    #[error("invalid worker profile: {0}")]
    InvalidWorkerProfile(String),
    #[error(
        "worker profile `{worker_id}` configuration digest mismatch: expected `{expected}`, found `{actual}`"
    )]
    WorkerConfigurationDigestMismatch {
        worker_id: WorkerId,
        expected: String,
        actual: String,
    },
    #[error("invalid routing quote audit: {0}")]
    InvalidQuoteAudit(String),
    #[error("invalid outcome-to-quote link: {0}")]
    InvalidOutcomeLink(String),
    #[error("invalid snapshot manifest `{snapshot_id}`: {reason}")]
    InvalidSnapshotManifest { snapshot_id: String, reason: String },
    #[error("snapshot `{snapshot_id}` digest mismatch: expected `{expected}`, found `{actual}`")]
    SnapshotDigestMismatch {
        snapshot_id: String,
        expected: String,
        actual: String,
    },
    #[error("snapshot `{0}` does not exist")]
    SnapshotNotFound(String),
    #[error("snapshot references missing {kind} `{id}`")]
    SnapshotMemberMissing { kind: &'static str, id: String },
    #[error("snapshot dependency is not closed: {0}")]
    SnapshotDependencyNotClosed(String),
}

#[derive(Debug)]
struct RawSnapshot(
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
);

#[derive(Debug)]
struct RawQuote {
    decision_id: String,
    task_id: String,
    selected_worker_id: Option<String>,
    selected_checker_worker_id: Option<String>,
    verification_policy: String,
    evidence_snapshot_id: String,
    policy_version: String,
    expected_cash_micros: Option<i64>,
    expected_quota_milliunits: Option<i64>,
    expected_success_probability: Option<f64>,
    p95_latency_ms: Option<i64>,
    eligible_candidates_json: String,
    rejected_candidates_json: String,
    pareto_worker_ids_json: String,
    selection_explanation_json: Option<String>,
    created_at: String,
    request_fingerprint: String,
}

impl TryFrom<RawQuote> for QuoteRecord {
    type Error = StoreError;

    fn try_from(value: RawQuote) -> Result<Self, Self::Error> {
        let record = Self {
            decision_id: DecisionId(value.decision_id),
            task_id: TaskId(value.task_id),
            selected_worker_id: value.selected_worker_id.map(WorkerId),
            selected_checker_worker_id: value.selected_checker_worker_id.map(WorkerId),
            verification_policy: decode_verification_policy(&value.verification_policy)?,
            evidence_snapshot_id: value.evidence_snapshot_id,
            policy_version: value.policy_version,
            expected_cash_micros: value
                .expected_cash_micros
                .map(|amount| from_i64("expected_cash_micros", amount))
                .transpose()?,
            expected_quota_milliunits: value
                .expected_quota_milliunits
                .map(|amount| from_i64("expected_quota_milliunits", amount))
                .transpose()?,
            expected_success_probability: value.expected_success_probability,
            p95_latency_ms: value
                .p95_latency_ms
                .map(|latency| from_i64("p95_latency_ms", latency))
                .transpose()?,
            eligible_candidates: serde_json::from_str(&value.eligible_candidates_json)?,
            rejected_candidates: serde_json::from_str(&value.rejected_candidates_json)?,
            pareto_worker_ids: serde_json::from_str(&value.pareto_worker_ids_json)?,
            selection_explanation: value
                .selection_explanation_json
                .map(|json| serde_json::from_str(&json))
                .transpose()?,
            created_at: value.created_at,
            request_fingerprint: value.request_fingerprint,
        };
        if let Some(probability) = record.expected_success_probability {
            validate_probability("expected_success_probability", probability)?;
        }
        validate_quote_audit(&record)?;
        Ok(record)
    }
}

fn quote_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawQuote> {
    Ok(RawQuote {
        decision_id: row.get(0)?,
        task_id: row.get(1)?,
        selected_worker_id: row.get(2)?,
        selected_checker_worker_id: row.get(3)?,
        verification_policy: row.get(4)?,
        evidence_snapshot_id: row.get(5)?,
        policy_version: row.get(6)?,
        expected_cash_micros: row.get(7)?,
        expected_quota_milliunits: row.get(8)?,
        expected_success_probability: row.get(9)?,
        p95_latency_ms: row.get(10)?,
        eligible_candidates_json: row.get(11)?,
        rejected_candidates_json: row.get(12)?,
        pareto_worker_ids_json: row.get(13)?,
        selection_explanation_json: row.get(14)?,
        created_at: row.get(15)?,
        request_fingerprint: row.get(16)?,
    })
}

impl TryFrom<RawSnapshot> for SnapshotRecord {
    type Error = StoreError;

    fn try_from(value: RawSnapshot) -> Result<Self, Self::Error> {
        let snapshot = Self {
            id: value.0,
            created_at: value.1,
            ontology_version: value.2,
            source_revision: value.3,
            content_sha256: value.4,
            model_release_ids: serde_json::from_str(&value.5)?,
            provider_offering_ids: serde_json::from_str(&value.6)?,
            worker_profile_ids: serde_json::from_str(&value.7)?,
            evidence_ids: serde_json::from_str(&value.8)?,
            model_release_count: from_i64("model_release_count", value.9)?,
            provider_offering_count: from_i64("provider_offering_count", value.10)?,
            worker_profile_count: from_i64("worker_profile_count", value.11)?,
            evidence_count: from_i64("evidence_count", value.12)?,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}

fn snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSnapshot> {
    Ok(RawSnapshot(
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
    ))
}

#[derive(Debug)]
struct RawWorkerProfile {
    id: String,
    offering_id: String,
    harness_id: String,
    harness_version: String,
    reasoning_configuration: String,
    system_prompt_sha256: String,
    skill_pack_version: String,
    toolset_version: String,
    execution_policy_sha256: String,
    supported_skill_ids_json: String,
    tools_json: String,
    privacy_clearance: String,
    configuration_sha256: String,
    recorded_at: String,
}

impl TryFrom<RawWorkerProfile> for WorkerProfileRecord {
    type Error = StoreError;

    fn try_from(value: RawWorkerProfile) -> Result<Self, Self::Error> {
        Ok(Self {
            id: WorkerId(value.id),
            offering_id: OfferingId(value.offering_id),
            harness_id: value.harness_id,
            harness_version: value.harness_version,
            reasoning_configuration: value.reasoning_configuration,
            system_prompt_sha256: value.system_prompt_sha256,
            skill_pack_version: value.skill_pack_version,
            toolset_version: value.toolset_version,
            execution_policy_sha256: value.execution_policy_sha256,
            supported_skill_ids: serde_json::from_str(&value.supported_skill_ids_json)?,
            tools: serde_json::from_str(&value.tools_json)?,
            privacy_clearance: decode_privacy_class(&value.privacy_clearance)?,
            configuration_sha256: value.configuration_sha256,
            recorded_at: value.recorded_at,
        })
    }
}

fn worker_profile_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawWorkerProfile> {
    Ok(RawWorkerProfile {
        id: row.get(0)?,
        offering_id: row.get(1)?,
        harness_id: row.get(2)?,
        harness_version: row.get(3)?,
        reasoning_configuration: row.get(4)?,
        system_prompt_sha256: row.get(5)?,
        skill_pack_version: row.get(6)?,
        toolset_version: row.get(7)?,
        execution_policy_sha256: row.get(8)?,
        supported_skill_ids_json: row.get(9)?,
        tools_json: row.get(10)?,
        privacy_clearance: row.get(11)?,
        configuration_sha256: row.get(12)?,
        recorded_at: row.get(13)?,
    })
}

#[derive(Debug)]
struct RawPublicEvidence {
    id: String,
    model_release_id: String,
    worker_id: Option<String>,
    skill_id: String,
    benchmark_id: String,
    evidence_tier: String,
    raw_score: f64,
    metric: String,
    unit: String,
    normalized_score: Option<f64>,
    adapter_version: String,
    sample_count: Option<i64>,
    observed_at: String,
    source_url: String,
    artifact_sha256: String,
    license: String,
}

impl TryFrom<RawPublicEvidence> for PublicEvidenceRecord {
    type Error = StoreError;

    fn try_from(value: RawPublicEvidence) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            model_release_id: ModelReleaseId(value.model_release_id),
            worker_id: value.worker_id.map(WorkerId),
            skill_id: SkillId(value.skill_id),
            benchmark_id: BenchmarkId(value.benchmark_id),
            evidence_tier: decode_evidence_tier(&value.evidence_tier)?,
            raw_score: value.raw_score,
            metric: value.metric,
            unit: value.unit,
            normalized_score: value.normalized_score,
            adapter_version: value.adapter_version,
            sample_count: value
                .sample_count
                .map(|count| from_i64("sample_count", count))
                .transpose()?,
            observed_at: value.observed_at,
            source_url: value.source_url,
            artifact_sha256: value.artifact_sha256,
            license: value.license,
        })
    }
}

fn public_evidence_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawPublicEvidence> {
    Ok(RawPublicEvidence {
        id: row.get(0)?,
        model_release_id: row.get(1)?,
        worker_id: row.get(2)?,
        skill_id: row.get(3)?,
        benchmark_id: row.get(4)?,
        evidence_tier: row.get(5)?,
        raw_score: row.get(6)?,
        metric: row.get(7)?,
        unit: row.get(8)?,
        normalized_score: row.get(9)?,
        adapter_version: row.get(10)?,
        sample_count: row.get(11)?,
        observed_at: row.get(12)?,
        source_url: row.get(13)?,
        artifact_sha256: row.get(14)?,
        license: row.get(15)?,
    })
}

fn read_provider_offering(
    connection: &Connection,
    id: &str,
) -> Result<Option<ProviderOfferingRecord>, StoreError> {
    let raw = connection
        .query_row(
            "SELECT id, model_release_id, provider, supersedes_offering_id,
                    effective_from_epoch_ms, effective_until_epoch_ms, currency,
                    input_micros_per_million_tokens, output_micros_per_million_tokens,
                    fixed_request_micros, quota_milliunits_per_request,
                    context_window_tokens, source_url, recorded_at
             FROM provider_offerings WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                ))
            },
        )
        .optional()?;
    raw.map(
        |(
            id,
            model,
            provider,
            supersedes,
            from,
            until,
            currency,
            input,
            output,
            fixed,
            quota,
            context,
            source,
            recorded,
        )| {
            Ok(ProviderOfferingRecord {
                id: OfferingId(id),
                model_release_id: ModelReleaseId(model),
                provider,
                supersedes_offering_id: supersedes.map(OfferingId),
                effective_from_epoch_ms: from,
                effective_until_epoch_ms: until,
                currency,
                input_micros_per_million_tokens: from_i64(
                    "input_micros_per_million_tokens",
                    input,
                )?,
                output_micros_per_million_tokens: from_i64(
                    "output_micros_per_million_tokens",
                    output,
                )?,
                fixed_request_micros: from_i64("fixed_request_micros", fixed)?,
                quota_milliunits_per_request: from_i64("quota_milliunits_per_request", quota)?,
                context_window_tokens: from_i64("context_window_tokens", context)?,
                source_url: source,
                recorded_at: recorded,
            })
        },
    )
    .transpose()
}

fn validate_snapshot_dependencies(
    connection: &Connection,
    snapshot: &SnapshotRecord,
) -> Result<(), StoreError> {
    snapshot.validate()?;
    let model_ids = snapshot
        .model_release_ids
        .iter()
        .map(|id| id.0.as_str())
        .collect::<BTreeSet<_>>();
    let offering_ids = snapshot
        .provider_offering_ids
        .iter()
        .map(|id| id.0.as_str())
        .collect::<BTreeSet<_>>();
    let worker_ids = snapshot
        .worker_profile_ids
        .iter()
        .map(|id| id.0.as_str())
        .collect::<BTreeSet<_>>();

    for id in &snapshot.model_release_ids {
        require_model_release(connection, &id.0)?;
    }

    for id in &snapshot.provider_offering_ids {
        let (release_id, predecessor_id) = connection
            .query_row(
                "SELECT model_release_id, supersedes_offering_id
                 FROM provider_offerings WHERE id = ?1",
                [&id.0],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?
            .ok_or_else(|| StoreError::SnapshotMemberMissing {
                kind: "provider offering",
                id: id.0.clone(),
            })?;
        if !model_ids.contains(release_id.as_str()) {
            return Err(StoreError::SnapshotDependencyNotClosed(format!(
                "offering `{}` requires model release `{release_id}`",
                id.0
            )));
        }
        if let Some(predecessor_id) = predecessor_id {
            if !offering_ids.contains(predecessor_id.as_str()) {
                return Err(StoreError::SnapshotDependencyNotClosed(format!(
                    "offering `{}` supersedes provider offering `{predecessor_id}`, which is absent",
                    id.0
                )));
            }
        }
    }

    for id in &snapshot.worker_profile_ids {
        let offering_id = connection
            .query_row(
                "SELECT offering_id FROM worker_profiles WHERE id = ?1",
                [&id.0],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::SnapshotMemberMissing {
                kind: "worker profile",
                id: id.0.clone(),
            })?;
        if !offering_ids.contains(offering_id.as_str()) {
            return Err(StoreError::SnapshotDependencyNotClosed(format!(
                "worker `{}` requires provider offering `{offering_id}`",
                id.0
            )));
        }
    }

    for id in &snapshot.evidence_ids {
        let dependency = connection
            .query_row(
                "SELECT model_release_id, worker_id
                 FROM evidence_observations WHERE id = ?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?
            .ok_or_else(|| StoreError::SnapshotMemberMissing {
                kind: "evidence observation",
                id: id.clone(),
            })?;
        if !model_ids.contains(dependency.0.as_str()) {
            return Err(StoreError::SnapshotDependencyNotClosed(format!(
                "evidence `{id}` requires model release `{}`",
                dependency.0
            )));
        }
        if let Some(worker_id) = dependency.1 {
            if !worker_ids.contains(worker_id.as_str()) {
                return Err(StoreError::SnapshotDependencyNotClosed(format!(
                    "evidence `{id}` requires worker `{worker_id}`"
                )));
            }
        }
    }

    Ok(())
}

fn require_model_release(connection: &Connection, id: &str) -> Result<(), StoreError> {
    let exists = connection
        .query_row("SELECT 1 FROM model_releases WHERE id = ?1", [id], |_| {
            Ok(())
        })
        .optional()?
        .is_some();
    if exists {
        Ok(())
    } else {
        Err(StoreError::SnapshotMemberMissing {
            kind: "model release",
            id: id.to_owned(),
        })
    }
}

fn configure_file_connection(connection: &Connection) -> Result<(), StoreError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(())
}

fn configure_memory_connection(connection: &Connection) -> Result<(), StoreError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

fn initialize_or_validate_store(
    connection: &Connection,
    expected_kind: &'static str,
    expected_version: i64,
    schema: &str,
) -> Result<(), StoreError> {
    let actual_version = schema_version(connection)?;
    if actual_version == 0 {
        let table_count: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        if table_count != 0 {
            return Err(StoreError::UnversionedDatabaseNotEmpty);
        }
        let transaction = connection.unchecked_transaction()?;
        initialize_identity(&transaction, expected_kind)?;
        transaction.execute_batch(schema)?;
        validate_schema_version(&transaction, expected_version)?;
        transaction.commit()?;
        Ok(())
    } else {
        validate_schema_version(connection, expected_version)?;
        validate_identity(connection, expected_kind)
    }
}

fn schema_version(connection: &Connection) -> Result<i64, StoreError> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(StoreError::from)
}

fn validate_schema_version(
    connection: &Connection,
    expected_version: i64,
) -> Result<(), StoreError> {
    let actual_version = schema_version(connection)?;
    if actual_version == expected_version {
        Ok(())
    } else {
        Err(StoreError::UnsupportedSchemaVersion {
            expected: expected_version,
            actual: actual_version,
        })
    }
}

fn initialize_identity(connection: &Connection, expected: &'static str) -> Result<(), StoreError> {
    connection.execute_batch(IDENTITY_SCHEMA)?;
    connection.execute(
        "INSERT OR IGNORE INTO workforce_store_identity (singleton, kind) VALUES (1, ?1)",
        [expected],
    )?;
    validate_identity(connection, expected)
}

fn validate_manifest_list<T: Ord + ToString>(
    snapshot_id: &str,
    field: &'static str,
    declared_count: u64,
    values: &[T],
) -> Result<(), StoreError> {
    let actual_count =
        u64::try_from(values.len()).map_err(|_| StoreError::IntegerOutOfRange { field })?;
    if actual_count != declared_count {
        return Err(StoreError::InvalidSnapshotManifest {
            snapshot_id: snapshot_id.to_owned(),
            reason: format!(
                "{field} declares {declared_count} members but contains {actual_count}"
            ),
        });
    }
    if values.iter().any(|value| {
        let value = value.to_string();
        value.trim().is_empty() || value.trim() != value
    }) {
        return Err(StoreError::InvalidSnapshotManifest {
            snapshot_id: snapshot_id.to_owned(),
            reason: format!("{field} contains a non-canonical identifier"),
        });
    }
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(StoreError::InvalidSnapshotManifest {
            snapshot_id: snapshot_id.to_owned(),
            reason: format!("{field} must be strictly sorted and duplicate-free"),
        });
    }
    Ok(())
}

fn hash_component(hasher: &mut Sha256, value: &str) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn hash_id_list<'value>(
    hasher: &mut Sha256,
    kind: &str,
    count: u64,
    values: impl Iterator<Item = &'value str>,
) {
    hash_component(hasher, kind);
    hasher.update(count.to_be_bytes());
    for value in values {
        hash_component(hasher, value);
    }
}

fn lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn required_snapshot_member<T>(
    kind: &'static str,
    id: &str,
    value: Option<T>,
) -> Result<T, StoreError> {
    value.ok_or_else(|| StoreError::SnapshotMemberMissing {
        kind,
        id: id.to_owned(),
    })
}

fn validate_export_dependency_closure(
    snapshot: &SnapshotRecord,
    models: &[ModelReleaseRecord],
    offerings: &[ProviderOfferingRecord],
    workers: &[WorkerProfileRecord],
    evidence: &[PublicEvidenceRecord],
) -> Result<(), StoreError> {
    let model_ids = models
        .iter()
        .map(|record| record.id.0.as_str())
        .collect::<BTreeSet<_>>();
    let offering_dependencies = offerings
        .iter()
        .map(|record| {
            (
                record.id.0.as_str(),
                (
                    record.model_release_id.0.as_str(),
                    record
                        .supersedes_offering_id
                        .as_ref()
                        .map(|id| id.0.as_str()),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let worker_offerings = workers
        .iter()
        .map(|record| (record.id.0.as_str(), record.offering_id.0.as_str()))
        .collect::<BTreeMap<_, _>>();

    if model_ids
        != snapshot
            .model_release_ids
            .iter()
            .map(|id| id.0.as_str())
            .collect()
        || offering_dependencies
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != snapshot
                .provider_offering_ids
                .iter()
                .map(|id| id.0.as_str())
                .collect()
        || worker_offerings.keys().copied().collect::<BTreeSet<_>>()
            != snapshot
                .worker_profile_ids
                .iter()
                .map(|id| id.0.as_str())
                .collect()
        || evidence
            .iter()
            .map(|record| record.id.as_str())
            .collect::<BTreeSet<_>>()
            != snapshot.evidence_ids.iter().map(String::as_str).collect()
    {
        return Err(StoreError::InvalidSnapshotManifest {
            snapshot_id: snapshot.id.clone(),
            reason: "reader returned records that do not exactly match the manifest".to_owned(),
        });
    }

    for (offering_id, (release_id, predecessor_id)) in &offering_dependencies {
        if !model_ids.contains(*release_id) {
            return Err(StoreError::SnapshotDependencyNotClosed(format!(
                "offering `{offering_id}` requires model release `{release_id}`"
            )));
        }
        if let Some(predecessor_id) = predecessor_id {
            if !offering_dependencies.contains_key(predecessor_id) {
                return Err(StoreError::SnapshotDependencyNotClosed(format!(
                    "offering `{offering_id}` supersedes provider offering `{predecessor_id}`, which is absent"
                )));
            }
        }
    }
    for (worker_id, offering_id) in &worker_offerings {
        if !offering_dependencies.contains_key(*offering_id) {
            return Err(StoreError::SnapshotDependencyNotClosed(format!(
                "worker `{worker_id}` requires provider offering `{offering_id}`"
            )));
        }
    }
    for observation in evidence {
        if !model_ids.contains(observation.model_release_id.0.as_str()) {
            return Err(StoreError::SnapshotDependencyNotClosed(format!(
                "evidence `{}` requires model release `{}`",
                observation.id, observation.model_release_id
            )));
        }
        if let Some(worker_id) = &observation.worker_id {
            let offering_id = *worker_offerings.get(worker_id.0.as_str()).ok_or_else(|| {
                StoreError::SnapshotDependencyNotClosed(format!(
                    "evidence `{}` requires worker `{worker_id}`",
                    observation.id
                ))
            })?;
            let release_id = offering_dependencies
                .get(offering_id)
                .map(|(release_id, _)| *release_id)
                .ok_or_else(|| {
                    StoreError::SnapshotDependencyNotClosed(format!(
                        "worker `{worker_id}` requires provider offering `{offering_id}`"
                    ))
                })?;
            if release_id != observation.model_release_id.0.as_str() {
                return Err(StoreError::SnapshotDependencyNotClosed(format!(
                    "evidence `{}` worker `{worker_id}` belongs to release `{release_id}`, not `{}`",
                    observation.id, observation.model_release_id
                )));
            }
        }
    }
    Ok(())
}

fn validate_identity(connection: &Connection, expected: &'static str) -> Result<(), StoreError> {
    let actual: String = connection.query_row(
        "SELECT kind FROM workforce_store_identity WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if actual == expected {
        Ok(())
    } else {
        Err(StoreError::StoreKindMismatch { expected, actual })
    }
}

fn to_i64(field: &'static str, value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::IntegerOutOfRange { field })
}

fn from_i64(field: &'static str, value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::IntegerOutOfRange { field })
}

fn validate_canonical_identifier(field: &'static str, value: &str) -> Result<(), StoreError> {
    if value.trim().is_empty() || value.trim() != value {
        Err(StoreError::NonCanonicalIdentifier { field })
    } else {
        Ok(())
    }
}

fn validate_required_text(field: &'static str, value: &str) -> Result<(), StoreError> {
    if value.trim().is_empty() {
        Err(StoreError::EmptyRequiredField { field })
    } else {
        Ok(())
    }
}

fn worker_identity(
    record: &WorkerProfileRecord,
    model_release_id: ModelReleaseId,
    provider: String,
) -> WorkerIdentity {
    WorkerIdentity {
        worker_id: record.id.clone(),
        model_release_id,
        offering_id: record.offering_id.clone(),
        provider,
        harness_id: record.harness_id.clone(),
        harness_version: record.harness_version.clone(),
        reasoning_configuration: record.reasoning_configuration.clone(),
        system_prompt_sha256: record.system_prompt_sha256.clone(),
        skill_pack_version: record.skill_pack_version.clone(),
        toolset_version: record.toolset_version.clone(),
        execution_policy_sha256: record.execution_policy_sha256.clone(),
    }
}

/// SHA-256 over a worker identity's canonical configuration key.
///
/// Exposed because any source adapter that appends a [`WorkerProfileRecord`]
/// must supply this digest, and the store rejects a record whose digest it
/// cannot reproduce.
pub fn worker_configuration_sha256(identity: &WorkerIdentity) -> String {
    lower_hex(&Sha256::digest(identity.configuration_key().as_bytes()))
}

fn validate_finite(field: &'static str, value: f64) -> Result<(), StoreError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(StoreError::InvalidReal { field, value })
    }
}

fn validate_probability(field: &'static str, value: f64) -> Result<(), StoreError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(StoreError::InvalidProbability { field, value })
    }
}

fn validate_outcome_quote_link(
    connection: &Connection,
    record: &PrivateOutcomeRecord,
) -> Result<(), StoreError> {
    if record.checker_worker_id.as_ref() == Some(&record.event.worker_id) {
        return Err(StoreError::InvalidOutcomeLink(
            "the maker cannot validate its own outcome".to_owned(),
        ));
    }

    let Some(decision_id) = &record.decision_id else {
        return Ok(());
    };
    let quote = connection
        .query_row(
            "SELECT task_id, selected_worker_id, selected_checker_worker_id,
                    verification_policy
             FROM routing_quotes WHERE decision_id = ?1",
            [&decision_id.0],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            StoreError::InvalidOutcomeLink(format!(
                "decision `{decision_id}` does not identify a recorded quote"
            ))
        })?;
    let (quoted_task_id, quoted_maker_id, quoted_checker_id, encoded_policy) = quote;

    if quoted_task_id != record.event.task_id.0 {
        return Err(StoreError::InvalidOutcomeLink(format!(
            "outcome task `{}` does not match quoted task `{quoted_task_id}`",
            record.event.task_id
        )));
    }
    if quoted_maker_id.as_deref() != Some(record.event.worker_id.0.as_str()) {
        return Err(StoreError::InvalidOutcomeLink(format!(
            "outcome maker `{}` does not match the quote's selected maker",
            record.event.worker_id
        )));
    }

    let policy = decode_verification_policy(&encoded_policy)?;
    match policy {
        VerificationPolicy::MakerChecker => {
            let expected_checker_id = quoted_checker_id.as_deref().ok_or_else(|| {
                StoreError::InvalidOutcomeLink(
                    "maker-checker quote has no selected checker".to_owned(),
                )
            })?;
            if record.checker_worker_id.as_ref().map(|id| id.0.as_str())
                != Some(expected_checker_id)
            {
                return Err(StoreError::InvalidOutcomeLink(format!(
                    "maker-checker outcome must use selected checker `{expected_checker_id}`"
                )));
            }
        }
        VerificationPolicy::Deterministic | VerificationPolicy::HumanApproval => {
            // These policies do not require a model checker. If an outcome does
            // name one, it must still be the checker preserved by the quote.
            if let Some(outcome_checker_id) = &record.checker_worker_id {
                if quoted_checker_id.as_deref() != Some(outcome_checker_id.0.as_str()) {
                    return Err(StoreError::InvalidOutcomeLink(format!(
                        "outcome checker `{outcome_checker_id}` was not selected by the quote"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), StoreError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(StoreError::InvalidSha256 { field })
    }
}

fn decode_bool(value: i64) -> Result<bool, StoreError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(StoreError::InvalidBoolean(value)),
    }
}

const fn encode_privacy_class(value: PrivacyClass) -> &'static str {
    match value {
        PrivacyClass::Public => "public",
        PrivacyClass::PrivateMetadata => "private_metadata",
        PrivacyClass::ConfidentialContent => "confidential_content",
        PrivacyClass::Secret => "secret",
    }
}

fn decode_privacy_class(value: &str) -> Result<PrivacyClass, StoreError> {
    match value {
        "public" => Ok(PrivacyClass::Public),
        "private_metadata" => Ok(PrivacyClass::PrivateMetadata),
        "confidential_content" => Ok(PrivacyClass::ConfidentialContent),
        "secret" => Ok(PrivacyClass::Secret),
        _ => Err(StoreError::UnknownEnum {
            kind: "privacy class",
            value: value.to_owned(),
        }),
    }
}

const fn encode_verification_policy(value: VerificationPolicy) -> &'static str {
    match value {
        VerificationPolicy::Deterministic => "deterministic",
        VerificationPolicy::MakerChecker => "maker_checker",
        VerificationPolicy::HumanApproval => "human_approval",
    }
}

fn decode_verification_policy(value: &str) -> Result<VerificationPolicy, StoreError> {
    match value {
        "deterministic" => Ok(VerificationPolicy::Deterministic),
        "maker_checker" => Ok(VerificationPolicy::MakerChecker),
        "human_approval" => Ok(VerificationPolicy::HumanApproval),
        _ => Err(StoreError::UnknownEnum {
            kind: "verification policy",
            value: value.to_owned(),
        }),
    }
}

fn validate_quote_audit(record: &QuoteRecord) -> Result<(), StoreError> {
    validate_sha256("quote.request_fingerprint", &record.request_fingerprint)?;
    if record
        .selected_worker_id
        .as_ref()
        .is_some_and(WorkerId::is_empty)
    {
        return Err(StoreError::InvalidQuoteAudit(
            "selected worker identifier must be non-empty".to_owned(),
        ));
    }
    if record.selected_worker_id.as_ref() == record.selected_checker_worker_id.as_ref()
        && record.selected_worker_id.is_some()
    {
        return Err(StoreError::InvalidQuoteAudit(
            "selected maker cannot be its own checker".to_owned(),
        ));
    }

    let mut candidate_ids = BTreeSet::new();
    for (index, candidate) in record.eligible_candidates.iter().enumerate() {
        if candidate.worker_id.is_empty() || !candidate_ids.insert(candidate.worker_id.clone()) {
            return Err(StoreError::InvalidQuoteAudit(
                "eligible candidate identifiers must be non-empty and unique".to_owned(),
            ));
        }
        let expected_rank = u64::try_from(index + 1).unwrap_or(u64::MAX);
        if candidate.rank != expected_rank {
            return Err(StoreError::InvalidQuoteAudit(
                "eligible candidate ranks must be contiguous and match vector order".to_owned(),
            ));
        }
        validate_probability("candidate.success_mean", candidate.success_mean)?;
        validate_probability(
            "candidate.success_lower_bound",
            candidate.success_lower_bound,
        )?;
        if candidate.success_lower_bound > candidate.success_mean {
            return Err(StoreError::InvalidQuoteAudit(
                "candidate lower confidence bound exceeds its mean".to_owned(),
            ));
        }
        if !candidate.cost_breakdown.is_object() {
            return Err(StoreError::InvalidQuoteAudit(
                "candidate cost breakdown must be a JSON object".to_owned(),
            ));
        }
        let cost = candidate
            .cost_breakdown
            .as_object()
            .expect("object checked above");
        if cost
            .get("expected_cash_micros")
            .and_then(serde_json::Value::as_u64)
            != Some(candidate.expected_cash_micros)
            || cost
                .get("expected_quota_milliunits")
                .and_then(serde_json::Value::as_u64)
                != Some(candidate.expected_quota_milliunits)
            || cost
                .get("expected_accepted_cost_micros")
                .and_then(serde_json::Value::as_u64)
                != Some(candidate.expected_accepted_cost_micros)
        {
            return Err(StoreError::InvalidQuoteAudit(
                "candidate aggregate costs conflict with its cost breakdown".to_owned(),
            ));
        }
        if candidate.checker_worker_id.as_ref() == Some(&candidate.worker_id) {
            return Err(StoreError::InvalidQuoteAudit(
                "candidate maker cannot be its own checker".to_owned(),
            ));
        }
    }

    let mut rejected_ids = BTreeSet::new();
    for rejected in &record.rejected_candidates {
        if rejected.worker_id.is_empty()
            || !rejected_ids.insert(rejected.worker_id.clone())
            || candidate_ids.contains(&rejected.worker_id)
        {
            return Err(StoreError::InvalidQuoteAudit(
                "rejected candidates must be non-empty, unique, and ineligible".to_owned(),
            ));
        }
        if rejected.reasons.is_empty()
            || rejected.reasons.iter().any(|reason| {
                reason
                    .as_object()
                    .and_then(|object| object.get("code"))
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(str::is_empty)
            })
        {
            return Err(StoreError::InvalidQuoteAudit(
                "every rejected candidate needs structured reason objects with codes".to_owned(),
            ));
        }
    }

    let pareto_ids = record
        .pareto_worker_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if pareto_ids.len() != record.pareto_worker_ids.len() || !pareto_ids.is_subset(&candidate_ids) {
        return Err(StoreError::InvalidQuoteAudit(
            "Pareto worker identifiers must be unique eligible candidates".to_owned(),
        ));
    }
    let flagged_pareto = record
        .eligible_candidates
        .iter()
        .filter(|candidate| candidate.pareto_efficient)
        .map(|candidate| candidate.worker_id.clone())
        .collect::<BTreeSet<_>>();
    if pareto_ids != flagged_pareto {
        return Err(StoreError::InvalidQuoteAudit(
            "Pareto manifest does not match candidate flags".to_owned(),
        ));
    }

    if let Some(selected_worker_id) = &record.selected_worker_id {
        if record.verification_policy == VerificationPolicy::MakerChecker
            && record.selected_checker_worker_id.is_none()
        {
            return Err(StoreError::InvalidQuoteAudit(
                "maker-checker verification requires a selected checker".to_owned(),
            ));
        }
        let selected = record
            .eligible_candidates
            .iter()
            .find(|candidate| &candidate.worker_id == selected_worker_id)
            .ok_or_else(|| {
                StoreError::InvalidQuoteAudit(
                    "selected worker is absent from eligible candidates".to_owned(),
                )
            })?;
        if selected.rank != 1 {
            return Err(StoreError::InvalidQuoteAudit(
                "selected candidate must have rank 1".to_owned(),
            ));
        }
        if selected.checker_worker_id != record.selected_checker_worker_id
            || record.expected_cash_micros != Some(selected.expected_cash_micros)
            || record.expected_quota_milliunits != Some(selected.expected_quota_milliunits)
            || record.expected_success_probability.map(f64::to_bits)
                != Some(selected.success_mean.to_bits())
            || record.p95_latency_ms != Some(selected.p95_latency_ms)
        {
            return Err(StoreError::InvalidQuoteAudit(
                "selected summary does not match the selected candidate audit".to_owned(),
            ));
        }
        let explanation = record.selection_explanation.as_ref().ok_or_else(|| {
            StoreError::InvalidQuoteAudit(
                "a selected routing decision requires an explanation".to_owned(),
            )
        })?;
        if explanation.objective.trim().is_empty()
            || explanation.tie_break_order.is_empty()
            || explanation
                .tie_break_order
                .iter()
                .any(|rule| rule.trim().is_empty())
            || explanation.eligible_candidate_count
                != u64::try_from(record.eligible_candidates.len()).unwrap_or(u64::MAX)
        {
            return Err(StoreError::InvalidQuoteAudit(
                "selection explanation is incomplete or has the wrong candidate count".to_owned(),
            ));
        }
    } else if !record.eligible_candidates.is_empty()
        || record.rejected_candidates.is_empty()
        || record.selected_checker_worker_id.is_some()
        || record.expected_cash_micros.is_some()
        || record.expected_quota_milliunits.is_some()
        || record.expected_success_probability.is_some()
        || record.p95_latency_ms.is_some()
        || record.selection_explanation.is_some()
    {
        return Err(StoreError::InvalidQuoteAudit(
            "an unselected decision requires zero eligible candidates, at least one rejection, and no winner fields"
                .to_owned(),
        ));
    }
    Ok(())
}

const fn encode_evidence_tier(value: EvidenceTier) -> &'static str {
    match value {
        EvidenceTier::ProjectReproduced => "project_reproduced",
        EvidenceTier::IndependentSigned => "independent_signed",
        EvidenceTier::CommunityReproducible => "community_reproducible",
        EvidenceTier::VendorReported => "vendor_reported",
    }
}

fn decode_evidence_tier(value: &str) -> Result<EvidenceTier, StoreError> {
    match value {
        "project_reproduced" => Ok(EvidenceTier::ProjectReproduced),
        "independent_signed" => Ok(EvidenceTier::IndependentSigned),
        "community_reproducible" => Ok(EvidenceTier::CommunityReproducible),
        "vendor_reported" => Ok(EvidenceTier::VendorReported),
        _ => Err(StoreError::UnknownEnum {
            kind: "evidence tier",
            value: value.to_owned(),
        }),
    }
}

const fn encode_validation_kind(value: ValidationKind) -> &'static str {
    match value {
        ValidationKind::Deterministic => "deterministic",
        ValidationKind::Human => "human",
        ValidationKind::IndependentModel => "independent_model",
        ValidationKind::SelfReported => "self_reported",
    }
}

fn decode_validation_kind(value: &str) -> Result<ValidationKind, StoreError> {
    match value {
        "deterministic" => Ok(ValidationKind::Deterministic),
        "human" => Ok(ValidationKind::Human),
        "independent_model" => Ok(ValidationKind::IndependentModel),
        "self_reported" => Ok(ValidationKind::SelfReported),
        _ => Err(StoreError::UnknownEnum {
            kind: "validation kind",
            value: value.to_owned(),
        }),
    }
}

#[cfg(unix)]
fn prepare_private_database_file(path: &Path) -> Result<(), std::io::Error> {
    use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};

    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(path)?;
    set_owner_only(path)
}

#[cfg(not(unix))]
fn prepare_private_database_file(path: &Path) -> Result<(), std::io::Error> {
    use std::fs::OpenOptions;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)?;
    Ok(())
}

#[cfg(unix)]
fn secure_private_sqlite_files(path: &Path) -> Result<(), std::io::Error> {
    set_owner_only(path)?;
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if sidecar.exists() {
            set_owner_only(&sidecar)?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn secure_private_sqlite_files(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), std::io::Error> {
    use std::{fs, os::unix::fs::PermissionsExt};

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

const IDENTITY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS workforce_store_identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    kind TEXT NOT NULL CHECK (kind IN ('public_index', 'private_local'))
) STRICT;

CREATE TRIGGER IF NOT EXISTS workforce_store_identity_no_update
BEFORE UPDATE ON workforce_store_identity
BEGIN
    SELECT RAISE(ABORT, 'store identity is immutable');
END;

CREATE TRIGGER IF NOT EXISTS workforce_store_identity_no_delete
BEFORE DELETE ON workforce_store_identity
BEGIN
    SELECT RAISE(ABORT, 'store identity is immutable');
END;
"#;

const PUBLIC_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS model_releases (
    id TEXT PRIMARY KEY,
    developer TEXT NOT NULL CHECK (length(trim(developer)) > 0),
    model_family TEXT NOT NULL CHECK (length(trim(model_family)) > 0),
    released_at TEXT NOT NULL CHECK (length(trim(released_at)) > 0),
    context_window_tokens INTEGER NOT NULL CHECK (context_window_tokens >= 0),
    source_url TEXT NOT NULL CHECK (length(trim(source_url)) > 0),
    artifact_sha256 TEXT NOT NULL CHECK (
        length(artifact_sha256) = 64
        AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    recorded_at TEXT NOT NULL CHECK (length(trim(recorded_at)) > 0)
) STRICT;

CREATE TABLE IF NOT EXISTS provider_offerings (
    id TEXT PRIMARY KEY,
    model_release_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (length(trim(provider)) > 0),
    supersedes_offering_id TEXT UNIQUE,
    effective_from_epoch_ms INTEGER NOT NULL,
    effective_until_epoch_ms INTEGER,
    currency TEXT NOT NULL CHECK (length(trim(currency)) > 0),
    input_micros_per_million_tokens INTEGER NOT NULL
        CHECK (input_micros_per_million_tokens >= 0),
    output_micros_per_million_tokens INTEGER NOT NULL
        CHECK (output_micros_per_million_tokens >= 0),
    fixed_request_micros INTEGER NOT NULL CHECK (fixed_request_micros >= 0),
    quota_milliunits_per_request INTEGER NOT NULL
        CHECK (quota_milliunits_per_request >= 0),
    context_window_tokens INTEGER NOT NULL CHECK (context_window_tokens >= 0),
    source_url TEXT NOT NULL CHECK (length(trim(source_url)) > 0),
    recorded_at TEXT NOT NULL CHECK (length(trim(recorded_at)) > 0),
    CHECK (
        effective_until_epoch_ms IS NULL
        OR effective_until_epoch_ms > effective_from_epoch_ms
    ),
    FOREIGN KEY (model_release_id) REFERENCES model_releases(id) ON DELETE RESTRICT,
    FOREIGN KEY (supersedes_offering_id)
        REFERENCES provider_offerings(id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX IF NOT EXISTS offerings_by_release_and_time
ON provider_offerings(
    model_release_id, effective_from_epoch_ms, effective_until_epoch_ms
);

CREATE TRIGGER IF NOT EXISTS offering_revision_matches_predecessor
BEFORE INSERT ON provider_offerings
WHEN NEW.supersedes_offering_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM provider_offerings AS predecessor
    WHERE predecessor.id = NEW.supersedes_offering_id
      AND predecessor.model_release_id = NEW.model_release_id
      AND predecessor.provider = NEW.provider
      AND predecessor.effective_from_epoch_ms <= NEW.effective_from_epoch_ms
      AND (
          predecessor.effective_until_epoch_ms IS NULL
          OR predecessor.effective_until_epoch_ms <= NEW.effective_from_epoch_ms
      )
)
BEGIN
    SELECT RAISE(
        ABORT,
        'offering revision must preserve provider and release and move time forward'
    );
END;

CREATE TABLE IF NOT EXISTS worker_profiles (
    id TEXT PRIMARY KEY,
    offering_id TEXT NOT NULL,
    harness_id TEXT NOT NULL CHECK (length(trim(harness_id)) > 0),
    harness_version TEXT NOT NULL CHECK (length(trim(harness_version)) > 0),
    reasoning_configuration TEXT NOT NULL
        CHECK (length(trim(reasoning_configuration)) > 0),
    system_prompt_sha256 TEXT NOT NULL CHECK (
        length(system_prompt_sha256) = 64
        AND system_prompt_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    skill_pack_version TEXT NOT NULL CHECK (length(trim(skill_pack_version)) > 0),
    toolset_version TEXT NOT NULL CHECK (length(trim(toolset_version)) > 0),
    execution_policy_sha256 TEXT NOT NULL CHECK (
        length(execution_policy_sha256) = 64
        AND execution_policy_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    supported_skill_ids_json TEXT NOT NULL CHECK (
        json_valid(supported_skill_ids_json)
        AND json_type(supported_skill_ids_json) = 'array'
    ),
    tools_json TEXT NOT NULL CHECK (
        json_valid(tools_json) AND json_type(tools_json) = 'array'
    ),
    privacy_clearance TEXT NOT NULL CHECK (privacy_clearance IN (
        'public', 'private_metadata', 'confidential_content', 'secret'
    )),
    configuration_sha256 TEXT NOT NULL UNIQUE CHECK (
        length(configuration_sha256) = 64
        AND configuration_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    recorded_at TEXT NOT NULL CHECK (length(trim(recorded_at)) > 0),
    FOREIGN KEY (offering_id) REFERENCES provider_offerings(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE IF NOT EXISTS evidence_observations (
    id TEXT PRIMARY KEY,
    model_release_id TEXT NOT NULL,
    worker_id TEXT,
    skill_id TEXT NOT NULL,
    benchmark_id TEXT NOT NULL CHECK (length(trim(benchmark_id)) > 0),
    evidence_tier TEXT NOT NULL CHECK (evidence_tier IN (
        'project_reproduced', 'independent_signed',
        'community_reproducible', 'vendor_reported'
    )),
    raw_score REAL NOT NULL,
    metric TEXT NOT NULL CHECK (length(trim(metric)) > 0),
    unit TEXT NOT NULL CHECK (length(trim(unit)) > 0),
    normalized_score REAL CHECK (
        normalized_score IS NULL OR
        (normalized_score >= 0.0 AND normalized_score <= 1.0)
    ),
    adapter_version TEXT NOT NULL CHECK (length(trim(adapter_version)) > 0),
    sample_count INTEGER CHECK (sample_count IS NULL OR sample_count >= 1),
    observed_at TEXT NOT NULL CHECK (length(trim(observed_at)) > 0),
    source_url TEXT NOT NULL CHECK (length(trim(source_url)) > 0),
    artifact_sha256 TEXT NOT NULL CHECK (
        length(artifact_sha256) = 64
        AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    license TEXT NOT NULL CHECK (length(trim(license)) > 0),
    FOREIGN KEY (model_release_id) REFERENCES model_releases(id) ON DELETE RESTRICT,
    FOREIGN KEY (worker_id) REFERENCES worker_profiles(id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER IF NOT EXISTS evidence_worker_release_matches
BEFORE INSERT ON evidence_observations
WHEN NEW.worker_id IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM worker_profiles AS worker
    JOIN provider_offerings AS offering ON offering.id = worker.offering_id
    WHERE worker.id = NEW.worker_id
      AND offering.model_release_id = NEW.model_release_id
)
BEGIN
    SELECT RAISE(ABORT, 'evidence worker and model release do not match');
END;

CREATE INDEX IF NOT EXISTS evidence_by_worker_skill
ON evidence_observations(worker_id, skill_id, observed_at);

CREATE TABLE IF NOT EXISTS snapshots (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    ontology_version TEXT NOT NULL CHECK (length(trim(ontology_version)) > 0),
    source_revision TEXT NOT NULL CHECK (length(trim(source_revision)) > 0),
    content_sha256 TEXT NOT NULL CHECK (
        length(content_sha256) = 64
        AND content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    model_release_ids_json TEXT NOT NULL CHECK (
        json_valid(model_release_ids_json)
        AND json_type(model_release_ids_json) = 'array'
    ),
    provider_offering_ids_json TEXT NOT NULL CHECK (
        json_valid(provider_offering_ids_json)
        AND json_type(provider_offering_ids_json) = 'array'
    ),
    worker_profile_ids_json TEXT NOT NULL CHECK (
        json_valid(worker_profile_ids_json)
        AND json_type(worker_profile_ids_json) = 'array'
    ),
    evidence_ids_json TEXT NOT NULL CHECK (
        json_valid(evidence_ids_json) AND json_type(evidence_ids_json) = 'array'
    ),
    model_release_count INTEGER NOT NULL CHECK (model_release_count >= 0),
    provider_offering_count INTEGER NOT NULL CHECK (provider_offering_count >= 0),
    worker_profile_count INTEGER NOT NULL CHECK (worker_profile_count >= 0),
    evidence_count INTEGER NOT NULL CHECK (evidence_count >= 0),
    CHECK (json_array_length(model_release_ids_json) = model_release_count),
    CHECK (json_array_length(provider_offering_ids_json) = provider_offering_count),
    CHECK (json_array_length(worker_profile_ids_json) = worker_profile_count),
    CHECK (json_array_length(evidence_ids_json) = evidence_count)
) STRICT;

CREATE TRIGGER IF NOT EXISTS model_releases_no_update
BEFORE UPDATE ON model_releases BEGIN
    SELECT RAISE(ABORT, 'model releases are append-only');
END;
CREATE TRIGGER IF NOT EXISTS model_releases_no_delete
BEFORE DELETE ON model_releases BEGIN
    SELECT RAISE(ABORT, 'model releases are append-only');
END;
CREATE TRIGGER IF NOT EXISTS provider_offerings_no_update
BEFORE UPDATE ON provider_offerings BEGIN
    SELECT RAISE(ABORT, 'provider offerings are append-only');
END;
CREATE TRIGGER IF NOT EXISTS provider_offerings_no_delete
BEFORE DELETE ON provider_offerings BEGIN
    SELECT RAISE(ABORT, 'provider offerings are append-only');
END;
CREATE TRIGGER IF NOT EXISTS worker_profiles_no_update
BEFORE UPDATE ON worker_profiles BEGIN
    SELECT RAISE(ABORT, 'worker profiles are append-only');
END;
CREATE TRIGGER IF NOT EXISTS worker_profiles_no_delete
BEFORE DELETE ON worker_profiles BEGIN
    SELECT RAISE(ABORT, 'worker profiles are append-only');
END;
CREATE TRIGGER IF NOT EXISTS evidence_observations_no_update
BEFORE UPDATE ON evidence_observations BEGIN
    SELECT RAISE(ABORT, 'evidence observations are append-only');
END;
CREATE TRIGGER IF NOT EXISTS evidence_observations_no_delete
BEFORE DELETE ON evidence_observations BEGIN
    SELECT RAISE(ABORT, 'evidence observations are append-only');
END;
CREATE TRIGGER IF NOT EXISTS snapshots_no_update
BEFORE UPDATE ON snapshots BEGIN
    SELECT RAISE(ABORT, 'snapshots are append-only');
END;
CREATE TRIGGER IF NOT EXISTS snapshots_no_delete
BEFORE DELETE ON snapshots BEGIN
    SELECT RAISE(ABORT, 'snapshots are append-only');
END;

PRAGMA user_version = 2;
"#;

const PRIVATE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS routing_quotes (
    decision_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    selected_worker_id TEXT,
    selected_checker_worker_id TEXT,
    verification_policy TEXT NOT NULL CHECK (verification_policy IN (
        'deterministic', 'maker_checker', 'human_approval'
    )),
    evidence_snapshot_id TEXT NOT NULL,
    policy_version TEXT NOT NULL CHECK (length(trim(policy_version)) > 0),
    expected_cash_micros INTEGER CHECK (
        expected_cash_micros IS NULL OR expected_cash_micros >= 0
    ),
    expected_quota_milliunits INTEGER CHECK (
        expected_quota_milliunits IS NULL OR expected_quota_milliunits >= 0
    ),
    expected_success_probability REAL CHECK (
        expected_success_probability IS NULL OR
        (expected_success_probability >= 0.0 AND expected_success_probability <= 1.0)
    ),
    p95_latency_ms INTEGER CHECK (p95_latency_ms IS NULL OR p95_latency_ms >= 0),
    eligible_candidates_json TEXT NOT NULL CHECK (
        json_valid(eligible_candidates_json)
        AND json_type(eligible_candidates_json) = 'array'
    ),
    rejected_candidates_json TEXT NOT NULL CHECK (
        json_valid(rejected_candidates_json)
        AND json_type(rejected_candidates_json) = 'array'
    ),
    pareto_worker_ids_json TEXT NOT NULL CHECK (
        json_valid(pareto_worker_ids_json)
        AND json_type(pareto_worker_ids_json) = 'array'
    ),
    selection_explanation_json TEXT CHECK (
        selection_explanation_json IS NULL OR (
            json_valid(selection_explanation_json)
            AND json_type(selection_explanation_json) = 'object'
        )
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    request_fingerprint TEXT NOT NULL CHECK (
        length(request_fingerprint) = 64
        AND request_fingerprint NOT GLOB '*[^0-9a-f]*'
    ),
    CHECK (
        selected_checker_worker_id IS NULL
        OR selected_checker_worker_id <> selected_worker_id
    ),
    CHECK (
        (
            selected_worker_id IS NULL
            AND selected_checker_worker_id IS NULL
            AND expected_cash_micros IS NULL
            AND expected_quota_milliunits IS NULL
            AND expected_success_probability IS NULL
            AND p95_latency_ms IS NULL
            AND selection_explanation_json IS NULL
        ) OR (
            selected_worker_id IS NOT NULL
            AND expected_cash_micros IS NOT NULL
            AND expected_quota_milliunits IS NOT NULL
            AND expected_success_probability IS NOT NULL
            AND p95_latency_ms IS NOT NULL
            AND selection_explanation_json IS NOT NULL
        )
    ),
    UNIQUE (decision_id, task_id, selected_worker_id)
) STRICT;

CREATE TABLE IF NOT EXISTS outcome_events (
    id TEXT PRIMARY KEY,
    decision_id TEXT,
    task_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    accepted INTEGER NOT NULL CHECK (accepted IN (0, 1)),
    validation_kind TEXT NOT NULL CHECK (validation_kind IN (
        'deterministic', 'human', 'independent_model', 'self_reported'
    )),
    actual_cash_micros INTEGER NOT NULL CHECK (actual_cash_micros >= 0),
    actual_quota_milliunits INTEGER NOT NULL CHECK (actual_quota_milliunits >= 0),
    latency_ms INTEGER NOT NULL CHECK (latency_ms >= 0),
    observed_at TEXT NOT NULL CHECK (length(trim(observed_at)) > 0),
    repository_scope TEXT,
    metadata_json TEXT NOT NULL CHECK (json_valid(metadata_json)),
    checker_worker_id TEXT,
    CHECK (checker_worker_id IS NULL OR checker_worker_id <> worker_id),
    FOREIGN KEY (decision_id, task_id, worker_id)
        REFERENCES routing_quotes(decision_id, task_id, selected_worker_id)
        ON DELETE RESTRICT
) STRICT;

CREATE INDEX IF NOT EXISTS outcomes_by_worker_skill
ON outcome_events(worker_id, skill_id, observed_at);

CREATE TRIGGER IF NOT EXISTS routing_quotes_no_update
BEFORE UPDATE ON routing_quotes BEGIN
    SELECT RAISE(ABORT, 'routing quotes are append-only');
END;
CREATE TRIGGER IF NOT EXISTS routing_quotes_no_delete
BEFORE DELETE ON routing_quotes BEGIN
    SELECT RAISE(ABORT, 'routing quotes are append-only');
END;
CREATE TRIGGER IF NOT EXISTS outcome_events_no_update
BEFORE UPDATE ON outcome_events BEGIN
    SELECT RAISE(ABORT, 'outcome events are append-only');
END;
CREATE TRIGGER IF NOT EXISTS outcome_events_no_delete
BEFORE DELETE ON outcome_events BEGIN
    SELECT RAISE(ABORT, 'outcome events are append-only');
END;

PRAGMA user_version = 2;
"#;

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, time::SystemTime};

    use serde_json::Value;

    use super::*;

    const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DIGEST_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    #[test]
    fn public_records_round_trip_through_export_boundary() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let model = sample_model();
        let offering = sample_offering();
        let worker = sample_worker();
        let evidence = sample_evidence();
        let snapshot = sample_snapshot();

        store.append_model_release(&model).expect("append model");
        store
            .append_provider_offering(&offering)
            .expect("append offering");
        store.append_worker_profile(&worker).expect("append worker");
        store.append_evidence(&evidence).expect("append evidence");
        store.append_snapshot(&snapshot).expect("append snapshot");

        let export = build_public_export(&store, &snapshot.id).expect("public export");
        assert_eq!(export.model_releases, vec![model]);
        assert_eq!(export.provider_offerings, vec![offering]);
        assert_eq!(export.worker_profiles, vec![worker]);
        assert_eq!(export.evidence, vec![evidence]);
        assert_eq!(export.snapshot, snapshot.clone());
        assert_eq!(
            store.snapshot(&snapshot.id).expect("snapshot"),
            Some(snapshot)
        );
        assert_eq!(store.snapshot("missing").expect("missing snapshot"), None);
    }

    #[test]
    fn snapshot_export_is_not_changed_by_later_appends() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_snapshot_chain(&store);

        let mut model = sample_model();
        model.id = ModelReleaseId("model:later".to_owned());
        model.artifact_sha256 = DIGEST_C.to_owned();
        store.append_model_release(&model).expect("later model");

        let mut offering = sample_offering();
        offering.id = OfferingId("offering:later".to_owned());
        offering.model_release_id = model.id;
        store
            .append_provider_offering(&offering)
            .expect("later offering");

        let mut worker = sample_worker();
        worker.id = WorkerId("worker:later".to_owned());
        worker.offering_id = offering.id.clone();
        worker.configuration_sha256 = worker_configuration_sha256(&worker_identity(
            &worker,
            offering.model_release_id,
            offering.provider,
        ));
        store.append_worker_profile(&worker).expect("later worker");

        let export = build_public_export(&store, "snapshot:test").expect("old export");
        assert_eq!(export.model_releases.len(), 1);
        assert_eq!(export.provider_offerings.len(), 1);
        assert_eq!(export.worker_profiles.len(), 1);
        assert_eq!(export.evidence.len(), 1);
        assert_eq!(export.model_releases[0].id.0, "model:test");
    }

    #[test]
    fn tampered_snapshot_digest_count_and_members_are_rejected() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_identity_chain(&store);
        store
            .append_evidence(&sample_evidence())
            .expect("append evidence");

        let mut digest = sample_snapshot();
        digest.content_sha256 = DIGEST_C.to_owned();
        assert!(matches!(
            store.append_snapshot(&digest),
            Err(StoreError::SnapshotDigestMismatch { .. })
        ));

        let mut count = sample_snapshot();
        count.evidence_count += 1;
        assert!(matches!(
            store.append_snapshot(&count),
            Err(StoreError::InvalidSnapshotManifest { .. })
        ));

        let mut member = sample_snapshot();
        member.id = "snapshot:bad-member".to_owned();
        member.evidence_ids[0] = "evidence:missing".to_owned();
        member.content_sha256 = member
            .calculate_content_sha256()
            .expect("recalculate tampered manifest");
        assert!(matches!(
            store.append_snapshot(&member),
            Err(StoreError::SnapshotMemberMissing { .. })
        ));

        let mut closure = sample_snapshot();
        closure.id = "snapshot:open-dependency".to_owned();
        closure.model_release_ids.clear();
        closure.model_release_count = 0;
        closure.content_sha256 = closure
            .calculate_content_sha256()
            .expect("recalculate open manifest");
        assert!(matches!(
            store.append_snapshot(&closure),
            Err(StoreError::SnapshotDependencyNotClosed(_))
        ));
    }

    #[test]
    fn snapshot_requires_the_complete_offering_revision_chain() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let offerings = append_offering_revision_chain(&store);

        let missing_immediate_predecessor = SnapshotRecord::new(
            "snapshot:missing-immediate-predecessor",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "revision-chain-1",
            vec![ModelReleaseId("model:test".to_owned())],
            vec![offerings[2].id.clone()],
            vec![],
            vec![],
        )
        .expect("canonical snapshot");
        assert!(matches!(
            store.append_snapshot(&missing_immediate_predecessor),
            Err(StoreError::SnapshotDependencyNotClosed(_))
        ));

        let missing_transitive_predecessor = SnapshotRecord::new(
            "snapshot:missing-transitive-predecessor",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "revision-chain-2",
            vec![ModelReleaseId("model:test".to_owned())],
            vec![offerings[1].id.clone(), offerings[2].id.clone()],
            vec![],
            vec![],
        )
        .expect("canonical snapshot");
        assert!(matches!(
            store.append_snapshot(&missing_transitive_predecessor),
            Err(StoreError::SnapshotDependencyNotClosed(_))
        ));

        let complete = SnapshotRecord::new(
            "snapshot:complete-revision-chain",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "revision-chain-3",
            vec![ModelReleaseId("model:test".to_owned())],
            offerings
                .iter()
                .map(|offering| offering.id.clone())
                .collect(),
            vec![],
            vec![],
        )
        .expect("canonical snapshot");
        store.append_snapshot(&complete).expect("complete snapshot");

        let export = build_public_export(&store, &complete.id).expect("complete export");
        assert_eq!(export.provider_offerings.len(), 3);
    }

    #[test]
    fn export_revalidates_offering_revision_closure() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let offerings = append_offering_revision_chain(&store);
        let incomplete = SnapshotRecord::new(
            "snapshot:unchecked-incomplete-chain",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "revision-chain-unchecked",
            vec![ModelReleaseId("model:test".to_owned())],
            vec![offerings[1].id.clone(), offerings[2].id.clone()],
            vec![],
            vec![],
        )
        .expect("canonical snapshot");
        insert_snapshot_without_validation(&store, &incomplete);

        assert!(matches!(
            build_public_export(&store, &incomplete.id),
            Err(StoreError::SnapshotDependencyNotClosed(_))
        ));
    }

    #[test]
    fn release_only_evidence_and_unknown_sample_count_round_trip() {
        let store = PublicIndexStore::in_memory().expect("public store");
        store
            .append_model_release(&sample_model())
            .expect("append model");
        let mut evidence = sample_evidence();
        evidence.worker_id = None;
        evidence.sample_count = None;
        store.append_evidence(&evidence).expect("release evidence");
        let snapshot = SnapshotRecord::new(
            "snapshot:release-only",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "abc124",
            vec![ModelReleaseId("model:test".to_owned())],
            vec![],
            vec![],
            vec![evidence.id.clone()],
        )
        .expect("release-only snapshot");
        store.append_snapshot(&snapshot).expect("snapshot");

        let export = build_public_export(&store, &snapshot.id).expect("export");
        assert_eq!(export.evidence, vec![evidence]);
    }

    #[test]
    fn worker_configuration_digest_is_unique_and_capabilities_round_trip() {
        let store = PublicIndexStore::in_memory().expect("public store");
        store
            .append_model_release(&sample_model())
            .expect("append model");
        store
            .append_provider_offering(&sample_offering())
            .expect("append offering");
        let worker = sample_worker();
        store.append_worker_profile(&worker).expect("append worker");
        assert_eq!(
            store.worker_profiles().expect("workers"),
            vec![worker.clone()]
        );

        let mut duplicate_configuration = worker;
        duplicate_configuration.id = WorkerId("worker:duplicate".to_owned());
        assert!(
            store
                .append_worker_profile(&duplicate_configuration)
                .is_err()
        );
    }

    #[test]
    fn worker_configuration_digest_is_recomputed_from_the_stored_offering() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let model = sample_model();
        let offering = sample_offering();
        store.append_model_release(&model).expect("append model");
        store
            .append_provider_offering(&offering)
            .expect("append offering");

        let mut tampered_configuration = sample_worker();
        tampered_configuration.harness_version = "tampered".to_owned();
        assert!(matches!(
            store.append_worker_profile(&tampered_configuration),
            Err(StoreError::WorkerConfigurationDigestMismatch { .. })
        ));

        let mut provider_relabel = sample_worker();
        provider_relabel.configuration_sha256 = worker_configuration_sha256(&worker_identity(
            &provider_relabel,
            model.id,
            "relabeled-provider".to_owned(),
        ));
        assert!(matches!(
            store.append_worker_profile(&provider_relabel),
            Err(StoreError::WorkerConfigurationDigestMismatch { .. })
        ));

        let worker = sample_worker();
        store
            .append_worker_profile(&worker)
            .expect("authoritative provider digest");
    }

    #[test]
    fn public_entity_ids_and_provider_are_canonical_before_insert() {
        let store = PublicIndexStore::in_memory().expect("public store");

        let mut model = sample_model();
        model.id = ModelReleaseId(" model:test".to_owned());
        assert!(matches!(
            store.append_model_release(&model),
            Err(StoreError::NonCanonicalIdentifier {
                field: "model_release.id"
            })
        ));
        store
            .append_model_release(&sample_model())
            .expect("append canonical model");

        let mut offering = sample_offering();
        offering.id = OfferingId(" offering:test".to_owned());
        assert!(matches!(
            store.append_provider_offering(&offering),
            Err(StoreError::NonCanonicalIdentifier {
                field: "provider_offering.id"
            })
        ));

        let mut offering = sample_offering();
        offering.provider = "example ".to_owned();
        assert!(matches!(
            store.append_provider_offering(&offering),
            Err(StoreError::NonCanonicalIdentifier {
                field: "provider_offering.provider"
            })
        ));
        store
            .append_provider_offering(&sample_offering())
            .expect("append canonical offering");

        let mut worker = sample_worker();
        worker.id = WorkerId("worker:test ".to_owned());
        assert!(matches!(
            store.append_worker_profile(&worker),
            Err(StoreError::NonCanonicalIdentifier {
                field: "worker_profile.id"
            })
        ));
    }

    #[test]
    fn evidence_required_identity_and_text_fields_are_validated_before_insert() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_identity_chain(&store);

        let mut evidence = sample_evidence();
        evidence.id = " evidence:test".to_owned();
        assert!(matches!(
            store.append_evidence(&evidence),
            Err(StoreError::NonCanonicalIdentifier {
                field: "evidence.id"
            })
        ));

        let mut evidence = sample_evidence();
        evidence.model_release_id = ModelReleaseId(" ".to_owned());
        assert!(matches!(
            store.append_evidence(&evidence),
            Err(StoreError::NonCanonicalIdentifier {
                field: "evidence.model_release_id"
            })
        ));

        let mut evidence = sample_evidence();
        evidence.skill_id = SkillId("skill:rust ".to_owned());
        assert!(matches!(
            store.append_evidence(&evidence),
            Err(StoreError::NonCanonicalIdentifier {
                field: "evidence.skill_id"
            })
        ));

        let mut evidence = sample_evidence();
        evidence.benchmark_id = BenchmarkId("".to_owned());
        assert!(matches!(
            store.append_evidence(&evidence),
            Err(StoreError::NonCanonicalIdentifier {
                field: "evidence.benchmark_id"
            })
        ));

        for field in [
            "evidence.metric",
            "evidence.unit",
            "evidence.adapter_version",
            "evidence.source_url",
            "evidence.license",
        ] {
            let mut evidence = sample_evidence();
            match field {
                "evidence.metric" => evidence.metric = " ".to_owned(),
                "evidence.unit" => evidence.unit = " ".to_owned(),
                "evidence.adapter_version" => evidence.adapter_version = " ".to_owned(),
                "evidence.source_url" => evidence.source_url = " ".to_owned(),
                "evidence.license" => evidence.license = " ".to_owned(),
                _ => unreachable!("test field is exhaustive"),
            }
            assert!(matches!(
                store.append_evidence(&evidence),
                Err(StoreError::EmptyRequiredField { field: actual }) if actual == field
            ));
        }
    }

    #[test]
    fn sha256_fields_reject_non_hex_and_uppercase_values() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let mut model = sample_model();
        model.artifact_sha256 = "G".repeat(64);
        assert!(matches!(
            store.append_model_release(&model),
            Err(StoreError::InvalidSha256 { .. })
        ));

        let mut snapshot = sample_snapshot();
        snapshot.content_sha256 = "z".repeat(64);
        assert!(matches!(
            snapshot.validate(),
            Err(StoreError::InvalidSha256 { .. })
        ));
    }

    #[test]
    fn offering_revisions_preserve_identity_and_current_query_excludes_predecessor() {
        let store = PublicIndexStore::in_memory().expect("public store");
        store
            .append_model_release(&sample_model())
            .expect("append model");
        let original = sample_offering();
        store
            .append_provider_offering(&original)
            .expect("original offering");

        let mut revision = original.clone();
        revision.id = OfferingId("offering:revision".to_owned());
        revision.supersedes_offering_id = Some(original.id.clone());
        revision.effective_from_epoch_ms += 1_000;
        revision.fixed_request_micros = 50;
        store
            .append_provider_offering(&revision)
            .expect("valid revision");

        assert_eq!(
            store
                .current_provider_offerings(original.effective_from_epoch_ms)
                .expect("current before revision"),
            vec![original.clone()]
        );
        assert_eq!(
            store
                .current_provider_offerings(revision.effective_from_epoch_ms)
                .expect("current after revision"),
            vec![revision]
        );

        let mut invalid = original;
        invalid.id = OfferingId("offering:invalid".to_owned());
        invalid.supersedes_offering_id = Some(OfferingId("offering:revision".to_owned()));
        invalid.provider = "different-provider".to_owned();
        invalid.effective_from_epoch_ms += 2_000;
        assert!(store.append_provider_offering(&invalid).is_err());

        let mut overlap_base = sample_offering();
        overlap_base.id = OfferingId("offering:overlap-base".to_owned());
        overlap_base.effective_until_epoch_ms = Some(overlap_base.effective_from_epoch_ms + 5_000);
        store
            .append_provider_offering(&overlap_base)
            .expect("explicit interval base");
        let mut overlap_revision = overlap_base.clone();
        overlap_revision.id = OfferingId("offering:overlap-revision".to_owned());
        overlap_revision.supersedes_offering_id = Some(overlap_base.id);
        overlap_revision.effective_from_epoch_ms += 1_000;
        overlap_revision.effective_until_epoch_ms = None;
        assert!(
            store.append_provider_offering(&overlap_revision).is_err(),
            "an explicit predecessor interval cannot overlap its successor"
        );
    }

    #[test]
    fn public_evidence_requires_an_existing_model_release() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let error = store
            .append_evidence(&sample_evidence())
            .expect_err("foreign key must reject orphan evidence");
        assert!(matches!(error, StoreError::Sqlite(_)));
    }

    #[test]
    fn worker_specific_evidence_must_match_its_release() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_identity_chain(&store);
        let mut other_model = sample_model();
        other_model.id = ModelReleaseId("model:other".to_owned());
        store
            .append_model_release(&other_model)
            .expect("other release");

        let mut mismatched = sample_evidence();
        mismatched.model_release_id = other_model.id;
        assert!(store.append_evidence(&mismatched).is_err());

        mismatched.id = "evidence:release-only".to_owned();
        mismatched.worker_id = None;
        store
            .append_evidence(&mismatched)
            .expect("release-only evidence is valid");
    }

    #[test]
    fn public_records_cannot_be_updated_or_deleted() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_identity_chain(&store);
        assert!(
            store
                .connection
                .execute(
                    "UPDATE model_releases SET developer = 'changed' WHERE id = ?1",
                    ["model:test"],
                )
                .is_err()
        );
        assert!(
            store
                .connection
                .execute("DELETE FROM model_releases WHERE id = ?1", ["model:test"])
                .is_err()
        );
        assert!(
            store
                .connection
                .execute(
                    "UPDATE provider_offerings SET fixed_request_micros = 1 WHERE id = ?1",
                    ["offering:test"],
                )
                .is_err()
        );
        assert!(
            store
                .connection
                .execute("DELETE FROM worker_profiles WHERE id = ?1", ["worker:test"])
                .is_err()
        );
    }

    #[test]
    fn read_only_public_handle_can_export_existing_index() {
        let path = temporary_database_path("read-only");
        {
            let store = PublicIndexStore::open(&path).expect("public file store");
            append_public_snapshot_chain(&store);
        }
        {
            let store = ReadOnlyPublicIndexStore::open(&path).expect("read-only store");
            let export = build_public_export(&store, "snapshot:test").expect("read-only export");
            assert_eq!(export.model_releases.len(), 1);
            assert_eq!(export.provider_offerings.len(), 1);
            assert_eq!(export.worker_profiles.len(), 1);
        }
        remove_sqlite_files(&path);
    }

    #[test]
    fn private_quote_and_outcome_round_trip() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let quote = sample_quote();
        let outcome = sample_outcome(Some(quote.decision_id.clone()));

        store.append_quote(&quote).expect("append quote");
        store.append_outcome(&outcome).expect("append outcome");

        assert_eq!(store.quotes().expect("quotes"), vec![quote]);
        assert_eq!(store.outcomes().expect("outcomes"), vec![outcome]);
    }

    #[test]
    fn incomplete_quote_audits_are_rejected() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let mut missing_selected_candidate = sample_quote();
        missing_selected_candidate.eligible_candidates.clear();
        missing_selected_candidate.pareto_worker_ids.clear();
        assert!(matches!(
            store.append_quote(&missing_selected_candidate),
            Err(StoreError::InvalidQuoteAudit(_))
        ));

        let mut missing_checker = sample_quote();
        missing_checker.verification_policy = VerificationPolicy::MakerChecker;
        assert!(matches!(
            store.append_quote(&missing_checker),
            Err(StoreError::InvalidQuoteAudit(_))
        ));

        let mut malformed_reason = sample_quote();
        malformed_reason.rejected_candidates[0].reasons = vec![serde_json::json!("opaque")];
        assert!(matches!(
            store.append_quote(&malformed_reason),
            Err(StoreError::InvalidQuoteAudit(_))
        ));

        let mut wrong_rank = sample_quote();
        wrong_rank.eligible_candidates[0].rank = 2;
        assert!(matches!(
            store.append_quote(&wrong_rank),
            Err(StoreError::InvalidQuoteAudit(_))
        ));

        let mut conflicting_cost = sample_quote();
        conflicting_cost.eligible_candidates[0].cost_breakdown["expected_cash_micros"] =
            serde_json::json!(1);
        assert!(matches!(
            store.append_quote(&conflicting_cost),
            Err(StoreError::InvalidQuoteAudit(_))
        ));

        let mut empty_failed_decision = sample_quote();
        empty_failed_decision.selected_worker_id = None;
        empty_failed_decision.selected_checker_worker_id = None;
        empty_failed_decision.expected_cash_micros = None;
        empty_failed_decision.expected_quota_milliunits = None;
        empty_failed_decision.expected_success_probability = None;
        empty_failed_decision.p95_latency_ms = None;
        empty_failed_decision.eligible_candidates.clear();
        empty_failed_decision.rejected_candidates.clear();
        empty_failed_decision.pareto_worker_ids.clear();
        empty_failed_decision.selection_explanation = None;
        assert!(matches!(
            store.append_quote(&empty_failed_decision),
            Err(StoreError::InvalidQuoteAudit(_))
        ));
    }

    #[test]
    fn linked_outcome_maker_must_match_the_quote() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let quote = sample_quote();
        store.append_quote(&quote).expect("append quote");

        let mut outcome = sample_outcome(Some(quote.decision_id));
        outcome.event.worker_id = WorkerId("worker:not-selected".to_owned());
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::InvalidOutcomeLink(_))
        ));
    }

    #[test]
    fn maker_checker_outcome_requires_the_selected_checker() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let checker_id = WorkerId("worker:checker".to_owned());
        let mut quote = sample_quote();
        quote.verification_policy = VerificationPolicy::MakerChecker;
        quote.selected_checker_worker_id = Some(checker_id.clone());
        quote.eligible_candidates[0].checker_worker_id = Some(checker_id.clone());
        store.append_quote(&quote).expect("append quote");

        let mut outcome = sample_outcome(Some(quote.decision_id.clone()));
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::InvalidOutcomeLink(_))
        ));

        outcome.checker_worker_id = Some(WorkerId("worker:wrong-checker".to_owned()));
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::InvalidOutcomeLink(_))
        ));

        outcome.checker_worker_id = Some(checker_id);
        outcome.event.validation_kind = ValidationKind::IndependentModel;
        store
            .append_outcome(&outcome)
            .expect("selected checker outcome");
    }

    #[test]
    fn non_checker_policies_reject_unselected_model_checkers() {
        for policy in [
            VerificationPolicy::Deterministic,
            VerificationPolicy::HumanApproval,
        ] {
            let store = PrivateLocalStore::in_memory().expect("private store");
            let mut quote = sample_quote();
            quote.verification_policy = policy;
            store.append_quote(&quote).expect("append quote");

            let mut outcome = sample_outcome(Some(quote.decision_id));
            outcome.checker_worker_id = Some(WorkerId("worker:unselected-checker".to_owned()));
            assert!(matches!(
                store.append_outcome(&outcome),
                Err(StoreError::InvalidOutcomeLink(_))
            ));
        }
    }

    #[test]
    fn all_rejected_routing_decision_round_trips_without_winner_fields() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let mut quote = sample_quote();
        quote.selected_worker_id = None;
        quote.selected_checker_worker_id = None;
        quote.verification_policy = VerificationPolicy::MakerChecker;
        quote.expected_cash_micros = None;
        quote.expected_quota_milliunits = None;
        quote.expected_success_probability = None;
        quote.p95_latency_ms = None;
        quote.eligible_candidates.clear();
        quote.pareto_worker_ids.clear();
        quote.selection_explanation = None;

        store.append_quote(&quote).expect("failed routing decision");
        assert_eq!(store.quotes().expect("quotes"), vec![quote]);
        assert!(
            store
                .append_outcome(&sample_outcome(Some(DecisionId(
                    "decision:test".to_owned()
                ))))
                .is_err(),
            "an outcome cannot attach to a quote without a selected worker"
        );
    }

    #[test]
    fn private_outcome_quote_link_is_enforced() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let outcome = sample_outcome(Some(DecisionId("missing".to_owned())));
        let error = store
            .append_outcome(&outcome)
            .expect_err("an unknown quote must be rejected");
        assert!(matches!(error, StoreError::InvalidOutcomeLink(_)));
    }

    #[test]
    fn maker_cannot_be_its_own_checker() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let mut outcome = sample_outcome(None);
        outcome.checker_worker_id = Some(outcome.event.worker_id.clone());
        assert!(store.append_outcome(&outcome).is_err());
    }

    #[test]
    fn private_outcome_identity_fields_are_canonical_before_insert() {
        let store = PrivateLocalStore::in_memory().expect("private store");

        let mut outcome = sample_outcome(None);
        outcome.event.task_id = TaskId(" ".to_owned());
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::NonCanonicalIdentifier {
                field: "outcome.task_id"
            })
        ));

        let mut outcome = sample_outcome(None);
        outcome.event.worker_id = WorkerId(" worker:test".to_owned());
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::NonCanonicalIdentifier {
                field: "outcome.worker_id"
            })
        ));

        let mut outcome = sample_outcome(None);
        outcome.event.skill_id = SkillId("skill:rust ".to_owned());
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::NonCanonicalIdentifier {
                field: "outcome.skill_id"
            })
        ));
    }

    #[test]
    fn private_records_cannot_be_updated_or_deleted() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        store.append_quote(&sample_quote()).expect("append quote");
        assert!(
            store
                .connection
                .execute(
                    "UPDATE routing_quotes SET expected_cash_micros = 0 WHERE decision_id = ?1",
                    ["decision:test"],
                )
                .is_err()
        );
        assert!(
            store
                .connection
                .execute(
                    "DELETE FROM routing_quotes WHERE decision_id = ?1",
                    ["decision:test"],
                )
                .is_err()
        );
    }

    #[test]
    fn public_and_private_schemas_are_physically_separate() {
        let public = PublicIndexStore::in_memory().expect("public store");
        let private = PrivateLocalStore::in_memory().expect("private store");

        assert!(table_exists(&public.connection, "model_releases"));
        assert!(!table_exists(&public.connection, "routing_quotes"));
        assert!(table_exists(&private.connection, "routing_quotes"));
        assert!(!table_exists(&private.connection, "model_releases"));
    }

    #[test]
    fn public_export_cannot_contain_private_outcome_markers() {
        const REPOSITORY_MARKER: &str = "private-repository-marker";
        const METADATA_MARKER: &str = "private-metadata-marker";

        let public = PublicIndexStore::in_memory().expect("public store");
        append_public_snapshot_chain(&public);

        let private = PrivateLocalStore::in_memory().expect("private store");
        let mut outcome = sample_outcome(None);
        outcome.event.repository_scope = Some(REPOSITORY_MARKER.to_owned());
        outcome.event.metadata = Value::String(METADATA_MARKER.to_owned());
        private
            .append_outcome(&outcome)
            .expect("append private outcome");

        let private_json = serde_json::to_string(&private.outcomes().expect("private outcomes"))
            .expect("serialize private outcomes");
        assert!(private_json.contains(REPOSITORY_MARKER));
        assert!(private_json.contains(METADATA_MARKER));

        let public_json =
            serde_json::to_string(&build_public_export(&public, "snapshot:test").expect("export"))
                .expect("serialize export");
        assert!(!public_json.contains(REPOSITORY_MARKER));
        assert!(!public_json.contains(METADATA_MARKER));
    }

    #[test]
    fn a_file_cannot_change_trust_domains() {
        let path = temporary_database_path("identity");
        {
            let public = PublicIndexStore::open(&path).expect("public file store");
            drop(public);
        }

        let error = match PrivateLocalStore::open(&path) {
            Ok(_) => panic!("private store must reject public database"),
            Err(error) => error,
        };
        assert!(matches!(error, StoreError::StoreKindMismatch { .. }));
        remove_sqlite_files(&path);
    }

    #[test]
    fn unknown_schema_versions_are_rejected_before_use() {
        let path = temporary_database_path("future-schema");
        {
            let connection = Connection::open(&path).expect("database");
            connection
                .pragma_update(None, "user_version", 99)
                .expect("future version");
        }
        let error = match PublicIndexStore::open(&path) {
            Ok(_) => panic!("future schema must be rejected"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            StoreError::UnsupportedSchemaVersion {
                expected: PUBLIC_SCHEMA_VERSION,
                actual: 99
            }
        ));
        remove_sqlite_files(&path);
    }

    #[test]
    fn failed_first_time_schema_initialization_rolls_back_completely() {
        let connection = Connection::open_in_memory().expect("database");
        configure_memory_connection(&connection).expect("configure");
        let failing_schema = "
            CREATE TABLE partial_initialization (id INTEGER PRIMARY KEY) STRICT;
            THIS IS DELIBERATELY INVALID SQL;
            PRAGMA user_version = 2;
        ";
        assert!(
            initialize_or_validate_store(
                &connection,
                PUBLIC_STORE_KIND,
                PUBLIC_SCHEMA_VERSION,
                failing_schema,
            )
            .is_err()
        );
        let application_tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .expect("table count");
        assert_eq!(application_tables, 0);
        assert_eq!(schema_version(&connection).expect("schema version"), 0);

        initialize_or_validate_store(
            &connection,
            PUBLIC_STORE_KIND,
            PUBLIC_SCHEMA_VERSION,
            PUBLIC_SCHEMA,
        )
        .expect("clean retry");
    }

    #[cfg(unix)]
    #[test]
    fn private_database_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = temporary_database_path("permissions");
        {
            let store = PrivateLocalStore::open(&path).expect("private file store");
            store.append_quote(&sample_quote()).expect("append quote");
            let mode = fs::metadata(&path)
                .expect("database metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        remove_sqlite_files(&path);
    }

    fn table_exists(connection: &Connection, table: &str) -> bool {
        connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |_| Ok(()),
            )
            .optional()
            .expect("schema query")
            .is_some()
    }

    fn sample_model() -> ModelReleaseRecord {
        ModelReleaseRecord {
            id: ModelReleaseId("model:test".to_owned()),
            developer: "example-developer".to_owned(),
            model_family: "example-family".to_owned(),
            released_at: "2026-08-01T00:00:00Z".to_owned(),
            context_window_tokens: 128_000,
            source_url: "https://example.test/model".to_owned(),
            artifact_sha256: DIGEST_A.to_owned(),
            recorded_at: "2026-08-02T00:00:00Z".to_owned(),
        }
    }

    fn sample_evidence() -> PublicEvidenceRecord {
        PublicEvidenceRecord {
            id: "evidence:test".to_owned(),
            model_release_id: ModelReleaseId("model:test".to_owned()),
            worker_id: Some(WorkerId("worker:test".to_owned())),
            skill_id: SkillId("skill:rust".to_owned()),
            benchmark_id: BenchmarkId("benchmark:test".to_owned()),
            evidence_tier: EvidenceTier::CommunityReproducible,
            raw_score: 82.0,
            metric: "pass_rate".to_owned(),
            unit: "percent".to_owned(),
            normalized_score: Some(0.82),
            adapter_version: "example-adapter@1".to_owned(),
            sample_count: Some(50),
            observed_at: "2026-08-02T00:00:00Z".to_owned(),
            source_url: "https://example.test/evidence".to_owned(),
            artifact_sha256: DIGEST_B.to_owned(),
            license: "Apache-2.0".to_owned(),
        }
    }

    fn sample_offering() -> ProviderOfferingRecord {
        ProviderOfferingRecord {
            id: OfferingId("offering:test".to_owned()),
            model_release_id: ModelReleaseId("model:test".to_owned()),
            provider: "example".to_owned(),
            supersedes_offering_id: None,
            effective_from_epoch_ms: 1_754_006_400_000,
            effective_until_epoch_ms: None,
            currency: "USD".to_owned(),
            input_micros_per_million_tokens: 1_000_000,
            output_micros_per_million_tokens: 3_000_000,
            fixed_request_micros: 0,
            quota_milliunits_per_request: 1_000,
            context_window_tokens: 128_000,
            source_url: "https://example.test/pricing".to_owned(),
            recorded_at: "2026-08-02T00:00:00Z".to_owned(),
        }
    }

    fn sample_worker() -> WorkerProfileRecord {
        let offering = sample_offering();
        let mut worker = WorkerProfileRecord {
            id: WorkerId("worker:test".to_owned()),
            offering_id: offering.id.clone(),
            harness_id: "raw-api".to_owned(),
            harness_version: "1".to_owned(),
            reasoning_configuration: "standard".to_owned(),
            system_prompt_sha256: DIGEST_A.to_owned(),
            skill_pack_version: "rust@1".to_owned(),
            toolset_version: "tools@1".to_owned(),
            execution_policy_sha256: DIGEST_A.to_owned(),
            supported_skill_ids: BTreeSet::from([SkillId("skill:rust".to_owned())]),
            tools: BTreeSet::from(["shell".to_owned()]),
            privacy_clearance: PrivacyClass::ConfidentialContent,
            configuration_sha256: String::new(),
            recorded_at: "2026-08-02T00:00:00Z".to_owned(),
        };
        worker.configuration_sha256 = worker_configuration_sha256(&worker_identity(
            &worker,
            offering.model_release_id,
            offering.provider,
        ));
        worker
    }

    fn sample_snapshot() -> SnapshotRecord {
        SnapshotRecord::new(
            "snapshot:test",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "abc123",
            vec![ModelReleaseId("model:test".to_owned())],
            vec![OfferingId("offering:test".to_owned())],
            vec![WorkerId("worker:test".to_owned())],
            vec!["evidence:test".to_owned()],
        )
        .expect("canonical snapshot")
    }

    fn append_public_identity_chain(store: &PublicIndexStore) {
        store
            .append_model_release(&sample_model())
            .expect("append model");
        store
            .append_provider_offering(&sample_offering())
            .expect("append offering");
        store
            .append_worker_profile(&sample_worker())
            .expect("append worker");
    }

    fn append_public_snapshot_chain(store: &PublicIndexStore) {
        append_public_identity_chain(store);
        store
            .append_evidence(&sample_evidence())
            .expect("append evidence");
        store
            .append_snapshot(&sample_snapshot())
            .expect("append snapshot");
    }

    fn append_offering_revision_chain(store: &PublicIndexStore) -> Vec<ProviderOfferingRecord> {
        store
            .append_model_release(&sample_model())
            .expect("append model");
        let original = sample_offering();
        store
            .append_provider_offering(&original)
            .expect("append original offering");

        let mut first_revision = original.clone();
        first_revision.id = OfferingId("offering:revision-1".to_owned());
        first_revision.supersedes_offering_id = Some(original.id.clone());
        first_revision.effective_from_epoch_ms += 1_000;
        first_revision.fixed_request_micros = 10;
        store
            .append_provider_offering(&first_revision)
            .expect("append first revision");

        let mut second_revision = first_revision.clone();
        second_revision.id = OfferingId("offering:revision-2".to_owned());
        second_revision.supersedes_offering_id = Some(first_revision.id.clone());
        second_revision.effective_from_epoch_ms += 1_000;
        second_revision.fixed_request_micros = 20;
        store
            .append_provider_offering(&second_revision)
            .expect("append second revision");

        vec![original, first_revision, second_revision]
    }

    fn insert_snapshot_without_validation(store: &PublicIndexStore, snapshot: &SnapshotRecord) {
        store
            .connection
            .execute(
                "INSERT INTO snapshots (
                    id, created_at, ontology_version, source_revision, content_sha256,
                    model_release_ids_json, provider_offering_ids_json,
                    worker_profile_ids_json, evidence_ids_json,
                    model_release_count, provider_offering_count, worker_profile_count,
                    evidence_count
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    snapshot.id,
                    snapshot.created_at,
                    snapshot.ontology_version,
                    snapshot.source_revision,
                    snapshot.content_sha256,
                    serde_json::to_string(&snapshot.model_release_ids).expect("model ids"),
                    serde_json::to_string(&snapshot.provider_offering_ids).expect("offering ids"),
                    serde_json::to_string(&snapshot.worker_profile_ids).expect("worker ids"),
                    serde_json::to_string(&snapshot.evidence_ids).expect("evidence ids"),
                    i64::try_from(snapshot.model_release_count).expect("model count"),
                    i64::try_from(snapshot.provider_offering_count).expect("offering count"),
                    i64::try_from(snapshot.worker_profile_count).expect("worker count"),
                    i64::try_from(snapshot.evidence_count).expect("evidence count"),
                ],
            )
            .expect("unchecked snapshot fixture");
    }

    fn sample_quote() -> QuoteRecord {
        let selected_worker_id = WorkerId("worker:test".to_owned());
        QuoteRecord {
            decision_id: DecisionId("decision:test".to_owned()),
            task_id: TaskId("task:test".to_owned()),
            selected_worker_id: Some(selected_worker_id.clone()),
            selected_checker_worker_id: None,
            verification_policy: VerificationPolicy::Deterministic,
            evidence_snapshot_id: "snapshot:test".to_owned(),
            policy_version: "policy:1".to_owned(),
            expected_cash_micros: Some(12_500),
            expected_quota_milliunits: Some(1_000),
            expected_success_probability: Some(0.82),
            p95_latency_ms: Some(3_000),
            eligible_candidates: vec![CandidateQuoteAuditRecord {
                rank: 1,
                worker_id: selected_worker_id.clone(),
                checker_worker_id: None,
                success_mean: 0.82,
                success_lower_bound: 0.75,
                p95_latency_ms: 3_000,
                expected_cash_micros: 12_500,
                expected_quota_milliunits: 1_000,
                expected_accepted_cost_micros: 13_000,
                pareto_efficient: true,
                cost_breakdown: serde_json::json!({
                    "currency": "USD",
                    "expected_cash_micros": 12_500,
                    "expected_quota_milliunits": 1_000,
                    "expected_accepted_cost_micros": 13_000
                }),
            }],
            rejected_candidates: vec![RejectedCandidateAuditRecord {
                worker_id: WorkerId("worker:rejected".to_owned()),
                reasons: vec![serde_json::json!({"code": "cash_budget_exceeded"})],
            }],
            pareto_worker_ids: vec![selected_worker_id],
            selection_explanation: Some(SelectionExplanationAuditRecord {
                objective: "minimize confidence-bounded expected accepted cost".to_owned(),
                eligible_candidate_count: 1,
                tie_break_order: vec!["expected_accepted_cost_micros".to_owned()],
            }),
            created_at: "2026-08-04T00:00:00Z".to_owned(),
            request_fingerprint: DIGEST_A.to_owned(),
        }
    }

    fn sample_outcome(decision_id: Option<DecisionId>) -> PrivateOutcomeRecord {
        PrivateOutcomeRecord {
            decision_id,
            event: OutcomeEvent {
                id: "outcome:test".to_owned(),
                task_id: TaskId("task:test".to_owned()),
                worker_id: WorkerId("worker:test".to_owned()),
                skill_id: SkillId("skill:rust".to_owned()),
                accepted: true,
                validation_kind: ValidationKind::Deterministic,
                actual_cash_micros: 11_000,
                actual_quota_milliunits: 1_000,
                latency_ms: 2_500,
                observed_at: "2026-08-04T00:01:00Z".to_owned(),
                repository_scope: Some("repo-hash".to_owned()),
                metadata: Value::Object(serde_json::Map::new()),
            },
            checker_worker_id: None,
        }
    }

    fn temporary_database_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "open-workforce-{label}-{}-{nonce}.sqlite",
            std::process::id()
        ))
    }

    fn remove_sqlite_files(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut candidate = path.as_os_str().to_owned();
            candidate.push(suffix);
            match fs::remove_file(PathBuf::from(candidate)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("cleanup failed: {error}"),
            }
        }
    }
}
