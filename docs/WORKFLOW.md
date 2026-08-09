# The complete workflow

Every step below was executed and verified before being written down. The
ladder has five rungs; each works on its own, and each records into the same
loop. Start at the top and climb only when you want to.

```text
you say a task
     │
     ▼
guess the skill  ──►  hard gates (capability · tools · privacy · quality)
     │                          │
     ▼                          ▼
price the survivors      turn away the rest, with reasons
(real published rates)
     │
     ▼
→ cheapest qualified worker, alternates, one price
     │
     ▼ (--run / run button)
your CLI executes it — your command, your key
     │
     ▼
accepted?  [y/n]  ──►  verified outcome in the private ledger
     │
     └──────────► next pick is smarter ─────────┐
                        ▲                       │
                        └───────────────────────┘
```

## Level 0 — the link (zero install)

Open the ask page, type a task, press enter.

- picks the cheapest qualified model at real prices
- a task that says "do X, then Y" is cut into parts right in the page
  (sequence markers, newlines, numbered steps — mechanical and visible);
  each part is classified and staffed on its own bench with its own launch
  button and verdict, so a code part and a CAD part land on different
  workers instead of one model getting everything
- give a checklist and it travels with the task; paste the result back and
  the mechanical items are verified right in the page — the verdict feeds
  the same learning update, no server involved
- `worked` / `didn't` applies the engine's own Beta update, stored in that
  browser only — the page says so itself
- cannot execute: a published page holds no credentials by design

## Level 1 — one command

```bash
git clone https://github.com/Morshedvarzandeh/Open-Workforce-Index
cd Open-Workforce-Index          # needs Rust 1.87+ and Python 3
tools/owi-do "rewrite this email to the supplier"
```

First run builds `.owi-quick/` automatically: real prices, the demo roster,
an empty private ledger, and `runners.json` — **prefilled with working
commands for Anthropic models when the `claude` CLI is on your PATH**. For
other models, paste in whatever CLI you use (`"gpt-5-mini": "llm -m
gpt-5-mini"`). Your command, your key; the tool only pipes the task to it.

```bash
tools/owi-do "rewrite this email to the supplier" --run
```

Picks, executes, shows the output, asks `accepted? [y/n]`, records the
answer to the real ledger. If the cheapest pick has no runner, it says so
and runs the cheapest *runnable* worker instead — a worker nobody can
execute is unavailable, not a bargain.

Two more flags complete the manager's loop:

```bash
# the verdict is earned, not asked for: a checklist rides with the task
tools/owi-do "write the release note" --run \
  --check "contains:thermal" --check "min-words:60" \
  --check "states that the feature is reachable through a wasm ABI"

# one request, several kinds of work: a cheap planner splits it,
# each part is staffed, run, and checked separately
tools/owi-do "draft the changelog and extract its fields as JSON" --split --run
```

Verified transcript from a real run:

```text
→ haiku-4-5/text   $0.0099 per accepted result
running via: claude --model claude-haiku-4-5-20251001 -p
...a complete, usable supplier email...
recorded: accepted — the next pick will know
```

## Level 2 — the running page

```bash
tools/owi-serve        # opens a token-gated local URL
```

This is a persisted project workflow, not a browser-only task splitter:

1. Create a project goal. A configured planning model may propose specialist
   tasks only when you explicitly enable that paid, unmetered planning call.
   It is disabled for zero-budget, confidential, and secret projects;
   otherwise the server exposes a clearly labelled deterministic fallback.
2. Inspect, add, edit, or remove draft tasks and their acceptance checks.
3. Press **Staff**. The Rust allocator evaluates each task independently and
   stores the exact worker, alternatives, exclusions, and accepted-result cost
   forecast. Drafting does not silently staff or execute work.
4. Press **Run** on a staffed task. The browser sends only the persisted task
   identifier; the server derives the saved worker, model, brief, and local
   runner command. A missing runner becomes `setup_required` and cannot run.
5. Mechanical checks run automatically. Uncheckable work becomes
   `needs_review`; the owner can accept it, reject it with a cause, retry the
   same assignment, or restaff after failure so new evidence can change the
   next allocation.

Projects, task states, outputs, check reports, and workflow events survive a
reload in the private `workflow.sqlite`. Decisive outcomes are also recorded
in the append-only OWI evidence ledger. The server enforces the forecast budget
before execution and keeps actual cash unknown until a provider or invoice
receipt exists.

The graphical office is intentionally disabled on a published static page: a
web page without your local credentials cannot pretend to run a model. In the
local server every bind requires a per-process token, JSON and same-origin
writes are enforced, and the legacy browser-supplied `/api/run` and
`/api/outcome` paths return `410`.

The launcher first resolves a packaged or installed `owi` CLI (`OWI_BIN`,
`PATH`, or repository release/debug binary), then uses Cargo only as a
developer fallback. Model execution requires an explicit command in the
workspace's `runners.json` keyed by the exact staffed worker ID. Model-only
keys are not enough for project execution because they cannot prove the
quoted provider/harness/tool/privacy identity. `--host 0.0.0.0` is only for a
trusted private network behind the token gate; see [ROUTER.md](ROUTER.md).

## Level 3 — a project, planned automatically

Stop asking task by task. List the work once, in the repo:

```bash
$EDITOR .owi/project.json        # the features you want built
tools/owi_plan.py --owi-repo . --project .owi/project.json \
  --index .data/index.sqlite --local .data/local.sqlite --work-dir /tmp/plan
```

- every feature staffed to the cheapest qualified worker, with run / setup /
  your-time / total separated, and three totals (right-sized, always-premium,
  cheapest-per-token) so the saving is falsifiable
- unstaffable work is reported with the exact gate that blocks it —
  capability, clearance, policy, quota — the four constraints no budget clears
- `--scenarios 0,15,60,200` sweeps what your hour is worth and shows where
  the staffing flips; `--budget-micros` and `--fail-on-unstaffed` give exit
  codes for CI
- a post-commit hook re-plans when the manifest changes; a GitHub workflow
  does the same on pull requests and posts the plan to the job summary
- `tools/owi_console.py` renders the plan as a live-dial GUI
  (see `projects/battery-design.json` for a real multi-library example)

## Level 4 — measurement, the step that makes it honest

Everything above prices real rates but *assumes* ability. This rung replaces
assumption with evidence:

```bash
python3 tools/extract_bench_tasks.py --repo /path/to/your-repo ...
python3 tools/run_bench.py --corpus corpus.json --adapter oracle ...   # must be 11/11
python3 tools/run_bench.py --corpus corpus.json --adapter stub ...     # must be 0/11
python3 tools/run_bench.py --corpus corpus.json --adapter command \
  --command "claude --model claude-haiku-4-5-20251001 -p" --outcome-dir out/
for f in out/*.json; do owi outcome --local .data/local.sqlite --input "$f"; done
```

Deterministic tasks from your own test suite, calibrated both ways before a
cent is spent, outcomes fed straight into the same ledger every level above
reads. Full detail: [BENCHMARKING.md](BENCHMARKING.md).

## The checklist: verdicts earned, not asked for

Every `--run` has a test procedure. Give one yourself with `--check` (or the
checklist box on the ask page); give none and the **smartest** runnable model
writes it for the task — printed before the run, `--check none` to opt out
into the manual `y/n`. The spec is the leverage point, so it comes from the
top of the roster; execution stays with the cheapest qualified worker and
grading with a cheap non-maker judge. Either way, three things happen:

1. **The checklist is given to the task.** The worker sees exactly what it
   will be checked against — a checklist you'd verify with is also the best
   spec you can hand over.
2. **Every item is verified against the output, by kind.** Mechanical kinds
   run in process: `contains:TEXT`, `regex:PATTERN`, `json`, `python`,
   `min-words:N`. Anything else is `judged` — by the cheapest runnable model
   that is **not** the maker, the same maker-is-not-checker rule the store
   enforces on outcome records.
3. **Decisive verdicts record themselves.** All items pass → accepted; any
   item fails → rejected with root cause `worker` and the failed items as
   detail. Only when an item cannot be checked (no independent judge) does
   the question come back to you.

Every item's labelled result — kind, pass/fail, reason — persists in the
outcome's metadata in the append-only ledger, accepted and rejected alike,
so per-item evidence accumulates instead of evaporating. The plan for
promoting those labels into the ontology proper (per-item posteriors, judge
reliability) is ADR 0009 in `docs/future/`.

In a real first run of the split flow, the judge rejected a changelog part
with "no changelog entry was drafted at all; the response only asked
clarifying questions" — a genuine small-model failure a `min-words` item
would have missed. Free-form items exist because that class of failure
exists.

## Inspection economics: quality control, not quality theater

Checking everything forever is as wasteful as checking nothing — so
inspection follows the discipline of a production line (CSP-1 skip-lot
sampling, the lean answer to "how much QC"):

| state | when | what runs |
|---|---|---|
| **full** (100%) | a worker×skill with fewer than 5 consecutive clean accepts, or any explicit `--check` | spec written by the smartest model, every judgement item judged |
| **reduced** (skip-lot) | 5+ consecutive clean accepts | spec by a cheap model; judgement sampled 1-in-3 runs; sampled-out items marked in the record, never silently passed |
| **reset** | any worker-caused failure | straight back to full — the streak and the discount are gone |

Mechanical items (`contains`/`regex`/`json`/`python`/`min-words`) are free
and never come off the line — the inline sensors run on every unit.
Excused rejections (task_spec / harness / environment) neither break nor
extend the streak, the same rule the posterior applies. The control chart
is the ledger itself: streaks are computed from recorded outcomes, and
every outcome records which inspection level produced it. The Beta lower
bound remains the ongoing capability index — sampling reduces *inspection
spend*, never the evidence standard for staffing.

## Splitting: one request, several kinds of work

`--split` sends the request to the smartest runnable model acting as a
planner — planning, like checklist-writing, is spec work and comes from the
top of the roster. It answers one question — does this hold different kinds
of work? — and returns standalone part summaries with a small checklist
each. The planner **never names models**; staffing stays with the engine,
part by part, so a text part can go to the cheap writer while the extraction
part goes to whoever the evidence favours. The summary table at the end
names who actually ran each part, its price, and the checked verdict.

## Teams: hand-picked rosters, benchmarked

People already staff multi-agent workshops by hand — a planner on one
premium model, a debugger on another, a designer on a third, chosen by feel
and pinned by loyalty. Those rosters are this index's benchmark scenarios:
the skill taxonomy covers the roles people actually hire
(`planning-decomposition`, `code-review-debugging`, `ui-design`, alongside
text, extraction, code, and CAD), each staffed across the price tiers, so a
hand-built team can be answered seat by seat.

```bash
python3 tools/owi_team.py examples/team-buzz-style.json
```

runs a real allocation per role — same gates, same Beta lower-bound floor,
same cost model — and prints the evidence pick next to the hand pick. A
hand pick lands in one of three states: **priced** (it's in the index and
qualified, so the premium is a number), **gated** (in the index but turned
away, with the reason), or **external** (not in the index — no price, no
evidence, which is the point). Every row still surfaces the quality option,
so upgrading a seat stays a visible trade, not a default. `--rate` charges
your own review time per hour and shows how a roster shifts upmarket when
reliability starts paying for itself.

A roster you can't price is a roster you can't manage.

## Confidentiality: a gate, not a preference

Every task carries a privacy level (`--privacy public|metadata|confidential|
secret`, the checkbox on the ask page) and every worker a clearance. A task
above a worker's clearance turns that worker away whatever it costs — the
same lexicographic rule as capability and tools. Cloud workers carry
`private_metadata`; only workers running on your own machine are cleared for
confidential content — the roster's `local-llama` (ollama, $0.00/Mtok) is
the first. Flip the gate and the pick moves from the cheap cloud worker to
the cleared local one; where no cleared worker exists for a skill, the
answer is "nobody cleared", never a quiet downgrade.

Confidential work also leaves only a redacted trace: the prevention memory
keeps the statistics (worker, skill, cause) but no task text, no failure
detail, no checklist items — nothing that could ride into a future prompt
sent to a less-cleared worker.

## Iteration: rejected work gets redone, within bounds

A failed checklist is not the end of the task — it is the start of the next
attempt. The runner iterates, up to `--attempts` (default 2, matching the
priced `max_attempts`):

1. the exact failed items are fed back as **corrections** the next attempt
   must fix — not a vague retry, a spec of what was wrong;
2. the work **escalates to the quality option** — a rejection is evidence
   the job was harder than its price — falling back to the next runnable
   worker when the strongest has no runner;
3. every attempt is recorded. Iteration never erases a failure; it answers
   one. The first worker's rejection stays in its ledger and in the
   prevention memory, and the verdict reports who finally earned the accept
   and after how many iterations.

## Prevention: failures that teach the next run

Every rejection is appended to `.owi-quick/root-causes.jsonl` — when, task,
worker, skill, root cause, what failed, and the labelled checklist. Before a
new run of the same skill, the most recent actionable notes (root causes
`worker` and `task_spec` only — no note to a model fixes broken plumbing)
ride along in the prompt as things not to repeat. Verified live: the
changelog task that failed with "asked clarifying questions instead of
drafting" was rerun carrying that note and passed its full checklist.

## When it fails: root cause before blame

A rejection is not automatically the model's fault. When you answer `n`, the
tools ask *why* — and only one answer counts against the worker:

| root cause | meaning | held against the model? |
|---|---|---|
| `worker` (default) | the model's output was wrong | **yes** |
| `task_spec` | the task was ambiguous or wrong | no — fix the manifest |
| `harness` | the plumbing mangled it | no — fix the tooling |
| `environment` | your setup broke it | no — fix the setup |

Excused failures stay in the ledger (reported as `excused_outcome_count`) but
never enter the worker's posterior. When the root cause *is* the model, the
ordinary mechanism does the rest: its estimate drops, it re-ranks, and below
the quality floor it is retired — the model gets changed by evidence, and only
for failures that were actually its own.

## The weights, and how they evolve

The "weights" are Beta posteriors per worker × skill × part. They move on
every recorded run — online, no batch step:

- an accepted run raises the worker's posterior; a worker-caused rejection
  lowers it (excused causes never do);
- outcomes recorded in the same repository part count in full; outcomes from
  other parts transfer at the manifest's declared `cross_repository_weight`
  (0.35 here, 0 for strict isolation, 1 to pool everything);
- vendor claims stay capped and discounted, so lived evidence always
  outweighs marketing.

Rise far enough and a worker takes over a part's bench; fall below the floor
and it is retired from that part — while its record elsewhere stands on its
own evidence.

## The frozen suite: growth without amnesia

The more automated the loop gets, the more one bad change could lose. So the
high-level behaviour that already works is frozen:
`examples/frozen-checks-v1.json` pins exact import micros, the roster's
required workers, the check engine's verdict on fixed fixtures, the
planner's validation rules, and the prevention filter. `tools/owi-selftest`
runs it all deterministically — no model calls, no network — and CI fails if
any expectation drifts.

The file is *evolutionary*: when a lived failure teaches a lesson, it is
frozen in as a new fixture with its provenance, and the version bumps. Old
lessons are superseded, never silently edited away.

## Two implementations, one rule

The ask page is a static file with no server, so it necessarily carries its
own copy of the decision rule in JavaScript — the exact Beta lower bound, the
capability, tool and clearance gates, and the expected-accepted-cost
arithmetic. Two implementations of a money path drift silently unless
something holds them together.

`tools/check_page_math.py` is that something, and it runs in CI. It slices
the marked `decision-math` block out of the built page, runs it in plain node
— no browser, no DOM, no storage — and asserts that for every skill it
returns the same eligible workers, in the same order, at the same micros as
the Rust allocator on a pristine ledger. The keyword classifier is held to
the same standard: the page and `owi-do` must route identical wording to
identical skills. The frozen suite adds a structural guard so the block
cannot quietly stop being extractable.

One divergence is expected and is *not* drift: **the page ranks on public
evidence only.** A published file must not carry your private ledger, so the
page never sees the outcomes your terminal has lived through. Ask both after
a few weeks of real work and the terminal will rank differently — it knows
more. Pristine is therefore the baseline the page is checked against, and
the page says so in its own footer.

## What lives where

| file | what it is |
|---|---|
| `.owi-quick/index.sqlite` | public facts: models, prices, snapshots (rebuildable) |
| `.owi-quick/local.sqlite` | your private ledger: decisions and outcomes (owner-only) |
| `.owi-quick/runners.json` | model → your command; the only place execution is defined |
| `.owi-quick/root-causes.jsonl` | the prevention memory: every rejection, labelled |
| `.owi/project.json` | a repo's work manifest (level 3) |
| `examples/frozen-checks-v1.json` | the frozen suite: behaviour pinned against regression |

## What is real, what is assumed

Real: prices (published, content-hashed import), every decision, every gate,
every recorded outcome, the Beta math end to end. Assumed: the *starting*
ability of commercial models, tagged `vendor_reported` and discounted 10× —
which is exactly what your `y/n` answers and level 4 measurements replace.
