# Roadmap

The roadmap is ordered by evidence, not UI surface area.

The single question that decides whether this project is worth building is
whether routing on expected accepted-result cost beats always using the
strongest model, measured on real tasks with real prices. Everything before
that answer is scaffolding; everything after it depends on the answer. The
roadmap is ordered accordingly.

## v0.1 — decision kernel and closed loop

- Exact, immutable worker identity
- Public/private SQLite trust boundary
- Exact Beta lower credible bounds and evidence-count gating
- Real published token prices imported with provenance and a content digest
- Hard eligibility filters and expected accepted-result cost
- Auditable quote, exclusion explanations, and Pareto frontier
- SKOS/PROV-O ontology plus SHACL contract
- **Closed loop**: index evidence and private outcomes are calibrated into
  candidates, quoted, recorded, and fed back into the next decision
  (`owi seed` → `owi allocate` → `owi outcome` → `owi allocate`)
- CLI examples and privacy regression tests

## v0.2 — real evidence

The point of this release is one number, not a feature list. Nothing else in
the roadmap is worth starting until it exists.

- A provider execution adapter for at least three real models, with
  credentials held locally
- A deterministic task class where acceptance is objective and free to check:
  *make this failing test pass*, scored by the test suite
- A benchmark runner that records real pass/fail, tokens, cost, and latency as
  `PublicEvidenceRecord` and `OutcomeEvent` rows
- ~~Real published prices with `source_url` and retrieval date~~ — done in
  v0.1 via `owi prices` and the LiteLLM adapter
- **The measurement**: expected accepted-result cost routing versus
  always-strongest and always-cheapest, over N tasks, reported with its
  acceptance rate — published whichever way it comes out

If routing does not win, that is the finding, and the rest of this roadmap
does not happen.

## v0.3 — reproducible index

- Versioned JSON source schema and import adapters
- Domain/task applicability metadata, keeping factual recall, calibrated
  abstention, and protocol separate from artifact and tool capabilities
- Model-release and provider-offering lifecycle gates
- Historical price/quota schedules
- Hash-addressed, canonicalized, and signed public snapshots
- Reproducibility manifest and source-license checks
- Scheduled discovery with human-reviewed promotion
- Typed SHACL execution over a projected snapshot

## v0.4 — personal workforce

- Repository context packs, worktree isolation, and permission leases
- Deterministic test/lint/build verification
- Cheap-first escalation and independent maker/checker policies
- Multi-dimensional budgets for API cash, subscriptions, local compute, and time
- Private Git project registry and an append-only usage ledger that reconciles
  exactly to every source total
- Local dashboard with plan preview and assignment override

## v0.5 — learned allocation

- Repository/task-class calibration and drift detection
- Low-risk, capped Thompson exploration
- Champion/challenger evaluation from opted-in tasks
- Whole-project DAG allocation under budget and deadline

## Deferred

Designs that are written but deliberately not scheduled live in
[`docs/future/`](future/): private project cost accounting, environmental
impact accounting, and the browser advisor. Each is a separate product, and
none of them makes the routing thesis more or less true. They come back when
the thing they account for exists.

## Not before evidence supports it

- A neural router trained on private telemetry
- An open marketplace or multi-tenant hosted execution service
- Full OWL-DL reasoning in the critical allocation path
- Anonymous upload of prompts, repositories, or private outcomes
