# ADR 0009: Hosted connections are the primary experience

Status: accepted

## Context

Bundling Rust/Python removes development prerequisites but still assumes a
desktop workflow. Many intended users work in a browser or Telegram and want
an add-on to their existing tool, with no OWI download or repeated UI work.

## Decision

Run the existing engine behind a hosted HTTP service. Offer remote MCP for
compatible clients, an authenticated API for websites/bots, and an optional
Telegram webhook adapter. The browser page is onboarding and a first test;
desktop packaging remains optional. Never imply universal interception of
third-party websites or bots.

The first pilot uses operator-provisioned accounts, each with its own access
token, provider key, private ledger, daily attempt limit and worker process.
Model execution is limited to configured OpenRouter text workers. No user
commands, public anonymous charges, automatic model retries or key pooling.
Telegram only accepts configured users in private chats with a verified
webhook secret. Persistent idempotency records survive webhook redelivery.

## Consequences

End users install nothing. The operator must deploy an HTTPS service with
persistent state and configure model accounts and the Telegram bot once.
This has a hosting cost and requires a hosting account connection. The
repository alone does not create a working public service.

OAuth onboarding for products that require it, self-service account signup,
broader client certification and a durable distributed job queue remain future
work. The current single-instance pilot bounds concurrency and does not replay
uncertain paid work after interruption. Existing instruction-learning and
quality-evidence limits remain in force.
