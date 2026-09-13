# Automatic task integration through MCP

Status: implemented, with local protocol and fake-provider validation.

The main product path is an add-on inside an existing agent environment.
The host owns the conversation, project context and final actions. A single
OWI tool call owns selection, one execution, local checks, usage and checked
feedback. The GUI is optional for reviewing evidence and settings.

Use MCP stdio for local clients, with fast discovery and a prebuilt engine.
Never block a tool on a human acceptance prompt or open a browser during work.
Limit work to one active MCP task per private home to avoid races in the current core
CLI's request/outcome scratch files. Cancellation stops the local process tree.
No additional model planning or grading calls are made on this default path.

OpenRouter is a distinct provider adapter with versioned offerings and worker
identities. It uses explicit model/provider choices and no automatic paid
fallback. Its catalog is cached and refreshed on demand after expiry. Initial
ability priors remain explicitly assumed; provider invoices remain unverified.

Consequences: the host decides when to delegate; this does not intercept every
Copilot completion. Cold setup still needs Python and a one-time Rust build.
The entry-point API key references are client-specific; live client testing is
separate from protocol and adapter tests. The cost ceiling filters quotes and
cannot guarantee the final invoice. Unknown verification stays undecided.
