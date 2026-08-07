#!/usr/bin/env python3
"""Render the project console: a small, serverless GUI over one manifest.

The console is a single self-contained HTML file. Eligibility is decided
server-side by the engine at generation time; the *weighting* — what your hour
is worth, how many tasks a worker will amortise its setup over, how much of
its latency you actually sit through — is recomputed live in the browser from
per-candidate primitives. Moving a slider re-ranks the same eligible set the
engine produced; it never invents a candidate the engine rejected.

One honest limit: a task with a hard cash budget would make eligibility itself
depend on the rate, which a client-side slider cannot re-evaluate. The
generator refuses such manifests rather than showing a dial that lies.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path

TOOLS_DIR = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("owi_plan", TOOLS_DIR / "owi_plan.py")
owi_plan = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_spec.loader and owi_plan)  # type: ignore[arg-type]


def build_console_data(repo: Path, project: dict, seeds: list[dict],
                       index_db: str, local_db: str, work_dir: Path) -> dict:
    work_dir.mkdir(parents=True, exist_ok=True)
    defaults = project.get("defaults", {})
    base_assumptions = defaults.get("assumptions", {})
    onboarding = base_assumptions.get("onboarding_cash_micros", {})
    default_onboarding = base_assumptions.get("default_onboarding_cash_micros", 0)

    roster_skills = {}
    for seed in seeds:
        for w in seed.get("worker_profiles", []):
            roster_skills[w["id"]] = set(w["supported_skill_ids"])

    tasks, measure_by_skill = [], {}
    for position, feature in enumerate(project["features"], start=1):
        if feature.get("max_expected_cash_micros") is not None:
            raise SystemExit(
                f"{feature['id']} sets a cash budget; eligibility would depend "
                "on the rate and the console's dial would be dishonest. Use "
                "`owi plan` for budgeted manifests.")
        merged = {**base_assumptions, **feature.get("assumptions", {})}
        request_path = work_dir / f"request-{position:04d}.json"
        request_path.write_text(json.dumps(
            owi_plan.allocation_request(project, feature, position), indent=2))
        result = owi_plan.owi(repo, "allocate", "--index", index_db,
                              "--local", local_db, "--input", str(request_path))
        quote = result["quote"]

        candidates = []
        for c in quote["eligible_candidates"]:
            worker_id = c["worker_id"]
            candidates.append({
                "id": worker_id,
                "s": c["success_mean"],
                "lcb": c["success_lower_bound"],
                "p95": c["p95_latency_ms"],
                # Rate-independent cash, straight from the engine.
                "run": c["cost"]["run_cash_micros"],
                "retry": c["cost"]["expected_failure_cash_micros"],
                "flatReview": merged.get("expected_review_cash_micros", 0),
                "setup": onboarding.get(worker_id, default_onboarding),
            })

        turned_away = {}
        for rejected in quote["rejected_candidates"]:
            for reason in rejected["reasons"]:
                klass = owi_plan.CLASS_OF.get(reason["code"], "other")
                turned_away[klass] = turned_away.get(klass, 0) + 1

        tasks.append({
            "id": feature["id"],
            "summary": feature["summary"],
            "skill": feature["skill_id"],
            "tools": feature.get("required_tools", []),
            "tin": feature["estimated_input_tokens"],
            "tout": feature["estimated_output_tokens"],
            "accMin": merged.get("review_minutes_on_accept", 0.0),
            "rejMin": merged.get("review_minutes_on_reject", 0.0),
            "wage": merged.get("review_cash_micros_per_hour", 0),
            "candidates": candidates,
            "turnedAway": turned_away,
            "unstaffed": not candidates,
            # Workers rejected purely on tradeable axes (quality) hold the
            # capability; for them measurement, not onboarding, is the fix.
            "qualityOnly": sum(
                1 for rejected in quote["rejected_candidates"]
                if not any(owi_plan.CLASS_OF.get(r["code"]) in owi_plan.GATES
                           for r in rejected["reasons"])
            ) if not candidates else 0,
            "gates": sorted({
                owi_plan.CLASS_OF.get(reason["code"], "other")
                for rejected in quote["rejected_candidates"]
                for reason in rejected["reasons"]
                if owi_plan.CLASS_OF.get(reason["code"]) in owi_plan.GATES
            }) if not candidates else [],
        })

        # Measurement panel: capability-holding workers and how measured they
        # are on this skill, from the same calibration the decision used.
        skill = feature["skill_id"]
        for cal in result["calibration"]:
            if skill not in roster_skills.get(cal["worker_id"], set()):
                continue
            for s in cal["skills"]:
                if s["skill_id"] != skill:
                    continue
                measure_by_skill.setdefault(skill, {})[cal["worker_id"]] = {
                    "id": cal["worker_id"],
                    "measured": s["private_outcome_count"],
                    "assumed": s["public_observation_count"],
                    "onboard": onboarding.get(cal["worker_id"], default_onboarding),
                }

    measure = [{"skill": skill,
                "workers": sorted(workers.values(), key=lambda w: w["onboard"])}
               for skill, workers in sorted(measure_by_skill.items())]

    return {
        "project_id": project["project_id"],
        "snapshot_id": project["snapshot_id"],
        "rosterCount": len(roster_skills),
        "controls": {
            "rate": base_assumptions.get("opportunity_micros_per_hour", 0) // 1_000_000,
            "volume": base_assumptions.get("expected_task_volume", 0) or 50,
            "blocking": base_assumptions.get("blocking_fraction", 0.0),
        },
        "tasks": tasks,
        "measure": measure,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--owi-repo", required=True, type=Path)
    parser.add_argument("--project", required=True, type=Path)
    parser.add_argument("--seed", required=True, type=Path, action="append",
                        help="Roster seed; repeat for layered seeds.")
    parser.add_argument("--index", required=True)
    parser.add_argument("--local", required=True)
    parser.add_argument("--template", type=Path,
                        default=TOOLS_DIR / "owi_console_template.html")
    parser.add_argument("--work-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    data = build_console_data(
        arguments.owi_repo.resolve(),
        json.loads(arguments.project.read_text()),
        [json.loads(path.read_text()) for path in arguments.seed],
        arguments.index, arguments.local, arguments.work_dir)

    template = arguments.template.read_text()
    if "__DATA__" not in template:
        raise SystemExit("template is missing the __DATA__ placeholder")
    payload = json.dumps(data).replace("</", "<\\/")
    arguments.output.write_text(template.replace("__DATA__", payload))

    staffed = sum(1 for t in data["tasks"] if not t["unstaffed"])
    print(f"console: {staffed}/{len(data['tasks'])} tasks staffable, "
          f"{data['rosterCount']} on roster -> {arguments.output}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
