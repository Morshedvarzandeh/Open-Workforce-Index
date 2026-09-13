# OWI inside your existing AI tools

Use OWI as an MCP add-on to your agent. After one-time connection, continue
working in Copilot, Cursor or Claude Code. The host agent can call `owi_work`
to choose a configured worker, execute a subtask, check its requirements and
return the result to the same conversation. There is no per-task GUI launch,
model picker, copy/paste step, or OWI acceptance question.

For browser tools and Telegram, start with the [hosted connection](HOSTED.md).
The operator deploys the service once; end users install nothing. That guide
states the current private-pilot, authentication and deployment limits.

The GUI remains optional. Local MCP and desktop downloads are alternative
paths; OWI is not a published VS Code Marketplace extension.

## Where it fits

| Tool | Connection | Scope |
|---|---|---|
| GitHub Copilot in VS Code | MCP server in your user profile or workspace | Agent tool calls, not inline autocomplete interception |
| GitHub Copilot CLI | Its own MCP configuration | Agent tool calls; honors enterprise restrictions |
| Cursor | MCP configuration | Delegated tasks in agent chat |
| Claude Code | Project MCP configuration | Delegated tasks using configured workers |
| OpenRouter | Model execution provider behind OWI | Separate OpenRouter API billing |

The host decides whether and when to call OWI. Its trust, tool permissions and
company policies still apply; the installer does not change approval settings.
OWI cannot replace every internal Copilot request or turn a Copilot/Claude
subscription into OpenRouter API credits. Copilot cloud agents and other IDE
Copilots require their own hosting/setup; they are not automatically configured
by this local installer.

## Connect once

Prefer the [hosted connection](HOSTED.md) for supported web tools and Telegram.
For optional local operation, use the [desktop download](DESKTOP.md): extract it, open OWI, choose your AI
app and project, then click **Connect**. The runtime and engine are included;
users do not install Rust or Python. Desktop OpenRouter keys can be saved in
the operating system's credential store directly from the setup screen.

The following commands are for developers running OWI from source.

## Connect from source (developers)

Prerequisites: a local OWI checkout, Python 3, and Rust 1.87+ for the initial
engine build. Existing authenticated model CLIs remain usable. Run from OWI:

```bash
python3 tools/owi-connect --client vscode --prepare
```

This builds the engine once, initializes the private index, and adds OWI to
VS Code using `code --add-mcp`. Trust the server and enable its tools in your
client. In chat, start with a real task; the host may delegate useful subtasks
to OWI. It can call `owi_status` if setup needs diagnosis.

For a specific workspace, or when `code` is not on PATH:

```bash
python3 tools/owi-connect --client vscode --project /path/to/your/project --prepare
```

Other supported clients:

```bash
python3 tools/owi-connect --client copilot-cli --prepare
python3 tools/owi-connect --client cursor --prepare
python3 tools/owi-connect --client claude-code --project /path/to/your/project --prepare
```

On Windows, use `python` if that is your Python command. With WSL/remote
workspaces, install and configure OWI in the environment where the MCP server
will run. Paths are local to that machine; do not copy them to another computer.
Existing unrelated servers are preserved. Invalid JSON/JSONC or a conflicting
OWI entry is left unchanged for review. Settings contain no provider key values.

## Use your OpenRouter account

Make your existing `OPENROUTER_API_KEY` available to the client environment,
then connect with:

```bash
python3 tools/owi-connect --client vscode --prepare --openrouter
```

Use `--client copilot-cli`, `cursor`, or `claude-code` similarly. Configuration
contains only an environment-variable reference. Launch the client from an
environment containing the key; desktop applications may not inherit terminal
variables. A missing or unresolved key produces a setup error, not a paid
fallback. Do not paste keys into chat, tracked config, or public issue reports.

The current OpenRouter starter catalog includes GPT-5 mini and Claude Haiku
4.5 when they appear in OpenRouter's model catalog. It supports writing,
structured extraction and planning; code suggestions can use existing native
CLI workers. OpenRouter text workers do not claim repository shell access.

Each OpenRouter offering, worker and provider policy has its own identity.
Direct-provider and subscription outcomes are not copied into its history.
Initial ability evidence is the project's clearly labelled assumed demo prior,
not proof of quality. The normal engine eligibility gates still apply.

Model/pricing metadata is cached for 24 hours. The first task after expiry
refreshes the catalog automatically; a failed refresh stops execution. New
snapshots preserve old records; refreshed identities start their own evidence.
The supported model set remains explicit rather than importing every model.

OpenRouter model fallback and provider fallback are disabled. Requests name a
specific model and provider, use provider rate ceilings from the cached catalog,
and cap output at 2048 tokens. There is one attempt per OWI tool call. Optional
provider features such as image generation, web search and provider tools are
not enabled. Reported credit costs and cache counters enter the usage ledger;
unknown invoice charges remain unknown.

## Keep it fast

MCP discovery does not build Rust, read provider accounts, or launch a browser.
Each task uses the already-built engine, performs local classification, selects
one worker and runs local checks. It makes no extra LLM planning/checking calls
and does not retry automatically. The host supplies context and requirements.

Supported local checks: JSON, required phrases, Python syntax and minimum word
count. Regex and subjective requirements are returned as requiring host review.
Without sufficient checks the verdict stays undecided. The host reviews meaning
and decides whether to apply returned edits; OWI never fakes a human approval.
Only computed results enter instruction learning, with confidential exclusions.

The default estimated-cost ceiling is USD 0.10 per delegated task; change it at
connection time with `--max-estimated-cost-micros`. This is a quote filter, not
an invoice cap. Provider billing controls remain necessary for a hard spending
limit. Runner execution times out after 120 seconds. MCP cancellation terminates
the local runner process tree; already-consumed provider usage may still be billed.

Each response includes routing time, total elapsed time, worker identity, checks
and usage. These expose overhead; no general speedup or cost-saving percentage
has been established. File edits and other effects of owner-configured agent
commands remain governed by those tools' own permissions. A temporary working
directory is not a filesystem security sandbox.

## Verification and protocol scope

`python3 tools/test_integrations.py` tests protocol discovery, noninteractive
execution, cancellation, configuration merging, billing limits and the OpenRouter
adapter with local fake runners. CI also runs `tools/check_bridge_engine.py`
against the real Rust engine to validate generated identities and feedback.
No tests spend provider credits. Actual IDE UI sessions and live provider calls
need testing in the user's environment before claiming end-to-end compatibility.

This stdio server negotiates MCP versions 2025-11-25, 2025-06-18 and 2024-11-05.
It exposes two tools, supports ping and cancellation, serializes work across MCP clients sharing a home, and sends
only JSON-RPC on stdout. It does not implement an HTTP gateway or sampling.
Nested OWI worker processes do not expose OWI again, preventing recursive routing.

Sources checked 13 September 2026:
[VS Code MCP](https://code.visualstudio.com/docs/agent-customization/mcp-servers),
[Copilot CLI MCP](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers),
[MCP transports](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
[OpenRouter API](https://openrouter.ai/docs/api_reference/overview),
[OpenRouter provider routing](https://openrouter.ai/docs/guides/routing/provider-selection).
