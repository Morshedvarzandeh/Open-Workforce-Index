"""Private billing declarations, runner telemetry, and bounded agent learning.

No network access, credential changes, code downloads, or model calls occur
here. Provider allowance is a dated user declaration, never inferred from a
token count. Learning selects only reviewed instructions from RULES.
"""
from __future__ import annotations

from contextlib import contextmanager
from contextvars import ContextVar
from decimal import Decimal, InvalidOperation, ROUND_CEILING
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import time
import uuid

CLAUDE_MODELS = ("haiku-4-5", "sonnet-4-5", "opus-4-5", "sonnet-5", "opus-5")
RULES = {
    "json": "When JSON is requested, verify that the final result is one valid JSON value.",
    "contains": "Check the completed answer against every explicitly required phrase.",
    "regex": "Review the requested output format against each supplied pattern.",
    "min-words": "Check that the completed answer meets the requested minimum word count.",
    "python": "Review generated Python for syntax errors before returning it.",
}
CATALOG_SHA256 = hashlib.sha256(json.dumps(RULES, sort_keys=True).encode()).hexdigest()
OPERATION = ContextVar("owi_operation", default=None)
RECENT = ContextVar("owi_recent_runs", default=None)
ALLOWANCE_MAX_AGE = 3600


@contextmanager
def database(home: Path):
    home.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(home / "runtime.sqlite", timeout=30)
    connection.row_factory = sqlite3.Row
    connection.executescript("""
      CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY, body TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS agents (model TEXT PRIMARY KEY, body TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS runs (
        id TEXT PRIMARY KEY, operation TEXT NOT NULL, model TEXT NOT NULL,
        role TEXT NOT NULL, revision INTEGER, rules TEXT NOT NULL,
        started REAL NOT NULL, finished REAL, exit_code INTEGER,
        billing TEXT NOT NULL, usage TEXT, checked INTEGER NOT NULL DEFAULT 0);
      CREATE INDEX IF NOT EXISTS runs_operation ON runs(operation);
      CREATE TABLE IF NOT EXISTS updates (
        id INTEGER PRIMARY KEY, model TEXT NOT NULL, at REAL NOT NULL,
        action TEXT NOT NULL, revision INTEGER NOT NULL);
    """)
    try:
        yield connection
        connection.commit()
    except BaseException:
        connection.rollback()
        raise
    finally:
        connection.close()


def _settings(db) -> dict:
    row = db.execute("SELECT body FROM settings WHERE id=1").fetchone()
    return json.loads(row[0]) if row else {"billing": {}, "learning": True, "formats": {}}


def settings(home: Path) -> dict:
    with database(home) as db:
        return _settings(db)


def save_settings(home: Path, value: dict, now: float | None = None) -> dict:
    now = time.time() if now is None else now
    if not isinstance(value, dict) or set(value) - {"billing", "learning", "formats"}:
        raise ValueError("Unknown runtime setting")
    if not isinstance(value.get("learning", True), bool):
        raise ValueError("Learning must be on or off")
    billing = value.get("billing", {})
    if not isinstance(billing, dict) or len(billing) > 100:
        raise ValueError("Billing must be a model-to-plan mapping")
    clean = {}
    for model, profile in billing.items():
        if not re.fullmatch(r"[A-Za-z0-9._-]{1,100}", model) or not isinstance(profile, dict):
            raise ValueError("Invalid model or billing profile")
        if set(profile) - {"mode", "plan", "remaining_percent", "reset_at",
                           "verified_at", "quota_value_micros"}:
            raise ValueError("Unknown billing field")
        mode = profile.get("mode", "unknown")
        if mode not in ("unknown", "api", "subscription", "local"):
            raise ValueError("Choose unknown, API, subscription, or local billing")
        remaining = profile.get("remaining_percent")
        if remaining is not None and (type(remaining) is not int or not 0 <= remaining <= 100):
            raise ValueError("Remaining allowance must be a whole percent from 0 to 100")
        reset = profile.get("reset_at")
        if reset is not None and (type(reset) not in (int, float)
                                  or not 0 < reset <= now + 32 * 86400):
            raise ValueError("Reset time must be a valid timestamp, at most 32 days ahead")
        verified = profile.get("verified_at", now)
        if type(verified) not in (int, float) or not 0 <= verified <= now:
            raise ValueError("Allowance timestamp cannot be in the future")
        quota = profile.get("quota_value_micros", 0)
        if type(quota) is not int or not 0 <= quota <= 1_000_000_000:
            raise ValueError("Quota value must be nonnegative integer currency micros")
        clean[model] = {"mode": mode, "plan": str(profile.get("plan", ""))[:60],
                        "remaining_percent": remaining, "reset_at": reset,
                        "verified_at": verified, "quota_value_micros": quota}
    formats = value.get("formats", {})
    if not isinstance(formats, dict) or any(
        not re.fullmatch(r"[A-Za-z0-9._-]{1,100}", m)
        or f not in ("text", "owi-json", "claude-json") for m, f in formats.items()
    ):
        raise ValueError("Invalid runner telemetry format")
    result = {"billing": clean, "learning": value.get("learning", True), "formats": formats}
    with database(home) as db:
        db.execute("INSERT OR REPLACE INTO settings VALUES(1,?)", (json.dumps(result),))
    return result


def billing_for(profile: dict | None, now: float | None = None, environ=None) -> dict:
    profile = profile or {}
    now = time.time() if now is None else now
    env = os.environ if environ is None else environ
    mode = profile.get("mode", "unknown")
    result = {**profile, "mode": mode, "eligible": True,
              "label": "API cost estimate", "reason": "", "expires_at": None}
    if mode == "subscription":
        reset = profile.get("reset_at")
        expiry = min(profile.get("verified_at", 0) + ALLOWANCE_MAX_AGE, reset or 0)
        result["expires_at"] = expiry
        if any(env.get(k) for k in ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN",
                                    "ANTHROPIC_BASE_URL", "CLAUDE_CODE_USE_BEDROCK",
                                    "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY")):
            result.update(eligible=False, reason="API/provider environment conflicts with subscription billing")
        elif not profile.get("remaining_percent"):
            result.update(eligible=False, reason="Subscription allowance is unknown or exhausted")
        elif now >= expiry:
            result.update(eligible=False, reason="Refresh the subscription allowance and reset time")
        result["label"] = "Included allowance (declared)" if result["eligible"] else result["reason"]
    elif mode == "local":
        result["label"] = "Local compute; running cost not measured"
    elif mode == "unknown":
        result["label"] = "API equivalent; billing method unknown"
    return result


def snapshot(home: Path, models=()) -> dict:
    current = settings(home)
    all_models = set(models) | set(current["billing"])
    with database(home) as db:
        agents = {r["model"]: json.loads(r["body"])
                  for r in db.execute("SELECT model,body FROM agents")}
        updates = [dict(r) for r in db.execute(
            "SELECT model,at,action,revision FROM updates ORDER BY id DESC LIMIT 20")]
    return {"settings": current,
            "billing": {m: billing_for(current["billing"].get(m)) for m in sorted(all_models)},
            "agents": {m: {"active": a["active"], "stable": a["stable"],
                            "revisions": a["revisions"]} for m, a in agents.items()},
            "updates": updates}


def model_billing(home: Path, model: str) -> dict:
    return billing_for(settings(home)["billing"].get(model))


def allowed(home: Path, model: str) -> bool:
    return model_billing(home, model)["eligible"]


def route_candidates(home: Path, candidates: list) -> list:
    """Apply an account policy only after the engine's hard eligibility gates."""
    current = settings(home)
    ranked = []
    for original in candidates:
        candidate = json.loads(json.dumps(original))
        model = candidate["worker_id"].split(":")[1].split("/")[0]
        billing = billing_for(current["billing"].get(model))
        if not billing["eligible"]:
            continue
        cost = candidate["cost"]
        cost["api_equivalent_accepted_micros"] = cost["expected_accepted_cost_micros"]
        if billing["mode"] == "subscription":
            # Review/time costs remain costs. Subscription maker retries consume
            # allowance; no paid fallback is implicitly purchased.
            cost["expected_accepted_cost_micros"] = (
                cost.get("review_cash_micros", 0) + billing.get("quota_value_micros", 0))
        candidate["billing"] = billing
        ranked.append(candidate)
    return sorted(ranked, key=lambda c: (
        c["cost"]["expected_accepted_cost_micros"],
        -c.get("success_lower_bound", 0), c["worker_id"]))


def start_operation() -> str:
    operation = uuid.uuid4().hex
    OPERATION.set(operation)
    RECENT.set({})
    return operation


def _agent(db, model: str) -> dict:
    row = db.execute("SELECT body FROM agents WHERE model=?", (model,)).fetchone()
    return json.loads(row[0]) if row else {
        "active": 0, "stable": 0, "failures": {}, "blocked": [],
        "revisions": [{"id": 0, "parent": None, "rules": [], "state": "stable",
                       "passes": 0, "failures": 0, "catalog_sha256": CATALOG_SHA256,
                       "instructions": {}}]}


def _put_agent(db, model: str, agent: dict):
    db.execute("INSERT OR REPLACE INTO agents VALUES(?,?)", (model, json.dumps(agent)))


def in_probation(home: Path, model: str) -> bool:
    with database(home) as db:
        if not _settings(db)["learning"]:
            return False
        agent = _agent(db, model)
        return any(r["id"] == agent["active"] and r["state"] == "probation"
                   for r in agent["revisions"])


def _update(db, model, action, revision):
    db.execute("INSERT INTO updates(model,at,action,revision) VALUES(?,?,?,?)",
               (model, time.time(), action, revision))


def _kind(item: str) -> str:
    return item.strip().lower().split(":", 1)[0]


def begin_run(home: Path, model: str, role: str, checklist: list | None) -> tuple[str, list[str]]:
    billing = model_billing(home, model)
    if not billing["eligible"]:
        raise ValueError(billing["reason"])
    run_id = uuid.uuid4().hex
    with database(home) as db:
        db.execute("BEGIN IMMEDIATE")
        agent = _agent(db, model)
        revision = next(r for r in agent["revisions"] if r["id"] == agent["active"])
        requested = {_kind(s) for s in checklist or []}
        rules = [k for k in revision["rules"] if k in requested and k in RULES] \
            if role == "worker" and _settings(db)["learning"] else []
        if revision.get("catalog_sha256") != CATALOG_SHA256 or any(
            revision.get("instructions", {}).get(k) != RULES[k] for k in rules
        ):
            rules = []
        db.execute("INSERT INTO runs(id,operation,model,role,revision,rules,started,billing) "
                   "VALUES(?,?,?,?,?,?,?,?)",
                   (run_id, OPERATION.get() or run_id, model, role, revision["id"],
                    json.dumps(rules), time.time(), json.dumps(billing)))
    recent = dict(RECENT.get() or {})
    recent[model] = run_id
    RECENT.set(recent)
    return run_id, [RULES[k] for k in rules]


def recent_run(model: str) -> str | None:
    return (RECENT.get() or {}).get(model)


def _micros(value) -> int | None:
    try:
        number = Decimal(str(value))
        if not number.is_finite() or number < 0:
            return None
        return int((number * 1_000_000).to_integral_value(rounding=ROUND_CEILING))
    except (InvalidOperation, ValueError):
        return None


def decode_output(stdout: str, format_name: str) -> tuple[str, dict, bool]:
    usage = {"source": "unreported", "input_tokens": None, "output_tokens": None,
             "cache_read_input_tokens": None, "cache_creation_input_tokens": None,
             "api_equivalent_micros": None, "reported_charge_micros": None}
    if format_name == "text":
        return stdout, usage, False
    try:
        envelope = json.loads(stdout, parse_float=Decimal)
        if not isinstance(envelope, dict):
            raise ValueError("Runner envelope is not an object")
        if format_name == "owi-json" and envelope.get("owi_usage_version") != 1:
            raise ValueError("Unknown usage envelope")
        output = envelope.get("result" if format_name == "claude-json" else "output")
        if not isinstance(output, str):
            raise ValueError("Runner envelope is missing its text output")
        raw = envelope.get("usage") or {}
        if not isinstance(raw, dict):
            raise ValueError("Malformed usage")
        usage["source"] = format_name
        for key in ("input_tokens", "output_tokens", "cache_read_input_tokens",
                    "cache_creation_input_tokens"):
            value = raw.get(key)
            usage[key] = value if type(value) is int and value >= 0 else None
        if format_name == "claude-json":
            usage["api_equivalent_micros"] = _micros(envelope.get("total_cost_usd"))
        else:
            for key in ("api_equivalent_micros", "reported_charge_micros"):
                value = raw.get(key)
                usage[key] = value if type(value) is int and value >= 0 else None
        return output, usage, bool(envelope.get("is_error"))
    except (ValueError, TypeError):
        return stdout, usage, True


def finish_run(home: Path, run_id: str, code: int, usage: dict):
    with database(home) as db:
        db.execute("UPDATE runs SET finished=?,exit_code=?,usage=? WHERE id=?",
                   (time.time(), code, json.dumps(usage), run_id))


def operation_usage(home: Path, operation: str | None = None) -> dict:
    with database(home) as db:
        rows = list(db.execute("SELECT id,model,role,revision,exit_code,usage,billing "
                               "FROM runs WHERE operation=? ORDER BY started",
                               (operation or OPERATION.get(),)))
    runs = []
    for row in rows:
        data = dict(row)
        data["usage"] = json.loads(data["usage"]) if data["usage"] else {"source": "pending"}
        data["billing"] = json.loads(data["billing"])
        runs.append(data)
    return {"operation": operation or OPERATION.get(), "runs": runs, "actual_invoice_charge_micros": None,
            "note": "Runner estimates and reported usage; not invoice reconciliation"}


def recent_usage(home: Path) -> list[dict]:
    with database(home) as db:
        operations = [r[0] for r in db.execute(
            "SELECT operation FROM runs GROUP BY operation ORDER BY MAX(started) DESC LIMIT 20")]
    return [operation_usage(home, operation) for operation in operations]


def observe_checks(home: Path, model: str, run_id: str | None, report: dict,
                   privacy: str = "private_metadata") -> dict | None:
    """Online probation: 3 consecutive checked passes promote; 2 failures revert.

    Only deterministic checks can propose a fixed instruction. No task text,
    free-form reviewer prose, prompts, or confidential outcomes enter a profile.
    Server-generated run IDs make observations idempotent and revision-bound.
    """
    if not run_id or privacy in ("confidential_content", "secret"):
        return None
    with database(home) as db:
        db.execute("BEGIN IMMEDIATE")
        run = db.execute("SELECT * FROM runs WHERE id=? AND model=?", (run_id, model)).fetchone()
        if not run or run["checked"] or run["exit_code"] != 0 or run["role"] != "worker":
            return None
        db.execute("UPDATE runs SET checked=1 WHERE id=?", (run_id,))
        if not _settings(db)["learning"]:
            return None
        items = [i for i in report.get("items", []) if i.get("kind") in RULES
                 and type(i.get("pass")) is bool and not i.get("sampled")]
        if not items:
            return None
        agent = _agent(db, model)
        active = next(r for r in agent["revisions"] if r["id"] == agent["active"])
        failures = {i["kind"] for i in items if i["pass"] is False}
        action = None
        applied = set(json.loads(run["rules"]))
        if active["state"] == "stable" and active["id"] > 0 \
                and run["revision"] == active["id"] and applied:
            if failures & applied:
                active["regressions"] = active.get("regressions", 0) + 1
            elif report.get("verdict") == "accepted":
                active["regressions"] = 0
            if active.get("regressions", 0) >= 2:
                parent = next(r for r in agent["revisions"] if r["id"] == active["parent"])
                active["state"] = "rolled_back"
                agent["active"] = agent["stable"] = parent["id"]
                agent["blocked"] = sorted(set(agent["blocked"]) | (set(active["rules"]) - set(parent["rules"])))
                _put_agent(db, model, agent)
                _update(db, model, "regression_rollback", active["id"])
                return {"model": model, "revision": parent["id"],
                        "action": "regression_rollback", "state": "stable"}
        if active["state"] == "probation" and run["revision"] == active["id"] and applied:
            if report.get("verdict") == "rejected":
                active["failures"] += 1
                active["passes"] = 0
            elif report.get("verdict") == "accepted" and not any(
                i.get("sampled") or i.get("pass") is None for i in report.get("items", [])
            ) and applied <= {i["kind"] for i in items}:
                active["passes"] += 1
            if active["failures"] >= 2:
                parent = next(r for r in agent["revisions"] if r["id"] == agent["stable"])
                agent["blocked"] = sorted(set(agent["blocked"]) | (set(active["rules"]) - set(parent["rules"])))
                active["state"] = "rolled_back"
                agent["active"] = agent["stable"]
                action = "rolled_back"
            elif active["passes"] >= 3:
                active["state"] = "stable"
                agent["stable"] = active["id"]
                action = "promoted"
        elif active["state"] == "stable" and run["revision"] == active["id"]:
            for kind in failures - set(active["rules"]) - set(agent["blocked"]):
                agent["failures"][kind] = agent["failures"].get(kind, 0) + 1
            additions = sorted(k for k, n in agent["failures"].items()
                               if n >= 2 and k not in active["rules"] and k not in agent["blocked"])
            if additions:
                revision = {"id": len(agent["revisions"]), "parent": active["id"],
                            "rules": sorted(set(active["rules"]) | set(additions)),
                            "state": "probation", "passes": 0, "failures": 0,
                            "catalog_sha256": CATALOG_SHA256}
                revision["instructions"] = {k: RULES[k] for k in revision["rules"]}
                # Validation is structural. Probation provides future outcome
                # evidence; it is not a claim of benchmarked quality improvement.
                assert all(k in RULES for k in revision["rules"]) and len(revision["rules"]) <= len(RULES)
                agent["revisions"].append(revision)
                agent["active"] = revision["id"]
                agent["failures"] = {}
                active = revision
                action = "probation_started"
        _put_agent(db, model, agent)
        if action:
            _update(db, model, action, active["id"])
        return {"model": model, "revision": agent["active"], "action": action,
                "state": next(r["state"] for r in agent["revisions"] if r["id"] == agent["active"])}


def rollback(home: Path, model: str) -> dict:
    with database(home) as db:
        db.execute("BEGIN IMMEDIATE")
        agent = _agent(db, model)
        active = next(r for r in agent["revisions"] if r["id"] == agent["active"])
        if active["parent"] is None:
            raise ValueError("This agent is already at its original version")
        parent = next(r for r in agent["revisions"] if r["id"] == active["parent"])
        agent["blocked"] = sorted(set(agent["blocked"]) | (set(active["rules"]) - set(parent["rules"])))
        active["state"] = "rolled_back"
        agent["active"] = agent["stable"] = parent["id"]
        parent["state"] = "stable"
        _put_agent(db, model, agent)
        _update(db, model, "manual_rollback", active["id"])
        return {"model": model, "revision": parent["id"]}
