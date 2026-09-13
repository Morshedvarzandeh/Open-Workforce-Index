# OWI without a download

The primary product direction is a hosted connection. OWI runs on a service
operated centrally; people keep using their browser editor or chat. They do
not install an OWI app, Rust, Python, or a local model runner.

**Status:** the hosted server and adapters are implemented for a private pilot.
A public service URL and a live Telegram bot still require deployment and
account setup. The repository's static demo is not a running hosted service.

| Where someone works | How OWI connects | What the user installs |
|---|---|---|
| Web editor or AI tool supporting remote MCP with a Bearer header | Add the operator's `/mcp` URL and connection key once | Nothing for OWI |
| Website or existing bot with an API integration | Its developer calls `/api/work` | Nothing for OWI |
| Telegram | Start the operator's OWI bot; send normal messages | No OWI app; Telegram Web works too |
| Browser first test | Open the hosted page and connect | Nothing |
| Local IDE where the user prefers local operation | Optional [desktop package](DESKTOP.md) | Optional OWI download |

This cannot silently intercept every website, chatbot or autocomplete request.
An existing third-party Telegram bot must integrate OWI itself. Web products
that require OAuth for remote tools need an additional OAuth integration; this
private-pilot server currently uses operator-issued Bearer connection keys.
Do not advertise it as a working connector for every ChatGPT/Claude web plan.

## End-user flow

1. Receive the service connection or Telegram bot link from the operator.
2. Connect once in a compatible tool, or open the bot in Telegram.
3. Continue doing normal work there. OWI handles the delegated subtask and
   returns the result to the same tool or conversation.

The website includes an optional first-task test. It is not a required extra
workspace that people must open for every task. Connection keys entered there
stay in page memory; they are not saved in browser storage.

## What the service does

- Reuses the existing Rust allocator, OpenRouter adapter, local checks and
  private feedback/instruction loop.
- Runs each account's task in a separate process with only that account's
  provider key. Ledgers and instruction history are separated by account.
- Accepts writing, extraction and planning through explicit OpenRouter workers.
  It does not run user-supplied shell commands or edit remote repositories.
- Requires authentication, validates browser origins, rejects confidential or
  secret tasks on the cloud connection, and limits work to one active task per
  account and two across the service.
- Reserves a daily task allowance before execution. Failed/uncertain attempts
  count toward it. Provider charges remain separate; the estimated-cost filter
  is not an invoice spending cap.
- Requires API idempotency keys and gives each authenticated MCP connection
  its own session. Repeated request identifiers cannot rerun a paid task.
- Accepts Telegram updates only with the webhook secret and from configured
  users in private chats. Webhook redelivery does not repeat model execution.

Telegram replies are sent asynchronously after the webhook is acknowledged.
Model calls are never retried automatically. A service interruption may leave
a request without a delivered answer; it is not automatically re-executed.
Telegram answers over 14,400 characters are explicitly truncated. No private
task content is published to GitHub or to other users' ledgers.

## Operator deployment — once for the service

These steps are for the operator, not every user. Any Docker host with HTTPS
and a persistent volume can run `hosting/Dockerfile`. The image builds the
engine automatically. Run one service process/instance with a persistent
volume mounted at `/data`, writable by container UID 10001. The HTTP port
defaults to 8080 and honors the host's `PORT` variable.

Do not use an ephemeral disk for a paid pilot: losing that disk also loses
daily request limits, duplicate-request protection and private history.

Configure secrets through the host's environment/secret settings, never in
Git. `OWI_ACCOUNTS_JSON` is an array of accounts; its schema is:

| Field | Meaning |
|---|---|
| `id` | Unique lowercase identifier containing letters, digits, `_` or `-` |
| `token` | Unique random OWI connection key, at least 32 characters |
| `openrouter_api_key` | This account's existing OpenRouter API key |
| `daily_tasks` | Daily attempt limit; default 20 |
| `estimated_cost_ceiling_micros` | Estimated USD cost filter; default 100000 (USD 0.10) |
| `telegram_user_id` | Optional numeric Telegram user ID allowed to use this account |

Other environment settings:

| Setting | Meaning |
|---|---|
| `OWI_PUBLIC_URL` | Actual HTTPS service origin, without a path; Render's external URL is detected automatically |
| `OWI_ALLOWED_ORIGINS_JSON` | Additional explicit browser origins allowed to connect, JSON array; no wildcard |
| `OWI_HOSTED_HOME` | Persistent state directory; default `/data/owi` |
| `OWI_TELEGRAM_BOT_TOKEN` | Optional bot token, set only on the service |
| `OWI_TELEGRAM_WEBHOOK_SECRET` | Separate random webhook secret, at least 32 characters |
| `OWI_TELEGRAM_BOT_NAME` | Public bot username, without `@`, to show its link on the connected page |

For Telegram, the operator registers the bot's HTTPS webhook as
`<service-origin>/telegram/webhook` using Telegram's `setWebhook` API, supplying
`secret_token` matching the configured webhook secret and limiting
`allowed_updates` to `["message"]`. Bot credentials must never be pasted into
an end-user chat or committed to the repository.

No public signup, OAuth authorization server, hosting subscription, bot
registration or provider account is silently provisioned by this code.
Operator-issued access is the scope of this first hosted pilot.
The page links to the project source. Operators deploying modified versions
must point that link at the corresponding source for their deployed version.

## HTTP interfaces

`GET /api/status` uses `Authorization: Bearer <OWI connection key>`.
`POST /api/work` additionally needs `Content-Type: application/json` and a
unique `Idempotency-Key` for each new task. Its body is the existing `owi_work`
argument object: `task`, optional `context`, `checks`, `skill`, `privacy`.

`POST /mcp` provides JSON responses over Streamable HTTP. Clients initialize
once, then preserve `MCP-Session-Id` and the negotiated `MCP-Protocol-Version`.
Sessions expire after one hour; a 404 requires reinitialization. Cancellation
notifications affect only that account and session. No SSE stream, sampling,
OAuth discovery, refresh token or public anonymous execution is claimed.

`GET /healthz` reports service readiness without credentials or private data.
The public page and scripts reveal no connection or provider keys.

## Verification

`tools/test_hosted.py` checks authentication, origins, per-account limits,
duplicate requests, confidential-input rejection, MCP session isolation,
cancellation and Telegram identity/redelivery with fake workers and senders.
`tools/check_hosted_engine.py` sends real HTTP requests through isolated child
processes and the Rust engine for two accounts, recording separate feedback
with local fake models. The hosted CI workflow builds the production Docker
image and exercises those requests and its actual Gunicorn entry point.

No tests send real Telegram messages or spend provider credits. A deployed
HTTPS host, real account credentials, bot webhook and actual web-tool session
still need verification before announcing a live public service.

References: [HTTP MCP transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
[Telegram webhook API](https://core.telegram.org/bots/api#setwebhook),
[Docker hosting on Render](https://render.com/docs/docker),
[persistent disks](https://render.com/docs/disks).
