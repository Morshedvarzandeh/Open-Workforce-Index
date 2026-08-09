#!/usr/bin/env python3
"""Benchmark a hand-picked agent team against evidence-priced staffing.

People are already staffing multi-agent workshops by hand: a planner on one
premium model, a debugger on another, a designer on a third — chosen by vibes
and loyalty, with no price or evidence attached. This tool takes such a
roster as a team spec and answers it role by role: for each role it runs a
real allocation (same gates, same Beta lower-bound floor, same cost model as
`owi-do`) and prints the evidence pick next to the hand pick.

Three things can happen to a hand pick:
  priced   — it is in the index and passed the gates, so it has a real
             expected cost per accepted result to compare
  gated    — it is in the index but was turned away, with the reason
  external — it is not in the index at all; there is no evidence and no
             price, which is precisely the point

The spec is JSON: {"name": ..., "source": ..., "team": [{"role", "task",
"skill" (optional, else keyword-classified), "tokens_in", "tokens_out",
"hand_pick" (worker id or null), "hand_label"}]}.
"""

from __future__ import annotations

import argparse
import importlib.machinery
import importlib.util
import json
import sys
from pathlib import Path

TOOLS = Path(__file__).resolve().parent

_loader = importlib.machinery.SourceFileLoader("owi_do", str(TOOLS / "owi-do"))
_spec = importlib.util.spec_from_loader("owi_do", _loader)
owi_do = importlib.util.module_from_spec(_spec)
_loader.exec_module(owi_do)

MICROS = 1_000_000


def cost_of(candidate: dict) -> float:
    return candidate["cost"]["expected_accepted_cost_micros"] / MICROS


def hand_pick_status(hand_pick: str | None, eligible: list,
                     rejected: list) -> tuple[str, float | None]:
    """(status text, cost) for the hand-picked worker within this role's
    real quote — priced, gated with the reason, or external."""
    if not hand_pick:
        return "external", None
    for candidate in eligible:
        if candidate["worker_id"] == hand_pick:
            return "priced", cost_of(candidate)
    for candidate in rejected:
        if candidate["worker_id"] == hand_pick:
            reasons = candidate.get("reasons") or candidate.get("reason") or []
            if isinstance(reasons, str):
                reasons = [reasons]
            return f"gated ({', '.join(str(r) for r in reasons)})", None
    return "external", None


def short(worker_id: str) -> str:
    return worker_id.replace("worker:", "")


def benchmark(home: Path, spec: dict, rate: int = 0) -> dict:
    rows = []
    for member in spec["team"]:
        task = member["task"]
        skill = member.get("skill") or owi_do.classify(task)
        result, quote, eligible = owi_do.choose(
            home, task, skill,
            int(member.get("tokens_in", 3000)),
            int(member.get("tokens_out", 1200)),
            rate=rate)
        if not eligible:
            rows.append({"role": member["role"], "skill": skill,
                         "evidence": None, "evidence_cost": None,
                         "hand_label": member.get("hand_label", ""),
                         "hand_status": "—", "hand_cost": None})
            continue
        top = eligible[0]
        quality, means = owi_do.quality_option(
            result.get("calibration"), eligible)
        status, hand_cost = hand_pick_status(
            member.get("hand_pick"), eligible,
            quote.get("rejected_candidates", []))
        rows.append({
            "role": member["role"], "skill": skill,
            "evidence": short(top["worker_id"]),
            "evidence_cost": cost_of(top),
            "quality": (short(quality["worker_id"])
                        if quality and quality["worker_id"] != top["worker_id"]
                        else None),
            "quality_cost": (cost_of(quality)
                             if quality
                             and quality["worker_id"] != top["worker_id"]
                             else None),
            "hand_label": member.get("hand_label", ""),
            "hand_status": status, "hand_cost": hand_cost,
        })
    return {"name": spec.get("name", "team"),
            "source": spec.get("source", ""), "rows": rows}


def render(report: dict) -> str:
    lines = [f"team: {report['name']}"]
    if report["source"]:
        lines.append(f"source: {report['source']}")
    lines.append("")
    header = (f"{'role':<11} {'skill':<32} {'evidence pick':<21} "
              f"{'':>8}  hand pick")
    lines.append(header)
    lines.append("-" * len(header))

    evidence_total = 0.0
    hand_total, hand_priced, hand_unpriced = 0.0, 0, 0
    for row in report["rows"]:
        if row["evidence"] is None:
            lines.append(f"{row['role']:<11} {row['skill'][6:]:<32} "
                         f"{'(no eligible worker)':<21} {'':>8}  "
                         f"{row['hand_label']}")
            hand_unpriced += 1
            continue
        evidence_total += row["evidence_cost"]
        if row["hand_cost"] is not None:
            hand_total += row["hand_cost"]
            hand_priced += 1
            hand_text = f"{row['hand_label']}  ${row['hand_cost']:.4f}"
        elif row["hand_status"] == "external":
            hand_unpriced += 1
            hand_text = f"{row['hand_label']}  · no price, no evidence"
        else:
            hand_unpriced += 1
            hand_text = f"{row['hand_label']}  — {row['hand_status']}"
        lines.append(f"{row['role']:<11} {row['skill'][6:]:<32} "
                     f"{row['evidence']:<21} ${row['evidence_cost']:.4f}  "
                     f"{hand_text}")
        if row.get("quality"):
            lines.append(f"{'':<11} {'':<32}   quality option: "
                         f"{row['quality']} ${row['quality_cost']:.4f}")

    lines.append("")
    lines.append(f"evidence-staffed team: ${evidence_total:.4f} "
                 f"per accepted round, every seat priced and gated")
    hand_line = f"hand-picked team:      ${hand_total:.4f} across " \
                f"{hand_priced} priceable seat(s)"
    if hand_unpriced:
        hand_line += (f", {hand_unpriced} seat(s) with no price and no "
                      f"evidence in the index")
    lines.append(hand_line)
    lines.append("")
    lines.append("A roster you can't price is a roster you can't manage.")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("spec", type=Path, help="team spec JSON")
    parser.add_argument("--home", type=Path,
                        default=TOOLS.parent / ".owi-quick")
    parser.add_argument("--rate", type=int, default=0,
                        help="advanced: also charge your own review time at "
                             "this many dollars per hour (default 0 = cash "
                             "only). A high rate makes reliability dominate "
                             "token prices and the roster shifts upmarket.")
    parser.add_argument("--json", action="store_true",
                        help="emit the report as JSON instead of a table")
    arguments = parser.parse_args()

    owi_do.bootstrap(arguments.home)
    spec = json.loads(arguments.spec.read_text(encoding="utf-8"))
    report = benchmark(arguments.home, spec, rate=arguments.rate)
    if arguments.json:
        print(json.dumps(report, indent=2))
    else:
        print(render(report))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
