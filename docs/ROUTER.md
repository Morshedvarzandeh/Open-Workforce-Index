# Your router: every pick runs as the exact model it priced

The hosted ask page can only *request* a model — provider apps keep their own
selection. The router is the same page running on your machine, where
execution goes through commands you control and no app can substitute the
model. It already exists in this repo; these steps switch it on.

## Step 1 — prerequisites (once)

```bash
git clone https://github.com/Morshedvarzandeh/Open-Workforce-Index
cd Open-Workforce-Index        # needs Rust 1.87+ and Python 3
```

## Step 2 — Anthropic models (once)

Install the `claude` CLI and log in. That is all: haiku-4-5, sonnet-4-5,
opus-4-5, sonnet-5 and opus-5 prefill into `.owi-quick/runners.json` on
first run, each invoked by its exact model id.

## Step 3 — OpenAI models (once)

```bash
pip install llm
llm keys set openai            # paste your OpenAI API key
```

gpt-5 and gpt-5-mini now prefill the same way (`llm -m gpt-5-mini`). Your
key stays in `llm`'s own keystore on your machine — owi never sees it.
Prefer one key for many providers? Point the commands at an aggregator
instead, e.g. `"gpt-5-mini": "llm -m openrouter/openai/gpt-5-mini"`.

## Step 4 — run the router

```bash
tools/owi-serve                # open http://127.0.0.1:7787
```

Ask in the page: pick (cheapest qualified, quality option one tap away) →
run button executes the exact model → checklist verified (mechanical items
in process, judgement by a non-maker model) → verdict recorded to the real
ledger with its inspection level → prevention notes carried into the next
run. Multi-part asks staff and run each part separately.

## Step 5 — from your phone (optional)

```bash
tools/owi-serve --host 0.0.0.0
```

The server prints an access token and the URL to open from your phone on
the same Wi-Fi (or Tailscale): `http://<computer-address>:7787/?token=...`.
Requests without the token are refused (401) — the gate is part of the
frozen suite. Plain HTTP: keep it to networks you trust; never port-forward
it to the open internet.

## What runs where — the honest boundary

| surface | picks | executes | model guaranteed? |
|---|---|---|---|
| hosted link | yes | deep link to provider app | no — the app decides; the link requests |
| `owi-serve` (this router) | yes | your commands, your keys | **yes — exact model id** |
| `owi-do` (terminal) | yes | same commands | **yes** |
