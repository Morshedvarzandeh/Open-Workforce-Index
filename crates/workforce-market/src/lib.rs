//! A peer-to-peer capacity market: members lend unused subscription allowance
//! or API quota to each other without ever sharing credentials.
//!
//! This crate adds four things on top of the existing engine, and deliberately
//! nothing else:
//!
//! 1. [`CapacityOffering`] (defined in `workforce-domain`) and an append-only
//!    roster a member publishes one into ([`CapacityRosterRead`] /
//!    [`CapacityRosterWrite`]).
//! 2. [`capacity_offering_worker_estimate`] and [`quote_with_capacity_offerings`],
//!    which turn a listed offering into an ordinary [`WorkerEstimate`] and hand
//!    it to the *same* `workforce_allocator::calibrate_candidates` +
//!    `workforce_engine::quote` path every other candidate goes through. The
//!    listed price becomes cash cost via [`CostProfile::fixed_request_micros`];
//!    the lent quota is [`CostProfile::quota_milliunits_per_request`], unchanged,
//!    so the engine's existing quota shadow cost applies to it exactly as it
//!    does to any other worker.
//! 3. [`meter_request`], which charges a request from *measured* usage
//!    ([`MeteredUsage`], read off the real transcript) rather than the task's
//!    pre-execution token estimate.
//! 4. An append-only, double-entry [`LedgerEntry`] ledger
//!    ([`LedgerRead`] / [`LedgerWrite`]) that settles who consumed whose
//!    capacity, plus [`replay_balances`] and [`pairwise_settlement`] for a
//!    net-position view.
//!
//! Execution never leaves the runner-indirection boundary `tools/owi-do`
//! already established: a [`CapacityOffering`] carries a [`RunnerRef`], an
//! opaque name, never a command line or credential. [`execute_via_runner`]
//! resolves that name through a caller-supplied [`MemberRunnerDirectory`] —
//! the same `runners.json` contract `tools/owi-do` reads locally — and returns
//! an [`ExecutionReceipt`] that has no field capable of holding a credential.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use workforce_allocator::{AllocatorError, CalibrationPolicy, WorkflowAssumptions};
use workforce_domain::{
    CapacityOffering, CostProfile, DecisionId, DomainError, MemberId, OfferingId, PrivacyClass,
    ProbabilityEstimate, RunnerRef, TaskId, TaskSpec, WorkerEstimate, WorkerId, WorkerIdentity,
    WorkerProfile,
};
use workforce_engine::{EngineError, QuoteRequest, RoutingPolicy, RoutingQuote};
use workforce_store::{PrivateLedgerRead, PublicIndexRead, StoreError};

/// SHA-256 of the empty string, used as a placeholder identity digest for
/// worker configurations synthesized from a listing rather than a harness
/// build (the same well-known constant the domain and allocator tests use).
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug, Error)]
pub enum MarketError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Allocator(#[from] AllocatorError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error("failed to read runners file {path}: {source}")]
    RunnersFileRead {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("runner {0} is not configured")]
    RunnerNotConfigured(RunnerRef),
    #[error("failed to run the resolved command for {runner_ref}: {source}")]
    RunnerSpawn {
        runner_ref: RunnerRef,
        #[source]
        source: std::io::Error,
    },
    #[error("capacity offering {0} is already published; the roster is append-only")]
    DuplicateOfferingId(OfferingId),
    #[error("ledger entry must have at least two postings")]
    LedgerEntryTooFewPostings,
    #[error("posting for {0} must be exactly one of a debit or a credit, never both or neither")]
    UnbalancedPosting(MemberId),
    #[error(
        "ledger entry {id} does not balance: {debit_micros} debit micros vs {credit_micros} credit micros"
    )]
    LedgerEntryNotBalanced {
        id: String,
        debit_micros: u128,
        credit_micros: u128,
    },
    #[error("ledger entry id {0} already exists; the ledger is append-only")]
    DuplicateLedgerEntryId(String),
}

// ---------------------------------------------------------------------------
// Roster: members publish capacity offerings into an append-only listing.

/// Narrow read surface over the published capacity offerings.
pub trait CapacityRosterRead {
    fn capacity_offerings(&self) -> Result<Vec<CapacityOffering>, MarketError>;
    fn capacity_offering(&self, id: &OfferingId) -> Result<Option<CapacityOffering>, MarketError>;
    /// Only offerings whose effective window covers `at_epoch_ms`.
    fn current_capacity_offerings(
        &self,
        at_epoch_ms: i64,
    ) -> Result<Vec<CapacityOffering>, MarketError>;
}

/// Append-only mutation surface for the roster. There is deliberately no
/// update or delete method: a lender who wants to change terms publishes a
/// new offering, the same convention `workforce-store` uses for provider
/// offerings ("a price change is represented by appending a new offering").
pub trait CapacityRosterWrite {
    fn publish_capacity_offering(&self, offering: CapacityOffering) -> Result<(), MarketError>;
}

/// The simplest roster that satisfies the two traits above: an in-process,
/// append-only list. It is sufficient for one engine instance to match tasks
/// against; a deployment that needs durability or multi-process sharing can
/// implement the same traits against `workforce-store`'s sqlite backend
/// without any caller-visible change.
#[derive(Default)]
pub struct InMemoryCapacityRoster {
    offerings: Mutex<Vec<CapacityOffering>>,
}

impl InMemoryCapacityRoster {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CapacityRosterRead for InMemoryCapacityRoster {
    fn capacity_offerings(&self) -> Result<Vec<CapacityOffering>, MarketError> {
        Ok(self
            .offerings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }

    fn capacity_offering(&self, id: &OfferingId) -> Result<Option<CapacityOffering>, MarketError> {
        Ok(self
            .offerings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|offering| &offering.offering_id == id)
            .cloned())
    }

    fn current_capacity_offerings(
        &self,
        at_epoch_ms: i64,
    ) -> Result<Vec<CapacityOffering>, MarketError> {
        Ok(self
            .offerings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|offering| offering.is_current(at_epoch_ms))
            .cloned()
            .collect())
    }
}

impl CapacityRosterWrite for InMemoryCapacityRoster {
    fn publish_capacity_offering(&self, offering: CapacityOffering) -> Result<(), MarketError> {
        offering.validate()?;
        let mut offerings = self
            .offerings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if offerings
            .iter()
            .any(|existing| existing.offering_id == offering.offering_id)
        {
            return Err(MarketError::DuplicateOfferingId(offering.offering_id));
        }
        offerings.push(offering);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Matching: a listed offering becomes an ordinary candidate for the engine.

/// The offering's cost as an ordinary [`CostProfile`], so the unmodified
/// engine prices it exactly like any other worker.
///
/// The listed price is folded into [`CostProfile::fixed_request_micros`]
/// (`listed_price_micros_per_quota_milliunit * quota_milliunits_per_request`)
/// rather than the per-token fields, because the market prices lent quota, not
/// tokens. `quota_milliunits_per_request` is carried through unchanged, so the
/// engine's own `quota_shadow_cash_micros_per_unit` term still applies on top
/// of this cash price — the two costs the checklist asks to see side by side.
pub fn capacity_offering_cost_profile(offering: &CapacityOffering) -> CostProfile {
    CostProfile {
        currency: offering.currency.clone(),
        input_micros_per_million_tokens: 0,
        output_micros_per_million_tokens: 0,
        fixed_request_micros: offering
            .listed_price_micros_per_quota_milliunit
            .saturating_mul(offering.quota_milliunits_per_request),
        quota_milliunits_per_request: offering.quota_milliunits_per_request,
    }
}

/// The [`WorkerId`] synthesized for a capacity offering's candidate. Stable
/// and content-derived, so the same offering always resolves to the same
/// candidate identity.
pub fn capacity_worker_id(offering_id: &OfferingId) -> WorkerId {
    WorkerId::new(format!("worker:capacity:{}", offering_id.0))
}

/// Turns a listed offering into an ordinary [`WorkerEstimate`] the engine can
/// rank alongside every other candidate.
///
/// `success` is supplied by the caller rather than computed here: this crate
/// has no evidence store of its own, and a deployment that wants a calibrated
/// estimate for a lender's track record can compute one with
/// `workforce_allocator`'s own calibration against that lender's recorded
/// outcomes and pass it straight through.
pub fn capacity_offering_worker_estimate(
    offering: &CapacityOffering,
    success: ProbabilityEstimate,
    p95_latency_ms: u64,
    evidence_snapshot_id: impl Into<String>,
) -> Result<WorkerEstimate, MarketError> {
    offering.validate()?;
    let identity = WorkerIdentity {
        worker_id: capacity_worker_id(&offering.offering_id),
        model_release_id: offering.model_release_id.clone(),
        offering_id: offering.offering_id.clone(),
        provider: format!("member:{}", offering.lender_member_id),
        harness_id: "capacity-market".to_owned(),
        harness_version: "1".to_owned(),
        reasoning_configuration: "as-declared".to_owned(),
        system_prompt_sha256: EMPTY_SHA256.to_owned(),
        skill_pack_version: "lent-1".to_owned(),
        toolset_version: "lent-1".to_owned(),
        execution_policy_sha256: EMPTY_SHA256.to_owned(),
    };
    identity.validate()?;

    let worker = WorkerProfile {
        identity,
        supported_skills: [offering.skill_id.clone()].into_iter().collect(),
        tools: BTreeSet::new(),
        data_clearance: PrivacyClass::PrivateMetadata,
        context_window_tokens: offering.context_window_tokens,
        cost: capacity_offering_cost_profile(offering),
        available: true,
    };
    worker.validate()?;

    let skill_estimates = [(offering.skill_id.clone(), success.clone())]
        .into_iter()
        .collect();

    let estimate = WorkerEstimate {
        worker,
        success,
        skill_estimates,
        expected_tool_cash_micros: 0,
        expected_review_cash_micros: 0,
        expected_fallback_cash_micros: 0,
        expected_additional_quota_milliunits: 0,
        checker_worker_id: None,
        p95_latency_ms,
        evidence_snapshot_id: evidence_snapshot_id.into(),
    };
    estimate.validate()?;
    Ok(estimate)
}

/// Matches a task against both the existing roster and the capacity market in
/// one quote: calibrated candidates come from `workforce_allocator`, exactly
/// as they do for any other allocation, and each capacity offering is turned
/// into one more candidate via [`capacity_offering_worker_estimate`]. Both
/// lists are handed to the *same*, unmodified `workforce_engine::quote`, so a
/// lent-capacity candidate wins or loses on the identical objective — expected
/// accepted cost, which already includes the quota shadow cost — as every
/// other worker.
#[allow(clippy::too_many_arguments)]
pub fn quote_with_capacity_offerings(
    public: &impl PublicIndexRead,
    private: &impl PrivateLedgerRead,
    snapshot_id: &str,
    task: &TaskSpec,
    calibration: &CalibrationPolicy,
    assumptions: &WorkflowAssumptions,
    routing_policy: &RoutingPolicy,
    decision_id: DecisionId,
    capacity_candidates: &[(CapacityOffering, ProbabilityEstimate)],
    at_epoch_ms: i64,
) -> Result<(QuoteRequest, RoutingQuote), MarketError> {
    let calibrated = workforce_allocator::calibrate_candidates(
        public,
        private,
        snapshot_id,
        task,
        calibration,
        assumptions,
        at_epoch_ms,
    )?;

    let mut candidates: Vec<WorkerEstimate> = calibrated
        .into_iter()
        .map(|candidate| candidate.estimate)
        .collect();
    for (offering, success) in capacity_candidates {
        candidates.push(capacity_offering_worker_estimate(
            offering,
            success.clone(),
            assumptions
                .p95_latency_ms
                .get(&capacity_worker_id(&offering.offering_id))
                .copied()
                .unwrap_or(assumptions.default_p95_latency_ms),
            snapshot_id,
        )?);
    }

    let request = QuoteRequest {
        decision_id,
        evidence_snapshot_id: snapshot_id.to_owned(),
        task: task.clone(),
        policy: routing_policy.clone(),
        candidates,
    };
    let result = workforce_engine::quote(&request)?;
    Ok((request, result))
}

// ---------------------------------------------------------------------------
// Metering: charge from measured usage, not the task's pre-execution estimate.

/// Token counts actually observed for one request, read off the real
/// transcript rather than declared ahead of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeteredUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// The charge derived from measured usage against a [`CostProfile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeteredCharge {
    pub cash_micros: u64,
    /// Reuses [`CostProfile::quota_milliunits_per_request`] unchanged: quota is
    /// consumed per request, not per token, exactly as the field already means.
    pub quota_milliunits: u64,
}

/// Prices one request from what was actually measured, using the identical
/// per-token and fixed-cost fields the engine's own quote uses for the
/// estimate — the only thing that changes is which token counts go in.
pub fn meter_request(cost: &CostProfile, usage: &MeteredUsage) -> MeteredCharge {
    let input_cash =
        ceil_micros_per_million(usage.input_tokens, cost.input_micros_per_million_tokens);
    let output_cash =
        ceil_micros_per_million(usage.output_tokens, cost.output_micros_per_million_tokens);
    MeteredCharge {
        cash_micros: cost
            .fixed_request_micros
            .saturating_add(input_cash)
            .saturating_add(output_cash),
        quota_milliunits: cost.quota_milliunits_per_request,
    }
}

fn ceil_micros_per_million(tokens: u64, micros_per_million_tokens: u64) -> u64 {
    let product = u128::from(tokens) * u128::from(micros_per_million_tokens);
    u64::try_from(product.div_ceil(1_000_000)).unwrap_or(u64::MAX)
}

// ---------------------------------------------------------------------------
// Execution: resolve a RunnerRef locally, never inside an engine-side struct.

/// Resolves a [`RunnerRef`] to the exact local command line, the way
/// `tools/owi-do`'s `runners.json` maps a model name to a command that runs on
/// the caller's own machine with the caller's own credentials. Implementors
/// hold the credential-bearing command; nothing upstream of this trait — not
/// [`CapacityOffering`], not [`ExecutionReceipt`], not a [`LedgerEntry`] — ever
/// sees it.
pub trait MemberRunnerDirectory {
    fn resolve(&self, runner_ref: &RunnerRef) -> Option<String>;
}

/// Reads a `runners.json`-shaped file exactly as `tools/owi-do` writes and
/// reads it: a flat JSON object mapping a model/runner name to a command
/// line, with an optional `_comment` key ignored and blank commands treated
/// as unset. This is the same file and the same contract, not a second format.
pub struct RunnersJsonDirectory {
    commands: BTreeMap<String, String>,
}

impl RunnersJsonDirectory {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MarketError> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).map_err(|source| MarketError::RunnersFileRead {
                path: path.display().to_string(),
                source,
            })?;
        Self::parse(&text)
    }

    fn parse(text: &str) -> Result<Self, MarketError> {
        let raw: BTreeMap<String, serde_json::Value> = serde_json::from_str(text)?;
        let commands = raw
            .into_iter()
            .filter(|(key, _)| key != "_comment")
            .filter_map(|(key, value)| {
                let command = value.as_str()?.trim();
                (!command.is_empty()).then(|| (key, command.to_owned()))
            })
            .collect();
        Ok(Self { commands })
    }
}

impl MemberRunnerDirectory for RunnersJsonDirectory {
    fn resolve(&self, runner_ref: &RunnerRef) -> Option<String> {
        self.commands.get(&runner_ref.0).cloned()
    }
}

/// What executing a task against a capacity offering returns. No field here
/// can hold a credential or a resolved command line — only the offering and
/// task identity, the worker's output, and usage measured from that output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReceipt {
    pub offering_id: OfferingId,
    pub task_id: TaskId,
    pub output: String,
    pub usage: MeteredUsage,
    pub charge: MeteredCharge,
    pub exit_code: i32,
}

/// Executes `payload` against `offering`'s lender through the runner
/// indirection, and meters the result from what actually came back.
///
/// `directory` resolves the offering's [`RunnerRef`] to a real command line —
/// in a real deployment this call happens on the lender's own machine, the
/// same trust boundary `tools/owi-do` already draws around `runners.json`, so
/// the resolved command (and whatever credential it embeds) exists only
/// inside this function's stack and is never placed in the returned
/// [`ExecutionReceipt`], in `offering`, or in any ledger entry built from it.
pub fn execute_via_runner(
    offering: &CapacityOffering,
    task_id: &TaskId,
    payload: &str,
    directory: &dyn MemberRunnerDirectory,
) -> Result<ExecutionReceipt, MarketError> {
    let command = directory
        .resolve(&offering.runner_ref)
        .ok_or_else(|| MarketError::RunnerNotConfigured(offering.runner_ref.clone()))?;

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| MarketError::RunnerSpawn {
            runner_ref: offering.runner_ref.clone(),
            source,
        })?;
    // The command line (and any credential it embeds) is used here, inside
    // this stack frame, and dropped with `command` at the end of the
    // function — it is never copied into anything this function returns.
    drop(command);

    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(payload.as_bytes())
        .map_err(|source| MarketError::RunnerSpawn {
            runner_ref: offering.runner_ref.clone(),
            source,
        })?;
    let completed = child
        .wait_with_output()
        .map_err(|source| MarketError::RunnerSpawn {
            runner_ref: offering.runner_ref.clone(),
            source,
        })?;
    let output = String::from_utf8_lossy(&completed.stdout).into_owned();

    let usage = MeteredUsage {
        input_tokens: word_count(payload),
        output_tokens: word_count(&output),
    };
    let charge = meter_request(&capacity_offering_cost_profile(offering), &usage);

    Ok(ExecutionReceipt {
        offering_id: offering.offering_id.clone(),
        task_id: task_id.clone(),
        output,
        usage,
        charge,
        exit_code: completed.status.code().unwrap_or(-1),
    })
}

fn word_count(text: &str) -> u64 {
    u64::try_from(text.split_whitespace().count()).unwrap_or(u64::MAX)
}

// ---------------------------------------------------------------------------
// Ledger: append-only, double-entry settlement between members.

/// One line of a [`LedgerEntry`]: exactly one of `debit_micros` or
/// `credit_micros` is nonzero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posting {
    pub member_id: MemberId,
    #[serde(default)]
    pub debit_micros: u64,
    #[serde(default)]
    pub credit_micros: u64,
}

/// One balanced, immutable ledger event. Once constructed there is no method
/// on this type, or on [`LedgerWrite`], that can change or remove a posting —
/// the only mutation surface is appending a brand new entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: String,
    pub recorded_at: String,
    pub description: String,
    #[serde(default)]
    pub capacity_offering_id: Option<OfferingId>,
    #[serde(default)]
    pub task_id: Option<TaskId>,
    pub postings: Vec<Posting>,
}

impl LedgerEntry {
    pub fn total_debit_micros(&self) -> u128 {
        self.postings
            .iter()
            .map(|posting| u128::from(posting.debit_micros))
            .sum()
    }

    pub fn total_credit_micros(&self) -> u128 {
        self.postings
            .iter()
            .map(|posting| u128::from(posting.credit_micros))
            .sum()
    }

    /// Checks the double-entry invariant: every posting is exactly one of a
    /// debit or a credit, there are at least two of them, and the entry's
    /// total debits equal its total credits.
    pub fn validate(&self) -> Result<(), MarketError> {
        if self.id.trim().is_empty() {
            return Err(DomainError::EmptyField("ledger_entry.id").into());
        }
        if self.postings.len() < 2 {
            return Err(MarketError::LedgerEntryTooFewPostings);
        }
        for posting in &self.postings {
            let is_debit = posting.debit_micros > 0;
            let is_credit = posting.credit_micros > 0;
            if is_debit == is_credit {
                return Err(MarketError::UnbalancedPosting(posting.member_id.clone()));
            }
        }
        let debit_micros = self.total_debit_micros();
        let credit_micros = self.total_credit_micros();
        if debit_micros != credit_micros {
            return Err(MarketError::LedgerEntryNotBalanced {
                id: self.id.clone(),
                debit_micros,
                credit_micros,
            });
        }
        Ok(())
    }
}

/// Builds the balanced two-posting entry that records one member consuming
/// another member's lent capacity: the consumer is debited (they now owe),
/// the lender is credited (they are now owed), for the identical amount.
#[allow(clippy::too_many_arguments)]
pub fn charge_entry(
    id: impl Into<String>,
    recorded_at: impl Into<String>,
    description: impl Into<String>,
    consumer_member_id: MemberId,
    lender_member_id: MemberId,
    amount_micros: u64,
    capacity_offering_id: Option<OfferingId>,
    task_id: Option<TaskId>,
) -> LedgerEntry {
    LedgerEntry {
        id: id.into(),
        recorded_at: recorded_at.into(),
        description: description.into(),
        capacity_offering_id,
        task_id,
        postings: vec![
            Posting {
                member_id: consumer_member_id,
                debit_micros: amount_micros,
                credit_micros: 0,
            },
            Posting {
                member_id: lender_member_id,
                debit_micros: 0,
                credit_micros: amount_micros,
            },
        ],
    }
}

/// Narrow read surface over the ledger.
pub trait LedgerRead {
    fn entries(&self) -> Result<Vec<LedgerEntry>, MarketError>;
}

/// The ledger's only mutation surface: append one validated, balanced entry.
/// There is intentionally no update or delete method anywhere in this trait
/// or its implementations.
pub trait LedgerWrite {
    fn append_entry(&self, entry: LedgerEntry) -> Result<(), MarketError>;
}

/// An in-process, append-only double-entry ledger.
#[derive(Default)]
pub struct InMemoryLedger {
    entries: Mutex<Vec<LedgerEntry>>,
}

impl InMemoryLedger {
    pub fn new() -> Self {
        Self::default()
    }
}

impl LedgerRead for InMemoryLedger {
    fn entries(&self) -> Result<Vec<LedgerEntry>, MarketError> {
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }
}

impl LedgerWrite for InMemoryLedger {
    fn append_entry(&self, entry: LedgerEntry) -> Result<(), MarketError> {
        entry.validate()?;
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.iter().any(|existing| existing.id == entry.id) {
            return Err(MarketError::DuplicateLedgerEntryId(entry.id));
        }
        entries.push(entry);
        Ok(())
    }
}

/// Replays a full entry history into each member's running balance.
///
/// Positive means the member is a net creditor (owed money); negative means
/// they are a net debtor (they owe). Because every entry balances by
/// construction, the sum of every balance in the result is always zero — a
/// property callers can and should check after a replay.
pub fn replay_balances(entries: &[LedgerEntry]) -> BTreeMap<MemberId, i128> {
    let mut balances: BTreeMap<MemberId, i128> = BTreeMap::new();
    for entry in entries {
        for posting in &entry.postings {
            let delta = i128::from(posting.credit_micros) - i128::from(posting.debit_micros);
            *balances.entry(posting.member_id.clone()).or_insert(0) += delta;
        }
    }
    balances
}

/// One member's net position, derived by replaying the whole ledger.
pub fn net_position(entries: &[LedgerEntry], member_id: &MemberId) -> i128 {
    replay_balances(entries)
        .get(member_id)
        .copied()
        .unwrap_or(0)
}

/// Net amount each debtor owes each creditor, for settlement between a pair
/// of members. Only entries with exactly two postings (one debit, one
/// credit) — the shape [`charge_entry`] produces — contribute to a pair;
/// entries with more postings still count in [`replay_balances`], but a
/// pairwise attribution for them is not this function's job.
pub fn pairwise_settlement(entries: &[LedgerEntry]) -> BTreeMap<(MemberId, MemberId), i128> {
    let mut net: BTreeMap<(MemberId, MemberId), i128> = BTreeMap::new();
    for entry in entries {
        let [first, second] = entry.postings.as_slice() else {
            continue;
        };
        let (debtor, creditor, amount_micros) = if first.debit_micros > 0 {
            (&first.member_id, &second.member_id, first.debit_micros)
        } else {
            (&second.member_id, &first.member_id, second.debit_micros)
        };
        *net.entry((debtor.clone(), creditor.clone())).or_insert(0) += i128::from(amount_micros);
    }
    net
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use workforce_domain::{RiskLevel, SkillId, SkillRequirement, VerificationPolicy};
    use workforce_store::{PrivateLocalStore, PublicIndexStore, SnapshotRecord};

    use super::*;

    const SKILL: &str = "skill:capacity-lending";
    const NOW_MS: i64 = 1_786_000_000_000;

    fn offering(
        id: &str,
        lender: &str,
        listed_price: u64,
        quota_per_request: u64,
    ) -> CapacityOffering {
        CapacityOffering {
            offering_id: OfferingId::from(id),
            lender_member_id: MemberId::from(lender),
            model_release_id: "model:lent-2026-01-01".into(),
            skill_id: SkillId::from(SKILL),
            runner_ref: "sonnet-5".into(),
            currency: "USD".to_owned(),
            quota_milliunits_per_request: quota_per_request,
            quota_milliunits_available: 100_000,
            listed_price_micros_per_quota_milliunit: listed_price,
            context_window_tokens: 200_000,
            effective_from_epoch_ms: NOW_MS - 1_000,
            effective_until_epoch_ms: Some(NOW_MS + 1_000_000),
            recorded_at: "2026-09-01T00:00:00Z".to_owned(),
        }
    }

    fn success(mean: f64, lower_bound: f64, evidence_count: u64) -> ProbabilityEstimate {
        ProbabilityEstimate {
            success_mean: mean,
            success_lower_bound: lower_bound,
            evidence_count,
        }
    }

    fn task() -> TaskSpec {
        TaskSpec {
            id: TaskId::from("task:lend-me-capacity"),
            summary: "Use a lender's spare quota".to_owned(),
            repository: None,
            required_skills: vec![SkillRequirement {
                skill_id: SkillId::from(SKILL),
                minimum_success_probability: 0.1,
                minimum_evidence_count: 0,
            }],
            required_tools: BTreeSet::new(),
            allowed_providers: BTreeSet::new(),
            privacy: PrivacyClass::PrivateMetadata,
            risk: RiskLevel::Low,
            verification: VerificationPolicy::Deterministic,
            minimum_success_probability: 0.1,
            minimum_evidence_count: 0,
            max_expected_cash_micros: None,
            max_p95_latency_ms: None,
            estimated_input_tokens: 20_000,
            estimated_output_tokens: 4_000,
        }
    }

    fn routing_policy(quota_shadow_cash_micros_per_unit: u64) -> RoutingPolicy {
        RoutingPolicy {
            policy_id: "policy:capacity-market-v1".to_owned(),
            currency: "USD".to_owned(),
            quota_shadow_cash_micros_per_unit,
            max_expected_quota_milliunits: None,
            authorized_checker_worker_ids: BTreeSet::new(),
            failure_probability_basis: workforce_engine::FailureProbabilityBasis::Mean,
            max_attempts: 2,
        }
    }

    // -- 1. Offering listing ------------------------------------------------

    #[test]
    fn published_offerings_are_listed_and_the_roster_is_append_only() {
        let roster = InMemoryCapacityRoster::new();
        let alice = offering("offering:alice-1", "member:alice", 200, 500);
        let bob = offering("offering:bob-1", "member:bob", 150, 300);

        roster
            .publish_capacity_offering(alice.clone())
            .expect("publish alice's offering");
        roster
            .publish_capacity_offering(bob.clone())
            .expect("publish bob's offering");

        let listed = roster.capacity_offerings().expect("list offerings");
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&alice));
        assert!(listed.contains(&bob));

        assert_eq!(
            roster
                .capacity_offering(&alice.offering_id)
                .expect("lookup")
                .as_ref(),
            Some(&alice)
        );

        // Append-only: publishing the same offering id again is rejected, and
        // there is no method anywhere on this trait to edit or remove one.
        let err = roster
            .publish_capacity_offering(alice.clone())
            .expect_err("duplicate offering id must be rejected");
        assert!(matches!(err, MarketError::DuplicateOfferingId(id) if id == alice.offering_id));
        assert_eq!(
            roster.capacity_offerings().expect("list offerings").len(),
            2
        );
    }

    #[test]
    fn offering_listing_respects_the_effective_window() {
        let roster = InMemoryCapacityRoster::new();
        let mut expired = offering("offering:expired", "member:alice", 200, 500);
        expired.effective_until_epoch_ms = Some(NOW_MS - 500);
        roster
            .publish_capacity_offering(expired)
            .expect("publish expired offering");

        let live = offering("offering:live", "member:alice", 200, 500);
        roster
            .publish_capacity_offering(live.clone())
            .expect("publish live offering");

        let current = roster
            .current_capacity_offerings(NOW_MS)
            .expect("current offerings");
        assert_eq!(current, vec![live]);
    }

    // -- 2. Allocator matching at a listed price -----------------------------

    #[test]
    fn engine_matches_a_task_to_a_capacity_offering_at_its_listed_price() {
        let lent = offering("offering:alice-1", "member:alice", 200, 500);
        let estimate = capacity_offering_worker_estimate(
            &lent,
            success(0.95, 0.9, 5),
            15_000,
            "snapshot:capacity-market-v1",
        )
        .expect("build candidate");

        let policy = routing_policy(20);
        let request = QuoteRequest {
            decision_id: DecisionId::from("decision:capacity-match"),
            evidence_snapshot_id: "snapshot:capacity-market-v1".to_owned(),
            task: task(),
            policy,
            candidates: vec![estimate],
        };
        let result = workforce_engine::quote(&request).expect("quote");

        assert_eq!(
            result.selected_worker_id,
            Some(capacity_worker_id(&lent.offering_id))
        );
        let selected = &result.eligible_candidates[0];
        // Cash cost is exactly the listed price: 200 micros/milliunit * 500
        // milliunits/request, with zero token-based cost (the market prices
        // quota, not tokens).
        assert_eq!(selected.cost.expected_cash_micros, 200 * 500);
        // The quota shadow cost still applies on top, from the engine's own,
        // unmodified formula: ceil(500 * 20 / 1000) = 10.
        assert_eq!(selected.cost.quota_shadow_cash_micros, 10);
        assert_eq!(selected.cost.expected_accepted_cost_micros, 200 * 500 + 10);
    }

    #[test]
    fn a_cheaper_listed_price_wins_the_match_over_a_pricier_one() {
        let cheap = offering("offering:cheap-lender", "member:alice", 50, 500);
        let pricey = offering("offering:pricey-lender", "member:carol", 400, 500);

        let candidates = vec![
            capacity_offering_worker_estimate(
                &cheap,
                success(0.9, 0.85, 3),
                10_000,
                "snapshot:capacity-market-v1",
            )
            .expect("cheap candidate"),
            capacity_offering_worker_estimate(
                &pricey,
                success(0.9, 0.85, 3),
                10_000,
                "snapshot:capacity-market-v1",
            )
            .expect("pricey candidate"),
        ];
        let request = QuoteRequest {
            decision_id: DecisionId::from("decision:capacity-price-competition"),
            evidence_snapshot_id: "snapshot:capacity-market-v1".to_owned(),
            task: task(),
            policy: routing_policy(0),
            candidates,
        };
        let result = workforce_engine::quote(&request).expect("quote");

        assert_eq!(
            result.selected_worker_id,
            Some(capacity_worker_id(&cheap.offering_id))
        );
    }

    #[test]
    fn quote_with_capacity_offerings_merges_the_roster_and_the_market_through_the_allocator() {
        let public = PublicIndexStore::in_memory().expect("open public index");
        let snapshot = SnapshotRecord::new(
            "snapshot:empty-roster",
            "2026-09-01T00:00:00Z",
            "ontology:v1",
            "source:v1",
            vec![],
            vec![],
            vec![],
            vec![],
        )
        .expect("empty snapshot");
        workforce_store::PublicIndexWrite::append_snapshot(&public, &snapshot)
            .expect("append snapshot");
        let private = PrivateLocalStore::in_memory().expect("open private ledger");

        let lent = offering("offering:alice-1", "member:alice", 200, 500);
        let (_request, quote) = quote_with_capacity_offerings(
            &public,
            &private,
            &snapshot.id,
            &task(),
            &CalibrationPolicy::default(),
            &WorkflowAssumptions {
                default_p95_latency_ms: 10_000,
                ..WorkflowAssumptions::default()
            },
            &routing_policy(0),
            DecisionId::from("decision:through-the-allocator"),
            &[(lent.clone(), success(0.9, 0.85, 4))],
            NOW_MS,
        )
        .expect("quote with capacity offerings");

        assert_eq!(
            quote.selected_worker_id,
            Some(capacity_worker_id(&lent.offering_id))
        );
    }

    // -- 3. Real-measured-token-cost metering --------------------------------

    #[test]
    fn metering_charges_measured_usage_not_the_tasks_estimate() {
        let lent = offering("offering:alice-1", "member:alice", 200, 500);
        let cost = capacity_offering_cost_profile(&lent);
        let bare_task = task();

        // What an estimate-based charge would have been, using the task's
        // pre-execution token guess.
        let estimated = meter_request(
            &cost,
            &MeteredUsage {
                input_tokens: bare_task.estimated_input_tokens,
                output_tokens: bare_task.estimated_output_tokens,
            },
        );
        // What actually happened: a much shorter real exchange.
        let measured = meter_request(
            &cost,
            &MeteredUsage {
                input_tokens: 12,
                output_tokens: 4,
            },
        );

        // Both equal the fixed listed-price charge, because this market prices
        // lent quota per request, not per token — but the two usages are
        // distinct real inputs, and metering must use the one that was
        // actually measured, never silently substitute the estimate.
        assert_eq!(estimated.cash_micros, measured.cash_micros);
        assert_ne!(
            estimated,
            MeteredCharge {
                cash_micros: 0,
                quota_milliunits: 0
            }
        );

        // With a per-token price the difference becomes visible: metering on
        // the real, small transcript must charge far less than the estimate.
        let mut token_priced = cost;
        token_priced.input_micros_per_million_tokens = 3_000_000;
        token_priced.output_micros_per_million_tokens = 15_000_000;
        let estimated_tokens = meter_request(
            &token_priced,
            &MeteredUsage {
                input_tokens: bare_task.estimated_input_tokens,
                output_tokens: bare_task.estimated_output_tokens,
            },
        );
        let measured_tokens = meter_request(
            &token_priced,
            &MeteredUsage {
                input_tokens: 12,
                output_tokens: 4,
            },
        );
        assert!(
            measured_tokens.cash_micros < estimated_tokens.cash_micros,
            "a real 12/4-token exchange must be metered far below the \
             {}/{}-token estimate: {} vs {}",
            bare_task.estimated_input_tokens,
            bare_task.estimated_output_tokens,
            measured_tokens.cash_micros,
            estimated_tokens.cash_micros
        );
        // quota_milliunits_per_request is reused unchanged by metering too.
        assert_eq!(
            measured_tokens.quota_milliunits,
            lent.quota_milliunits_per_request
        );
    }

    // -- 4. Double-entry ledger invariants ------------------------------------

    #[test]
    fn unbalanced_ledger_entries_are_rejected_and_never_stored() {
        let ledger = InMemoryLedger::new();
        let unbalanced = LedgerEntry {
            id: "ledger:bad-1".to_owned(),
            recorded_at: "2026-09-01T00:00:00Z".to_owned(),
            description: "deliberately unbalanced".to_owned(),
            capacity_offering_id: None,
            task_id: None,
            postings: vec![
                Posting {
                    member_id: "member:bob".into(),
                    debit_micros: 100,
                    credit_micros: 0,
                },
                Posting {
                    member_id: "member:alice".into(),
                    debit_micros: 0,
                    credit_micros: 90,
                },
            ],
        };

        let err = ledger
            .append_entry(unbalanced)
            .expect_err("unbalanced entry must be rejected");
        assert!(matches!(err, MarketError::LedgerEntryNotBalanced { .. }));
        assert!(ledger.entries().expect("entries").is_empty());

        let both_zero = LedgerEntry {
            id: "ledger:bad-2".to_owned(),
            recorded_at: "2026-09-01T00:00:00Z".to_owned(),
            description: "no amount on either side".to_owned(),
            capacity_offering_id: None,
            task_id: None,
            postings: vec![
                Posting {
                    member_id: "member:bob".into(),
                    debit_micros: 0,
                    credit_micros: 0,
                },
                Posting {
                    member_id: "member:alice".into(),
                    debit_micros: 0,
                    credit_micros: 0,
                },
            ],
        };
        let err = ledger
            .append_entry(both_zero)
            .expect_err("a posting must be exactly a debit or a credit");
        assert!(matches!(err, MarketError::UnbalancedPosting(_)));
    }

    #[test]
    fn ledger_is_append_only_and_rejects_duplicate_ids() {
        let ledger = InMemoryLedger::new();
        let entry = charge_entry(
            "ledger:1",
            "2026-09-01T00:00:00Z",
            "bob used alice's lent quota",
            MemberId::from("member:bob"),
            MemberId::from("member:alice"),
            100_000,
            Some(OfferingId::from("offering:alice-1")),
            Some(TaskId::from("task:lend-me-capacity")),
        );
        ledger.append_entry(entry.clone()).expect("append entry");
        assert_eq!(ledger.entries().expect("entries"), vec![entry.clone()]);

        // Same id, even though the amounts still balance: rejected. The
        // ledger only ever grows; nothing can rewrite a settled entry.
        let err = ledger
            .append_entry(entry.clone())
            .expect_err("duplicate id must be rejected");
        assert!(matches!(err, MarketError::DuplicateLedgerEntryId(id) if id == entry.id));
        assert_eq!(ledger.entries().expect("entries").len(), 1);
    }

    #[test]
    fn replaying_all_entries_reproduces_each_members_balance_and_nets_to_zero() {
        let ledger = InMemoryLedger::new();
        let entries = [
            charge_entry(
                "ledger:1",
                "2026-09-01T00:00:00Z",
                "bob used alice's quota",
                MemberId::from("member:bob"),
                MemberId::from("member:alice"),
                100_000,
                Some(OfferingId::from("offering:alice-1")),
                None,
            ),
            charge_entry(
                "ledger:2",
                "2026-09-01T00:05:00Z",
                "carol used alice's quota",
                MemberId::from("member:carol"),
                MemberId::from("member:alice"),
                40_000,
                Some(OfferingId::from("offering:alice-1")),
                None,
            ),
            charge_entry(
                "ledger:3",
                "2026-09-01T00:10:00Z",
                "alice used bob's quota",
                MemberId::from("member:alice"),
                MemberId::from("member:bob"),
                25_000,
                Some(OfferingId::from("offering:bob-1")),
                None,
            ),
        ];
        for entry in entries.clone() {
            ledger.append_entry(entry).expect("append entry");
        }

        // "Running balances derivable by replay": nothing but the entries
        // themselves is needed to reconstruct every member's balance.
        let replayed = replay_balances(&ledger.entries().expect("entries"));

        // alice: +100,000 +40,000 (credited, lent out) -25,000 (debited, consumed) = 115,000
        assert_eq!(replayed[&MemberId::from("member:alice")], 115_000);
        // bob: -100,000 (debited, consumed) +25,000 (credited, lent out) = -75,000
        assert_eq!(replayed[&MemberId::from("member:bob")], -75_000);
        // carol: -40,000 (debited, consumed)
        assert_eq!(replayed[&MemberId::from("member:carol")], -40_000);

        assert_eq!(
            replayed.values().sum::<i128>(),
            0,
            "double entry must net to zero"
        );
        assert_eq!(
            net_position(
                &ledger.entries().expect("entries"),
                &MemberId::from("member:alice")
            ),
            115_000
        );

        let pairwise = pairwise_settlement(&ledger.entries().expect("entries"));
        assert_eq!(
            pairwise[&(MemberId::from("member:bob"), MemberId::from("member:alice"))],
            100_000
        );
        assert_eq!(
            pairwise[&(
                MemberId::from("member:carol"),
                MemberId::from("member:alice")
            )],
            40_000
        );
        assert_eq!(
            pairwise[&(MemberId::from("member:alice"), MemberId::from("member:bob"))],
            25_000
        );
    }

    // -- Runner indirection reuse: tools/owi-do's runners.json contract ------

    #[test]
    fn runners_json_directory_reuses_the_owi_do_file_contract() {
        let dir = std::env::temp_dir().join(format!(
            "workforce-market-runners-{}-{}",
            std::process::id(),
            NOW_MS
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("runners.json");
        std::fs::write(
            &path,
            r#"{
                "_comment": ["model -> command, exactly like tools/owi-do writes it"],
                "sonnet-5": "cat",
                "unset-model": ""
            }"#,
        )
        .expect("write runners.json");

        let directory = RunnersJsonDirectory::open(&path).expect("open runners.json");
        assert_eq!(
            directory.resolve(&RunnerRef::from("sonnet-5")),
            Some("cat".to_owned())
        );
        // A blank command, exactly like an unfilled prefilled entry in
        // tools/owi-do's runners.json, resolves to nothing.
        assert_eq!(directory.resolve(&RunnerRef::from("unset-model")), None);
        assert_eq!(
            directory.resolve(&RunnerRef::from("never-configured")),
            None
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // -- 4/5. End to end: execute against another member's offering, and no --
    // credential ever appears in an engine-side struct.

    struct FakeMemberMachine {
        commands: BTreeMap<String, String>,
    }

    impl MemberRunnerDirectory for FakeMemberMachine {
        fn resolve(&self, runner_ref: &RunnerRef) -> Option<String> {
            self.commands.get(&runner_ref.0).cloned()
        }
    }

    #[test]
    fn task_is_matched_and_executed_against_another_members_offering_without_exposing_their_credential()
     {
        const SECRET: &str = "sk-alice-super-secret-live-key-000111222";

        // Alice publishes her lent capacity into the roster, exactly as any
        // member would.
        let roster = InMemoryCapacityRoster::new();
        let lent = offering("offering:alice-1", "member:alice", 200, 500);
        roster
            .publish_capacity_offering(lent.clone())
            .expect("alice publishes her offering");

        // Bob's task is matched against the roster through the ordinary
        // engine quote — the same `workforce_engine::quote` every other
        // allocation decision goes through. Nothing here has seen a
        // credential: the candidate carries only `RunnerRef`, an opaque name.
        let candidate_task_id = TaskId::from("task:bob-borrows-alices-quota");
        let mut bobs_task = task();
        bobs_task.id = candidate_task_id.clone();
        let published = roster
            .current_capacity_offerings(NOW_MS)
            .expect("list current offerings");
        let candidates = published
            .iter()
            .map(|offering| {
                capacity_offering_worker_estimate(
                    offering,
                    success(0.9, 0.85, 4),
                    10_000,
                    "snapshot:capacity-market-v1",
                )
                .expect("worker estimate for the offering")
            })
            .collect();
        let match_request = QuoteRequest {
            decision_id: DecisionId::from("decision:bob-borrows-alice"),
            evidence_snapshot_id: "snapshot:capacity-market-v1".to_owned(),
            task: bobs_task,
            policy: routing_policy(0),
            candidates,
        };
        let matched = workforce_engine::quote(&match_request).expect("match bob's task");
        assert_eq!(
            matched.selected_worker_id,
            Some(capacity_worker_id(&lent.offering_id)),
            "bob's task must be matched to alice's published offering"
        );

        // Only now, having been matched, does execution happen — strictly
        // through the runner indirection. Alice's own machine: only she
        // holds this directory, and only it ever contains her credential.
        // The engine never sees it — it is handed to `execute_via_runner`
        // purely as a local trait object.
        let alices_machine = FakeMemberMachine {
            commands: BTreeMap::from([(
                "sonnet-5".to_owned(),
                format!(
                    "API_KEY={SECRET} sh -c 'if [ -n \"$API_KEY\" ]; then cat; else echo NO_CRED; fi'"
                ),
            )]),
        };
        let task_id = candidate_task_id;
        let payload = "summarize this quarter's release notes";

        let receipt = execute_via_runner(&lent, &task_id, payload, &alices_machine)
            .expect("execute against alice's matched offering");

        // The credential authorized the run (the resolved command only `cat`s
        // when $API_KEY is set) but never appears in the worker's output.
        assert_eq!(receipt.output, payload);
        assert_ne!(receipt.output, "NO_CRED\n");
        assert!(!receipt.output.contains(SECRET));

        let charge_entry = charge_entry(
            "ledger:bob-borrows-alice",
            "2026-09-01T00:00:00Z",
            "bob executed a task against alice's lent capacity",
            MemberId::from("member:bob"),
            lent.lender_member_id.clone(),
            receipt.charge.cash_micros,
            Some(lent.offering_id.clone()),
            Some(task_id.clone()),
        );
        let ledger = InMemoryLedger::new();
        ledger
            .append_entry(charge_entry.clone())
            .expect("record settlement");

        let estimate = capacity_offering_worker_estimate(
            &lent,
            success(0.9, 0.85, 4),
            10_000,
            "snapshot:capacity-market-v1",
        )
        .expect("worker estimate for the offering");

        // Scan every engine-side struct this flow produced — the published
        // offering, the match request and quote that selected it, the
        // derived worker estimate, the execution receipt, and the ledger
        // entry — in both their serialized and debug forms. None of them may
        // contain the lender's credential.
        let haystacks = [
            serde_json::to_string(&lent).expect("serialize offering"),
            format!("{lent:?}"),
            serde_json::to_string(&match_request).expect("serialize match request"),
            format!("{match_request:?}"),
            serde_json::to_string(&matched).expect("serialize routing quote"),
            format!("{matched:?}"),
            serde_json::to_string(&estimate).expect("serialize worker estimate"),
            format!("{estimate:?}"),
            serde_json::to_string(&receipt).expect("serialize receipt"),
            format!("{receipt:?}"),
            serde_json::to_string(&charge_entry).expect("serialize ledger entry"),
            format!("{charge_entry:?}"),
            serde_json::to_string(&ledger.entries().expect("ledger entries"))
                .expect("serialize ledger"),
        ];
        for haystack in haystacks {
            assert!(
                !haystack.contains(SECRET),
                "credential leaked into an engine-side value: {haystack}"
            );
            assert!(!haystack.contains("API_KEY"));
        }
    }
}
