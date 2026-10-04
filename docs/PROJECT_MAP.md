# Generic project map source preview

The OWI project map is an early, project-neutral view of a local repository's
components, APIs, agents, relationships, working-tree changes and local OWI
runtime evidence. It opens as a full browser tab with two views:

- **Project map** shows declared components and directed relationships. A
  component with an owned changed file shows its direct line churn; downstream
  components show an impact path without inheriting those line counts.
- **OWI activity & management** shows evidence grouped by its measurement basis,
  local agents and the small set of trusted management actions described below.

This is a **source preview**, not a production browser extension. It is not in
the Chrome Web Store, Firefox Add-ons or the packaged OWI desktop download.
Browser loading and local-host registration are still developer steps.

The design is intentionally independent of any one repository or industry. The
repository supplies descriptive data; OWI supplies observation and authority:

```text
.owi/map.json + Git working tree
             | local, read-only observation
             v
       owi.system-map.v1 + local OWI runtime evidence
             | bounded Native Messaging
             v
       full-page browser client

browser confirmation -> opaque action ID -> trusted OWI adapter -> audit receipt
```

The design decision and acceptance gates are recorded in
[ADR 0010](adr/0010-generic-project-map-client.md).

## Describe any project

Add `.owi/map.json` to the repository you want to inspect. This file is separate
from `.owi/project.json`: the map describes architecture and ownership, while
the existing project file describes work staffing. See the complete
[repository-neutral example](../examples/project-map.json).

The declaration uses the strict `owi.project-map.v1` schema:

| Field | Meaning |
|---|---|
| `project` | Stable ID, display name and optional description |
| `lanes` | Ordered visual groups such as Experience, Services or Assurance |
| `components` | Project parts and the repository-relative files or directory prefixes they own |
| `relations` | Directed `from` -> `to` edges used to calculate downstream impact |

A minimal declaration is:

```json
{
  "schemaVersion": "owi.project-map.v1",
  "project": { "id": "project-example", "name": "Example" },
  "lanes": [
    { "id": "lane-app", "name": "Application", "order": 0 }
  ],
  "components": [
    {
      "id": "component-api",
      "name": "Public API",
      "kind": "api",
      "laneId": "lane-app",
      "lifecycle": "implemented",
      "owns": ["src/api/"],
      "facts": ["Accepts versioned requests."]
    }
  ],
  "relations": []
}
```

Component kinds are `surface`, `api`, `router`, `agent`, `runtime`, `trigger`,
`ledger`, `human` and `module`. Lifecycle is `implemented`, `declared` or
`planned`. An ownership entry without a trailing slash is one exact file; an
entry ending in `/` owns that directory prefix. Paths must be normalized,
repository-relative paths without `..`, globs, backslashes or `.git` access.
Duplicate IDs, unknown fields, dangling relationships and unsafe paths are
rejected rather than guessed.

The declaration has no structured command, action, credential, provider-request
or absolute-path fields. Display strings such as names, descriptions and facts
are ordinary repository content and are shown verbatim; they are not a secret
scanner. In particular, committing a map file does not grant the repository
permission to run anything.

### Automatic discovery

If `.owi/map.json` is absent, the core produces a deterministic starter map. It
creates one Repository lane, a component for each top-level directory and a
Root files component when needed. Discovery is useful for opening an unfamiliar
repository, but it does not infer APIs, agents or meaningful dependency edges.
Add a declaration when those distinctions matter.

## Generate and inspect a snapshot

From an OWI source checkout, run:

```sh
python3 tools/owi_project_map.py /absolute/path/to/project > /tmp/project-map.json
```

Use `--compact` for compact JSON. `--generated-at` accepts a trusted ISO-8601
observation time; the standalone command otherwise leaves the generation time
unknown so it does not invent one.

The output is a content-addressed `owi.system-map.v1` snapshot. Its local Git
adapter compares the working tree with `HEAD` (or the empty tree before a first
commit) and uses Git-style line counts:

- text files have integer additions and deletions;
- binary files and files that cannot be measured have `null` line counts;
- only components that directly own changed files receive churn totals; and
- a component reached through directed relationships is `affected`, with the
  path that reached it, but has no inherited line churn.

Changed files with no declared owner remain visible as `unmappedFiles`. When a
comparison cannot be made, impact is `unavailable`, not silently unchanged.

Repository observation is bounded before data is materialized in the host.
Version 1 caps Git output at 512 KB, Git input at 256 KB, discovery at 4,096
files/192 KB of paths, and working-tree evidence at 768 changed files/96 KB of
paths. Git commands also have a 15-second deadline. Crossing a limit produces
an explicit `unavailable` comparison reason and an empty file list (or makes
automatic discovery fail closed); it never returns a partial count as complete.

## Configure the trusted local host

Run the following from the OWI checkout. Choose a short, opaque local ID; it is
what the browser sees in its project selector. The path must be absolute:

```sh
tools/owi-browser-host \
  --configure-project my-project=/absolute/path/to/project
tools/owi-browser-host --list-configured-projects
```

Repeat `--configure-project` to add another project or update an existing ID.
Remove one with:

```sh
tools/owi-browser-host --remove-project my-project
```

In a source checkout the durable `owi.browser-projects.v1` registry is stored at
`.owi-quick/browser-projects.json`. OWI creates it with owner-only permissions.
It contains the trusted absolute roots used by the host, but the host returns
only the opaque IDs to the browser. Do not commit this file.

The host speaks length-prefixed Native Messaging on standard input and output.
Running it directly in a terminal therefore waits for protocol frames; it is
normally launched by the browser after registration. The browser messages name
only opaque project, snapshot and action IDs. They never supply a repository
path or command. Chromium and Firefox append launch metadata when starting a
native host; OWI recognizes and ignores only their expected origin/parent-window
or manifest-path/extension-ID shapes. No setup action is needed for those
arguments, and arbitrary launch arguments do not become project configuration.

## Load the browser client

Loading the files shows the extension UI, but the map can connect only after
the native host `org.openworkforceindex.project_map` is registered for that
extension ID. The source helper below installs a per-user registration on Linux
or macOS; Windows registration is not implemented in this preview.

### Chromium, Chrome or Edge

1. Open `chrome://extensions` in Chrome/Chromium, or `edge://extensions` in
   Edge.
2. Enable **Developer mode**.
3. Choose **Load unpacked** and select this checkout's `browser-extension/`
   directory. The checked-in `manifest.json` is the Chromium Manifest V3 file.
4. Copy the 32-character extension ID shown on the extension card. From the OWI
   checkout, register the executable for the browser you loaded it into:

   ```sh
   tools/owi-browser-host --install-native-host chrome \
     --extension-id abcdefghijklmnopabcdefghijklmnop
   ```

   Replace the example ID with the displayed value. Use `chromium` or `edge`
   instead of `chrome` for those browsers.
5. Select **Open OWI project map** from the browser toolbar. OWI opens one full
   tab rather than injecting content into the current page; later clicks focus
   that same tab instead of opening duplicates.

### Firefox 140 or newer

Stage a copy so Firefox sees its manifest as `manifest.json` without modifying
the Chromium source tree:

```sh
owi_firefox_staging=$(mktemp -d)
cp -R browser-extension/. "$owi_firefox_staging/"
cp browser-extension/manifest.firefox.json \
  "$owi_firefox_staging/manifest.json"
printf '%s\n' "$owi_firefox_staging"
```

Then open `about:debugging`, choose **This Firefox**, select **Load Temporary
Add-on**, and open `manifest.json` inside the printed temporary directory.
Temporary extensions disappear when Firefox closes.

Register the fixed Firefox extension ID from the OWI checkout:

```sh
tools/owi-browser-host --install-native-host firefox
```

Select the extension's toolbar action to open the project map.

The two packages share the same HTML, CSS, JavaScript and protocol. Their base
permissions are only `storage` and `nativeMessaging`; neither manifest requests
page, cookie, history, clipboard, content-script or broad host access.

## Current management boundary

The browser displays only actions returned by trusted local OWI adapters. The
initial allowlist is deliberately narrow:

- pause or resume local instruction learning; and
- roll an OWI agent back to its preceding available revision.

Every action requires a review click and explicit confirmation. The request
includes the expected snapshot and expected state, so the host rejects stale
pages. An idempotency key prevents an ordinary retry from applying the same
request twice, and the host records a local receipt in its private runtime
directory. The browser cannot add arguments, provide a command, choose a
provider run or expand an action's authority.

If the browser loses the connection after dispatch, it reports that the outcome
needs review rather than claiming failure. The review panel can resend the exact
same idempotent request, or you can refresh and inspect the current state first;
it never creates a fresh action key for that retry. Only an explicit host
rejection is shown as not applied.

## Cost evidence is not one number

The activity view keeps three USD `currency-micros` values separate. One
currency micro is one millionth of a dollar.

| Basis | What it means |
|---|---|
| `runner-reported` | Charge reported by the runner for recent local OWI runs |
| `api-equivalent` | API-equivalent amount supplied or derived for those runs; an estimate, not an invoice |
| `reconciled-invoice` | Charge reconciled to provider billing evidence |

The current adapter aggregates up to the 20 most recent operations in the
selected local OWI runtime. It does not yet allocate those amounts to one mapped
repository. It sums the non-negative amounts that were actually reported, so a
total can be partial when other runs have unknown usage. Invoice reconciliation
is not implemented by the current runtime, so `reconciled-invoice` is normally
unavailable. A missing value is `null` and renders as **Unknown**; it is never
changed to zero or combined with another basis. When a total is present, the
activity row also shows its reported/observed item coverage (for example,
`USD 0.125 · 2/5 reported`).

## Privacy and security properties

- The map and runtime database stay local. The browser is an untrusted renderer;
  repository reads and runtime writes remain in the local host.
- Client snapshots contain normalized repository-relative source references and
  have no structured fields for absolute roots, remotes, credentials, secrets,
  prompts, model output, commands or provider requests. Repository-supplied
  names, descriptions, facts and relative filenames are forwarded as display
  text and are not scanned for sensitive words or embedded paths.
- Both declaration and snapshot schemas are versioned and bounded. Unknown
  declaration fields, unknown top-level snapshot/action fields and unsupported
  schema versions fail closed; display-only nested snapshot fields that are not
  part of the client model are ignored and cannot add authority.
- Native messages are size-limited and accept exact request fields. The host
  revalidates confirmation, action authority and stale-state preconditions.
- Native-host registration allowlists the exact owner-installed extension ID;
  that extension is the confirmation surface. Repository content and ordinary
  web pages cannot submit management actions.
- The feature does not contact a provider merely to draw or refresh the map.

Treat map descriptions, component facts and repository-relative filenames as
project information: they are displayed to anyone who can open the installed
extension and reach that local host.

## Source-preview limitations

This preview does **not** yet provide:

- Chrome Web Store, Firefox Add-ons or Safari packaging/signing;
- inclusion in the downloadable OWI desktop bundle;
- Windows native-host registration or a one-step end-user installer;
- provider/model execution, arbitrary repository commands or Git mutations;
- schedule inventory, schedule creation or schedule execution;
- project-attributed invoice reconciliation or billing controls;
- remote/shared dashboards, page scraping or automatic discovery of semantic
  API and agent relationships; or
- a claim that the packaging acceptance gate in ADR 0010 has passed.

Run the dependency-free checks while developing:

```sh
python3 tools/test_project_map.py
python3 tools/test_browser_host.py
(cd browser-extension && npm test)
```

These checks do not contact an AI provider or spend provider credits.
