# OWI guided tours

Tours are versioned, declarative presentation content. They point to stable
`data-tour-id` values rendered by the application and to a registered
coach asset. They do not call APIs, complete work, approve a budget, accept an
outcome, or create a pull request.

`first-project.v1.json` contains exactly seven steps for features that exist in
the current OWI Office: company overview, roster, project creation, task-plan
inspection, staffing, run/review, and results evidence. It intentionally makes
no GitHub connection or Manager HQ claims before those backend capabilities
exist.

A runtime should:

1. change to the declared `view` without performing the highlighted action;
2. resolve `[data-tour-id="target_anchor"]`, then the fallback anchor;
3. scroll the target into view and recompute its rectangle after resize,
   orientation, `visualViewport`, or content changes;
4. draw a non-interactive spotlight while keeping the application inert, so a
   training step can never accidentally trigger staffing, execution, or spend;
5. present an anchored coach card on larger screens and a safe-area-aware
   bottom sheet on phones;
6. trap focus inside the coach controls, restore focus on exit, and provide
   Back, Skip, Next, Restart, and Escape behavior;
7. persist only `{version, status, step}` under `owi-office-tour-v1`, so an
   older saved tour cannot silently corrupt a newer contract and private work
   never enters browser preferences.

With `prefers-reduced-motion: reduce`, scrolling is instant and the spotlight,
coach, and progress indicator do not animate. Tour progress is a calm checklist,
not a streak, currency, loot box, timer, or artificial urgency mechanic.
