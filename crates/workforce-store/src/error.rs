//! Every way a store operation can refuse.

use thiserror::Error;
use workforce_domain::WorkerId;

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
