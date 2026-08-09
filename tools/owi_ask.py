#!/usr/bin/env python3
"""Build the ask page: one input box backed by the real decision math.

The page is a single self-contained HTML file. It bakes in the roster, real
prices, and each worker's per-skill Beta posterior harvested from actual
`owi allocate` runs against a pristine ledger, then reproduces the simple-mode
arithmetic in the browser: keyword skill guess, capability and tool gates, the
exact Beta lower-bound quality floor, and expected accepted-result cost.

Saying "worked" or "didn't" updates the posterior in the browser's own
storage — the same update rule the allocator applies, but deliberately local:
the page is the front door, and the terminal (`owi-do`) remains the path that
records to the real private ledger.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SKILLS = {
    "skill:text-editing": [],
    "skill:structured-extraction": ["json-schema-validator"],
    "skill:python-numerical-implementation": ["shell"],
    "skill:parametric-cad": ["cad-kernel", "geometry-validator", "step-exporter"],
}


def owi(*args: str) -> dict:
    completed = subprocess.run(
        ["cargo", "run", "-q", "-p", "workforce-cli", "--", *args],
        cwd=REPO, capture_output=True, text=True)
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr[-1500:])
        raise SystemExit(f"owi {args[0]} failed")
    return json.loads(completed.stdout)


def harvest() -> dict:
    """Posteriors and prices from the engine itself, on a pristine ledger."""
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        index, local = home / "index.sqlite", home / "local.sqlite"
        owi("prices", "--index", str(index),
            "--input", str(REPO / "examples/litellm-prices-sample.json"),
            "--options", str(REPO / "examples/price-import-options.json"))
        owi("seed", "--index", str(index),
            "--input", str(REPO / "examples/manager-scenario-seed.json"))
        owi("database", "init", "--index", str(index), "--local", str(local))

        prices = owi("prices", "--input",
                     str(REPO / "examples/litellm-prices-sample.json"),
                     "--options",
                     str(REPO / "examples/price-import-options.json"),
                     "--dry-run")
        rates = {o["id"]: (o["input_micros_per_million_tokens"],
                           o["output_micros_per_million_tokens"])
                 for o in prices["offerings"]}

        seed = json.loads(
            (REPO / "examples/manager-scenario-seed.json").read_text())
        workers = []
        for w in seed["worker_profiles"]:
            rate = rates.get(w["offering_id"])
            if rate is None:
                continue
            workers.append({
                "id": w["id"],
                "skills": w["supported_skill_ids"],
                "tools": w["tools"],
                "clearance": w.get("privacy_clearance", "private_metadata"),
                "inRate": rate[0], "outRate": rate[1],
            })

        posteriors: dict = {}
        for position, (skill, tools) in enumerate(SKILLS.items(), start=1):
            request = {
                "decision_id": f"decision:ask-harvest-{position}",
                "snapshot_id": "snapshot:manager-scenario-v1",
                "at_epoch_ms": 1_785_500_000_000,
                "created_at": "2026-08-07T18:00:00Z",
                "task": {"id": f"task:ask-harvest-{position}",
                         "summary": f"harvest posteriors for {skill}",
                         "required_skills": [{"skill_id": skill,
                                              "minimum_success_probability": 0.0,
                                              "minimum_evidence_count": 0}],
                         "required_tools": tools,
                         "privacy": "private_metadata", "risk": "low",
                         "verification": "deterministic",
                         "minimum_success_probability": 0.0,
                         "minimum_evidence_count": 0,
                         "estimated_input_tokens": 2000,
                         "estimated_output_tokens": 800},
                "policy": {"policy_id": "policy:economy-v1", "currency": "USD",
                           "quota_shadow_cash_micros_per_unit": 0,
                           "failure_probability_basis": "mean",
                           "max_attempts": 2},
                "calibration": {"calibration_id": "calibration:v1",
                                "confidence_tail_probability": 0.05,
                                "prior_alpha": 1.0, "prior_beta": 1.0,
                                "max_public_prior_weight": 8.0,
                                "private_outcome_weight": 1.0},
                "assumptions": {"default_p95_latency_ms": 30000},
            }
            request_path = home / f"harvest-{position}.json"
            request_path.write_text(json.dumps(request))
            result = owi("allocate", "--index", str(index),
                         "--local", str(local), "--input", str(request_path))
            posteriors[skill] = {
                cal["worker_id"]: {
                    "a": cal["skills"][0]["posterior"]["alpha"],
                    "b": cal["skills"][0]["posterior"]["beta"],
                }
                for cal in result["calibration"] if cal["skills"]
            }

        return {"workers": workers, "posteriors": posteriors,
                "skillTools": SKILLS,
                "fallbackMicros": 20000, "floor": 0.24}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--template", type=Path,
                        default=REPO / "tools/owi_ask_template.html")
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    data = harvest()
    template = arguments.template.read_text()
    if "__DATA__" not in template:
        raise SystemExit("template is missing the __DATA__ placeholder")
    payload = json.dumps(data).replace("</", "<\\/")
    import time
    built = time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime())
    arguments.output.write_text(
        template.replace("__DATA__", payload).replace("__BUILT__", built))
    print(f"ask page: {len(data['workers'])} workers, "
          f"{len(data['posteriors'])} skills -> {arguments.output}",
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
