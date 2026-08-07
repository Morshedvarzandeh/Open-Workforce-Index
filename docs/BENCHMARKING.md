# Producing ability evidence

Prices are published; ability is not. No benchmark measures *your* exact
worker configuration — this model, this harness, this prompt, this toolset —
on the work you actually do. That evidence has to be produced locally, and
this is how.

The first corpus comes from
[`battery-core`](https://github.com/Morshedvarzandeh/battery-core), a Python
library with 198 tests that run in under half a second.

## The task class

One task is: *this documented function has had its body removed; restore an
implementation that makes the test suite pass.*

The signature and docstring stay. The task therefore measures implementation
ability against a stated contract, not the ability to guess an unstated one.
Acceptance is the test suite's exit status — deterministic, free to check, and
not something the model under test gets a vote on.

## Extracting a corpus

```bash
python3 tools/extract_bench_tasks.py \
  --repo /path/to/battery-core \
  --source-glob "src/battery_core/*.py" \
  --verify-command ".venv/bin/python -m pytest -q" \
  --skill-id "skill:python-numerical-implementation" \
  --corpus-id "corpus:battery-core-v1" \
  --repo-url "https://github.com/Morshedvarzandeh/battery-core" \
  --commit "$(git -C /path/to/battery-core rev-parse HEAD)" \
  --output corpus.json
```

Every candidate is validated twice before it is allowed into the corpus:

1. With the body stubbed, the suite must **fail**. A task whose tests still
   pass measures nothing — the function is not covered, and scoring a model
   against it would record noise as ability.
2. With the body restored, the suite must **pass**. This proves the failure in
   step 1 came from the removed body rather than a pre-existing breakage.

Anything failing either check is reported and discarded. On `battery-core` all
11 documented functions passed both checks — its coverage is genuinely tight.

## Calibrating the harness before spending anything

Two built-in adapters bound the measurement:

```bash
python3 tools/run_bench.py --corpus corpus.json --repo /path/to/battery-core \
  --worker-id worker:bench-oracle --adapter oracle --observed-at "$(date -u +%FT%TZ)"

python3 tools/run_bench.py --corpus corpus.json --repo /path/to/battery-core \
  --worker-id worker:bench-stub --adapter stub --observed-at "$(date -u +%FT%TZ)"
```

`oracle` restores the repository's own code and must score 1.0. `stub` leaves
the body unimplemented and must score 0.0. Observed on `battery-core`:

```text
worker:bench-oracle via oracle: 11/11 accepted (100.0%)
worker:bench-stub   via stub:    0/11 accepted (0.0%)
```

If the oracle does not score 1.0 the harness is broken, not the model. If the
stub scores above 0.0 the corpus contains a task that measures nothing. Run
both before trusting a single paid result — a miscalibrated instrument
produces confident numbers about nothing.

## Running a real model

The runner holds no credentials. The `command` adapter shells out to a program
you supply, hands it the task as JSON on stdin, and reads the function body
from stdout:

```bash
python3 tools/run_bench.py --corpus corpus.json --repo /path/to/battery-core \
  --worker-id worker:your-model-agent \
  --adapter command --command "./my-model-adapter.sh" \
  --observed-at "$(date -u +%FT%TZ)" \
  --outcome-dir outcomes/ --report report.json
```

Any CLI that can reach a model works, and your key stays wherever you already
keep it. If the program writes a final line of JSON to stderr containing
`input_tokens`, `output_tokens`, or `cash_micros`, those are recorded; without
them the outcome is still valid, just less informative about cost.

## Feeding results back

```bash
for outcome in outcomes/*.json; do
  owi outcome --local .data/local.sqlite --input "$outcome"
done
owi allocate --index .data/index.sqlite --local .data/local.sqlite --input allocation.json
```

Ingesting the two calibration runs produces exactly the posteriors they should:

```text
worker                        posterior    mean  lcb(95%)    n
bench-oracle           Beta( 12.0,  1.0)   0.923     0.779   11
bench-stub             Beta(  1.0, 12.0)   0.077     0.004   11
```

Note that eleven straight successes yield a mean of 0.923, not 1.0, and a 95%
lower bound of 0.779. That shrinkage is the point: eleven observations do not
justify claiming certainty, and the bound the allocator gates on stays well
below the observed rate until far more evidence accumulates.

## What this does and does not establish

It establishes a *measured* pass rate for one worker configuration, on one
skill, on one repository's task class, scored deterministically.

It does not establish general coding ability, and OWI will not let that
evidence route a task it did not measure. Eleven tasks from one library is a
starting corpus, not a benchmark — the honest next steps are more repositories,
more task classes, and several attempts per task so the estimate reflects
run-to-run variance rather than a single sample.
