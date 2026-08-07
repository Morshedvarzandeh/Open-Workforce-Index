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
tools/owi-serve        # open http://127.0.0.1:7787
```

The same one-box page, served locally: it detects the server, shows
"local — runs for real", and grows a run button. Output streams into the
page; `worked` / `didn't` goes to the real ledger and every number refreshes
from the engine immediately. Localhost only; the browser can only *name* a
model — the command executed is resolved server-side from `runners.json`.

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

Attach acceptance criteria with `--check` (or the checklist box on the ask
page) and three things happen:

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

## Splitting: one request, several kinds of work

`--split` sends the request to the cheapest runnable model acting as a
planner. It answers one question — does this hold different kinds of work? —
and returns standalone part summaries with a small checklist each. The
planner **never names models**; staffing stays with the engine, part by
part, so a text part can go to the cheap writer while the extraction part
goes to whoever the evidence favours. The summary table at the end names who
actually ran each part, its price, and the checked verdict.

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
