# Your router: every pick runs as the exact model it priced

The hosted ask page can only *request* a model — provider apps keep their own
selection. The router is this repo executing the picks itself, where no app
can substitute the model.

## Zero install — open it in Claude Code and speak

The simplest router requires nothing at all: open this repository in Claude
Code (claude.ai/code, also inside the Claude app) and say what you want
done. The session clones the repo itself, reads `CLAUDE.md`, and routes your
task through the engine: split into parts, priced picks, Anthropic models
executed on your existing Claude login, checklists verified by a non-maker
judge, outcomes recorded to the ledger. No git, no keys, no tokens — the
subscription you already have is the credential.

Everything below is the optional, on-your-own-machine version — for adding
OpenAI models with your own key, or running the page locally.

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
