use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use workforce_engine::{BetaPosterior, QuoteRequest, quote};
use workforce_kg::{PublicGraph, validate_builtin_rdf};
use workforce_store::{PrivateLocalStore, PublicIndexStore};

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
    Quote {
        /// QuoteRequest JSON file.
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
    confidence_z: f64,
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

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Quote { input } => run_quote(&input),
        Command::Learn { input } => run_learn(&input),
        Command::Ontology { command } => run_ontology(command),
        Command::Database { command } => run_database(command),
    }
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
            request.confidence_z,
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
                "valid": true,
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
            create_parent(&index)?;
            create_parent(&local)?;
            let _public = PublicIndexStore::open(&index)
                .with_context(|| format!("initialize {}", index.display()))?;
            let _private = PrivateLocalStore::open(&local)
                .with_context(|| format!("initialize {}", local.display()))?;
            print_json(&serde_json::json!({
                "public_index": index,
                "private_local_ledger": local,
            }))
        }
    }
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
