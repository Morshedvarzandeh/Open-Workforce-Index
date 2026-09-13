# OWI early-user launch kit

Goal for the first two weeks: 10 testers, five completed real tasks, three
voluntary returns within seven days. These are learning targets, not forecasts.

## Assets

- [Interactive example](../../demo/index.html): download the HTML and open it
  in a browser. No installation, account, or API key. Sample prices and assumed
  abilities are prominently labelled. It does not execute models.
- [53-second walkthrough](demo.mp4): recorded browser example, with captions.
- [Benchmark protocol and 24 tasks](../../benchmarks/launch/README.md).
- [Ready-to-edit announcement](announcement.md).
- [Tester feedback](https://github.com/Morshedvarzandeh/Open-Workforce-Index/issues/new?template=first-task.yml).

## Publish the demo URL

GitHub Pages is not enabled for this repository. In repository **Settings →
Pages**, choose **Deploy from a branch**, branch **main**, folder **/docs**.
The committed docs/index.html redirects to the self-contained docs/demo.html.
After deployment succeeds, verify the page at
https://morshedvarzandeh.github.io/Open-Workforce-Index/ before advertising it.
Set that verified URL in the repository About → Website field.
Suggested topics: ai, llm, model-routing, cost-optimization, local-first.
The current connector cannot change Pages settings or repository metadata.

## First two weeks

1. Day 1–2: verify the published URL on phone and desktop; watch three people
   try the example without guidance. Fix their first blocking problem.
2. Day 3–5: invite 10 developers who already use multiple models, individually.
   Ask for one real task and permission to follow up after seven days.
3. Day 6–10: run the benchmark protocol with authorized provider access.
   Publish failures and unknown charges alongside successes. Keep raw private
   tasks, credentials, and personal details out of this public repository.
4. Day 11–14: share the video, usable demo, limitations, and any measured
   results on LinkedIn and communities that allow project posts. Consider
   Show HN after the public demo works. Answer feedback yourself.

Do not solicit votes, mass-message people, or post the same announcement in
unrelated discussions. Outreach and social publication remain manual.

## Measure adoption without collecting prompts

Record aggregate weekly counts privately: landing visits if hosting provides
analytics, consenting testers invited, first tasks completed, installation
failures, and seven-day returning testers. Ask which alternative each person
would otherwise use. Stars and downloads are separate from active use.

Do not add invisible browser telemetry. This launch adds no remote analytics.
Ask testers to self-report success and return usage; explain that GitHub issue
reports are public. Never publish identities or task content without permission.

Decision: low visits → improve discovery; visits without first tasks → fix
onboarding; completed tasks without returns → improve the recurring use case.
Avoid paid advertising until at least some testers return voluntarily.
