#!/usr/bin/env python3
"""CI check: the browser page decides exactly what the engine decides.

The ask page is a static file with no server, so it carries its own copy of
the decision rule in JavaScript — the Beta lower bound, the capability, tool
and clearance gates, and the expected-accepted-cost arithmetic. Two
implementations of a money path drift silently unless something holds them
together; this is that something.

It slices the marked `decision-math` block out of the built page, runs it in
plain node (no browser, no DOM, no storage), and asserts that for every skill
in the taxonomy it returns the same eligible workers, in the same order, at
the same micros as `owi allocate` on a pristine ledger.

Pristine is the honest baseline. The page ships public evidence only — your
private outcomes stay on your machine and never ride into a published file —
so the terminal, which has lived through real work, will rank differently and
should. What must never differ is the arithmetic.

The keyword classifier is checked the same way: the page and `owi-do` must
route identical wording to identical skills, or the page recommends a worker
for a job the terminal would have called something else.
"""

from __future__ import annotations

import argparse
import importlib.machinery
import importlib.util
import json
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TOOLS = REPO / "tools"

_loader = importlib.machinery.SourceFileLoader("owi_do", str(TOOLS / "owi-do"))
_spec = importlib.util.spec_from_loader("owi_do", _loader)
owi_do = importlib.util.module_from_spec(_spec)
_loader.exec_module(owi_do)

START = "// >>> decision-math >>>"
END = "// <<< decision-math <<<"

# Wording the page and the terminal must agree about. Each phrase is the kind
# of thing a person actually types, not a keyword drill.
PHRASES = [
    "Rewrite this paragraph and fix the grammar",
    "Extract the part numbers into json fields",
    "Implement the python function and add a test",
    "Model a parametric bracket and export step",
    "Write the roadmap and decompose it into milestones",
    "Debug the failing regression from this stack trace",
    "Design the landing page layout and css",
]


def owi(*args: str) -> dict:
    completed = subprocess.run(
        ["cargo", "run", "-q", "-p", "workforce-cli", "--", *args],
        cwd=REPO, capture_output=True, text=True)
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr[-1500:])
        raise SystemExit(f"owi {args[0]} failed")
    return json.loads(completed.stdout) if completed.stdout.strip() else {}


def extract_math(page: str) -> str:
    if START not in page or END not in page:
        raise SystemExit("the built page has no decision-math markers — the "
                         "template lost the block the check depends on")
    block = page.split(START, 1)[1].split(END, 1)[0]
    for forbidden in ("document.", "localStorage", "window."):
        if forbidden in block:
            raise SystemExit(
                f"the decision-math block reaches for {forbidden} — it must "
                "stay pure so it can be verified outside a browser")
    return block


def baked_data(page: str) -> dict:
    match = re.search(
        r'<script id="ask-data" type="application/json">(.*?)</script>',
        page, re.S)
    if not match:
        raise SystemExit("the built page has no baked roster payload")
    return json.loads(match.group(1).replace("<\\/", "</"))


def page_answers(page: str, skills: list[str], tokens_in: int,
                 tokens_out: int) -> dict:
    """Run the page's own arithmetic in node and return its ranking."""
    math = extract_math(page)
    keywords = re.search(r"const KEYWORDS = (\{.*?\n  \});", page, re.S)
    harness = f"""
const DATA = {json.dumps(baked_data(page))};
const KEYWORDS = {keywords.group(1) if keywords else "{}"};
const data = () => DATA;
const local = () => ({{}});
{math}
const skills = {json.dumps(skills)};
const phrases = {json.dumps(PHRASES)};
const out = {{ranking: {{}}, classified: {{}}}};
for (const s of skills) {{
  out.ranking[s] = evaluate(s, {tokens_in}, {tokens_out}, "private_metadata")
    .rows.map(r => [r.id, r.total]);
}}
for (const p of phrases) out.classified[p] = guess(p);
console.log(JSON.stringify(out));
"""
    with tempfile.NamedTemporaryFile("w", suffix=".js", delete=False) as handle:
        handle.write(harness)
        script = handle.name
    try:
        completed = subprocess.run(["node", script], capture_output=True,
                                   text=True)
    except FileNotFoundError:
        raise SystemExit("node is required: this check runs the page's own "
                         "JavaScript, so a Python reimplementation of it "
                         "would defeat the point")
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr[-1500:])
        raise SystemExit("the page's decision math failed to run in node")
    return json.loads(completed.stdout)


def engine_answers(home: Path, skills: list[str], tokens_in: int,
                   tokens_out: int) -> dict:
    """The same question asked of the Rust allocator, pristine ledger."""
    index, local = home / "index.sqlite", home / "local.sqlite"
    owi("prices", "--index", str(index),
        "--input", str(REPO / "examples/litellm-prices-sample.json"),
        "--options", str(REPO / "examples/price-import-options.json"))
    owi("seed", "--index", str(index),
        "--input", str(REPO / "examples/manager-scenario-seed.json"))
    owi("database", "init", "--index", str(index), "--local", str(local))

    answers = {}
    for skill in skills:
        request_path = home / "request.json"
        request_path.write_text(json.dumps({
            "decision_id": f"decision:page-check-{time.time_ns()}",
            "snapshot_id": "snapshot:manager-scenario-v1",
            "at_epoch_ms": owi_do.AT_EPOCH_MS,
            "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "task": {
                "id": f"task:page-check-{time.time_ns()}",
                "summary": f"page cross-check for {skill}",
                "required_skills": [{"skill_id": skill,
                                     "minimum_success_probability": 0.24,
                                     "minimum_evidence_count": 0}],
                "required_tools": owi_do.SKILLS[skill]["tools"],
                "privacy": "private_metadata", "risk": "low",
                "verification": "deterministic",
                "minimum_success_probability": 0.24,
                "minimum_evidence_count": 0,
                "estimated_input_tokens": tokens_in,
                "estimated_output_tokens": tokens_out},
            "policy": {"policy_id": "policy:economy-v1", "currency": "USD",
                       "quota_shadow_cash_micros_per_unit": 0,
                       "failure_probability_basis": "mean",
                       "max_attempts": 2},
            "calibration": {"calibration_id": "calibration:v1",
                            "confidence_tail_probability": 0.05,
                            "prior_alpha": 1.0, "prior_beta": 1.0,
                            "max_public_prior_weight": 8.0,
                            "private_outcome_weight": 1.0},
            "assumptions": {"default_p95_latency_ms": 30000,
                            "opportunity_micros_per_hour": 0,
                            "review_minutes_on_accept": 3.0,
                            "review_minutes_on_reject": 25.0,
                            "expected_fallback_cash_micros": 20000},
        }))
        quote = owi("allocate", "--index", str(index), "--local", str(local),
                    "--input", str(request_path))["quote"]
        answers[skill] = [[c["worker_id"],
                           c["cost"]["expected_accepted_cost_micros"]]
                          for c in quote["eligible_candidates"]]
    return answers


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--page", type=Path,
                        help="a built ask page; one is built if omitted")
    parser.add_argument("--tokens-in", type=int, default=2000)
    parser.add_argument("--tokens-out", type=int, default=800)
    arguments = parser.parse_args()

    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        page_path = arguments.page
        if page_path is None:
            page_path = home / "ask.html"
            build = subprocess.run(
                [sys.executable, str(TOOLS / "owi_ask.py"),
                 "--output", str(page_path)],
                cwd=REPO, capture_output=True, text=True)
            if build.returncode != 0:
                sys.stderr.write(build.stderr[-1500:])
                raise SystemExit("could not build the ask page")
        page = page_path.read_text(encoding="utf-8")

        skills = list(owi_do.SKILLS)
        page_out = page_answers(page, skills, arguments.tokens_in,
                                arguments.tokens_out)
        engine = engine_answers(home, skills, arguments.tokens_in,
                                arguments.tokens_out)

    failures = []
    for skill in skills:
        mine, theirs = page_out["ranking"].get(skill, []), engine[skill]
        if mine != theirs:
            failures.append(f"{skill}: page {mine[:3]} vs engine {theirs[:3]}")
        else:
            print(f"  [ok] {skill[6:]:<32} {len(theirs)} rows identical")

    for phrase, skill in page_out["classified"].items():
        mine = owi_do.classify(phrase)
        if mine != skill:
            failures.append(f"classify {phrase!r}: page {skill} vs cli {mine}")
    if not failures:
        print(f"  [ok] {'keyword classifier':<32} "
              f"{len(PHRASES)} phrases route identically")

    if failures:
        print("\nthe page and the engine disagree:", file=sys.stderr)
        for line in failures:
            print(f"  - {line}", file=sys.stderr)
        return 1
    print(f"\npage math verified: {len(skills)} skills, same eligible set, "
          f"same order, same micros as the engine")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
