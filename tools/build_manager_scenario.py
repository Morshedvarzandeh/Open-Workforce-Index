#!/usr/bin/env python3
"""Build a multi-task staffing scenario and allocate every task through OWI.

The point being demonstrated is right-sizing: an email rewrite must not be
staffed to the most capable worker on the roster, and a CAD part must not be
staffed to a cheap text worker that has no geometry toolchain. The first is
waste; the second is a wrong answer delivered confidently.

A worker is a model *plus* a harness, prompt pack, and toolset, so the same
model appears more than once with different declared capabilities. That is an
operator configuration choice, not a claim about the model.

Prices are real, imported from the published source. Ability evidence for the
commercial workers is NOT real: it is marked `vendor_reported`, which the
calibration discounts to a tenth of a reproduced observation, and every output
records which workers are measured and which are assumed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

EMPTY = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
DIGEST = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
AT_EPOCH_MS = 1_785_110_400_000

# Harness configurations. A model becomes a worker only through one of these.
HARNESSES = {
    "text": {
        "harness_id": "owi-text-agent",
        "skill_pack_version": "text-v1",
        "toolset_version": "none-v1",
        "skills": ["skill:text-editing"],
        "tools": [],
    },
    "extract": {
        "harness_id": "owi-extraction-agent",
        "skill_pack_version": "extract-v1",
        "toolset_version": "json-schema-v1",
        "skills": ["skill:structured-extraction"],
        "tools": ["json-schema-validator"],
    },
    "code": {
        "harness_id": "owi-code-agent",
        "skill_pack_version": "code-v1",
        "toolset_version": "shell-v1",
        "skills": ["skill:python-numerical-implementation"],
        "tools": ["shell"],
    },
    "cad": {
        "harness_id": "owi-cad-agent",
        "skill_pack_version": "cad-v1",
        "toolset_version": "cad-kernel-v1",
        "skills": ["skill:parametric-cad"],
        "tools": ["cad-kernel", "geometry-validator", "step-exporter"],
    },
}

# Which model is configured into which harness. Deliberately uneven: not every
# model is wired for CAD, which is what makes the capability gate bite.
ROSTER = [
    ("gpt-5-mini", "offering:openai/gpt-5-mini", "model:gpt-5-mini", ["text", "extract"]),
    ("haiku-4-5", "offering:anthropic/claude-haiku-4-5", "model:claude-haiku-4-5",
     ["text", "extract", "code"]),
    ("gpt-5", "offering:openai/gpt-5", "model:gpt-5", ["text", "extract", "code"]),
    ("sonnet-4-5", "offering:anthropic/claude-sonnet-4-5", "model:claude-sonnet-4-5",
     ["text", "extract", "code", "cad"]),
    ("opus-4-5", "offering:anthropic/claude-opus-4-5", "model:claude-opus-4-5",
     ["text", "extract", "code", "cad"]),
]

# Assumed pass rates, by harness and rough model tier. NOT measured.
ASSUMED = {
    "text":    {"gpt-5-mini": 0.93, "haiku-4-5": 0.95, "gpt-5": 0.96, "sonnet-4-5": 0.97, "opus-4-5": 0.97},
    "extract": {"gpt-5-mini": 0.88, "haiku-4-5": 0.90, "gpt-5": 0.94, "sonnet-4-5": 0.95, "opus-4-5": 0.96},
    "code":    {"haiku-4-5": 0.62, "gpt-5": 0.80, "sonnet-4-5": 0.86, "opus-4-5": 0.89},
    "cad":     {"sonnet-4-5": 0.68, "opus-4-5": 0.78},
}

# The work to staff. Token estimates are order-of-magnitude, and the retry cost
# is what a failure actually costs: redo plus escalation.
#
# The quality floor is the last field, and it is deliberately low. Vendor-
# reported evidence is discounted to a tenth of a reproduced observation, so
# even a claimed 0.93 pass rate only supports a 95% lower bound near 0.33. The
# floor a manager can demand is bounded by the quality of the evidence they
# hold, not by how confident they would like to feel. Measured local runs raise
# it; a press release does not.
TASKS = [
    ("task:rewrite-customer-email", "Rewrite a customer email for tone and clarity",
     "skill:text-editing", [], 2_000, 800, 6_000, "low", 0.30),
    ("task:extract-invoice-fields", "Extract structured fields from a supplier invoice",
     "skill:structured-extraction", ["json-schema-validator"], 4_000, 1_200, 12_000, "low", 0.30),
    ("task:summarize-meeting-notes", "Summarize meeting notes into action items",
     "skill:text-editing", [], 6_000, 1_000, 8_000, "low", 0.30),
    ("task:implement-capacity-function", "Implement a documented battery_core function",
     "skill:python-numerical-implementation", ["shell"], 20_000, 4_000, 150_000, "medium", 0.24),
    ("task:parametric-bracket", "Generate a parametric mounting bracket and export STEP",
     "skill:parametric-cad", ["cad-kernel", "geometry-validator", "step-exporter"],
     30_000, 8_000, 400_000, "medium", 0.20),
]


def configuration_sha256(release: str, offering: str, provider: str, harness: dict) -> str:
    parts = [
        release, offering, provider,
        harness["harness_id"], "1.0.0", "standard", EMPTY,
        harness["skill_pack_version"], harness["toolset_version"], EMPTY,
    ]
    return hashlib.sha256("".join(f"{len(p)}:{p}" for p in parts).encode()).hexdigest()


def provider_of(offering_id: str) -> str:
    return offering_id.split(":", 1)[1].split("/", 1)[0]


def build_seed() -> dict:
    workers, evidence = [], []
    for model_name, offering_id, release_id, harness_keys in ROSTER:
        provider = provider_of(offering_id)
        for key in harness_keys:
            harness = HARNESSES[key]
            worker_id = f"worker:{model_name}/{key}"
            workers.append({
                "id": worker_id,
                "offering_id": offering_id,
                "harness_id": harness["harness_id"],
                "harness_version": "1.0.0",
                "reasoning_configuration": "standard",
                "system_prompt_sha256": EMPTY,
                "skill_pack_version": harness["skill_pack_version"],
                "toolset_version": harness["toolset_version"],
                "execution_policy_sha256": EMPTY,
                "supported_skill_ids": harness["skills"],
                "tools": harness["tools"],
                "privacy_clearance": "private_metadata",
                "configuration_sha256": configuration_sha256(
                    release_id, offering_id, provider, harness),
                "recorded_at": "2026-08-07T18:00:00Z",
            })
            score = ASSUMED.get(key, {}).get(model_name)
            if score is None:
                continue
            evidence.append({
                "id": f"evidence:assumed:{model_name}:{key}",
                "model_release_id": release_id,
                "worker_id": worker_id,
                "skill_id": harness["skills"][0],
                "benchmark_id": "benchmark:assumed-for-demonstration",
                "evidence_tier": "vendor_reported",
                "raw_score": score * 100.0,
                "metric": "pass_rate",
                "unit": "percent",
                "normalized_score": score,
                "adapter_version": "assumed@0",
                "sample_count": 20,
                "observed_at": "2026-08-07T18:00:00Z",
                "source_url": "https://example.invalid/assumed-not-measured",
                "artifact_sha256": DIGEST,
                "license": "CC0-1.0",
            })
    return {
        "_comment": [
            "ASSUMED ABILITY, NOT MEASURED. Every evidence row here is tagged",
            "vendor_reported so the calibration discounts it to a tenth of a",
            "reproduced observation, and benchmark:assumed-for-demonstration",
            "makes it greppable. It exists so the staffing board has something",
            "to rank. Replace it with real runs from tools/run_bench.py before",
            "treating any assignment here as a recommendation.",
            "",
            "Prices and context windows ARE real, imported by `owi prices`.",
        ],
        "snapshot_id": "snapshot:manager-scenario-v1",
        "created_at": "2026-08-07T18:00:00Z",
        "ontology_version": "ontology:v1",
        "source_revision": "scenario:manager-v1",
        "worker_profiles": workers,
        "evidence": evidence,
    }


def allocation_request(task: tuple, index: int) -> dict:
    task_id, summary, skill, tools, tokens_in, tokens_out, fallback, risk, floor = task
    return {
        "decision_id": f"decision:manager-{index:04d}",
        "snapshot_id": "snapshot:manager-scenario-v1",
        "at_epoch_ms": AT_EPOCH_MS,
        "created_at": "2026-08-07T18:00:00Z",
        "task": {
            "id": task_id,
            "summary": summary,
            "required_skills": [{
                "skill_id": skill,
                "minimum_success_probability": floor,
                "minimum_evidence_count": 0,
            }],
            "required_tools": tools,
            "privacy": "private_metadata",
            "risk": risk,
            "verification": "deterministic",
            "minimum_success_probability": floor,
            "minimum_evidence_count": 0,
            "estimated_input_tokens": tokens_in,
            "estimated_output_tokens": tokens_out,
        },
        "policy": {
            "policy_id": "policy:economy-v1",
            "currency": "USD",
            "quota_shadow_cash_micros_per_unit": 0,
            "failure_probability_basis": "mean",
            "max_attempts": 2,
        },
        "calibration": {
            "calibration_id": "calibration:v1",
            "confidence_tail_probability": 0.05,
            "prior_alpha": 1.0,
            "prior_beta": 1.0,
            "max_public_prior_weight": 8.0,
            "private_outcome_weight": 1.0,
        },
        "assumptions": {
            "expected_fallback_cash_micros": fallback,
            "default_p95_latency_ms": 30_000,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, type=Path, help="OWI checkout")
    parser.add_argument("--index", required=True, type=Path)
    parser.add_argument("--local", required=True, type=Path)
    parser.add_argument("--work-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    arguments.work_dir.mkdir(parents=True, exist_ok=True)
    seed_path = arguments.work_dir / "manager-seed.json"
    seed_path.write_text(json.dumps(build_seed(), indent=2) + "\n")

    def owi(*args: str) -> dict:
        completed = subprocess.run(
            ["cargo", "run", "-q", "-p", "workforce-cli", "--", *args],
            cwd=arguments.repo, capture_output=True, text=True,
        )
        if completed.returncode != 0:
            print(completed.stderr[-2000:], file=sys.stderr)
            raise SystemExit(f"owi {' '.join(args)} failed")
        return json.loads(completed.stdout)

    owi("seed", "--index", str(arguments.index), "--input", str(seed_path))

    results = []
    for index, task in enumerate(TASKS, start=1):
        request_path = arguments.work_dir / f"request-{index:04d}.json"
        request_path.write_text(json.dumps(allocation_request(task, index), indent=2))
        result = owi("allocate", "--index", str(arguments.index),
                     "--local", str(arguments.local), "--input", str(request_path))
        results.append({
            "task_id": task[0],
            "summary": task[1],
            "skill_id": task[2],
            "required_tools": task[3],
            "estimated_input_tokens": task[4],
            "estimated_output_tokens": task[5],
            "minimum_success_probability": task[8],
            "quote": result["quote"],
            "calibration": result["calibration"],
        })

    payload = {
        "scenario_id": "scenario:manager-v1",
        "generated_from": "tools/build_manager_scenario.py",
        "prices_are_real": True,
        "ability_is_measured": False,
        "results": results,
    }
    arguments.output.write_text(json.dumps(payload, indent=2) + "\n")
    print(f"allocated {len(results)} tasks -> {arguments.output}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
