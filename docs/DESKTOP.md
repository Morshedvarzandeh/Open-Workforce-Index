# Optional desktop installation

For browser tools and Telegram, use the [hosted connection](HOSTED.md).
End users of that service install nothing. This guide is the optional local
alternative for people who want OWI running on their own computer.

OWI desktop downloads include the compiled engine, Python runtime and setup
screen. You do **not** need Git, Rust, Python, pip or Cargo on your computer.

1. Open [Downloads](https://github.com/Morshedvarzandeh/Open-Workforce-Index/releases).
   Choose the newest **OWI desktop preview** and expand **Assets**.
2. Download the archive for your computer and extract the whole archive.
3. Open OWI, choose your AI app and project folder, then click **Connect**.
4. In your AI app, enable OWI's tools. Ask it to check OWI status, then try a
   small task such as writing a delivery confirmation containing “Friday”.

| Computer | Download | Open after extraction |
|---|---|---|
| Windows 10/11, Intel or AMD 64-bit | `OWI-windows-x64.zip` | `OWI.exe` |
| macOS 15+, Apple Silicon | `OWI-macos-arm64.tar.gz` | `Open OWI.command` |
| macOS 15+, Intel | `OWI-macos-x64.tar.gz` | `Open OWI.command` |
| Ubuntu 22.04+ or compatible Linux, 64-bit Intel/AMD | `OWI-linux-x64.tar.gz` | `Open OWI.sh` (Run as Program), or `./OWI` |

These initial downloads are unsigned previews. Windows or macOS may ask you
to approve opening the downloaded application. Signing/notarization and a
Marketplace extension are not included yet. SHA-256 files accompany every
archive. Windows ARM and Linux ARM packages are not included in this preview.

## Your AI account

Choose **Existing Claude Code login** if you already use Claude Code on that
computer. Its existing login, account limits and billing apply. The presence
of the CLI is checked; setup does not spend tokens testing its login.

Or choose **OpenRouter**, enter your existing API key once, and connect.
OWI saves it in Windows Credential Manager, macOS Keychain, or the Linux
desktop's Secret Service keyring. No key is written into project settings or
OWI files. Linux needs a working, unlocked desktop keyring for this option;
there is no plaintext fallback. Existing `OPENROUTER_API_KEY` environment
configuration remains supported and takes precedence over a saved key.

OpenRouter charges separately. Copilot or Claude subscription access is not
converted to API credits. The setup screen fetches public model metadata but
does not run a paid task. The initial OpenRouter workers handle writing,
extraction and planning; they do not modify your repository directly.

## After connecting

Close the setup window and keep working in your AI app. It starts OWI when it
uses the connection; you do not need to open OWI for every task. The host
chooses when to delegate and controls tool permissions. You can explicitly ask
it to use OWI for your first test. Inline autocomplete is unchanged.

Setup copies the entire bundle to a stable folder in your user account before
connecting. You can delete the original extracted download afterward. Model
history is stored separately from the installed app version:

| System | OWI data and installed versions |
|---|---|
| Windows | `%LOCALAPPDATA%\OWI` |
| macOS | `~/Library/Application Support/OWI` |
| Linux | `$XDG_DATA_HOME/owi`, normally `~/.local/share/owi` |

The source-checkout workflow keeps its existing private data location; desktop
setup does not silently import or overwrite it. Keep the desktop data folder
to retain outcomes and learned reminders.

Existing unrelated MCP entries are preserved. If an existing `owi` connection
points somewhere else, setup stops rather than replacing it. Remove that OWI
entry in your client's MCP settings, then reconnect to move to a new version.
Downloaded code does not update itself without you installing another release.

To disconnect, disable/remove OWI in your AI client's MCP settings. You can
then remove its installed `app` folder. To erase history, remove the OWI data
folder separately. Saved OpenRouter keys can be removed from the system
credential store under **Open Workforce Index / OpenRouter**.

## Terminal option

The downloaded executable also supports `connect`, `mcp`, and `diagnose`.
These commands use its included runtime:

```sh
./OWI connect --client vscode --project /path/to/project --prepare
./OWI diagnose
```

Run the graphical setup first if you want a managed installed copy and a saved
OpenRouter key. For terminal-only installation, keep the whole extracted folder
at its final location. On Windows use `OWI.exe`.

## Verification

The release workflow builds natively for all four platforms, extracts each
actual archive into a path containing spaces and removes Python/Rust from
`PATH`. It tests the compiled engine, all four connection configurations, an
MCP task with a local fake worker, and real deterministic feedback. It also
opens the bundled setup window on each platform. No paid provider calls are
made by these checks. Real host sessions and OS keychain prompts still need
verification on the user's computer.

Developers can still use the [source setup](INTEGRATIONS.md#connect-from-source-developers).
