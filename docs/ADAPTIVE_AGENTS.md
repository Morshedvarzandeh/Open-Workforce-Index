# Agents that update their working instructions

Connected agents can improve their working reminders automatically after
checked runs. This first implementation changes bounded instructions and
retains the existing outcome-based model selection. It does not retrain
model weights, install new tools, download code, or update provider prices.

The control is under **Help when you need it → My plan & self-updating
agents → Automatically improve working instructions**. It is enabled by
default. Browser-only comparisons do not run this update loop.

## The update cycle

1. A model starts with instruction revision **v0**.
2. Two distinct executed runs fail the same deterministic check category.
   OWI proposes a reminder from a reviewed, fixed catalog: JSON validity,
   required phrases, output patterns, minimum word count, or Python syntax.
3. The revision's schema, catalog digest, and exact instruction text are
   validated before it enters **probation**.
4. Later tasks use reminders only when their current checklist contains
   the matching category. The explicit task and its checks stay authoritative.
   Runs during probation receive full inspection instead of sampled checks.
5. Three consecutive fully checked passes on applicable tasks promote the
   revision to **stable**. Two failed tasks during probation restore the
   previous stable revision. Two repeated regressions on an applied category
   also roll back a promoted revision.

Rolled-back additions are blocked from being proposed again automatically.
This prevents an endless retry loop with the same unsuccessful instruction.
Other check categories can still produce later revisions.

Probation is a bounded online trial. Three passes do not establish general
quality improvement or statistical significance. The existing independent
result checks and quality gates continue to apply.

## What can become evidence

Only results from an executed maker command and server/CLI-computed
deterministic checks propose reminders. Model-generated evaluations,
free-form feedback, unknown results, sampled-out checks, plumbing failures,
and planner/checker runs cannot create instruction updates.

Confidential and secret task outcomes are excluded from instruction learning.
The instruction catalog contains no user task text or provider output.
Existing confidential outcome details are also redacted before persistence.
Run IDs make observations idempotent, and each observation is tied to the
revision actually used. Concurrent or late results cannot promote a different
revision.

There is no extra paid training call or background model loop: learning
runs after the checks of a task the user already started. Learned reminders
add a small amount of prompt context to later applicable tasks. Billing
settings, credentials, privacy gates, runner commands, and verification rules
cannot be modified by this process.

## History, pause, and rollback

The GUI shows each model's active version and status. **Roll back** restores
its parent version. The private `runtime.sqlite` database retains instruction
snapshots, their catalog digests, run/revision associations, and transition
events.

```bash
tools/owi-maintain status
tools/owi-maintain pause
tools/owi-maintain resume
tools/owi-maintain rollback --model haiku-4-5
```

Pause stops instruction changes and the application of learned reminders.
Ordinary quality feedback and its ledger continue to work. Resume does not
change the account's allowance timestamps or billing mode.

Runtime tests exercise promotion, automatic and manual rollback, duplicate
observations, concurrent feedback, confidential exclusions, pause/resume,
and altered instruction rejection. Local fake runners test the workflow
without spending provider credits. Real-model quality benchmarking remains
separate from these software correctness checks.
