"""Carrying what the roster learned out of the machine it learned it on.

The private ledger is deliberately not in git. It is the *local* half of a
deliberately split design: the public index holds facts anybody could check,
and the ledger holds what happened on your machine, including task text. A
public repository is exactly the wrong place for it.

That separation is right, and it had a hole in it. A daily learning loop ran
for eleven benchmark rounds and roughly a hundred and twenty measured code
outcomes, all of it living only in `.owi-quick/local.sqlite` inside an
ephemeral container. When the container was reclaimed, every posterior the
roster had earned went with it and the roster silently reverted to the
`vendor_reported` assumptions it shipped with. Nothing errored. Nothing was
corrupted. The learning simply stopped existing, and the next decision was
made on assumptions while looking exactly like a decision made on evidence.

So this module exports what can safely leave: **redacted statistics**. Who
did what work, how often it was accepted, and why it was rejected — never the
task text, never the failure detail, never the checklist contents. That is
the same line the prevention memory already draws for confidential failures,
applied to the whole ledger rather than to one privacy level.

A summary is not the ledger. Restoring one does not resurrect individual
outcomes, and it is honest about that: every restored record says so, carries
the digest of the summary it came from, and names the export it belongs to.
An outcome you can trace to a machine that no longer exists is weaker
evidence than one you measured yourself, and the record says which it is.
"""

from __future__ import annotations

import hashlib
import json
import sqlite3
import time
from pathlib import Path

# What a restored outcome is called, so a posterior built partly from a
# restored summary can never be mistaken for one built from live measurement.
RESTORED_VALIDATION = "self_reported"
RESTORED_MARKER = "restored_from_summary"
SUMMARY_VERSION = "owi-ledger-summary@1"


def summarize(local: Path) -> dict:
    """Redacted per worker × skill statistics from a private ledger.

    Everything that could carry content is dropped at the SQL level rather
    than filtered afterwards, so a column added later cannot leak by default:
    the query names exactly the columns that may leave.
    """
    path = Path(local)
    if not path.exists():
        raise FileNotFoundError(f"no ledger at {path}")
    connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        rows = connection.execute(
            "SELECT worker_id, skill_id, accepted, "
            "       json_extract(metadata_json, '$.root_cause') AS root_cause, "
            "       observed_at "
            "FROM outcome_events ORDER BY observed_at"
        ).fetchall()
    finally:
        connection.close()

    buckets: dict[tuple[str, str], dict] = {}
    for worker_id, skill_id, accepted, root_cause, observed_at in rows:
        key = (worker_id, skill_id)
        bucket = buckets.setdefault(key, {
            "worker_id": worker_id, "skill_id": skill_id,
            "accepted": 0, "rejected_worker": 0, "excused": 0,
            "first_observed_at": observed_at, "last_observed_at": observed_at,
        })
        if accepted:
            bucket["accepted"] += 1
        elif (root_cause or "worker") == "worker":
            # Only a worker-caused rejection counts against the model. The
            # same rule the posterior applies, applied here, so a summary and
            # a live ledger produce the same numbers.
            bucket["rejected_worker"] += 1
        else:
            bucket["excused"] += 1
        if observed_at < bucket["first_observed_at"]:
            bucket["first_observed_at"] = observed_at
        if observed_at > bucket["last_observed_at"]:
            bucket["last_observed_at"] = observed_at

    statistics = sorted(buckets.values(),
                        key=lambda b: (b["worker_id"], b["skill_id"]))
    summary = {
        "version": SUMMARY_VERSION,
        "exported_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "source_outcome_count": len(rows),
        "statistics": statistics,
    }
    summary["content_sha256"] = hashlib.sha256(
        json.dumps(statistics, sort_keys=True,
                   separators=(",", ":")).encode()).hexdigest()
    return summary


def restore_records(summary: dict, observed_at: str | None = None) -> list[dict]:
    """One PrivateOutcomeRecord per accepted or worker-rejected outcome.

    Counts are expanded back into individual records because that is what the
    posterior reads; an aggregate would need a second, parallel code path in
    the engine, and two ways to compute the same number is how they drift.
    Excused rejections are NOT expanded — they never counted, so restoring
    them would invent evidence that the original ledger deliberately withheld.
    """
    digest = summary.get("content_sha256", "")
    stamp = observed_at or summary.get("exported_at") or time.strftime(
        "%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    records = []
    for bucket in summary.get("statistics", []):
        for accepted, count in ((True, bucket.get("accepted", 0)),
                                (False, bucket.get("rejected_worker", 0))):
            for index in range(count):
                verdict = "accepted" if accepted else "rejected"
                key = (f"{digest[:12]}:{bucket['worker_id']}:"
                       f"{bucket['skill_id']}:{verdict}:{index}")
                # The store's own shape: an event wrapped in a record. No
                # decision_id — a restored outcome answers no quote this
                # machine ever made, and claiming one would fail the link
                # check for the right reason.
                records.append({"event": {
                    "id": f"outcome:restored:{key}",
                    "task_id": f"task:restored:{key}",
                    "worker_id": bucket["worker_id"],
                    "skill_id": bucket["skill_id"],
                    "accepted": accepted,
                    # Never 'deterministic' or 'human': nobody re-ran this.
                    "validation_kind": RESTORED_VALIDATION,
                    "actual_cash_micros": 0,
                    "actual_quota_milliunits": 0,
                    "latency_ms": 0,
                    "observed_at": stamp,
                    "metadata": {
                        RESTORED_MARKER: True,
                        "summary_sha256": digest,
                        "summary_version": summary.get("version"),
                        "originally_observed_between": [
                            bucket.get("first_observed_at"),
                            bucket.get("last_observed_at"),
                        ],
                        **({} if accepted else {"root_cause": "worker"}),
                    },
                }})
    return records
