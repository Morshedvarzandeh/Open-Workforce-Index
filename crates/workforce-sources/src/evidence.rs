//! Importing published benchmark results as public evidence.
//!
//! The roster ships with *assumed* ability: every seeded evidence row is
//! tagged `vendor_reported`, points at `example.invalid`, and is discounted to
//! a tenth of its weight for exactly that reason. This module is how real
//! numbers replace them.
//!
//! It deliberately mirrors the price importer, because the same discipline
//! applies to a claim about ability as to a claim about cost: the exact bytes
//! are hashed before parsing so the digest names what was actually read, the
//! models to import are listed explicitly rather than swept up wholesale, the
//! provenance travels on every record, and a later import appends rather than
//! edits.
//!
//! Two things are deliberately NOT inferred here.
//!
//! The **tier** is stated by the caller, never guessed from the payload. It is
//! the difference between a number somebody published about themselves and a
//! number somebody reproduced, and the allocator weights them an order of
//! magnitude apart. A source cannot be trusted to grade its own trust.
//!
//! The **worker** is left unset unless the source names a full configuration.
//! A leaderboard measures a model release; a worker is a release plus a
//! harness, a skill pack, a toolset and an execution policy. Attaching a
//! release-level score to one specific worker would claim a measurement nobody
//! made. Left unset, the score informs every worker on that release as a prior
//! — capped, and outranked the moment your own ledger has anything to say.

use serde::{Deserialize, Serialize};
use workforce_domain::{BenchmarkId, EvidenceTier, ModelReleaseId, SkillId, WorkerId};
use workforce_store::PublicEvidenceRecord;

use crate::{SourceError, sha256_hex};

/// One row of a leaderboard export: a model and what it scored.
#[derive(Debug, Clone, Deserialize)]
pub struct LeaderboardResult {
    /// The source's own name for the model, matched against `include_models`.
    pub model: String,
    /// The score exactly as the source reports it, before any scaling.
    pub raw_score: f64,
    /// How many benchmark items produced it, when the source says.
    #[serde(default)]
    pub sample_count: Option<u64>,
    /// The exact configuration measured, when the source identifies one.
    #[serde(default)]
    pub worker_id: Option<String>,
}

/// A leaderboard export: `leaderboard@1`.
#[derive(Debug, Clone, Deserialize)]
pub struct LeaderboardPayload {
    pub results: Vec<LeaderboardResult>,
}

/// Provenance and filters for one evidence import run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceImportOptions {
    /// Immutable identifier of the adapter that produced these records.
    pub adapter_version: String,
    /// Where the payload was retrieved from.
    pub source_url: String,
    /// When it was retrieved, as an RFC 3339 timestamp. This becomes each
    /// record's `observed_at`: when the score was *seen*, which is the
    /// strongest claim the source supports.
    pub retrieved_at: String,
    /// How well this source was verified. Stated, never inferred — the
    /// allocator weights `project_reproduced` ten times a `vendor_reported`
    /// claim, so getting this wrong is not a labelling mistake, it is a
    /// staffing one.
    pub evidence_tier: EvidenceTier,
    pub license: String,
    /// The benchmark these scores came from.
    pub benchmark_id: String,
    /// The skill it measures.
    pub skill_id: String,
    pub metric: String,
    pub unit: String,
    /// What a raw score is divided by to reach `[0, 1]`: 100 for a percentage,
    /// 1 for a fraction. Normalisation is explicit because a leaderboard's
    /// units are a fact about the leaderboard, not something to guess.
    pub score_scale: f64,
    /// Exact model names to import. Empty means every row in the payload —
    /// but listing them makes a model appearing or vanishing upstream a
    /// visible diff rather than a silent change.
    #[serde(default)]
    pub include_models: Vec<String>,
}

impl EvidenceImportOptions {
    fn validate(&self) -> Result<(), SourceError> {
        for (field, value) in [
            ("adapter_version", &self.adapter_version),
            ("source_url", &self.source_url),
            ("retrieved_at", &self.retrieved_at),
            ("license", &self.license),
            ("benchmark_id", &self.benchmark_id),
            ("skill_id", &self.skill_id),
            ("metric", &self.metric),
            ("unit", &self.unit),
        ] {
            if value.trim().is_empty() {
                return Err(SourceError::EmptyField(field));
            }
        }
        if !self.score_scale.is_finite() || self.score_scale <= 0.0 {
            return Err(SourceError::InvalidOptions {
                field: "score_scale",
            });
        }
        Ok(())
    }
}

/// Why a row in the payload did not become evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceSkipReason {
    NotRequested,
    /// A score that is negative, infinite, NaN, or outside `[0, 1]` once
    /// scaled. A benchmark that reports 110% is reporting something this
    /// importer does not understand, and guessing is worse than skipping.
    UnusableScore {
        raw_score: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SkippedResult {
    pub model: String,
    pub reason: EvidenceSkipReason,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceImport {
    pub evidence: Vec<PublicEvidenceRecord>,
    pub skipped: Vec<SkippedResult>,
    /// SHA-256 of the exact payload these records were derived from.
    pub artifact_sha256: String,
}

/// Converts a leaderboard export into public evidence records.
///
/// The payload is hashed before parsing so the digest names exactly the bytes
/// that were read, and every emitted record carries it.
pub fn import_leaderboard(
    payload: &str,
    options: &EvidenceImportOptions,
) -> Result<EvidenceImport, SourceError> {
    options.validate()?;
    let artifact_sha256 = sha256_hex(payload.as_bytes());

    let parsed: LeaderboardPayload = serde_json::from_str(payload).map_err(SourceError::Parse)?;

    let mut evidence = Vec::new();
    let mut skipped = Vec::new();

    for result in parsed.results {
        if !options.include_models.is_empty()
            && !options
                .include_models
                .iter()
                .any(|want| want == &result.model)
        {
            skipped.push(SkippedResult {
                model: result.model,
                reason: EvidenceSkipReason::NotRequested,
            });
            continue;
        }

        let normalized = result.raw_score / options.score_scale;
        if !result.raw_score.is_finite() || !(0.0..=1.0).contains(&normalized) {
            skipped.push(SkippedResult {
                model: result.model,
                reason: EvidenceSkipReason::UnusableScore {
                    raw_score: result.raw_score.to_string(),
                },
            });
            continue;
        }

        // Same convention as the price importer: upstream keys that already
        // carry their provider keep it, so an identifier never repeats a
        // segment and the two importers agree about what a release is called.
        let model_release_id = ModelReleaseId(format!("model:{}", result.model));
        evidence.push(PublicEvidenceRecord {
            id: format!(
                "evidence:{}:{}:{}",
                options.benchmark_id, result.model, options.retrieved_at
            ),
            model_release_id,
            worker_id: result.worker_id.map(WorkerId),
            skill_id: SkillId(options.skill_id.clone()),
            benchmark_id: BenchmarkId(options.benchmark_id.clone()),
            evidence_tier: options.evidence_tier,
            raw_score: result.raw_score,
            metric: options.metric.clone(),
            unit: options.unit.clone(),
            normalized_score: Some(normalized),
            adapter_version: options.adapter_version.clone(),
            sample_count: result.sample_count,
            observed_at: options.retrieved_at.clone(),
            source_url: options.source_url.clone(),
            artifact_sha256: artifact_sha256.clone(),
            license: options.license.clone(),
        });
    }

    evidence.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(EvidenceImport {
        evidence,
        skipped,
        artifact_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> EvidenceImportOptions {
        EvidenceImportOptions {
            adapter_version: "leaderboard@1".to_owned(),
            source_url: "https://example.test/board".to_owned(),
            retrieved_at: "2026-08-14T00:00:00Z".to_owned(),
            evidence_tier: EvidenceTier::CommunityReproducible,
            license: "CC0-1.0".to_owned(),
            benchmark_id: "benchmark:demo".to_owned(),
            skill_id: "skill:python-numerical-implementation".to_owned(),
            metric: "pass_rate".to_owned(),
            unit: "percent".to_owned(),
            score_scale: 100.0,
            include_models: Vec::new(),
        }
    }

    #[test]
    fn percentages_are_normalized_and_provenance_travels() {
        let payload = r#"{"results":[{"model":"m-1","raw_score":72.7,
            "sample_count":11}]}"#;
        let import = import_leaderboard(payload, &options()).expect("import");
        let record = &import.evidence[0];
        assert_eq!(record.model_release_id.0, "model:m-1");
        assert_eq!(record.raw_score, 72.7);
        assert!((record.normalized_score.expect("normalized") - 0.727).abs() < 1e-12);
        assert_eq!(record.sample_count, Some(11));
        assert_eq!(record.artifact_sha256, import.artifact_sha256);
        assert_eq!(record.evidence_tier, EvidenceTier::CommunityReproducible);
        // A leaderboard measures a release, not a configuration.
        assert!(record.worker_id.is_none());
    }

    #[test]
    fn a_score_outside_the_unit_interval_is_skipped_not_clamped() {
        let payload = r#"{"results":[{"model":"m-1","raw_score":110.0}]}"#;
        let import = import_leaderboard(payload, &options()).expect("import");
        assert!(import.evidence.is_empty());
        assert!(matches!(
            import.skipped[0].reason,
            EvidenceSkipReason::UnusableScore { .. }
        ));
    }

    #[test]
    fn only_requested_models_are_imported() {
        let payload = r#"{"results":[{"model":"m-1","raw_score":50.0},
            {"model":"m-2","raw_score":60.0}]}"#;
        let mut chosen = options();
        chosen.include_models = vec!["m-2".to_owned()];
        let import = import_leaderboard(payload, &chosen).expect("import");
        assert_eq!(import.evidence.len(), 1);
        assert_eq!(import.evidence[0].model_release_id.0, "model:m-2");
        assert_eq!(import.skipped[0].reason, EvidenceSkipReason::NotRequested);
    }

    #[test]
    fn a_source_that_names_its_configuration_keeps_it() {
        let payload = r#"{"results":[{"model":"m-1","raw_score":50.0,
            "worker_id":"worker:m-1/code"}]}"#;
        let import = import_leaderboard(payload, &options()).expect("import");
        assert_eq!(
            import.evidence[0].worker_id.as_ref().expect("worker").0,
            "worker:m-1/code"
        );
    }

    #[test]
    fn the_digest_names_the_exact_bytes_read() {
        let payload = r#"{"results":[{"model":"m-1","raw_score":50.0}]}"#;
        let first = import_leaderboard(payload, &options()).expect("import");
        let spaced = format!("{payload} ");
        let second = import_leaderboard(&spaced, &options()).expect("import");
        assert_ne!(first.artifact_sha256, second.artifact_sha256);
    }
}
