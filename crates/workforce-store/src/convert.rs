//! Row DTOs and their fallible conversions into records. A column that
//! cannot be trusted stops here rather than entering the index.

use crate::PublicEvidenceRecord;
use crate::QuoteRecord;
use crate::SnapshotRecord;
use crate::StoreError;
use crate::WorkerProfileRecord;
use crate::decode_evidence_tier;
use crate::decode_privacy_class;
use crate::decode_verification_policy;
use crate::from_i64;
use crate::validate_probability;
use crate::validate_quote_audit;
use workforce_domain::{
    BenchmarkId, DecisionId, ModelReleaseId, OfferingId, SkillId, TaskId, WorkerId,
};

#[derive(Debug)]
pub(crate) struct RawSnapshot(
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
pub(crate) struct RawQuote {
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

pub(crate) fn quote_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawQuote> {
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

pub(crate) fn snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSnapshot> {
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
pub(crate) struct RawWorkerProfile {
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

pub(crate) fn worker_profile_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawWorkerProfile> {
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
pub(crate) struct RawPublicEvidence {
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

pub(crate) fn public_evidence_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawPublicEvidence> {
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
