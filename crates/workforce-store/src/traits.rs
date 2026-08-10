//! The four capability boundaries. Read them side by side: nothing a
//! private-ledger reader exposes can reach the public export.

use crate::ModelReleaseRecord;
use crate::PrivateOutcomeRecord;
use crate::ProviderOfferingRecord;
use crate::PublicEvidenceRecord;
use crate::QuoteRecord;
use crate::SnapshotRecord;
use crate::StoreError;
use crate::WorkerProfileRecord;
use workforce_domain::{ModelReleaseId, OfferingId, WorkerId};

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
