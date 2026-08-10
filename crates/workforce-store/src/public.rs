//! The rebuildable public catalog: reads, writes, and the snapshot rules.

use std::{path::Path, time::Duration};

use crate::ModelReleaseRecord;
use crate::PUBLIC_SCHEMA;
use crate::PUBLIC_SCHEMA_VERSION;
use crate::PUBLIC_STORE_KIND;
use crate::ProviderOfferingRecord;
use crate::PublicEvidenceRecord;
use crate::PublicIndexRead;
use crate::PublicIndexWrite;
use crate::SnapshotRecord;
use crate::StoreError;
use crate::WorkerProfileRecord;
use crate::configure_file_connection;
use crate::configure_memory_connection;
use crate::decode_evidence_tier;
use crate::decode_privacy_class;
use crate::encode_evidence_tier;
use crate::encode_privacy_class;
use crate::from_i64;
use crate::initialize_or_validate_store;
use crate::public_evidence_row;
use crate::read_provider_offering;
use crate::snapshot_row;
use crate::to_i64;
use crate::validate_canonical_identifier;
use crate::validate_finite;
use crate::validate_identity;
use crate::validate_probability;
use crate::validate_required_text;
use crate::validate_schema_version;
use crate::validate_sha256;
use crate::validate_snapshot_dependencies;
use crate::worker_configuration_sha256;
use crate::worker_identity;
use crate::worker_profile_row;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use workforce_domain::{BenchmarkId, ModelReleaseId, OfferingId, SkillId, WorkerId};

/// Rebuildable public catalog and evidence store.
pub struct PublicIndexStore {
    // pub(crate) only because the crate-root test module inspects the
    // live connection directly; nothing outside this crate can see it.
    pub(crate) connection: Connection,
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

pub(crate) struct ConnectionPublicReader<'connection>(&'connection Connection);

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
