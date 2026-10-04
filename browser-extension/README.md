# OWI project map browser client

This directory contains the project-neutral, full-page browser client for an
`owi.system-map.v1` snapshot. It has no page access and cannot read a
repository. The background worker is an untrusted Native Messaging client of
the local OWI host; that host remains the policy and repository boundary.

The checked-in `manifest.json` is the Chromium Manifest V3 package manifest.
For Firefox packaging, copy the directory to a staging directory and replace
`manifest.json` with `manifest.firefox.json`. Both packages use the same HTML,
CSS, JavaScript, and Native Messaging protocol.

The native host name is `org.openworkforceindex.project_map`. Its request
envelopes are deliberately small:

- `list_projects`
- `get_snapshot` with an opaque `projectId`
- `invoke_action` with opaque project/action IDs, an expected snapshot and
  state, and an idempotency key

No command, path, prompt, credential, provider request, or arbitrary argument
can be sent through the client protocol. Every displayed management action is
confirmed by the person before it is invoked. The host revalidates all fields
and is authoritative. Native-host registration allowlists the exact installed
extension identity; that owner-installed extension is the confirmation surface,
not an arbitrary web page or repository declaration.

Run the dependency-free checks with:

```sh
npm test
```
