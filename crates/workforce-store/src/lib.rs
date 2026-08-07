//! Persistence boundaries for the public index and the private local allocator.
//!
//! The two store types deliberately have unrelated read/write traits and reject
//! opening a database initialized for the other trust domain. Public export
//! functions accept only [`PublicIndexRead`], so private ledger values cannot
//! accidentally enter a public snapshot through this API.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use workforce_domain::{
    BenchmarkId, DecisionId, EvidenceTier, ModelReleaseId, OfferingId, OutcomeEvent,
    SkillId, TaskId, ValidationKind, WorkerId,
};

const PUBLIC_STORE_KIND: &str = "public_index";
const PRIVATE_STORE_KIND: &str = "private_local";

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
    pub effective_from: String,
    #[serde(default)]
    pub effective_until: Option<String>,
    pub currency: String,
    pub input_micros_per_million_tokens: u64,
    pub output_micros_per_million_tokens: u64,
    pub fixed_request_micros: u64,
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
    /// Digest over the canonicalized full worker configuration.
    pub configuration_sha256: String,
    pub recorded_at: String,
}

/// Public evidence tied to a concrete model release and exact worker identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicEvidenceRecord {
    pub id: String,
    pub model_release_id: ModelReleaseId,
    pub worker_id: WorkerId,
    pub skill_id: SkillId,
    #[serde(default)]
    pub benchmark_id: Option<BenchmarkId>,
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
    pub sample_count: u64,
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
    pub model_release_count: u64,
    pub provider_offering_count: u64,
    pub worker_profile_count: u64,
    pub evidence_count: u64,
}

/// A private allocator quote. No prompt or repository content is persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteRecord {
    pub decision_id: DecisionId,
    pub task_id: TaskId,
    pub selected_worker_id: WorkerId,
    pub evidence_snapshot_id: String,
    pub policy_version: String,
    pub expected_cash_micros: u64,
    pub expected_quota_milliunits: u64,
    pub expected_success_probability: f64,
    pub p95_latency_ms: u64,
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
    pub snapshots: Vec<SnapshotRecord>,
}

/// Narrow read surface for public, publishable data only.
pub trait PublicIndexRead {
    fn model_releases(&self) -> Result<Vec<ModelReleaseRecord>, StoreError>;
    fn provider_offerings(&self) -> Result<Vec<ProviderOfferingRecord>, StoreError>;
    fn worker_profiles(&self) -> Result<Vec<WorkerProfileRecord>, StoreError>;
    fn evidence(&self) -> Result<Vec<PublicEvidenceRecord>, StoreError>;
    fn snapshots(&self) -> Result<Vec<SnapshotRecord>, StoreError>;
    fn snapshot(&self, id: &str) -> Result<Option<SnapshotRecord>, StoreError>;
}

/// Append-only mutation surface for curating the public index.
pub trait PublicIndexWrite {
    fn append_model_release(&self, record: &ModelReleaseRecord) -> Result<(), StoreError>;
    fn append_provider_offering(
        &self,
        record: &ProviderOfferingRecord,
    ) -> Result<(), StoreError>;
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
pub fn build_public_export(source: &impl PublicIndexRead) -> Result<PublicIndexExport, StoreError> {
    Ok(PublicIndexExport {
        model_releases: source.model_releases()?,
        provider_offerings: source.provider_offerings()?,
        worker_profiles: source.worker_profiles()?,
        evidence: source.evidence()?,
        snapshots: source.snapshots()?,
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
        initialize_identity(&connection, PUBLIC_STORE_KIND)?;
        connection.execute_batch(PUBLIC_SCHEMA)?;
        Ok(Self { connection })
    }

    /// Creates an isolated in-memory public index, primarily for tests/tools.
    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        configure_memory_connection(&connection)?;
        initialize_identity(&connection, PUBLIC_STORE_KIND)?;
        connection.execute_batch(PUBLIC_SCHEMA)?;
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
            "SELECT id, model_release_id, provider, effective_from, effective_until,
                    currency, input_micros_per_million_tokens,
                    output_micros_per_million_tokens, fixed_request_micros,
                    context_window_tokens, source_url, recorded_at
             FROM provider_offerings ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
            ))
        })?;

        rows.map(|row| {
            let (id, model, provider, from, until, currency, input, output, fixed, context, url, at) =
                row?;
            Ok(ProviderOfferingRecord {
                id: OfferingId(id),
                model_release_id: ModelReleaseId(model),
                provider,
                effective_from: from,
                effective_until: until,
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
                    execution_policy_sha256, configuration_sha256, recorded_at
             FROM worker_profiles ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(WorkerProfileRecord {
                id: WorkerId(row.get(0)?),
                offering_id: OfferingId(row.get(1)?),
                harness_id: row.get(2)?,
                harness_version: row.get(3)?,
                reasoning_configuration: row.get(4)?,
                system_prompt_sha256: row.get(5)?,
                skill_pack_version: row.get(6)?,
                toolset_version: row.get(7)?,
                execution_policy_sha256: row.get(8)?,
                configuration_sha256: row.get(9)?,
                recorded_at: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
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
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<f64>>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, i64>(11)?,
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
                worker_id: WorkerId(worker_id),
                skill_id: SkillId(skill_id),
                benchmark_id: benchmark_id.map(BenchmarkId),
                evidence_tier: decode_evidence_tier(&tier)?,
                raw_score,
                metric,
                unit,
                normalized_score,
                adapter_version,
                sample_count: from_i64("sample_count", sample_count)?,
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
             model_release_count, provider_offering_count, worker_profile_count, evidence_count \
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
                 model_release_count, provider_offering_count, worker_profile_count, evidence_count \
                 FROM snapshots WHERE id = ?1",
                [id],
                snapshot_row,
            )
            .optional()?;
        raw.map(SnapshotRecord::try_from).transpose()
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
        }
    };
}

delegate_public_reads!(PublicIndexStore);
delegate_public_reads!(ReadOnlyPublicIndexStore);

impl PublicIndexWrite for PublicIndexStore {
    fn append_model_release(&self, record: &ModelReleaseRecord) -> Result<(), StoreError> {
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

    fn append_provider_offering(
        &self,
        record: &ProviderOfferingRecord,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO provider_offerings (
                id, model_release_id, provider, effective_from, effective_until,
                currency, input_micros_per_million_tokens,
                output_micros_per_million_tokens, fixed_request_micros,
                context_window_tokens, source_url, recorded_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                record.id.0,
                record.model_release_id.0,
                record.provider,
                record.effective_from,
                record.effective_until,
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
                to_i64("context_window_tokens", record.context_window_tokens)?,
                record.source_url,
                record.recorded_at,
            ],
        )?;
        Ok(())
    }

    fn append_worker_profile(&self, record: &WorkerProfileRecord) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO worker_profiles (
                id, offering_id, harness_id, harness_version, reasoning_configuration,
                system_prompt_sha256, skill_pack_version, toolset_version,
                execution_policy_sha256, configuration_sha256, recorded_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                record.configuration_sha256,
                record.recorded_at,
            ],
        )?;
        Ok(())
    }

    fn append_evidence(&self, record: &PublicEvidenceRecord) -> Result<(), StoreError> {
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
                record.worker_id.0,
                record.skill_id.0,
                record.benchmark_id.as_ref().map(|id| id.0.as_str()),
                encode_evidence_tier(record.evidence_tier),
                record.raw_score,
                record.metric,
                record.unit,
                record.normalized_score,
                record.adapter_version,
                to_i64("sample_count", record.sample_count)?,
                record.observed_at,
                record.source_url,
                record.artifact_sha256,
                record.license,
            ],
        )?;
        Ok(())
    }

    fn append_snapshot(&self, record: &SnapshotRecord) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO snapshots (
                id, created_at, ontology_version, source_revision, content_sha256,
                model_release_count, provider_offering_count, worker_profile_count,
                evidence_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.id,
                record.created_at,
                record.ontology_version,
                record.source_revision,
                record.content_sha256,
                to_i64("model_release_count", record.model_release_count)?,
                to_i64(
                    "provider_offering_count",
                    record.provider_offering_count,
                )?,
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
        initialize_identity(&connection, PRIVATE_STORE_KIND)?;
        connection.execute_batch(PRIVATE_SCHEMA)?;
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
        initialize_identity(&connection, PRIVATE_STORE_KIND)?;
        connection.execute_batch(PRIVATE_SCHEMA)?;
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
            "SELECT decision_id, task_id, selected_worker_id, evidence_snapshot_id,
                    policy_version, expected_cash_micros, expected_quota_milliunits,
                    expected_success_probability, p95_latency_ms, created_at,
                    request_fingerprint
             FROM routing_quotes ORDER BY created_at, decision_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
            ))
        })?;

        rows.map(|row| {
            let (decision, task, worker, snapshot, policy, cash, quota, success, latency, at, hash) =
                row?;
            Ok(QuoteRecord {
                decision_id: DecisionId(decision),
                task_id: TaskId(task),
                selected_worker_id: WorkerId(worker),
                evidence_snapshot_id: snapshot,
                policy_version: policy,
                expected_cash_micros: from_i64("expected_cash_micros", cash)?,
                expected_quota_milliunits: from_i64("expected_quota_milliunits", quota)?,
                expected_success_probability: success,
                p95_latency_ms: from_i64("p95_latency_ms", latency)?,
                created_at: at,
                request_fingerprint: hash,
            })
        })
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
        validate_probability(
            "expected_success_probability",
            record.expected_success_probability,
        )?;
        self.connection.execute(
            "INSERT INTO routing_quotes (
                decision_id, task_id, selected_worker_id, evidence_snapshot_id,
                policy_version, expected_cash_micros, expected_quota_milliunits,
                expected_success_probability, p95_latency_ms, created_at,
                request_fingerprint
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                record.decision_id.0,
                record.task_id.0,
                record.selected_worker_id.0,
                record.evidence_snapshot_id,
                record.policy_version,
                to_i64("expected_cash_micros", record.expected_cash_micros)?,
                to_i64(
                    "expected_quota_milliunits",
                    record.expected_quota_milliunits,
                )?,
                record.expected_success_probability,
                to_i64("p95_latency_ms", record.p95_latency_ms)?,
                record.created_at,
                record.request_fingerprint,
            ],
        )?;
        self.secure_files()?;
        Ok(())
    }

    fn append_outcome(&self, record: &PrivateOutcomeRecord) -> Result<(), StoreError> {
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
    #[error("{field} is outside SQLite's non-negative 64-bit integer range")]
    IntegerOutOfRange { field: &'static str },
    #[error("unknown {kind} value `{value}` in database")]
    UnknownEnum {
        kind: &'static str,
        value: String,
    },
    #[error("invalid SQLite boolean value {0}")]
    InvalidBoolean(i64),
    #[error("{field} must be finite, got {value}")]
    InvalidReal { field: &'static str, value: f64 },
    #[error("{field} must be a finite value between 0 and 1, got {value}")]
    InvalidProbability { field: &'static str, value: f64 },
}

#[derive(Debug)]
struct RawSnapshot(String, String, String, String, String, i64, i64, i64, i64);

impl TryFrom<RawSnapshot> for SnapshotRecord {
    type Error = StoreError;

    fn try_from(value: RawSnapshot) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.0,
            created_at: value.1,
            ontology_version: value.2,
            source_revision: value.3,
            content_sha256: value.4,
            model_release_count: from_i64("model_release_count", value.5)?,
            provider_offering_count: from_i64("provider_offering_count", value.6)?,
            worker_profile_count: from_i64("worker_profile_count", value.7)?,
            evidence_count: from_i64("evidence_count", value.8)?,
        })
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
    ))
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

fn initialize_identity(connection: &Connection, expected: &'static str) -> Result<(), StoreError> {
    connection.execute_batch(IDENTITY_SCHEMA)?;
    connection.execute(
        "INSERT OR IGNORE INTO workforce_store_identity (singleton, kind) VALUES (1, ?1)",
        [expected],
    )?;
    validate_identity(connection, expected)
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

fn decode_bool(value: i64) -> Result<bool, StoreError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(StoreError::InvalidBoolean(value)),
    }
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
    artifact_sha256 TEXT NOT NULL CHECK (length(artifact_sha256) = 64),
    recorded_at TEXT NOT NULL CHECK (length(trim(recorded_at)) > 0)
) STRICT;

CREATE TABLE IF NOT EXISTS provider_offerings (
    id TEXT PRIMARY KEY,
    model_release_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (length(trim(provider)) > 0),
    effective_from TEXT NOT NULL CHECK (length(trim(effective_from)) > 0),
    effective_until TEXT,
    currency TEXT NOT NULL CHECK (length(trim(currency)) > 0),
    input_micros_per_million_tokens INTEGER NOT NULL
        CHECK (input_micros_per_million_tokens >= 0),
    output_micros_per_million_tokens INTEGER NOT NULL
        CHECK (output_micros_per_million_tokens >= 0),
    fixed_request_micros INTEGER NOT NULL CHECK (fixed_request_micros >= 0),
    context_window_tokens INTEGER NOT NULL CHECK (context_window_tokens >= 0),
    source_url TEXT NOT NULL CHECK (length(trim(source_url)) > 0),
    recorded_at TEXT NOT NULL CHECK (length(trim(recorded_at)) > 0),
    CHECK (effective_until IS NULL OR effective_until > effective_from),
    FOREIGN KEY (model_release_id) REFERENCES model_releases(id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX IF NOT EXISTS offerings_by_release_and_time
ON provider_offerings(model_release_id, effective_from, effective_until);

CREATE TABLE IF NOT EXISTS worker_profiles (
    id TEXT PRIMARY KEY,
    offering_id TEXT NOT NULL,
    harness_id TEXT NOT NULL CHECK (length(trim(harness_id)) > 0),
    harness_version TEXT NOT NULL CHECK (length(trim(harness_version)) > 0),
    reasoning_configuration TEXT NOT NULL
        CHECK (length(trim(reasoning_configuration)) > 0),
    system_prompt_sha256 TEXT NOT NULL CHECK (length(system_prompt_sha256) = 64),
    skill_pack_version TEXT NOT NULL CHECK (length(trim(skill_pack_version)) > 0),
    toolset_version TEXT NOT NULL CHECK (length(trim(toolset_version)) > 0),
    execution_policy_sha256 TEXT NOT NULL CHECK (length(execution_policy_sha256) = 64),
    configuration_sha256 TEXT NOT NULL CHECK (length(configuration_sha256) = 64),
    recorded_at TEXT NOT NULL CHECK (length(trim(recorded_at)) > 0),
    FOREIGN KEY (offering_id) REFERENCES provider_offerings(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE IF NOT EXISTS evidence_observations (
    id TEXT PRIMARY KEY,
    model_release_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    benchmark_id TEXT,
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
    sample_count INTEGER NOT NULL CHECK (sample_count >= 1),
    observed_at TEXT NOT NULL CHECK (length(trim(observed_at)) > 0),
    source_url TEXT NOT NULL CHECK (length(trim(source_url)) > 0),
    artifact_sha256 TEXT NOT NULL CHECK (length(artifact_sha256) = 64),
    license TEXT NOT NULL CHECK (length(trim(license)) > 0),
    FOREIGN KEY (model_release_id) REFERENCES model_releases(id) ON DELETE RESTRICT,
    FOREIGN KEY (worker_id) REFERENCES worker_profiles(id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER IF NOT EXISTS evidence_worker_release_matches
BEFORE INSERT ON evidence_observations
WHEN NOT EXISTS (
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
    content_sha256 TEXT NOT NULL CHECK (length(content_sha256) = 64),
    model_release_count INTEGER NOT NULL CHECK (model_release_count >= 0),
    provider_offering_count INTEGER NOT NULL CHECK (provider_offering_count >= 0),
    worker_profile_count INTEGER NOT NULL CHECK (worker_profile_count >= 0),
    evidence_count INTEGER NOT NULL CHECK (evidence_count >= 0)
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

PRAGMA user_version = 1;
"#;

const PRIVATE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS routing_quotes (
    decision_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    selected_worker_id TEXT NOT NULL,
    evidence_snapshot_id TEXT NOT NULL,
    policy_version TEXT NOT NULL CHECK (length(trim(policy_version)) > 0),
    expected_cash_micros INTEGER NOT NULL CHECK (expected_cash_micros >= 0),
    expected_quota_milliunits INTEGER NOT NULL CHECK (expected_quota_milliunits >= 0),
    expected_success_probability REAL NOT NULL
        CHECK (expected_success_probability >= 0.0 AND expected_success_probability <= 1.0),
    p95_latency_ms INTEGER NOT NULL CHECK (p95_latency_ms >= 0),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    request_fingerprint TEXT NOT NULL CHECK (length(request_fingerprint) = 64),
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

PRAGMA user_version = 1;
"#;

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, time::SystemTime};

    use serde_json::Value;

    use super::*;

    const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

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
        store
            .append_worker_profile(&worker)
            .expect("append worker");
        store.append_evidence(&evidence).expect("append evidence");
        store.append_snapshot(&snapshot).expect("append snapshot");

        let export = build_public_export(&store).expect("public export");
        assert_eq!(export.model_releases, vec![model]);
        assert_eq!(export.provider_offerings, vec![offering]);
        assert_eq!(export.worker_profiles, vec![worker]);
        assert_eq!(export.evidence, vec![evidence]);
        assert_eq!(export.snapshots, vec![snapshot.clone()]);
        assert_eq!(store.snapshot(&snapshot.id).expect("snapshot"), Some(snapshot));
        assert_eq!(store.snapshot("missing").expect("missing snapshot"), None);
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
    fn public_records_cannot_be_updated_or_deleted() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_identity_chain(&store);
        assert!(store
            .connection
            .execute(
                "UPDATE model_releases SET developer = 'changed' WHERE id = ?1",
                ["model:test"],
            )
            .is_err());
        assert!(store
            .connection
            .execute("DELETE FROM model_releases WHERE id = ?1", ["model:test"])
            .is_err());
        assert!(store
            .connection
            .execute(
                "UPDATE provider_offerings SET fixed_request_micros = 1 WHERE id = ?1",
                ["offering:test"],
            )
            .is_err());
        assert!(store
            .connection
            .execute("DELETE FROM worker_profiles WHERE id = ?1", ["worker:test"])
            .is_err());
    }

    #[test]
    fn read_only_public_handle_can_export_existing_index() {
        let path = temporary_database_path("read-only");
        {
            let store = PublicIndexStore::open(&path).expect("public file store");
            append_public_identity_chain(&store);
        }
        {
            let store = ReadOnlyPublicIndexStore::open(&path).expect("read-only store");
            let export = build_public_export(&store).expect("read-only export");
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
    fn private_outcome_quote_link_is_enforced() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let outcome = sample_outcome(Some(DecisionId("missing".to_owned())));
        let error = store
            .append_outcome(&outcome)
            .expect_err("foreign key must reject an unknown quote");
        assert!(matches!(error, StoreError::Sqlite(_)));
    }

    #[test]
    fn maker_cannot_be_its_own_checker() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let mut outcome = sample_outcome(None);
        outcome.checker_worker_id = Some(outcome.event.worker_id.clone());
        assert!(store.append_outcome(&outcome).is_err());
    }

    #[test]
    fn private_records_cannot_be_updated_or_deleted() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        store.append_quote(&sample_quote()).expect("append quote");
        assert!(store
            .connection
            .execute(
                "UPDATE routing_quotes SET expected_cash_micros = 0 WHERE decision_id = ?1",
                ["decision:test"],
            )
            .is_err());
        assert!(store
            .connection
            .execute(
                "DELETE FROM routing_quotes WHERE decision_id = ?1",
                ["decision:test"],
            )
            .is_err());
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
        append_public_identity_chain(&public);

        let private = PrivateLocalStore::in_memory().expect("private store");
        let mut outcome = sample_outcome(None);
        outcome.event.repository_scope = Some(REPOSITORY_MARKER.to_owned());
        outcome.event.metadata = Value::String(METADATA_MARKER.to_owned());
        private.append_outcome(&outcome).expect("append private outcome");

        let private_json = serde_json::to_string(&private.outcomes().expect("private outcomes"))
            .expect("serialize private outcomes");
        assert!(private_json.contains(REPOSITORY_MARKER));
        assert!(private_json.contains(METADATA_MARKER));

        let public_json = serde_json::to_string(&build_public_export(&public).expect("export"))
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
            worker_id: WorkerId("worker:test".to_owned()),
            skill_id: SkillId("skill:rust".to_owned()),
            benchmark_id: Some(BenchmarkId("benchmark:test".to_owned())),
            evidence_tier: EvidenceTier::CommunityReproducible,
            raw_score: 82.0,
            metric: "pass_rate".to_owned(),
            unit: "percent".to_owned(),
            normalized_score: Some(0.82),
            adapter_version: "example-adapter@1".to_owned(),
            sample_count: 50,
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
            effective_from: "2026-08-01T00:00:00Z".to_owned(),
            effective_until: None,
            currency: "USD".to_owned(),
            input_micros_per_million_tokens: 1_000_000,
            output_micros_per_million_tokens: 3_000_000,
            fixed_request_micros: 0,
            context_window_tokens: 128_000,
            source_url: "https://example.test/pricing".to_owned(),
            recorded_at: "2026-08-02T00:00:00Z".to_owned(),
        }
    }

    fn sample_worker() -> WorkerProfileRecord {
        WorkerProfileRecord {
            id: WorkerId("worker:test".to_owned()),
            offering_id: OfferingId("offering:test".to_owned()),
            harness_id: "raw-api".to_owned(),
            harness_version: "1".to_owned(),
            reasoning_configuration: "standard".to_owned(),
            system_prompt_sha256: DIGEST_A.to_owned(),
            skill_pack_version: "rust@1".to_owned(),
            toolset_version: "tools@1".to_owned(),
            execution_policy_sha256: DIGEST_A.to_owned(),
            configuration_sha256: DIGEST_B.to_owned(),
            recorded_at: "2026-08-02T00:00:00Z".to_owned(),
        }
    }

    fn sample_snapshot() -> SnapshotRecord {
        SnapshotRecord {
            id: "snapshot:test".to_owned(),
            created_at: "2026-08-03T00:00:00Z".to_owned(),
            ontology_version: "0.1.0".to_owned(),
            source_revision: "abc123".to_owned(),
            content_sha256: DIGEST_A.to_owned(),
            model_release_count: 1,
            provider_offering_count: 1,
            worker_profile_count: 1,
            evidence_count: 1,
        }
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

    fn sample_quote() -> QuoteRecord {
        QuoteRecord {
            decision_id: DecisionId("decision:test".to_owned()),
            task_id: TaskId("task:test".to_owned()),
            selected_worker_id: WorkerId("worker:test".to_owned()),
            evidence_snapshot_id: "snapshot:test".to_owned(),
            policy_version: "policy:1".to_owned(),
            expected_cash_micros: 12_500,
            expected_quota_milliunits: 1_000,
            expected_success_probability: 0.82,
            p95_latency_ms: 3_000,
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
