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

## What lives where

| file | what it is |
|---|---|
| `.owi-quick/index.sqlite` | public facts: models, prices, snapshots (rebuildable) |
| `.owi-quick/local.sqlite` | your private ledger: decisions and outcomes (owner-only) |
| `.owi-quick/runners.json` | model → your command; the only place execution is defined |
| `.owi/project.json` | a repo's work manifest (level 3) |

## What is real, what is assumed

Real: prices (published, content-hashed import), every decision, every gate,
every recorded outcome, the Beta math end to end. Assumed: the *starting*
ability of commercial models, tagged `vendor_reported` and discounted 10× —
which is exactly what your `y/n` answers and level 4 measurements replace.
