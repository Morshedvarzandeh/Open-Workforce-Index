use std::{
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use workforce_allocator::{
    CalibrationPolicy, WorkflowAssumptions, calibrate_candidates, quote_record,
};
use workforce_domain::{DecisionId, TaskSpec};
use workforce_engine::{BetaPosterior, QuoteRequest, RoutingPolicy, quote};
use workforce_kg::{PublicGraph, validate_builtin_rdf};
use workforce_sources::{PriceImportOptions, import_litellm_prices};
use workforce_store::{
    ModelReleaseRecord, PrivateLedgerRead, PrivateLedgerWrite, PrivateLocalStore,
    PrivateOutcomeRecord, ProviderOfferingRecord, PublicEvidenceRecord, PublicIndexRead,
    PublicIndexStore, PublicIndexWrite, SnapshotRecord, WorkerProfileRecord,
};

#[derive(Debug, Parser)]
#[command(
    name = "owi",
    version,
    about = "Evidence-backed, cost-aware AI workforce allocation"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Produce an explainable worker assignment from a JSON request.
    ///
    /// This path takes pre-computed candidate estimates and is useful for
    /// testing the engine in isolation. For a decision derived from stored
    /// evidence, use `allocate`.
    Quote {
        /// QuoteRequest JSON file.
        #[arg(short, long)]
        input: PathBuf,
    },
    /// Seed the public index from a versioned source file, then snapshot it.
    Seed {
        #[arg(long, default_value = ".data/index.sqlite")]
        index: PathBuf,
        /// IndexSeed JSON file.
        #[arg(short, long)]
        input: PathBuf,
    },
    /// Derive candidates from the index and private history, quote, and record.
    ///
    /// This is the closed loop: evidence in, decision out, decision persisted.
    Allocate {
        #[arg(long, default_value = ".data/index.sqlite")]
        index: PathBuf,
        #[arg(long, default_value = ".data/local.sqlite")]
        local: PathBuf,
        /// AllocationRequest JSON file.
        #[arg(short, long)]
        input: PathBuf,
        /// Persist the decision to the private ledger.
        #[arg(long)]
        record: bool,
    },
    /// Import published token prices into the public index.
    ///
    /// Reads a payload already downloaded to disk. Fetching is deliberately a
    /// separate step so the exact bytes that were imported can be archived and
    /// re-verified against the recorded digest.
    Prices {
        #[arg(long, default_value = ".data/index.sqlite")]
        index: PathBuf,
        /// Downloaded source payload, e.g. LiteLLM's price file.
        #[arg(short, long)]
        input: PathBuf,
        /// PriceImportOptions JSON file with provenance and filters.
        #[arg(long)]
        options: PathBuf,
        /// Print the derived records without writing to the index.
        #[arg(long)]
        dry_run: bool,
    },
    /// Append a verified local outcome, which strengthens the next decision.
    Outcome {
        #[arg(long, default_value = ".data/local.sqlite")]
        local: PathBuf,
        /// PrivateOutcomeRecord JSON file.
        #[arg(short, long)]
        input: PathBuf,
    },
    /// Demonstrate a transparent posterior update from verified outcomes.
    Learn {
        /// LearningRequest JSON file.
        #[arg(short, long)]
        input: PathBuf,
    },
    /// Inspect or export the public semantic model.
    Ontology {
        #[command(subcommand)]
        command: OntologyCommand,
    },
    /// Initialize the physically separated public and private databases.
    Database {
        #[command(subcommand)]
        command: DatabaseCommand,
    },
}

#[derive(Debug, Subcommand)]
enum OntologyCommand {
    /// Parse the bundled ontology and SHACL graphs.
    Validate,
    /// Export the bundled public ontology as sorted N-Quads.
    Export {
        /// Output path. Omit to write to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum DatabaseCommand {
    /// Create the public index and private local ledger.
    Init {
        #[arg(long, default_value = ".data/index.sqlite")]
        index: PathBuf,
        #[arg(long, default_value = ".data/local.sqlite")]
        local: PathBuf,
    },
}

#[derive(Debug, Deserialize)]
struct LearningRequest {
    prior: BetaPosterior,
    #[serde(default)]
    prior_evidence_count: u64,
    /// One-sided tail probability; 0.05 gives a 95% lower bound.
    confidence_tail_probability: f64,
    outcomes: Vec<WeightedOutcome>,
}

#[derive(Debug, Deserialize)]
struct WeightedOutcome {
    accepted: bool,
    weight: f64,
}

#[derive(Debug, Serialize)]
struct LearningResult {
    posterior: BetaPosterior,
    estimate: workforce_domain::ProbabilityEstimate,
}

/// A versioned public-index source file. Everything here is reviewable data,
/// not generated state.
#[derive(Debug, Deserialize)]
struct IndexSeed {
    snapshot_id: String,
    created_at: String,
    ontology_version: String,
    source_revision: String,
    #[serde(default)]
    model_releases: Vec<ModelReleaseRecord>,
    #[serde(default)]
    provider_offerings: Vec<ProviderOfferingRecord>,
    #[serde(default)]
    worker_profiles: Vec<WorkerProfileRecord>,
    #[serde(default)]
    evidence: Vec<PublicEvidenceRecord>,
}

/// Everything the allocator needs that is not already in the index.
#[derive(Debug, Deserialize)]
struct AllocationRequest {
    decision_id: DecisionId,
    snapshot_id: String,
    task: TaskSpec,
    policy: RoutingPolicy,
    #[serde(default)]
    calibration: CalibrationPolicy,
    #[serde(default)]
    assumptions: WorkflowAssumptions,
    /// Evaluation instant for time-bounded offerings, in Unix epoch
    /// milliseconds. Supplied rather than read from the clock so a decision
    /// stays reproducible.
    at_epoch_ms: i64,
    /// Timestamp recorded with the decision.
    created_at: String,
}

#[derive(Debug, Serialize)]
struct AllocationResult<'a> {
    quote: &'a workforce_engine::RoutingQuote,
    calibration: Vec<CalibrationSummary<'a>>,
    recorded: bool,
    request_fingerprint: String,
}

/// Where every candidate's numbers came from.
#[derive(Debug, Serialize)]
struct CalibrationSummary<'a> {
    worker_id: &'a workforce_domain::WorkerId,
    available: bool,
    skills: &'a [workforce_allocator::SkillCalibration],
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Quote { input } => run_quote(&input),
        Command::Seed { index, input } => run_seed(&index, &input),
        Command::Allocate {
            index,
            local,
            input,
            record,
        } => run_allocate(&index, &local, &input, record),
        Command::Prices {
            index,
            input,
            options,
            dry_run,
        } => run_prices(&index, &input, &options, dry_run),
        Command::Outcome { local, input } => run_outcome(&local, &input),
        Command::Learn { input } => run_learn(&input),
        Command::Ontology { command } => run_ontology(command),
        Command::Database { command } => run_database(command),
    }
}

fn run_seed(index: &Path, input: &Path) -> Result<()> {
    let seed: IndexSeed = read_json(input)?;
    let resolved = resolve_target(index)?;
    create_parent(&resolved)?;
    let store = PublicIndexStore::open(&resolved)
        .with_context(|| format!("open {}", resolved.display()))?;

    for record in &seed.model_releases {
        store
            .append_model_release(record)
            .with_context(|| format!("append model release {}", record.id))?;
    }
    for record in &seed.provider_offerings {
        store
            .append_provider_offering(record)
            .with_context(|| format!("append provider offering {}", record.id))?;
    }
    for record in &seed.worker_profiles {
        store
            .append_worker_profile(record)
            .with_context(|| format!("append worker profile {}", record.id))?;
    }
    for record in &seed.evidence {
        store
            .append_evidence(record)
            .with_context(|| format!("append evidence {}", record.id))?;
    }

    // Snapshot everything the index now holds, not just this file's records.
    // A snapshot is a closed dependency set: seeding worker profiles that sit
    // on offerings imported by a separate `owi prices` run would otherwise
    // produce a manifest whose closure check fails at read time.
    let snapshot = SnapshotRecord::new(
        seed.snapshot_id.clone(),
        seed.created_at.clone(),
        seed.ontology_version.clone(),
        seed.source_revision.clone(),
        store
            .model_releases()?
            .into_iter()
            .map(|record| record.id)
            .collect(),
        store
            .provider_offerings()?
            .into_iter()
            .map(|record| record.id)
            .collect(),
        store
            .worker_profiles()?
            .into_iter()
            .map(|record| record.id)
            .collect(),
        store
            .evidence()?
            .into_iter()
            .map(|record| record.id)
            .collect(),
    )
    .context("build snapshot manifest")?;
    store
        .append_snapshot(&snapshot)
        .context("append snapshot")?;

    print_json(&serde_json::json!({
        "public_index": resolved,
        "snapshot_id": snapshot.id,
        "content_sha256": snapshot.content_sha256,
        "appended": {
            "model_releases": seed.model_releases.len(),
            "provider_offerings": seed.provider_offerings.len(),
            "worker_profiles": seed.worker_profiles.len(),
            "evidence": seed.evidence.len(),
        },
        "snapshot_members": {
            "model_releases": snapshot.model_release_count,
            "provider_offerings": snapshot.provider_offering_count,
            "worker_profiles": snapshot.worker_profile_count,
            "evidence": snapshot.evidence_count,
        },
    }))
}

fn run_allocate(index: &Path, local: &Path, input: &Path, record: bool) -> Result<()> {
    let request: AllocationRequest = read_json(input)?;
    let resolved_index = resolve_target(index)?;
    let resolved_local = resolve_target(local)?;
    if resolved_index == resolved_local {
        bail!(
            "public index and private ledger must use different files: {}",
            resolved_index.display()
        );
    }
    create_parent(&resolved_index)?;
    create_parent(&resolved_local)?;

    let public = PublicIndexStore::open(&resolved_index)
        .with_context(|| format!("open {}", resolved_index.display()))?;
    let private = PrivateLocalStore::open(&resolved_local)
        .with_context(|| format!("open {}", resolved_local.display()))?;

    let calibrated = calibrate_candidates(
        &public,
        &private,
        &request.snapshot_id,
        &request.task,
        &request.calibration,
        &request.assumptions,
        request.at_epoch_ms,
    )
    .context("calibrate candidates from the index")?;

    let quote_request = QuoteRequest {
        decision_id: request.decision_id.clone(),
        evidence_snapshot_id: request.snapshot_id.clone(),
        task: request.task.clone(),
        policy: request.policy.clone(),
        candidates: calibrated
            .iter()
            .map(|candidate| candidate.estimate.clone())
            .collect(),
    };
    let result = quote(&quote_request).context("quote request failed")?;

    let quote_record_value = quote_record(
        &quote_request,
        &result,
        &request.calibration,
        &request.created_at,
    )
    .context("build quote record")?;
    if record {
        private
            .append_quote(&quote_record_value)
            .context("append quote to the private ledger")?;
    }

    print_json(&AllocationResult {
        quote: &result,
        calibration: calibrated
            .iter()
            .map(|candidate| CalibrationSummary {
                worker_id: &candidate.estimate.worker.identity.worker_id,
                available: candidate.estimate.worker.available,
                skills: &candidate.skill_calibrations,
            })
            .collect(),
        recorded: record,
        request_fingerprint: quote_record_value.request_fingerprint.clone(),
    })
}

fn run_prices(index: &Path, input: &Path, options: &Path, dry_run: bool) -> Result<()> {
    let payload = fs::read_to_string(input).with_context(|| format!("read {}", input.display()))?;
    let options: PriceImportOptions = read_json(options)?;
    let import = import_litellm_prices(&payload, &options).context("import prices")?;

    if !dry_run {
        let resolved = resolve_target(index)?;
        create_parent(&resolved)?;
        let store = PublicIndexStore::open(&resolved)
            .with_context(|| format!("open {}", resolved.display()))?;
        for record in &import.model_releases {
            store
                .append_model_release(record)
                .with_context(|| format!("append model release {}", record.id))?;
        }
        for record in &import.provider_offerings {
            store
                .append_provider_offering(record)
                .with_context(|| format!("append provider offering {}", record.id))?;
        }
    }

    print_json(&serde_json::json!({
        "adapter_version": options.adapter_version,
        "source_url": options.source_url,
        "retrieved_at": options.retrieved_at,
        "artifact_sha256": import.artifact_sha256,
        "imported_model_releases": import.model_releases.len(),
        "imported_provider_offerings": import.provider_offerings.len(),
        "skipped": import.skipped,
        "dry_run": dry_run,
        "offerings": import.provider_offerings,
    }))
}

fn run_outcome(local: &Path, input: &Path) -> Result<()> {
    let record: PrivateOutcomeRecord = read_json(input)?;
    let resolved = resolve_target(local)?;
    create_parent(&resolved)?;
    let store = PrivateLocalStore::open(&resolved)
        .with_context(|| format!("open {}", resolved.display()))?;
    store.append_outcome(&record).context("append outcome")?;
    print_json(&serde_json::json!({
        "private_local_ledger": resolved,
        "worker_id": record.event.worker_id,
        "skill_id": record.event.skill_id,
        "accepted": record.event.accepted,
        "recorded_outcomes": store.outcomes().context("read outcomes")?.len(),
    }))
}

fn run_quote(path: &Path) -> Result<()> {
    let request: QuoteRequest = read_json(path)?;
    let result = quote(&request).context("quote request failed")?;
    print_json(&result)
}

fn run_learn(path: &Path) -> Result<()> {
    let request: LearningRequest = read_json(path)?;
    let mut posterior = request.prior;
    posterior.validate().context("invalid prior")?;
    for outcome in &request.outcomes {
        posterior
            .observe_outcome(outcome.accepted, outcome.weight)
            .context("invalid outcome weight")?;
    }
    let outcome_count = u64::try_from(request.outcomes.len()).unwrap_or(u64::MAX);
    let estimate = posterior
        .estimate(
            request.prior_evidence_count.saturating_add(outcome_count),
            request.confidence_tail_probability,
        )
        .context("estimate posterior")?;
    print_json(&LearningResult {
        posterior,
        estimate,
    })
}

fn run_ontology(command: OntologyCommand) -> Result<()> {
    match command {
        OntologyCommand::Validate => {
            let (ontology_statements, shape_statements) = validate_builtin_rdf()?;
            print_json(&serde_json::json!({
                "rdf_syntax_valid": true,
                "shacl_executed": false,
                "ontology_statements": ontology_statements,
                "shape_statements": shape_statements,
            }))
        }
        OntologyCommand::Export { output } => {
            let nquads = PublicGraph::with_builtin_ontology()?.sorted_nquads()?;
            if let Some(path) = output {
                create_parent(&path)?;
                fs::write(&path, nquads).with_context(|| format!("write {}", path.display()))?;
                Ok(())
            } else {
                io::stdout().write_all(nquads.as_bytes())?;
                Ok(())
            }
        }
    }
}

fn run_database(command: DatabaseCommand) -> Result<()> {
    match command {
        DatabaseCommand::Init { index, local } => {
            let resolved_index = resolve_target(&index)?;
            let resolved_local = resolve_target(&local)?;
            if resolved_index == resolved_local {
                bail!(
                    "public index and private ledger must use different files: {}",
                    resolved_index.display()
                );
            }
            create_parent(&resolved_index)?;
            create_parent(&resolved_local)?;
            let _public = PublicIndexStore::open(&resolved_index)
                .with_context(|| format!("initialize {}", resolved_index.display()))?;
            let _private = PrivateLocalStore::open(&resolved_local)
                .with_context(|| format!("initialize {}", resolved_local.display()))?;
            print_json(&serde_json::json!({
                "public_index": resolved_index,
                "private_local_ledger": resolved_local,
            }))
        }
    }
}

/// Resolves existing ancestors and normalizes a not-yet-created target without
/// creating anything. This catches aliases through `..` and existing symlinks
/// before either trust-domain database is opened.
fn resolve_target(path: &Path) -> Result<PathBuf> {
    if path.file_name().is_none() {
        bail!("database path must name a file");
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let normalized = lexical_normalize_absolute(&absolute)?;
    let mut existing = normalized;
    let mut suffix = Vec::new();
    while !existing.exists() {
        let component = existing
            .file_name()
            .context("database path must name a file")?
            .to_os_string();
        suffix.push(component);
        if !existing.pop() {
            bail!("database path has no existing ancestor");
        }
    }
    let mut resolved =
        fs::canonicalize(&existing).with_context(|| format!("resolve {}", existing.display()))?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn lexical_normalize_absolute(path: &Path) -> Result<PathBuf> {
    debug_assert!(path.is_absolute());
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() || !normalized.is_absolute() {
                    bail!("database path escapes the filesystem root");
                }
            }
        }
    }
    Ok(normalized)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

fn print_json(value: &impl Serialize) -> Result<()> {
    serde_json::to_writer_pretty(io::stdout().lock(), value)?;
    println!();
    Ok(())
}
fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nonexistent_test_target(label: &str) -> PathBuf {
        let unique = format!(
            "owi-{label}-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn database_target_resolution_handles_a_direct_missing_file_without_creating_it() -> Result<()>
    {
        let target = nonexistent_test_target("direct");
        let resolved = resolve_target(&target)?;
        let expected_parent = fs::canonicalize(target.parent().context("test target parent")?)?;
        assert_eq!(
            resolved,
            expected_parent.join(target.file_name().context("test target name")?)
        );
        assert!(!target.exists());
        Ok(())
    }

    #[test]
    fn database_target_resolution_normalizes_missing_parent_segments() -> Result<()> {
        let target = nonexistent_test_target("same");
        let missing_parent = format!(
            "owi-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let aliased = target
            .parent()
            .context("test target parent")?
            .join(missing_parent)
            .join("..")
            .join(target.file_name().context("test target name")?);
        assert_eq!(resolve_target(&target)?, resolve_target(&aliased)?);
        assert!(!target.exists());
        assert!(!aliased.exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn database_target_resolution_resolves_an_existing_symlink_alias() -> Result<()> {
        use std::os::unix::fs::symlink;

        let root = nonexistent_test_target("symlink-root").with_extension("");
        let actual = root.join("actual");
        let alias = root.join("alias");
        fs::create_dir_all(&actual)?;
        symlink(&actual, &alias)?;

        let result = (|| -> Result<()> {
            let direct_target = actual.join("ledger.sqlite");
            let alias_target = alias.join("ledger.sqlite");
            assert_eq!(
                resolve_target(&direct_target)?,
                resolve_target(&alias_target)?
            );
            assert!(!direct_target.exists());
            assert!(!alias_target.exists());
            Ok(())
        })();

        let _ = fs::remove_file(&alias);
        let _ = fs::remove_dir(&actual);
        let _ = fs::remove_dir(&root);
        result
    }
}
