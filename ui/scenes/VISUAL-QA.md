# Seven-iteration visual QA gate

Run the passes in order. A pass is not complete until its evidence is kept as
desktop, phone portrait, and phone landscape screenshots. Later passes may
change polish, but must not reopen a previously closed layout or truth defect.

## 1. Information hierarchy

- One primary action is obvious in every view.
- Owner, project, task, worker, result, and evidence hierarchy can be explained
  from a five-second screenshot.
- No horizontal scrolling at 320, 375, 390, 430, 768, 1024, or 1440 CSS pixels.
- The first useful action remains visible without decorative art hiding it.

## 2. Scene composition

- Background, environment, rear props, characters, front props, effects, HUD,
  and tutorial overlays remain in their declared z-index bands.
- Characters have a believable floor/desk relationship and do not float,
  intersect furniture, or cover factual HTML.
- The training route and all four stations remain legible at every profile.
- Phone portrait uses a contained 16:9 scene rather than an unreadable crop.

## 3. Workflow truth

- Draft, staffed, running, waiting for review, accepted, needs work, blocked,
  and offline states have a text label and non-color-only cue.
- Exact worker ID, capability, runner readiness, and cost forecast stay visible
  beside any friendly character presentation.
- No animation implies success before the server returns it.
- Forecast, measured, and unknown values are visually and verbally distinct.

## 4. Complete training tour

- All seven steps resolve their target or fallback anchor in the correct view.
- Back, Next, Skip, Restart, Escape, focus restoration, and saved resume state
  work with touch and keyboard.
- The spotlight follows resize, orientation, scrolling, and dynamic task cards.
- The tour never invokes model planning, staffing, execution, review, or PR
  creation; only a deliberate application action can do those things.

## 5. Mobile ergonomics

- Every interactive target is at least 44 by 44 CSS pixels; primary actions are
  at least 48 pixels high.
- iPhone/Android safe areas, `100dvh`, software keyboard movement, and
  `visualViewport` changes do not hide the coach or primary action.
- The phone roster is one readable active worker or a snap carousel, not three
  miniature identities.
- The 844 by 390 landscape profile has no trapped scroll or off-screen dialog.

## 6. Accessibility and performance

- Text and controls meet WCAG AA contrast; focus order and labels match the
  visual order; status announcements are useful and not noisy.
- Reduced motion removes ambient movement, animated scrolling, spotlight
  pulsing, and transition travel without removing information.
- Critical runtime art stays below the manifest's 750 KiB budget, total runtime
  art stays below 2 MiB, local LCP is below 2.5 seconds, CLS below 0.05, and
  motion remains 55–60 FPS on a mid-range phone.

## 7. Polish and regression

- Complete create → inspect → staff → run → review → results on phone, tablet,
  and desktop with no dead control or hidden output.
- Static/read-only mode never pretends that a model ran or evidence exists.
- Golden screenshots show no overlap, clipping, accidental text in bitmaps,
  inconsistent radius/shadow language, or unintentional skin mismatch.
- Development validation passes, release validation remains closed while art
  rights are pending, and all existing workflow/security tests remain green.
