#!/usr/bin/env python3
"""What each worker costs, across the shapes of work people actually send.

A price per million tokens is not a price for a job. What a worker costs to
get one *accepted* result depends on three things at once: what it charges for
input, what it charges for output, and how often its work has to be redone.
Those pull in different directions, so the cheapest worker is not a fixed
fact — it changes with the shape of the task.

This runs the real allocator once per (skill x usage profile) and reports
every eligible worker's expected cost per accepted result, so the change is
visible rather than asserted.
"""

from __future__ import annotations

import argparse
import importlib.machinery
import importlib.util
import json
import sys
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
REPO = TOOLS.parent
sys.path.insert(0, str(TOOLS))

_loader = importlib.machinery.SourceFileLoader("owi_do", str(TOOLS / "owi-do"))
_spec = importlib.util.spec_from_loader("owi_do", _loader)
owi_do = importlib.util.module_from_spec(_spec)
_loader.exec_module(owi_do)

MICROS = 1_000_000

# The shapes work actually arrives in. Token counts are what the front door
# already estimates for tasks of this size.
PROFILES = [
    {"key": "quick", "name": "Quick edit",
     "detail": "a sentence to fix, a line to rewrite",
     "tokens_in": 500, "tokens_out": 200},
    {"key": "standard", "name": "Standard task",
     "detail": "the everyday request the front door assumes",
     "tokens_in": 1_500, "tokens_out": 800},
    {"key": "long_input", "name": "Long document in",
     "detail": "a report to read, a log to summarise",
     "tokens_in": 20_000, "tokens_out": 1_000},
    {"key": "long_output", "name": "Long output",
     "detail": "drafting, generating, writing at length",
     "tokens_in": 2_000, "tokens_out": 8_000},
    {"key": "heavy", "name": "Heavy job",
     "detail": "a large refactor with a large answer",
     "tokens_in": 50_000, "tokens_out": 10_000},
]


def run(home: Path, skill: str, profile: dict, rate: int) -> dict:
    result = owi_do.pick(home, f"pricing probe {profile['key']}", skill,
                         profile["tokens_in"], profile["tokens_out"], rate)
    quote = result["quote"]
    means = {}
    for calibrated in result.get("calibration", []):
        if calibrated.get("skills"):
            posterior = calibrated["skills"][0]["posterior"]
            means[calibrated["worker_id"]] = (
                posterior["alpha"]
                / (posterior["alpha"] + posterior["beta"]))
    rows = [{
        "worker": candidate["worker_id"].replace("worker:", ""),
        "cost": candidate["cost"]["expected_accepted_cost_micros"] / MICROS,
        "mean": means.get(candidate["worker_id"]),
    } for candidate in quote["eligible_candidates"]]
    return {"skill": skill, "profile": profile["key"], "rows": rows,
            "turned_away": len(quote["rejected_candidates"])}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, default=REPO / ".owi-quick")
    parser.add_argument("--rate", type=int, default=0,
                        help="also charge your own review time, $/hour")
    parser.add_argument("--json", type=Path,
                        help="write the full table as JSON")
    arguments = parser.parse_args()

    owi_do.bootstrap(arguments.home)
    report: dict = {"rate": arguments.rate, "profiles": PROFILES, "skills": {}}

    for skill in owi_do.SKILLS:
        per_profile = {}
        for profile in PROFILES:
            per_profile[profile["key"]] = run(arguments.home, skill, profile,
                                              arguments.rate)
        report["skills"][skill] = per_profile

        winners = {key: (data["rows"][0]["worker"] if data["rows"] else None)
                   for key, data in per_profile.items()}
        changes = len(set(winners.values()))
        print(f"\n{skill[6:]}"
              + ("  — the cheapest worker CHANGES with usage"
                 if changes > 1 else "  — one worker is cheapest throughout"))
        header = f"  {'usage':<18}" + "".join(
            f"{p['name'][:16]:>18}" for p in PROFILES)
        print(header)
        seen = []
        for data in per_profile.values():
            for row in data["rows"]:
                if row["worker"] not in seen:
                    seen.append(row["worker"])
        for worker in seen:
            line = f"  {worker:<18}"
            for profile in PROFILES:
                row = next((r for r in per_profile[profile["key"]]["rows"]
                            if r["worker"] == worker), None)
                cheapest = per_profile[profile["key"]]["rows"][0]["worker"] \
                    if per_profile[profile["key"]]["rows"] else None
                cell = f"${row['cost']:.4f}" if row else "—"
                if row and worker == cheapest:
                    cell = "* " + cell
                line += f"{cell:>18}"
            print(line)

    if arguments.json:
        arguments.json.write_text(json.dumps(report, indent=2))
        print(f"\nfull table -> {arguments.json}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
