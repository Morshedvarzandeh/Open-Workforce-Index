"""Reading your own ledger back to you.

The router answers one question well: who should do this job. It has never
answered the other one — whether the way you are using it is getting you what
you think. Those are different questions and the second is only answerable
from history.

Everything here is computed from the private ledger and the public index on
this machine. Nothing is fetched, nothing is sent. That is not a courtesy: the
ledger holds task text, which is exactly why it is gitignored, and a coach
that phoned home would defeat the separation the whole design rests on.

The findings are deliberately uncomfortable. A report that congratulates you
is not worth running twice.
"""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path

# Skills a report covers, with the token shape each one is quoted at. These
# are the three the daily loop exercises; a job of a different shape gets a
# different ranking, and pretending otherwise would be the same estimate
# problem the receipts already exposed.
REPORT_SKILLS = [
    ("skill:python-numerical-implementation", "implement a function", 4000, 1000),
    ("skill:text-editing", "rewrite a document", 1500, 800),
    ("skill:structured-extraction", "pull out structured data", 2000, 800),
]


def _counted(accepted: int, metadata: str | None) -> bool:
    """Whether an outcome counts toward a worker's record.

    The same rule the posterior applies: an accept counts, and a rejection
    counts only when the worker caused it. A timeout, an empty reply, a
    missing runner or a task whose input was never supplied are excluded, so
    plumbing is never charged to a model.
    """
    if accepted:
        return True
    try:
        cause = (json.loads(metadata or "{}") or {}).get("root_cause")
    except json.JSONDecodeError:
        cause = None
    return (cause or "worker") == "worker"


def coverage(index: Path, ledger: Path) -> dict:
    """How much of the roster you have any real evidence about.

    Counted in worker-and-skill pairs rather than workers, because a model
    measured on extraction tells you nothing about how it writes code.
    """
    pairs = set()
    with sqlite3.connect(f"file:{index}?mode=ro", uri=True) as index_db:
        for worker, skills_json in index_db.execute(
                "SELECT id, supported_skill_ids_json FROM worker_profiles"):
            for skill in json.loads(skills_json):
                pairs.add((worker, skill))
    with sqlite3.connect(f"file:{ledger}?mode=ro", uri=True) as ledger_db:
        measured = {
            pair for pair in ledger_db.execute(
                "SELECT DISTINCT worker_id, skill_id FROM outcome_events")
            # Calibration runs prove the harness, not a worker.
            if "calibration" not in pair[0]
        }
    return {"measured": len(measured), "total": len(pairs),
            "pairs": sorted(measured)}


def spend(ledger: Path) -> dict:
    """What the router quoted against what you were actually billed.

    Only calls whose runner reported a receipt appear here. A runner that
    says nothing leaves the cost unknown, and an unknown cost must never be
    averaged in as zero.
    """
    with sqlite3.connect(f"file:{ledger}?mode=ro", uri=True) as connection:
        rows = connection.execute(
            "SELECT accepted, actual_cash_micros, metadata_json "
            "FROM outcome_events WHERE actual_cash_micros > 0").fetchall()
    billed = sum(row[1] for row in rows)
    quoted = 0
    for _, _, metadata in rows:
        try:
            quoted += int((json.loads(metadata or "{}") or {})
                          .get("quoted_micros", 0))
        except (json.JSONDecodeError, TypeError, ValueError):
            pass
    return {
        "calls": len(rows),
        "quoted": quoted,
        "billed": billed,
        "ratio": round(billed / quoted, 2) if quoted else None,
        "rejected": sum(row[1] for row in rows if not row[0]),
    }


def escalations(ledger: Path) -> dict:
    """What a retry actually cost.

    An escalation is a rejected attempt followed immediately by a different
    worker on the same run. Cheap-first is right on average and wrong per
    job; this is the size of being wrong.
    """
    with sqlite3.connect(f"file:{ledger}?mode=ro", uri=True) as connection:
        rows = connection.execute(
            "SELECT worker_id, accepted, actual_cash_micros, skill_id "
            "FROM outcome_events WHERE actual_cash_micros > 0 "
            "ORDER BY rowid").fetchall()
    cases, previous = [], None
    for worker, accepted, cash, skill in rows:
        if (previous and not previous[1] and worker != previous[0]
                and skill == previous[3]):
            cases.append({"from": previous[0], "from_micros": previous[2],
                          "to": worker, "to_micros": cash})
        previous = (worker, accepted, cash, skill)
    spent = sum(case["from_micros"] + case["to_micros"] for case in cases)
    needed = sum(case["to_micros"] for case in cases)
    return {"count": len(cases), "spent": spent, "needed": needed,
            "cases": cases}


def record(ledger: Path, worker: str, skill: str) -> tuple[int, int]:
    """(accepted, counted) for one worker on one skill."""
    with sqlite3.connect(f"file:{ledger}?mode=ro", uri=True) as connection:
        rows = connection.execute(
            "SELECT accepted, metadata_json FROM outcome_events "
            "WHERE worker_id = ? AND skill_id = ?", (worker, skill)).fetchall()
    counted = [row for row in rows if _counted(row[0], row[1])]
    return sum(1 for row in counted if row[0]), len(counted)


def routes(home: Path, owi_do, ledger: Path) -> list[dict]:
    """Where each kind of work actually goes, and what evidence stands behind it.

    The comparison that matters is not first against second on price. It is
    the worker the router *takes* against the worker you know most about,
    because those are routinely different and the difference is usually
    fractions of a cent.
    """
    report = []
    for skill, plain, tokens_in, tokens_out in REPORT_SKILLS:
        result = owi_do.pick(home, "coach probe", skill, tokens_in,
                             tokens_out, 0)
        eligible = result["quote"]["eligible_candidates"]
        if not eligible:
            continue
        pick = eligible[0]
        best = max(eligible, key=lambda c: c["success_lower_bound"])
        pick_accepted, pick_n = record(ledger, pick["worker_id"], skill)
        best_accepted, best_n = record(ledger, best["worker_id"], skill)
        model = owi_do.model_of(pick)
        report.append({
            "skill": skill, "plain": plain,
            "pick": pick["worker_id"].replace("worker:", ""),
            "pick_n": pick_n, "pick_accepted": pick_accepted,
            "pick_lcb": round(pick["success_lower_bound"], 3),
            "pick_micros": pick["cost"]["expected_accepted_cost_micros"],
            # A pick you cannot execute is not an answer, and the engine
            # ranks the whole roster whether or not you have a runner.
            "runnable": owi_do.resolve_runner(home, model) is not None,
            "best": best["worker_id"].replace("worker:", ""),
            "best_n": best_n, "best_accepted": best_accepted,
            "best_lcb": round(best["success_lower_bound"], 3),
            "delta": (best["cost"]["expected_accepted_cost_micros"]
                      - pick["cost"]["expected_accepted_cost_micros"]),
            "eligible": len(eligible),
        })
    return report


def prevention_count(home: Path) -> int:
    path = Path(home) / "root-causes.jsonl"
    if not path.exists():
        return 0
    return sum(1 for line in path.read_text().splitlines() if line.strip())


def report(home: Path, owi_do) -> dict:
    home = Path(home)
    index, ledger = home / "index.sqlite", home / "local.sqlite"
    if not ledger.exists():
        raise FileNotFoundError(f"no ledger at {ledger} — run tools/owi-do first")
    return {
        "home": str(home),
        "coverage": coverage(index, ledger),
        "spend": spend(ledger),
        "escalations": escalations(ledger),
        "routes": routes(home, owi_do, ledger),
        "prevention": prevention_count(home),
    }


# --- findings ---------------------------------------------------------------
#
# A number is not a finding. A finding says what it costs and what to do, and
# earns its severity from the ledger rather than from a threshold somebody
# felt was about right.

def findings(data: dict) -> list[dict]:
    found = []
    routed = data["routes"]

    untested = [r for r in routed if r["pick_n"] == 0]
    if untested:
        unrunnable = [r for r in untested if not r["runnable"]]
        cheapest_fix = min(
            (r for r in untested if r["best_n"] > 0), default=None,
            key=lambda r: r["delta"])
        found.append({
            "severity": "bad",
            "tag": "default routes",
            "headline": f"{len(untested)} of {len(routed)} default routes go to "
                        f"a worker you have never tested",
            "detail": "The router takes the cheapest worker that clears the "
                      "gate, and cheapest keeps landing on a rate nobody has "
                      "checked."
                      + (f" {len(unrunnable)} of them have no runner on this "
                         f"machine, so the headline answer is one you could "
                         f"not execute even if you trusted it."
                         if unrunnable else ""),
            "action": (
                f"For '{cheapest_fix['plain']}', {cheapest_fix['best']} costs "
                f"${cheapest_fix['delta'] / 1e6:.4f} more and is known to work "
                f"{cheapest_fix['best_accepted']} of {cheapest_fix['best_n']} "
                f"times." if cheapest_fix else
                "Measure them, or route past them."),
            "rows": untested,
        })

    escalated = data["escalations"]
    if escalated["count"]:
        overhead = escalated["spent"] - escalated["needed"]
        found.append({
            "severity": "warn",
            "tag": "escalation",
            "headline": f"Retries cost "
                        f"{100 * overhead / escalated['needed']:.0f}% on top",
            "detail": f"{escalated['count']} job(s) were rejected and re-run on "
                      f"another worker, costing ${escalated['spent'] / 1e6:.4f} "
                      f"where starting there would have cost "
                      f"${escalated['needed'] / 1e6:.4f}.",
            "action": "Cheap-first is right on average and wrong per job. "
                      "Where a worker has already failed a kind of work, "
                      "start one rung up.",
            "rows": escalated["cases"],
        })

    paid = data["spend"]
    if paid["ratio"] and paid["ratio"] > 1.2:
        found.append({
            "severity": "warn",
            "tag": "estimates",
            "headline": f"You are billed {paid['ratio']}x what the router quotes",
            "detail": f"Across {paid['calls']} receipted calls: quoted "
                      f"${paid['quoted'] / 1e6:.4f}, billed "
                      f"${paid['billed'] / 1e6:.4f}. The task's own tokens are "
                      f"estimated about right; the harness context re-read on "
                      f"every call is not modelled at all.",
            "action": "Read the ratio, not the quote. It scales with model "
                      "price, so routing cheap saves more than the quote says.",
            "rows": [],
        })
    elif paid["calls"] == 0:
        found.append({
            "severity": "warn",
            "tag": "estimates",
            "headline": "No call has ever reported what it cost",
            "detail": "Every outcome records a cost of zero, which means the "
                      "quotes have never been checked against a bill.",
            "action": "Point a runner at tools/owi-claude-runner so receipts "
                      "come back.",
            "rows": [],
        })

    covered = data["coverage"]
    share = covered["measured"] / covered["total"] if covered["total"] else 0
    if share < 0.5:
        found.append({
            "severity": "warn" if share > 0.1 else "bad",
            "tag": "coverage",
            "headline": f"{share:.0%} of the roster has ever been tested",
            "detail": f"{covered['measured']} of {covered['total']} "
                      f"worker-and-skill pairs have a real result behind them. "
                      f"The rest is the guess the roster shipped with.",
            "action": "Measure the workers you actually reach for first; the "
                      "long tail you never route to can stay unmeasured.",
            "rows": [],
        })

    found.append({
        "severity": "good",
        "tag": "blame",
        "headline": "Nothing was charged to a worker that never got a fair run",
        "detail": "Timeouts, empty output, a missing runner and a task whose "
                  "input was never supplied are recorded against the "
                  "environment or the task spec, never the model.",
        "action": "No action. This is the part holding the rest up.",
        "rows": [],
    })
    return found
