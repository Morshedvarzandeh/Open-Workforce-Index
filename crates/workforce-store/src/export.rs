//! The public export aggregate and the one function that builds it —
//! which accepts only a public reader, so private values cannot leak in.

use crate::ModelReleaseRecord;
use crate::ProviderOfferingRecord;
use crate::PublicEvidenceRecord;
use crate::PublicIndexRead;
use crate::SnapshotRecord;
use crate::StoreError;
use crate::WorkerProfileRecord;
use crate::required_snapshot_member;
use crate::validate_export_dependency_closure;
use serde::{Deserialize, Serialize};

/// The only aggregate accepted by the public export boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicIndexExport {
    pub model_releases: Vec<ModelReleaseRecord>,
    pub provider_offerings: Vec<ProviderOfferingRecord>,
    pub worker_profiles: Vec<WorkerProfileRecord>,
    pub evidence: Vec<PublicEvidenceRecord>,
    pub snapshot: SnapshotRecord,
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
