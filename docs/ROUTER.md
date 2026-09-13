# Your router: every pick runs as the exact model it priced

The hosted ask page can only *request* a model — provider apps keep their own
selection. The router is this repo executing the picks itself, where no app
can substitute the model.

## Use it inside your existing tool

[Connect to a hosted OWI service](HOSTED.md) from compatible browser tools
or Telegram, without installing OWI. For local operation, the optional
[desktop package](DESKTOP.md) connects to VS Code Copilot, Copilot CLI,
Cursor or Claude Code. The host can delegate tasks automatically after one-time
setup. No OWI page or acceptance question is needed for each task.
The desktop download includes everything OWI needs; the source-development
steps below are optional.

Opening this repository in an AI tool does not itself establish provider
credentials or free usage. Client trust, tool permissions and model access
must work independently. The local GUI below is an optional review interface.

Your actual billing follows the CLI's authentication. Declare API or
subscription billing under **My plan & self-updating agents** in the connected
page; OWI does not infer included allowance from a login or a dollar estimate.
See [usage and plans](USAGE_AND_PLANS.md). Built-in Claude commands request JSON
telemetry; existing commands keep their configured output format.

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

## The confidential worker: a model on your own machine

Install [ollama](https://ollama.com) and pull `llama3.1`; the runner
prefills automatically. `worker:local-llama` is priced $0.00/Mtok and is
the only worker cleared for confidential content — `--privacy confidential`
staffs it and nothing else, and confidential failures are redacted in the
prevention memory. Nothing leaves your machine.

## Hiring an external platform (OpenHands, or any agent server)

A worker is a command; a platform that runs agents is therefore a hire away.
The roster already carries the first one: `worker:openhands-sonnet-5/code` —
the same claude-sonnet-5 offering, but `harness_id: openhands`, because the
same model in a different workshop is a **different worker** with its own
record (that is what configuration identity is for).

External hires follow the measurement-first rule: they join with **zero
assumed evidence**, so the quality floor keeps them unstaffable until the
bench proves them. Onboarding:

1. Install the platform's CLI/agent server on your machine (see the
   platform's own install docs) and give it your model credentials.
2. Point a runner at it in `.owi-quick/runners.json`, e.g.
   `"openhands-sonnet-5": "<the platform's headless run command>"` — task on
   stdin, result on stdout, same contract as every runner.
3. Earn the seat on the bench:

   ```bash
   python3 tools/run_bench.py --corpus corpus.json --repo <repo> \
     --worker-id worker:openhands-sonnet-5/code --adapter command \
     --command "<the same headless command>" --observed-at <now> \
     --outcome-dir out/ && for f in out/*.json; do \
     cargo run -q -p workforce-cli -- outcome --local .owi-quick/local.sqlite --input "$f"; done
   ```

Pass enough deterministic tasks and the posterior clears the floor — the
platform's agent starts winning staffing decisions on its measured record,
priced at its offering's real rates, retireable like everyone else.

## What runs where — the honest boundary

| surface | picks | executes | model guaranteed? |
|---|---|---|---|
| hosted link | yes | deep link to provider app | no — the app decides; the link requests |
| `owi-serve` (this router) | yes | your commands, your keys | **yes — exact model id** |
| `owi-do` (terminal) | yes | same commands | **yes** |
