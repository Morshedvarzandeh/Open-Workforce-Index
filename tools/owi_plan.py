#!/usr/bin/env python3
"""Plan and cost a repository's outstanding work, without asking anyone.

Drop a `.owi/project.json` into a repository listing the features you want
built and who is on the roster. This reads it, allocates every task through
the engine, and writes a total-cost-of-ownership report next to it. Wire it to
a git hook or CI and the plan refreshes itself whenever the manifest changes.

Total cost of ownership here means the full cost of getting an *accepted*
result, not the price of one API call:

    tokens + tools + expected retries + expected human review time

The last term is usually the largest and is the one normally left out. A cheap
worker that fails two times in five costs you the diagnosis every time it
does, and that is charged at your hourly rate, not the model's.

The report always states three totals so the saving is not a single
unfalsifiable number:

    right-sized     what the plan costs
    always-premium  the same work given to the dearest qualified worker
    always-cheapest what it would cost ignoring reliability entirely

The middle figure is the honest baseline for "what we saved". The third exists
to show that cheapest-per-token is not the same as cheapest overall, and is
sometimes worse than the plan.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

MICROS = 1_000_000

# Cost, time, and quality trade against each other: you can buy your way from
# one to another. These four do not trade -- no budget clears them, so they are
# applied as gates before any ranking happens, and a task that fails one is
# unstaffable rather than expensive.
GATES = {
    "capability": ["missing_skill", "missing_tool"],
    "clearance": ["insufficient_privacy_clearance"],
    "policy": [
        "missing_checker_worker",
        "checker_matches_maker",
        "unauthorized_checker_worker",
        "provider_not_allowed",
        "evidence_snapshot_mismatch",
        "unavailable",
        "context_window_too_small",
    ],
    "quota": ["quota_budget_exceeded"],
}
TRADEABLE = {
    "cost": ["cash_budget_exceeded", "currency_mismatch"],
    "time": ["latency_limit_exceeded"],
    "quality": [
        "task_confidence_below_minimum",
        "skill_confidence_below_minimum",
        "missing_skill_estimate",
        "insufficient_task_evidence",
        "insufficient_skill_evidence",
    ],
}
CLASS_OF = {code: name for group in (GATES, TRADEABLE)
            for name, codes in group.items() for code in codes}


def classify(codes) -> dict:
    """Group rejection reasons into gates and tradeable axes."""
    counts: dict = {}
    for code in codes:
        counts[CLASS_OF.get(code, "other")] = counts.get(CLASS_OF.get(code, "other"), 0) + 1
    return counts


def owi(repo: Path, *args: str) -> dict:
    """Invoke the CLI and return its JSON, failing loudly on a bad exit."""
    completed = subprocess.run(
        ["cargo", "run", "-q", "-p", "workforce-cli", "--", *args],
        cwd=repo,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr[-2000:])
        raise SystemExit(f"owi {args[0]} failed")
    return json.loads(completed.stdout)


def allocation_request(project: dict, feature: dict, index: int) -> dict:
    defaults = project.get("defaults", {})
    assumptions = {**defaults.get("assumptions", {}), **feature.get("assumptions", {})}
    return {
        "decision_id": f"decision:{project['project_id']}-{index:04d}",
        "snapshot_id": project["snapshot_id"],
        "at_epoch_ms": project["at_epoch_ms"],
        "created_at": project["created_at"],
        "task": {
            "id": feature["id"],
            "summary": feature["summary"],
            "required_skills": [{
                "skill_id": feature["skill_id"],
                "minimum_success_probability": feature.get(
                    "minimum_success_probability",
                    defaults.get("minimum_success_probability", 0.3)),
                "minimum_evidence_count": feature.get("minimum_evidence_count", 0),
            }],
            "required_tools": feature.get("required_tools", []),
            "privacy": feature.get("privacy", defaults.get("privacy", "private_metadata")),
            "risk": feature.get("risk", "low"),
            "verification": feature.get("verification", "deterministic"),
            "minimum_success_probability": feature.get(
                "minimum_success_probability",
                defaults.get("minimum_success_probability", 0.3)),
            "minimum_evidence_count": feature.get("minimum_evidence_count", 0),
            "estimated_input_tokens": feature["estimated_input_tokens"],
            "estimated_output_tokens": feature["estimated_output_tokens"],
        },
        "policy": defaults["policy"],
        "calibration": defaults.get("calibration", {}),
        "assumptions": assumptions,
    }


def plan(repo: Path, project: dict, index_db: str, local_db: str, work_dir: Path) -> dict:
    work_dir.mkdir(parents=True, exist_ok=True)
    entries, unstaffed = [], []
    right_sized = premium = cheapest = 0

    for position, feature in enumerate(project["features"], start=1):
        request_path = work_dir / f"request-{position:04d}.json"
        request_path.write_text(
            json.dumps(allocation_request(project, feature, position), indent=2))
        result = owi(repo, "allocate", "--index", index_db, "--local", local_db,
                     "--input", str(request_path))
        quote = result["quote"]
        eligible = quote["eligible_candidates"]

        if not eligible:
            unstaffed.append({
                "id": feature["id"],
                "summary": feature["summary"],
                # Every rejection is a recorded reason, so an unstaffable task
                # says exactly what is missing rather than failing silently.
                "reasons": sorted({
                    reason["code"]
                    for rejected in quote["rejected_candidates"]
                    for reason in rejected["reasons"]
                }),
                "blocked_by": classify(
                    reason["code"]
                    for rejected in quote["rejected_candidates"]
                    for reason in rejected["reasons"]
                ),
                "gated": sorted({
                    CLASS_OF.get(reason["code"], "other")
                    for rejected in quote["rejected_candidates"]
                    for reason in rejected["reasons"]
                    if CLASS_OF.get(reason["code"]) in GATES
                }),
            })
            continue

        selected = quote["selected_worker_id"]
        chosen = next(c for c in eligible if c["worker_id"] == selected)
        dearest = max(eligible, key=lambda c: c["cost"]["expected_accepted_cost_micros"])
        by_run = min(eligible, key=lambda c: c["cost"]["run_cash_micros"])

        right_sized += chosen["cost"]["expected_accepted_cost_micros"]
        premium += dearest["cost"]["expected_accepted_cost_micros"]
        cheapest += by_run["cost"]["expected_accepted_cost_micros"]

        entries.append({
            "id": feature["id"],
            "summary": feature["summary"],
            "assigned": selected,
            "success_mean": chosen["success_mean"],
            "success_lower_bound": chosen["success_lower_bound"],
            "p95_latency_ms": chosen["p95_latency_ms"],
            "run_micros": chosen["cost"]["run_cash_micros"],
            "review_micros": chosen["cost"]["review_cash_micros"],
            "retry_micros": chosen["cost"]["expected_failure_cash_micros"],
            "total_micros": chosen["cost"]["expected_accepted_cost_micros"],
            "premium_alternative": dearest["worker_id"],
            "premium_total_micros": dearest["cost"]["expected_accepted_cost_micros"],
            "cheapest_by_run": by_run["worker_id"],
            "cheapest_by_run_total_micros": by_run["cost"]["expected_accepted_cost_micros"],
            "bench_size": len(eligible),
            "turned_away": len(quote["rejected_candidates"]),
            "privacy": feature.get("privacy", project.get("defaults", {}).get(
                "privacy", "private_metadata")),
            "risk": feature.get("risk", "low"),
            "verification": feature.get("verification", "deterministic"),
            "turned_away_by": classify(
                reason["code"]
                for rejected in quote["rejected_candidates"]
                for reason in rejected["reasons"]),
        })

    return {
        "project_id": project["project_id"],
        "snapshot_id": project["snapshot_id"],
        "planned": entries,
        "unstaffed": unstaffed,
        "totals": {
            "right_sized_micros": right_sized,
            "always_premium_micros": premium,
            "always_cheapest_by_run_micros": cheapest,
            "saved_vs_premium_micros": premium - right_sized,
            "saved_vs_premium_pct": (
                100 * (1 - right_sized / premium) if premium else 0.0),
            "saved_vs_cheapest_micros": cheapest - right_sized,
        },
        "total_latency_ms": sum(e["p95_latency_ms"] for e in entries),
    }


def render(report: dict) -> str:
    t = report["totals"]
    money = lambda m: f"${m / MICROS:,.4f}"
    lines = [
        f"OWI plan · {report['project_id']} · {report['snapshot_id']}",
        "",
        f"{'feature':32} {'assigned to':26} {'run':>9} {'review':>9} {'retry':>9} {'total':>10}",
        "-" * 100,
    ]
    for e in report["planned"]:
        lines.append(
            f"{e['id'].replace('task:', '')[:32]:32} "
            f"{e['assigned'].replace('worker:', '')[:26]:26} "
            f"{money(e['run_micros']):>9} {money(e['review_micros']):>9} "
            f"{money(e['retry_micros']):>9} {money(e['total_micros']):>10}")
        gates = ", ".join(
            f"{n} on {name}" for name, n in sorted(
                e["turned_away_by"].items(), key=lambda kv: -kv[1]))
        lines.append(
            f"{'':32} gates: privacy={e['privacy']} risk={e['risk']} "
            f"verify={e['verification']} | turned away: {gates}")
    lines += [
        "-" * 100,
        f"{'RIGHT-SIZED PLAN':59} {money(t['right_sized_micros']):>40}",
        f"{'if every task went to the dearest qualified worker':59} "
        f"{money(t['always_premium_micros']):>40}",
        f"{'if every task went to the cheapest per-token worker':59} "
        f"{money(t['always_cheapest_by_run_micros']):>40}",
        "",
        f"SAVED vs premium: {money(t['saved_vs_premium_micros'])} "
        f"({t['saved_vs_premium_pct']:.1f}%)",
        f"SAVED vs cheapest-per-token: {money(t['saved_vs_cheapest_micros'])} "
        f"(negative means picking on sticker price would have been cheaper here)",
        f"Wall-clock budget (sum of p95): {report['total_latency_ms'] / 1000:.0f}s",
    ]
    if report["unstaffed"]:
        lines += ["", "UNSTAFFED — no worker on the roster qualifies:"]
        for u in report["unstaffed"]:
            gated = ", ".join(u["gated"]) if u["gated"] else "none"
            lines.append(
                f"  {u['id']}\n"
                f"    hard gates hit : {gated}"
                f"{'  <- no budget clears these' if u['gated'] else ''}\n"
                f"    reasons        : {', '.join(u['reasons'])}")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--owi-repo", required=True, type=Path,
                        help="Checkout containing the workforce CLI.")
    parser.add_argument("--project", required=True, type=Path,
                        help="Path to .owi/project.json")
    parser.add_argument("--index", required=True)
    parser.add_argument("--local", required=True)
    parser.add_argument("--work-dir", required=True, type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--fail-on-unstaffed", action="store_true",
                        help="Exit non-zero if any task has no qualified worker.")
    parser.add_argument("--budget-micros", type=int,
                        help="Exit non-zero if the plan exceeds this total.")
    arguments = parser.parse_args()

    project = json.loads(arguments.project.read_text())
    report = plan(arguments.owi_repo.resolve(), project,
                  arguments.index, arguments.local, arguments.work_dir)

    print(render(report))
    if arguments.report:
        arguments.report.write_text(json.dumps(report, indent=2) + "\n")

    total = report["totals"]["right_sized_micros"]
    if arguments.budget_micros is not None and total > arguments.budget_micros:
        print(f"\nBUDGET EXCEEDED: {total} > {arguments.budget_micros} micros",
              file=sys.stderr)
        return 2
    if arguments.fail_on_unstaffed and report["unstaffed"]:
        print(f"\n{len(report['unstaffed'])} task(s) have no qualified worker",
              file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
