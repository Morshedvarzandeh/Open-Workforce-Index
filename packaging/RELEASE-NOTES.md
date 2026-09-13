Download OWI for your computer, extract it, and open its setup screen.
Choose your AI app, select your project, and connect. Rust, Python, Git and
developer package managers are included or unnecessary for users.

- Windows: `OWI-windows-x64.zip` → `OWI.exe`.
- Apple Silicon Mac: `OWI-macos-arm64.tar.gz` → `Open OWI.command`.
- Intel Mac: `OWI-macos-x64.tar.gz` → `Open OWI.command`.
- Linux: `OWI-linux-x64.tar.gz` → `Open OWI.sh` or `./OWI`.

Setup keeps a stable installed copy, preserves unrelated MCP settings, and
uses your existing Claude Code login or OpenRouter account. OpenRouter keys
are kept in the system credential store. Provider billing remains separate.

These are unsigned preview downloads; OS opening approval may be required.
See `READ-ME.txt` inside each archive for requirements and disconnect steps.
This release was gated on native package extraction, engine/MCP execution and
setup-window checks on all four platforms, without paid provider calls.
Live IDE sessions and OS credential prompts still need user-machine testing.

The accompanying SHA-256 files verify archive integrity. The source archive,
build scripts, dependency lockfile and included notices document the matching
AGPL-3.0-only source and separately licensed bundled components.
