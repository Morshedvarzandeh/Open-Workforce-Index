# Office launch review

Reviewed `main` at `3968278` and the newer graphical Office in PR #26 at
`17ed186`. These fixes build on PR #26. That Office is not yet merged into
`main`, so changing only the default branch's old interface would miss the
latest product work.

## Functional issues fixed

| Finding | Result of this change |
| --- | --- |
| First launch disabled the entire project form until a workforce was configured. | Users can create, edit and persist drafts with Python alone. Model planning, staffing, running and review still require configuration. |
| Only the most recently created project was accessible through the UI/API. | Saved-project list, project-specific GET and selected-project URLs reopen previous work. |
| Runtime configuration was cached until server restart. | Explicit reload rereads the same workspace and reports engine, catalog and runner-binding status. It does not seed sample data or invoke workers. |
| A run held the same lock used to load the page and runtime data. | Page/status reads remain available while a worker runs. |
| A second run request could wait on that lock and run the same task again after the first completed. | Concurrent run requests receive 409 instead of queuing another paid invocation. |
| A failed create request left the submit button disabled. | Clearing the busy state restores form controls so the user can correct and retry. |
| Opening the local URL without its token showed raw JSON. | A readable page explains how to reopen the session; APIs remain token-gated. |
| Static-preview instructions described an older interactive workflow. | Documentation now separates preview, local drafting and configured execution. |

## Verification

- Four new HTTP regression tests exercise temporary workspaces, draft CRUD,
  project history after reopening, blocked model calls, setup reload, access
  gates, and a deliberately slow test worker with a duplicate request.
- Existing launcher, persisted workflow, GitHub Manager, GUI structure and
  JavaScript syntax, and training-tour checks pass locally.
- The frozen Python suite passes 57/57 checks.
- The slow worker and allocator in the new tests are deterministic test
  doubles. These tests are not evidence of live provider authentication,
  model quality or metered costs.
- Browser visual verification was blocked by the preview environment
  (`ERR_BLOCKED_BY_CLIENT` on local addresses). Desktop/mobile visual QA
  remains outstanding; HTML and JavaScript contract checks do not replace it.
- Cargo is unavailable in the editing environment. The existing Rust,
  minimum-supported-version and engine integration jobs remain required in CI.

## Remaining release blockers

1. The existing bitmap rights metadata is pending. The maintainer must complete
   the review required by `ui/assets/office/v2/LICENSE.md` and pass its release
   validator before distributing the graphical release. This change does not
   alter or claim approval for that metadata.
2. Complete desktop/mobile visual QA and obtain green CI for the final commit.
3. Run a real, explicitly chosen worker through staffing, execution and owner
   review in the intended deployment environment. Credentials, availability
   and actual spending were not verified here. Sample abilities and historical
   prices must stay labelled as such.
4. Public multi-user hosting is not implemented by this local HTTP server.
   Authentication, per-user isolation and production hosting need their own
   implementation before offering a public service. Keep the default loopback
   bind for the local product.

The changes improve the usable local workflow. They do not establish that a
public hosted launch or a paid production service is ready.
