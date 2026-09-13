# Launch benchmark — results pending

The 24 tasks in tasks.json are public synthetic inputs, not model results.
They repeat two templates to test consistency, not broad task coverage. Add
diverse held-out user tasks before making general performance claims.
No savings or model-quality claim is supported until the protocol below runs.
Browser demo priors and software regression tests are not benchmark evidence.

## Reproducible paired comparison

Compare (A) always using one named strong model against (B) OWI routing.
Run the same tasks under both policies. Pin the git commit, exact model IDs,
runner versions, prompts, reasoning settings, token caps, prices and date.
Use a fresh private OWI ledger for each repetition. Fix a task order and use
at least three repetitions; alternate which policy runs first to reduce timing
bias. Hold the reference model and acceptance criteria fixed before execution.
Include every retry, planner, checker and failed attempt in totals. Set equal
maximum attempts and time limits. Do not tune on the evaluation tasks.

The corpus covers only text editing and structured extraction. It does not
support conclusions about coding, CAD, privacy, or other task categories.
Mechanical checks are minimum gates. A reviewer unaware of the selected policy
must also verify factual preservation and usability; a phrase match alone is
not evidence of a good answer. Record reviewer time and acceptance separately.

## Recording

For each task, policy and repetition record:
- task ID, run ID, commit, exact worker and instruction revision;
- accepted after human review, deterministic check results, attempts;
- wall-clock milliseconds and reviewer seconds;
- reported input/output/cache counters;
- API-equivalent estimate and measured/reported charge separately;
- billing mode and whether a helper or fallback was used.

Missing token counts and charges must be null, never zero. Subscription runs
must not be presented as cash savings against API invoices; report them in a
separate allowance cohort. Use tools/owi-maintain usage to inspect telemetry.

Report acceptance rate with counts, total attempts, median and p95 wall time,
reviewer time and total measured cash per accepted task (all attempts in the
numerator). If any charge is unknown, mark that cohort's total unknown and
report coverage. Publish raw redacted records and paired task differences.
Do not publish only successful tasks or average only accepted attempts.

## Current status

Provider comparison: **not run**. It requires working authorized model commands
and a spending limit. The task corpus and software checks can be inspected
without spending credits. Existing corpus-based code benchmarking is documented
in ../../docs/BENCHMARKING.md (see the repository's docs folder).
