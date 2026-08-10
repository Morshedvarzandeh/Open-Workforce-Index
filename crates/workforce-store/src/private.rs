//! The private local ledger: quotes and verified outcomes, append-only.

use std::path::{Path, PathBuf};

use crate::PRIVATE_SCHEMA;
use crate::PRIVATE_SCHEMA_VERSION;
use crate::PRIVATE_STORE_KIND;
use crate::PrivateLedgerRead;
use crate::PrivateLedgerWrite;
use crate::PrivateOutcomeRecord;
use crate::QuoteRecord;
use crate::StoreError;
use crate::configure_file_connection;
use crate::configure_memory_connection;
use crate::decode_bool;
use crate::decode_validation_kind;
use crate::encode_validation_kind;
use crate::encode_verification_policy;
use crate::from_i64;
use crate::initialize_or_validate_store;
use crate::prepare_private_database_file;
use crate::quote_row;
use crate::secure_private_sqlite_files;
use crate::to_i64;
use crate::validate_canonical_identifier;
use crate::validate_outcome_quote_link;
use crate::validate_probability;
use crate::validate_quote_audit;
use crate::validate_required_text;
use crate::validate_sha256;
use rusqlite::{Connection, params};
use workforce_domain::{DecisionId, OutcomeEvent, SkillId, TaskId, WorkerId};

/// Private local quotes and verified outcomes. This type never implements a
/// public export trait.
pub struct PrivateLocalStore {
    // pub(crate) only because the crate-root test module inspects the
    // live connection directly; nothing outside this crate can see it.
    pub(crate) connection: Connection,
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
