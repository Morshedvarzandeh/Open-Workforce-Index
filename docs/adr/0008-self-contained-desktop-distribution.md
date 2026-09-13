# ADR 0008: Self-contained desktop distribution

Status: accepted

## Problem

The MCP integration removed per-task UI work but still asked users to install
Rust and Python. That makes an add-on feel like another development project.

## Decision

Build native archives containing the Rust engine, frozen Python runtime,
resources, certificates and a small Tk setup screen. The executable dispatches
`setup`, `connect`, `mcp`, and `openrouter`. Normal MCP startup has no GUI and
never builds or downloads a runtime. Source development remains supported.

Use one-folder packaging, preserve symbolic links, and install a versioned copy
under the user's data directory before creating MCP connections. Keep mutable
history outside application resources. No administrator access is needed.

API credentials use only explicit OS-protected keyring backends. No plaintext
fallback or provider key in MCP JSON. Environment keys remain supported.
External worker processes receive the original system library search path,
not frozen Python's bundled library path.

## Verification and limits

Build on Windows x64, macOS Intel/ARM and Linux x64. Gate publication on testing
the extracted downloads with no Python/Rust on PATH, real engine allocation
and feedback, configuration preservation, and setup-window startup. Source
tests protect credential failure behavior, paths and managed copies.

This is a portable desktop preview, not a signed native installer or a
Marketplace extension. Host tool selection and account permissions still
govern work. Keychain availability, live IDE sessions and provider accounts
cannot be proved by simulated CI calls. Existing conflicting OWI connections
require explicit removal before reconnecting; runtime upgrades are not silent.

Sources: [PyInstaller runtime paths](https://pyinstaller.org/en/stable/runtime-information.html),
[subprocess and archive boundaries](https://pyinstaller.org/en/stable/common-issues-and-pitfalls.html),
[keyring backends](https://pypi.org/project/keyring/).
