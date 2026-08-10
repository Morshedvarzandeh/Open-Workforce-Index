//! Guards, digests and the on-disk spellings of domain enums — the layer
//! that decides what is allowed to become a record.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

use crate::IDENTITY_SCHEMA;
use crate::ModelReleaseRecord;
use crate::PrivateOutcomeRecord;
use crate::ProviderOfferingRecord;
use crate::PublicEvidenceRecord;
use crate::QuoteRecord;
use crate::SnapshotRecord;
use crate::StoreError;
use crate::WorkerProfileRecord;
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use workforce_domain::{
    EvidenceTier, ModelReleaseId, OfferingId, PrivacyClass, ValidationKind, VerificationPolicy,
    WorkerId, WorkerIdentity,
};

pub(crate) fn read_provider_offering(
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

pub(crate) fn validate_snapshot_dependencies(
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

pub(crate) fn require_model_release(connection: &Connection, id: &str) -> Result<(), StoreError> {
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

pub(crate) fn configure_file_connection(connection: &Connection) -> Result<(), StoreError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(())
}

pub(crate) fn configure_memory_connection(connection: &Connection) -> Result<(), StoreError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

pub(crate) fn initialize_or_validate_store(
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

pub(crate) fn schema_version(connection: &Connection) -> Result<i64, StoreError> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(StoreError::from)
}

pub(crate) fn validate_schema_version(
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

pub(crate) fn initialize_identity(
    connection: &Connection,
    expected: &'static str,
) -> Result<(), StoreError> {
    connection.execute_batch(IDENTITY_SCHEMA)?;
    connection.execute(
        "INSERT OR IGNORE INTO workforce_store_identity (singleton, kind) VALUES (1, ?1)",
        [expected],
    )?;
    validate_identity(connection, expected)
}

pub(crate) fn validate_manifest_list<T: Ord + ToString>(
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

pub(crate) fn hash_component(hasher: &mut Sha256, value: &str) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value.as_bytes());
}

pub(crate) fn hash_id_list<'value>(
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

pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

pub(crate) fn required_snapshot_member<T>(
    kind: &'static str,
    id: &str,
    value: Option<T>,
) -> Result<T, StoreError> {
    value.ok_or_else(|| StoreError::SnapshotMemberMissing {
        kind,
        id: id.to_owned(),
    })
}

pub(crate) fn validate_export_dependency_closure(
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

pub(crate) fn validate_identity(
    connection: &Connection,
    expected: &'static str,
) -> Result<(), StoreError> {
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

pub(crate) fn to_i64(field: &'static str, value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::IntegerOutOfRange { field })
}

pub(crate) fn from_i64(field: &'static str, value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::IntegerOutOfRange { field })
}

pub(crate) fn validate_canonical_identifier(
    field: &'static str,
    value: &str,
) -> Result<(), StoreError> {
    if value.trim().is_empty() || value.trim() != value {
        Err(StoreError::NonCanonicalIdentifier { field })
    } else {
        Ok(())
    }
}

pub(crate) fn validate_required_text(field: &'static str, value: &str) -> Result<(), StoreError> {
    if value.trim().is_empty() {
        Err(StoreError::EmptyRequiredField { field })
    } else {
        Ok(())
    }
}

pub(crate) fn worker_identity(
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

pub(crate) fn validate_finite(field: &'static str, value: f64) -> Result<(), StoreError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(StoreError::InvalidReal { field, value })
    }
}

pub(crate) fn validate_probability(field: &'static str, value: f64) -> Result<(), StoreError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(StoreError::InvalidProbability { field, value })
    }
}

pub(crate) fn validate_outcome_quote_link(
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

pub(crate) fn validate_sha256(field: &'static str, value: &str) -> Result<(), StoreError> {
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

pub(crate) fn decode_bool(value: i64) -> Result<bool, StoreError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(StoreError::InvalidBoolean(value)),
    }
}

pub(crate) const fn encode_privacy_class(value: PrivacyClass) -> &'static str {
    match value {
        PrivacyClass::Public => "public",
        PrivacyClass::PrivateMetadata => "private_metadata",
        PrivacyClass::ConfidentialContent => "confidential_content",
        PrivacyClass::Secret => "secret",
    }
}

pub(crate) fn decode_privacy_class(value: &str) -> Result<PrivacyClass, StoreError> {
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

pub(crate) const fn encode_verification_policy(value: VerificationPolicy) -> &'static str {
    match value {
        VerificationPolicy::Deterministic => "deterministic",
        VerificationPolicy::MakerChecker => "maker_checker",
        VerificationPolicy::HumanApproval => "human_approval",
    }
}

pub(crate) fn decode_verification_policy(value: &str) -> Result<VerificationPolicy, StoreError> {
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

pub(crate) fn validate_quote_audit(record: &QuoteRecord) -> Result<(), StoreError> {
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

pub(crate) const fn encode_evidence_tier(value: EvidenceTier) -> &'static str {
    match value {
        EvidenceTier::ProjectReproduced => "project_reproduced",
        EvidenceTier::IndependentSigned => "independent_signed",
        EvidenceTier::CommunityReproducible => "community_reproducible",
        EvidenceTier::VendorReported => "vendor_reported",
    }
}

pub(crate) fn decode_evidence_tier(value: &str) -> Result<EvidenceTier, StoreError> {
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

pub(crate) const fn encode_validation_kind(value: ValidationKind) -> &'static str {
    match value {
        ValidationKind::Deterministic => "deterministic",
        ValidationKind::Human => "human",
        ValidationKind::IndependentModel => "independent_model",
        ValidationKind::SelfReported => "self_reported",
    }
}

pub(crate) fn decode_validation_kind(value: &str) -> Result<ValidationKind, StoreError> {
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
pub(crate) fn prepare_private_database_file(path: &Path) -> Result<(), std::io::Error> {
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
pub(crate) fn prepare_private_database_file(path: &Path) -> Result<(), std::io::Error> {
    use std::fs::OpenOptions;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn secure_private_sqlite_files(path: &Path) -> Result<(), std::io::Error> {
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
pub(crate) fn secure_private_sqlite_files(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn set_owner_only(path: &Path) -> Result<(), std::io::Error> {
    use std::{fs, os::unix::fs::PermissionsExt};

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}
