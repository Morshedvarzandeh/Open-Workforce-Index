#!/usr/bin/env python3
"""Render the staffing board from a scenario produced by build_manager_scenario.py.

The board is a manager's view, not a report: it shows who was assigned each
job, who was on the bench and what they would have cost, and who was turned
away with the recorded reason. Every number is read from the engine's own
output — nothing here is illustrative except where the page says so.
"""

from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path

# Onboarding a new hire is measured against the battery-core corpus.
CORPUS_TASKS = 11
SMOKE_TASKS = 3
TOKENS_IN = 20_000
TOKENS_OUT = 4_000


def build_board(scenario: dict, seed: dict, offerings: dict) -> dict:
    roster = []
    for worker in seed["worker_profiles"]:
        price = offerings.get(worker["offering_id"], {})
        roster.append({
            "id": worker["id"],
            "model": worker["id"].split(":")[1].split("/")[0],
            "role": worker["id"].split("/")[1],
            "skills": worker["supported_skill_ids"],
            "tools": worker["tools"],
            "in": price.get("in"),
            "out": price.get("out"),
            "ctx": price.get("ctx"),
        })

    tasks = []
    right_sized = top_tier = 0
    for result in scenario["results"]:
        quote = result["quote"]
        eligible = quote["eligible_candidates"]
        selected = quote["selected_worker_id"]
        reasons: Counter = Counter()
        for rejected in quote["rejected_candidates"]:
            for reason in rejected["reasons"]:
                reasons[reason["code"]] += 1
        if eligible:
            chosen = next(c for c in eligible if c["worker_id"] == selected)
            right_sized += chosen["cost"]["expected_accepted_cost_micros"]
            top_tier += max(c["cost"]["expected_accepted_cost_micros"] for c in eligible)
        tasks.append({
            "id": result["task_id"],
            "summary": result["summary"],
            "skill": result["skill_id"],
            "tools": result["required_tools"],
            "tin": result["estimated_input_tokens"],
            "tout": result["estimated_output_tokens"],
            "assigned": selected,
            "eligible": [{
                "id": c["worker_id"],
                "lcb": c["success_lower_bound"],
                "mean": c["success_mean"],
                "run": c["cost"]["run_cash_micros"] / 1e6,
                "retry": c["cost"]["expected_failure_cash_micros"] / 1e6,
                "total": c["cost"]["expected_accepted_cost_micros"] / 1e6,
            } for c in eligible],
            "rejected_count": len(quote["rejected_candidates"]),
            "rejected_by": dict(reasons),
        })

    rates = {r["model"]: (r["in"], r["out"]) for r in roster if r["in"] is not None}
    onboarding = {
        "corpus_tasks": CORPUS_TASKS,
        "smoke_tasks": SMOKE_TASKS,
        "tokens_in": TOKENS_IN,
        "tokens_out": TOKENS_OUT,
        "cost": {
            model: {
                "full": CORPUS_TASKS * (TOKENS_IN * i / 1e6 + TOKENS_OUT * o / 1e6),
                "smoke": SMOKE_TASKS * (TOKENS_IN * i / 1e6 + TOKENS_OUT * o / 1e6),
            }
            for model, (i, o) in rates.items()
        },
    }

    return {
        "tasks": tasks,
        "roster": roster,
        "onboarding": onboarding,
        "totals": {
            "rightsized": right_sized / 1e6,
            "toptier": top_tier / 1e6,
            "saving_pct": 100 * (1 - right_sized / top_tier) if top_tier else 0.0,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", required=True, type=Path)
    parser.add_argument("--seed", required=True, type=Path)
    parser.add_argument(
        "--prices",
        required=True,
        type=Path,
        help="JSON emitted by `owi prices --dry-run`.",
    )
    parser.add_argument("--template", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    price_import = json.loads(arguments.prices.read_text())
    offerings = {
        o["id"]: {
            "in": o["input_micros_per_million_tokens"] / 1e6,
            "out": o["output_micros_per_million_tokens"] / 1e6,
            "ctx": o["context_window_tokens"],
        }
        for o in price_import["offerings"]
    }

    board = build_board(
        json.loads(arguments.scenario.read_text()),
        json.loads(arguments.seed.read_text()),
        offerings,
    )
    template = arguments.template.read_text()
    if "__DATA__" not in template:
        raise SystemExit("template is missing the __DATA__ placeholder")
    # Escape any closing tag so the payload cannot terminate the script block.
    payload = json.dumps(board).replace("</", "<\\/")
    arguments.output.write_text(template.replace("__DATA__", payload))
    print(f"rendered {len(board['tasks'])} jobs and {len(board['roster'])} people "
          f"-> {arguments.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
