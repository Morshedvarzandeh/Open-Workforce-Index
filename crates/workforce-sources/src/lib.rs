//! Versioned import adapters for published price and capability sources.
//!
//! An adapter's job is narrow and adversarial: read someone else's file, emit
//! only the records it can fully justify, and report everything it refused.
//! Nothing here infers, normalizes by guesswork, or fills a missing field with
//! a plausible value — the index is worth exactly as much as its worst
//! unjustified row.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use workforce_domain::{ModelReleaseId, OfferingId};
use workforce_store::{ModelReleaseRecord, ProviderOfferingRecord};

/// Micros of currency per million tokens, given a per-token cost in whole
/// units. One token-unit cost of `1e-6` is `1_000_000` micros per million
/// tokens, hence `1e12`.
const MICROS_PER_MILLION_TOKENS_SCALE: f64 = 1e12;

/// Marker for a fact the source does not carry.
///
/// LiteLLM's price file has no release dates. Writing the retrieval date into
/// `released_at` would be inventing a fact, so the adapter writes this instead;
/// it is greppable and cannot be mistaken for a measurement.
pub const UNKNOWN: &str = "unknown";

/// One model entry in LiteLLM's `model_prices_and_context_window.json`.
///
/// Only the fields this adapter can justify are deserialized. Costs are in
/// whole currency units per single token.
#[derive(Debug, Clone, Deserialize)]
pub struct LiteLlmModelEntry {
    #[serde(default)]
    pub litellm_provider: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub input_cost_per_token: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token: Option<f64>,
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

/// Provenance and filtering for one import run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceImportOptions {
    /// Immutable identifier of the adapter that produced these records.
    pub adapter_version: String,
    /// Where the payload was retrieved from.
    pub source_url: String,
    /// When it was retrieved, as an RFC 3339 timestamp.
    pub retrieved_at: String,
    /// The same instant in Unix epoch milliseconds, used as the offering's
    /// `effective_from`.
    ///
    /// This records when the price was *observed*, which is the strongest claim
    /// the source supports. A later import appends a new revision rather than
    /// editing this one.
    pub retrieved_at_epoch_ms: i64,
    /// Currency of the source's costs. LiteLLM publishes USD.
    pub currency: String,
    /// Exact model keys to import. Empty means every entry that passes the
    /// other filters — deliberately opt-in, because a 2,988-entry dump is not
    /// a reviewable source record.
    #[serde(default)]
    pub include_models: Vec<String>,
    /// Restrict to these `litellm_provider` values. Empty means all.
    #[serde(default)]
    pub include_providers: Vec<String>,
    /// Require `mode` to equal this value. `None` disables the check.
    #[serde(default)]
    pub required_mode: Option<String>,
}

/// Why one entry produced no records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkippedEntry {
    pub model_key: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum SkipReason {
    NotRequested,
    ProviderNotIncluded {
        provider: String,
    },
    WrongMode {
        mode: Option<String>,
    },
    MissingProvider,
    MissingInputCost,
    MissingOutputCost,
    MissingContextWindow,
    /// A cost that is negative, infinite, NaN, or too large to represent as
    /// integer micros.
    UnrepresentableCost {
        field: String,
        value: f64,
    },
}

/// Records the adapter is willing to stand behind, plus everything it refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceImport {
    pub model_releases: Vec<ModelReleaseRecord>,
    pub provider_offerings: Vec<ProviderOfferingRecord>,
    pub skipped: Vec<SkippedEntry>,
    /// SHA-256 of the exact payload these records were derived from.
    pub artifact_sha256: String,
}

/// Converts a LiteLLM price payload into public index records.
///
/// The payload is hashed before parsing so the digest names exactly the bytes
/// that were read, and every emitted record carries it.
pub fn import_litellm_prices(
    payload: &str,
    options: &PriceImportOptions,
) -> Result<PriceImport, SourceError> {
    options.validate()?;
    let artifact_sha256 = sha256_hex(payload.as_bytes());

    let entries: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(payload).map_err(SourceError::Parse)?;

    let mut model_releases = Vec::new();
    let mut provider_offerings = Vec::new();
    let mut skipped = Vec::new();

    for (model_key, raw) in entries {
        // LiteLLM stores a `sample_spec` documentation stub alongside real
        // entries; anything that does not deserialize is not a model.
        let Ok(entry) = serde_json::from_value::<LiteLlmModelEntry>(raw) else {
            continue;
        };

        match convert_entry(&model_key, &entry, options, &artifact_sha256) {
            Ok(Some((release, offering))) => {
                model_releases.push(release);
                provider_offerings.push(offering);
            }
            Ok(None) => {}
            Err(reason) => skipped.push(SkippedEntry {
                model_key: model_key.clone(),
                reason,
            }),
        }
    }

    model_releases.sort_by(|left, right| left.id.cmp(&right.id));
    provider_offerings.sort_by(|left, right| left.id.cmp(&right.id));

    Ok(PriceImport {
        model_releases,
        provider_offerings,
        skipped,
        artifact_sha256,
    })
}

/// `Ok(None)` means the entry was filtered out before evaluation and is not
/// worth reporting; `Err` means it was in scope but could not be justified.
fn convert_entry(
    model_key: &str,
    entry: &LiteLlmModelEntry,
    options: &PriceImportOptions,
    artifact_sha256: &str,
) -> Result<Option<(ModelReleaseRecord, ProviderOfferingRecord)>, SkipReason> {
    if !options.include_models.is_empty()
        && !options.include_models.iter().any(|want| want == model_key)
    {
        return Ok(None);
    }

    let provider = entry
        .litellm_provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(SkipReason::MissingProvider)?;

    if !options.include_providers.is_empty()
        && !options
            .include_providers
            .iter()
            .any(|want| want == provider)
    {
        return Err(SkipReason::ProviderNotIncluded {
            provider: provider.to_owned(),
        });
    }

    if let Some(required) = &options.required_mode {
        if entry.mode.as_deref() != Some(required.as_str()) {
            return Err(SkipReason::WrongMode {
                mode: entry.mode.clone(),
            });
        }
    }

    let input_cost = entry
        .input_cost_per_token
        .ok_or(SkipReason::MissingInputCost)?;
    let output_cost = entry
        .output_cost_per_token
        .ok_or(SkipReason::MissingOutputCost)?;
    let context_window_tokens = entry
        .max_input_tokens
        .or(entry.max_tokens)
        .ok_or(SkipReason::MissingContextWindow)?;

    let input_micros_per_million_tokens =
        micros_per_million_tokens(input_cost, "input_cost_per_token")?;
    let output_micros_per_million_tokens =
        micros_per_million_tokens(output_cost, "output_cost_per_token")?;

    let model_release_id = ModelReleaseId(format!("model:{model_key}"));
    // Some upstream keys already carry their provider (`gemini/gemini-2.5-pro`)
    // and some do not (`claude-opus-4-5`). Qualify only the unqualified ones so
    // the identifier never repeats a segment.
    let offering_id = OfferingId(
        if model_key
            .strip_prefix(provider)
            .is_some_and(|rest| rest.starts_with('/'))
        {
            format!("offering:{model_key}")
        } else {
            format!("offering:{provider}/{model_key}")
        },
    );

    let release = ModelReleaseRecord {
        id: model_release_id.clone(),
        // The source names a provider, not a developer, and the two are not
        // the same thing — Bedrock serves Anthropic models. Rather than guess,
        // record what is known and leave the rest explicit.
        developer: UNKNOWN.to_owned(),
        model_family: model_key.to_owned(),
        released_at: UNKNOWN.to_owned(),
        context_window_tokens,
        source_url: options.source_url.clone(),
        artifact_sha256: artifact_sha256.to_owned(),
        recorded_at: options.retrieved_at.clone(),
    };

    let offering = ProviderOfferingRecord {
        id: offering_id,
        model_release_id,
        provider: provider.to_owned(),
        supersedes_offering_id: None,
        effective_from_epoch_ms: options.retrieved_at_epoch_ms,
        effective_until_epoch_ms: None,
        currency: options.currency.clone(),
        input_micros_per_million_tokens,
        output_micros_per_million_tokens,
        // The source carries neither per-request fees nor subscription quota
        // consumption. Zero is the correct value for a pure per-token API
        // price, not a stand-in for an unknown.
        fixed_request_micros: 0,
        quota_milliunits_per_request: 0,
        context_window_tokens,
        source_url: options.source_url.clone(),
        recorded_at: options.retrieved_at.clone(),
    };

    Ok(Some((release, offering)))
}

/// Converts a per-token cost in whole currency units into integer micros per
/// million tokens.
///
/// Realistic prices land far inside `f64`'s exactly-representable integer
/// range after scaling, so rounding here is exact rather than approximate.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn micros_per_million_tokens(cost_per_token: f64, field: &'static str) -> Result<u64, SkipReason> {
    if !cost_per_token.is_finite() || cost_per_token < 0.0 {
        return Err(SkipReason::UnrepresentableCost {
            field: field.to_owned(),
            value: cost_per_token,
        });
    }
    let scaled = (cost_per_token * MICROS_PER_MILLION_TOKENS_SCALE).round();
    if scaled > u64::MAX as f64 {
        return Err(SkipReason::UnrepresentableCost {
            field: field.to_owned(),
            value: cost_per_token,
        });
    }
    Ok(scaled as u64)
}

impl PriceImportOptions {
    pub fn validate(&self) -> Result<(), SourceError> {
        for (field, value) in [
            ("adapter_version", &self.adapter_version),
            ("source_url", &self.source_url),
            ("retrieved_at", &self.retrieved_at),
            ("currency", &self.currency),
        ] {
            if value.trim().is_empty() {
                return Err(SourceError::EmptyField(field));
            }
        }
        Ok(())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut accumulator, byte| {
            let _ = write!(accumulator, "{byte:02x}");
            accumulator
        })
}

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("could not parse the price payload as JSON: {0}")]
    Parse(#[source] serde_json::Error),
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> PriceImportOptions {
        PriceImportOptions {
            adapter_version: "litellm-prices@1".to_owned(),
            source_url: "https://example.test/model_prices.json".to_owned(),
            retrieved_at: "2026-08-07T00:00:00Z".to_owned(),
            retrieved_at_epoch_ms: 1_785_024_000_000,
            currency: "USD".to_owned(),
            include_models: Vec::new(),
            include_providers: Vec::new(),
            required_mode: Some("chat".to_owned()),
        }
    }

    const PAYLOAD: &str = r#"{
        "sample_spec": {"note": "this is a documentation stub, not a model"},
        "priced-model": {
            "litellm_provider": "example",
            "mode": "chat",
            "input_cost_per_token": 3e-06,
            "output_cost_per_token": 1.5e-05,
            "max_input_tokens": 200000
        },
        "embedding-model": {
            "litellm_provider": "example",
            "mode": "embedding",
            "input_cost_per_token": 1e-07,
            "output_cost_per_token": 0.0,
            "max_input_tokens": 8192
        },
        "no-price-model": {
            "litellm_provider": "example",
            "mode": "chat",
            "max_input_tokens": 128000
        },
        "no-context-model": {
            "litellm_provider": "example",
            "mode": "chat",
            "input_cost_per_token": 1e-06,
            "output_cost_per_token": 2e-06
        },
        "negative-model": {
            "litellm_provider": "example",
            "mode": "chat",
            "input_cost_per_token": -1e-06,
            "output_cost_per_token": 2e-06,
            "max_input_tokens": 1000
        }
    }"#;

    /// The conversion that matters: a per-token cost becomes integer micros per
    /// million tokens with no drift. $3/Mtok is 3,000,000 micros.
    #[test]
    fn per_token_costs_become_exact_integer_micros() {
        let import = import_litellm_prices(PAYLOAD, &options()).expect("import");
        let offering = import
            .provider_offerings
            .iter()
            .find(|offering| offering.id.0.ends_with("priced-model"))
            .expect("priced model");

        assert_eq!(offering.input_micros_per_million_tokens, 3_000_000);
        assert_eq!(offering.output_micros_per_million_tokens, 15_000_000);
        assert_eq!(offering.context_window_tokens, 200_000);
        assert_eq!(offering.currency, "USD");
    }

    #[test]
    fn known_price_points_round_trip_without_drift() {
        for (cost_per_token, expected_micros) in [
            (2.5e-07, 250_000_u64),
            (1e-06, 1_000_000),
            (1.25e-06, 1_250_000),
            (3e-06, 3_000_000),
            (5e-06, 5_000_000),
            (2.5e-05, 25_000_000),
            (0.0, 0),
        ] {
            assert_eq!(
                micros_per_million_tokens(cost_per_token, "test"),
                Ok(expected_micros),
                "{cost_per_token} should scale to {expected_micros}"
            );
        }
    }

    /// Every refusal is reported. An adapter that silently drops rows produces
    /// an index that looks complete and is not.
    #[test]
    fn unusable_entries_are_reported_rather_than_dropped() {
        let import = import_litellm_prices(PAYLOAD, &options()).expect("import");
        let reasons: BTreeMap<_, _> = import
            .skipped
            .iter()
            .map(|entry| (entry.model_key.as_str(), &entry.reason))
            .collect();

        assert_eq!(import.provider_offerings.len(), 1);
        assert!(matches!(
            reasons.get("embedding-model"),
            Some(SkipReason::WrongMode { .. })
        ));
        assert!(matches!(
            reasons.get("no-price-model"),
            Some(SkipReason::MissingInputCost)
        ));
        assert!(matches!(
            reasons.get("no-context-model"),
            Some(SkipReason::MissingContextWindow)
        ));
        assert!(matches!(
            reasons.get("negative-model"),
            Some(SkipReason::UnrepresentableCost { .. })
        ));
    }

    /// The digest names the exact bytes the records came from, so a later
    /// import against a changed upstream file is visibly a different source.
    #[test]
    fn the_artifact_digest_covers_the_payload() {
        let import = import_litellm_prices(PAYLOAD, &options()).expect("import");
        assert_eq!(import.artifact_sha256, sha256_hex(PAYLOAD.as_bytes()));
        assert!(
            import
                .model_releases
                .iter()
                .all(|release| release.artifact_sha256 == import.artifact_sha256)
        );

        let changed = import_litellm_prices(&PAYLOAD.replace("3e-06", "4e-06"), &options())
            .expect("import changed payload");
        assert_ne!(changed.artifact_sha256, import.artifact_sha256);
    }

    /// The source has no release dates, and the adapter must not invent them.
    #[test]
    fn absent_facts_are_marked_unknown_not_guessed() {
        let import = import_litellm_prices(PAYLOAD, &options()).expect("import");
        let release = &import.model_releases[0];
        assert_eq!(release.released_at, UNKNOWN);
        assert_eq!(release.developer, UNKNOWN);
        assert_ne!(release.released_at, options().retrieved_at);
    }

    /// Upstream keys are inconsistent about carrying their own provider.
    /// Neither shape may produce a doubled identifier segment.
    #[test]
    fn offering_identifiers_never_repeat_the_provider() {
        let payload = r#"{
            "unqualified": {
                "litellm_provider": "example", "mode": "chat",
                "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06,
                "max_input_tokens": 1000
            },
            "example/qualified": {
                "litellm_provider": "example", "mode": "chat",
                "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06,
                "max_input_tokens": 1000
            },
            "example-lookalike/other": {
                "litellm_provider": "example", "mode": "chat",
                "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06,
                "max_input_tokens": 1000
            }
        }"#;
        let import = import_litellm_prices(payload, &options()).expect("import");
        let ids: Vec<_> = import
            .provider_offerings
            .iter()
            .map(|offering| offering.id.0.as_str())
            .collect();

        assert!(ids.contains(&"offering:example/unqualified"));
        assert!(ids.contains(&"offering:example/qualified"));
        // A key that merely starts with the provider's characters is still
        // qualified, because the boundary is the separator, not the prefix.
        assert!(ids.contains(&"offering:example/example-lookalike/other"));
        assert!(!ids.iter().any(|id| id.contains("example/example/")));
    }

    #[test]
    fn an_explicit_model_list_excludes_everything_else() {
        let mut selective = options();
        selective.include_models = vec!["priced-model".to_owned()];
        let import = import_litellm_prices(PAYLOAD, &selective).expect("import");

        assert_eq!(import.model_releases.len(), 1);
        assert_eq!(import.model_releases[0].id.0, "model:priced-model");
        // Filtered-out entries are not reported as refusals.
        assert!(import.skipped.is_empty());
    }

    #[test]
    fn a_provider_filter_reports_what_it_excluded() {
        let mut selective = options();
        selective.include_providers = vec!["other".to_owned()];
        let import = import_litellm_prices(PAYLOAD, &selective).expect("import");

        assert!(import.provider_offerings.is_empty());
        assert!(
            import
                .skipped
                .iter()
                .any(|entry| matches!(entry.reason, SkipReason::ProviderNotIncluded { .. }))
        );
    }
}
