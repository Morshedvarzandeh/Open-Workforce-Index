"""Trusted project-map adapter for the untrusted browser client.

Repository declarations remain descriptive.  This module is the only place
where the first project-map release turns a displayed action into a mutation,
and those mutations are deliberately limited to local OWI runtime state.
"""
from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import sqlite3
import time
from typing import Callable, Mapping

import owi_runtime as runtime

try:
    import owi_project_map as project_map
except ModuleNotFoundError as error:  # Keep packaged-host failures intelligible.
    if error.name != "owi_project_map":
        raise
    project_map = None


PROJECT_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:@-]{0,127}\Z")
IDEMPOTENCY_KEY = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:@-]{7,127}\Z")
CLIENT_SNAPSHOT_KEYS = {
    "schemaVersion", "snapshot", "lanes", "nodes", "edges", "files",
    "unmappedFiles", "evidence", "actions",
}
FORBIDDEN_CLIENT_KEYS = {
    "absolutepath", "command", "commands", "credential", "credentials",
    "prompt", "prompts", "providerrequest", "providerrun", "providerruns",
    "remote", "remotes", "remoteurl", "repositoryroot", "root", "secret",
    "secrets", "token", "tokens", "output", "modeloutput",
}
REMOTE_VALUE = re.compile(r"(?:https?|ssh|git|file)://|^[^@\s]+@[^:\s]+:")


class MapServiceError(ValueError):
    """A safe, typed failure suitable for the native-message error envelope."""

    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code


@dataclass(frozen=True)
class _Action:
    descriptor: dict
    operation: str
    expected_state: str
    subject: str | None = None


def _canonical(value) -> bytes:
    try:
        return json.dumps(
            value, ensure_ascii=False, allow_nan=False, sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise MapServiceError("unsafe_snapshot", "The project map is not valid JSON") from error


def _digest(prefix: str, value, length: int = 40) -> str:
    return prefix + hashlib.sha256(_canonical(value)).hexdigest()[:length]


def _state_token(value) -> str:
    return _digest("state_", value)


def _agent_control_state(agent: dict) -> dict:
    active_id = agent.get("active")
    active = next(
        (revision for revision in agent.get("revisions", [])
         if revision.get("id") == active_id),
        None,
    )
    parent_id = active.get("parent") if isinstance(active, dict) else None
    parent = next(
        (revision for revision in agent.get("revisions", [])
         if revision.get("id") == parent_id),
        None,
    )
    return {
        "activeRevision": active_id,
        "activeState": active.get("state") if isinstance(active, dict) else None,
        "stableRevision": agent.get("stable"),
        "targetRevision": parent_id,
        "targetState": parent.get("state") if isinstance(parent, dict) else None,
    }


def _safe_snapshot(value: dict, root: Path, *, trusted_actions: bool = False) -> dict:
    """Validate the final core boundary before any value reaches the browser."""
    if not isinstance(value, dict) or set(value) != CLIENT_SNAPSHOT_KEYS:
        raise MapServiceError("unsafe_snapshot", "The project map has an unexpected schema")
    if value.get("schemaVersion") != "owi.system-map.v1":
        raise MapServiceError("unsafe_snapshot", "The project map has an unsupported version")
    if not trusted_actions and value.get("actions") != []:
        # A checked-in declaration must never manufacture authority.
        raise MapServiceError("unsafe_snapshot", "Repository actions are not allowed")
    if trusted_actions:
        actions = value.get("actions")
        if not isinstance(actions, list):
            raise MapServiceError("unsafe_snapshot", "The action list is invalid")
        expected = {
            "id", "kind", "label", "description", "authority", "approval",
            "availability", "unavailableReason", "expectedState", "targetId", "state",
        }
        node_ids = {
            node.get("id") for node in value.get("nodes", []) if isinstance(node, dict)
        }
        action_ids = set()
        for action in actions:
            if not isinstance(action, dict) or set(action) != expected:
                raise MapServiceError("unsafe_snapshot", "A trusted action has invalid fields")
            if action.get("authority") != "local-owi-runtime" \
                    or action.get("approval") != "confirmation-required" \
                    or action.get("availability") not in ("available", "unavailable") \
                    or action.get("targetId") not in node_ids \
                    or action.get("id") in action_ids:
                raise MapServiceError("unsafe_snapshot", "A trusted action is invalid")
            action_ids.add(action["id"])

    root_text = str(root.resolve())

    def visit(item, key: str | None = None):
        if key is not None and key.lower().replace("_", "").replace("-", "") \
                in FORBIDDEN_CLIENT_KEYS:
            raise MapServiceError("unsafe_snapshot", "The project map contains private data")
        if isinstance(item, dict):
            for child_key, child in item.items():
                if not isinstance(child_key, str):
                    raise MapServiceError("unsafe_snapshot", "The project map has an invalid field")
                visit(child, child_key)
        elif isinstance(item, list):
            for child in item:
                visit(child, key)
        elif isinstance(item, str):
            if "\x00" in item or root_text in item \
                    or PurePosixPath(item).is_absolute() \
                    or PureWindowsPath(item).is_absolute() \
                    or REMOTE_VALUE.search(item):
                raise MapServiceError("unsafe_snapshot", "The project map contains private data")
        elif item is not None and type(item) not in (bool, int, float):
            raise MapServiceError("unsafe_snapshot", "The project map has an invalid value")

    visit(value)
    # A JSON round trip both copies the core result and rejects NaN/infinity.
    return json.loads(_canonical(value))


class _ReceiptStore:
    """Small local audit and replay ledger; it contains no project root paths."""

    def __init__(self, home: Path):
        self.path = home / "browser-map.sqlite"
        home.mkdir(parents=True, exist_ok=True, mode=0o700)
        with self._database() as db:
            db.executescript("""
              CREATE TABLE IF NOT EXISTS action_receipts (
                idempotency_key TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                request_sha256 TEXT NOT NULL,
                status TEXT NOT NULL,
                response TEXT,
                error_code TEXT,
                error_message TEXT,
                receipt_id TEXT NOT NULL,
                action_id TEXT NOT NULL,
                before_snapshot_id TEXT NOT NULL,
                after_snapshot_id TEXT,
                created_at TEXT NOT NULL
              );
              CREATE INDEX IF NOT EXISTS action_receipts_project
                ON action_receipts(project_id, created_at);
            """)
        try:
            self.path.chmod(0o600)
        except OSError:
            pass

    @contextmanager
    def _database(self):
        connection = sqlite3.connect(self.path, timeout=30)
        connection.row_factory = sqlite3.Row
        try:
            yield connection
            connection.commit()
        except BaseException:
            connection.rollback()
            raise
        finally:
            connection.close()

    def existing(self, key: str, request_sha256: str) -> dict | None:
        with self._database() as db:
            row = db.execute(
                "SELECT * FROM action_receipts WHERE idempotency_key=?", (key,),
            ).fetchone()
        if row is None:
            return None
        if row["request_sha256"] != request_sha256:
            raise MapServiceError(
                "idempotency_conflict",
                "The idempotency key was already used for a different action",
            )
        if row["status"] == "completed":
            return json.loads(row["response"])
        if row["status"] == "failed":
            raise MapServiceError(row["error_code"], row["error_message"])
        raise MapServiceError(
            "action_outcome_unknown",
            "This action may already have run; inspect the latest snapshot",
        )

    def reserve(self, *, key: str, project_id: str, request_sha256: str,
                receipt_id: str, action_id: str, before_snapshot_id: str,
                created_at: str) -> dict | None:
        try:
            with self._database() as db:
                db.execute(
                    "INSERT INTO action_receipts VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                    (key, project_id, request_sha256, "pending", None, None, None,
                     receipt_id, action_id, before_snapshot_id, None, created_at),
                )
        except sqlite3.IntegrityError:
            # A second host won the reservation.  Resolve it through the same
            # conflict/replay rules as an ordinary retry.
            return self.existing(key, request_sha256)
        return None

    def complete(self, key: str, after_snapshot_id: str | None, response: dict) -> None:
        with self._database() as db:
            db.execute(
                "UPDATE action_receipts SET status='completed',response=?,"
                "after_snapshot_id=? WHERE idempotency_key=? AND status='pending'",
                (_canonical(response).decode("utf-8"), after_snapshot_id, key),
            )

    def fail(self, key: str, error: MapServiceError) -> None:
        with self._database() as db:
            db.execute(
                "UPDATE action_receipts SET status='failed',error_code=?,"
                "error_message=? WHERE idempotency_key=? AND status='pending'",
                (error.code, str(error), key),
            )


class ProjectMapService:
    """Build safe snapshots and invoke the fixed local-runtime action set."""

    def __init__(self, projects: Mapping[str, Path], runtime_home: Path, *,
                 core=None, clock: Callable[[], float] = time.time):
        if not isinstance(projects, Mapping) or not projects or len(projects) > 64:
            raise ValueError("Configure between one and 64 projects")
        clean = {}
        for project_id, root in projects.items():
            if not isinstance(project_id, str) or not PROJECT_ID.fullmatch(project_id):
                raise ValueError("Project IDs must be opaque letters, numbers, '_' or '-'")
            path = Path(root).resolve()
            if not path.is_dir() or path.parent == path:
                raise ValueError("Every configured project root must be a directory")
            clean[project_id] = path
        self.projects = clean
        self.runtime_home = Path(runtime_home).resolve()
        self.core = project_map if core is None else core
        self.clock = clock
        self.generated_at = datetime.fromtimestamp(self.clock(), timezone.utc) \
            .isoformat().replace("+00:00", "Z")
        self.receipts = _ReceiptStore(self.runtime_home)

    def list_projects(self) -> dict:
        return {"projects": [{"projectId": project_id}
                             for project_id in sorted(self.projects)]}

    def _project(self, project_id: str) -> Path:
        if not isinstance(project_id, str) or not PROJECT_ID.fullmatch(project_id):
            raise MapServiceError("unknown_project", "Unknown project")
        try:
            return self.projects[project_id]
        except KeyError as error:
            raise MapServiceError("unknown_project", "Unknown project") from error

    def _runtime_state(self) -> tuple[dict, dict]:
        with runtime.database(self.runtime_home) as db:
            settings = runtime._settings(db)
            agents = {
                row["model"]: json.loads(row["body"])
                for row in db.execute("SELECT model,body FROM agents ORDER BY model")
            }
        return settings, agents

    @staticmethod
    def _action_id(project_id: str, operation: str, subject: str | None = None) -> str:
        return _digest("act_", [project_id, operation, subject], 32)

    @staticmethod
    def _runtime_ids(project_id: str, agents: dict) -> tuple[str, str, dict[str, str]]:
        suffix = hashlib.sha256(project_id.encode("utf-8")).hexdigest()[:16]
        lane_id = f"owi-runtime-{suffix}"
        runtime_id = f"owi-learning-{suffix}"
        agent_ids = {
            model: "owi-agent-" + hashlib.sha256(
                (project_id + "\0" + model).encode("utf-8")
            ).hexdigest()[:20]
            for model in agents
        }
        return lane_id, runtime_id, agent_ids

    def _actions(self, project_id: str, settings=None, agents=None) -> list[_Action]:
        if settings is None or agents is None:
            settings, agents = self._runtime_state()
        _, runtime_id, agent_ids = self._runtime_ids(project_id, agents)
        learning = settings.get("learning", True)
        learning_state = _state_token({"learning": learning})
        actions = []
        for operation, target, label in (
            ("pause_learning", False, "Pause instruction learning"),
            ("resume_learning", True, "Resume instruction learning"),
        ):
            available = learning is not target
            descriptor = {
                "id": self._action_id(project_id, operation),
                "kind": "runtime.learning.pause" if not target else "runtime.learning.resume",
                "label": label,
                "description": (
                    "Changes instruction learning for the shared local OWI runtime, "
                    "not only this mapped project."
                ),
                "authority": "local-owi-runtime",
                "approval": "confirmation-required",
                "availability": "available" if available else "unavailable",
                "unavailableReason": "" if available else (
                    "Instruction learning is already in this state"
                ),
                "expectedState": learning_state,
                "targetId": runtime_id,
                "state": {"learning": "running" if learning else "paused"},
            }
            actions.append(_Action(descriptor, operation, learning_state))

        for model in sorted(agents):
            state = _agent_control_state(agents[model])
            expected_state = _state_token(state)
            descriptor = {
                "id": self._action_id(project_id, "rollback_agent", model),
                "kind": "runtime.agent.rollback",
                "label": f"Roll back {model}",
                "description": (
                    "Rolls back this agent in the shared local OWI runtime, "
                    "not only this mapped project."
                ),
                "authority": "local-owi-runtime",
                "approval": "confirmation-required",
                "availability": "available" if state["targetRevision"] is not None
                    else "unavailable",
                "unavailableReason": "" if state["targetRevision"] is not None
                    else "The agent is already at its original revision",
                "expectedState": expected_state,
                "targetId": agent_ids[model],
                "state": {
                    "agent": model,
                    "activeRevision": state["activeRevision"],
                    "targetRevision": state["targetRevision"],
                },
            }
            actions.append(_Action(descriptor, "rollback_agent", expected_state, model))
        return actions

    def _runtime_fragment(self, project_id: str, settings: dict,
                          agents: dict) -> tuple[dict, list[dict], list[dict], list[dict]]:
        lane_id, runtime_id, agent_ids = self._runtime_ids(project_id, agents)
        learning = settings.get("learning", True)
        lane = {
            "id": lane_id,
            "name": "OWI runtime",
            "description": "Local instruction learning, agent revisions, and usage evidence.",
        }
        nodes = [{
            "id": runtime_id,
            "name": "Instruction learning",
            "description": "Learning is running." if learning else "Learning is paused.",
            "kind": "runtime",
            "laneId": lane_id,
            "lifecycle": "implemented",
            "impact": {"state": "unchanged", "direct": False, "paths": []},
            "churn": {},
            "facts": ["learning:running" if learning else "learning:paused"],
            "state": {"learning": "running" if learning else "paused"},
        }]
        edges = []
        for model in sorted(agents):
            state = _agent_control_state(agents[model])
            agent_id = agent_ids[model]
            nodes.append({
                "id": agent_id,
                "name": model,
                "description": (
                    f"Active revision {state['activeRevision']}; "
                    f"stable revision {state['stableRevision']}."
                ),
                "kind": "agent",
                "laneId": lane_id,
                "lifecycle": "implemented",
                "impact": {"state": "unchanged", "direct": False, "paths": []},
                "churn": {},
                "facts": [
                    f"active-revision:{state['activeRevision']}",
                    f"stable-revision:{state['stableRevision']}",
                ],
                "state": {
                    "activeRevision": state["activeRevision"],
                    "stableRevision": state["stableRevision"],
                },
            })
            edges.append({
                "id": _digest("owi-edge-", [project_id, model], 20),
                "from": runtime_id,
                "to": agent_id,
                "type": "governs-learning-for",
                "label": "learning profile",
            })

        usages = runtime.recent_usage(self.runtime_home)
        runs = [run for operation in usages for run in operation.get("runs", [])]

        def measured_usage(field: str) -> tuple[int | None, str, int]:
            values = [
                run.get("usage", {}).get(field) for run in runs
                if type(run.get("usage", {}).get(field)) is int
                and run["usage"][field] >= 0
            ]
            state = "unavailable" if not values else (
                "measured" if len(values) == len(runs) else "partial"
            )
            return (sum(values) if values else None, state, len(values))

        invoices = [
            operation.get("actual_invoice_charge_micros") for operation in usages
            if type(operation.get("actual_invoice_charge_micros")) is int
            and operation["actual_invoice_charge_micros"] >= 0
        ]
        invoice_state = "unavailable" if not invoices else (
            "measured" if len(invoices) == len(usages) else "partial"
        )
        values = (
            ("runner-reported", *measured_usage("reported_charge_micros"), len(runs)),
            ("api-equivalent", *measured_usage("api_equivalent_micros"), len(runs)),
            ("reconciled-invoice", sum(invoices) if invoices else None,
             invoice_state, len(invoices), len(usages)),
        )
        evidence = [{
            "id": _digest("owi-evidence-", [project_id, basis], 20),
            "type": "shared-runtime-usage",
            "claimState": claim_state,
            "basis": basis,
            "subjectId": runtime_id,
            "value": {
                "amount": amount,
                "unit": "currency-micros",
                "currency": "USD",
                "reportedItems": reported_items,
                "observedItems": observed_items,
            },
        } for basis, amount, claim_state, reported_items, observed_items in values]
        evidence.append({
            "id": _digest("owi-evidence-", [project_id, "learning-state"], 20),
            "type": "runtime-setting",
            "claimState": "observed",
            "basis": "local-runtime-settings",
            "subjectId": runtime_id,
            "value": "running" if learning else "paused",
        })
        return lane, nodes, edges, evidence

    def _build_core_snapshot(self, root: Path) -> dict:
        if self.core is None:
            raise MapServiceError("service_unavailable", "Project-map support is unavailable")
        try:
            value = self.core.build_snapshot(root, generated_at=self.generated_at)
        except MapServiceError:
            raise
        except (OSError, ValueError) as error:
            raise MapServiceError("project_invalid", "The configured project map is invalid") from error
        return _safe_snapshot(value, root)

    @staticmethod
    def _core_snapshot_id(snapshot: dict) -> str:
        metadata = snapshot.get("snapshot")
        if isinstance(metadata, dict):
            for key in ("id", "snapshotId", "digest"):
                if isinstance(metadata.get(key), str) and metadata[key]:
                    return metadata[key]
        if isinstance(metadata, str) and metadata:
            return metadata
        return _digest("core_", snapshot)

    def get_snapshot(self, project_id: str) -> dict:
        root = self._project(project_id)
        snapshot = self._build_core_snapshot(root)
        declared_project_id = snapshot["snapshot"]["projectId"]
        snapshot["snapshot"]["projectId"] = project_id
        for record in snapshot["evidence"]:
            if record.get("subjectId") == declared_project_id:
                record["subjectId"] = project_id
        settings, agents = self._runtime_state()
        lane, nodes, edges, evidence = self._runtime_fragment(
            project_id, settings, agents,
        )
        additions = (
            ("lane", snapshot["lanes"], [lane]),
            ("node", snapshot["nodes"], nodes),
            ("edge", snapshot["edges"], edges),
            ("evidence", snapshot["evidence"], evidence),
        )
        for label, current, extra in additions:
            current_ids = {item.get("id") for item in current if isinstance(item, dict)}
            extra_ids = [item.get("id") for item in extra if isinstance(item, dict)]
            if len(extra_ids) != len(set(extra_ids)) or current_ids.intersection(extra_ids):
                raise MapServiceError(
                    "unsafe_snapshot", f"The project map has a reserved {label} identifier",
                )
        snapshot["lanes"].append(lane)
        snapshot["nodes"].extend(nodes)
        snapshot["edges"].extend(edges)
        snapshot["evidence"].extend(evidence)
        snapshot["snapshot"]["adapters"].append(
            {"id": "owi-local-runtime", "version": "1"}
        )
        actions = self._actions(project_id, settings, agents)
        snapshot["actions"] = [action.descriptor for action in actions]
        if callable(getattr(self.core, "seal_snapshot", None)):
            snapshot = self.core.seal_snapshot(snapshot)
            snapshot_id = self._core_snapshot_id(snapshot)
        else:
            # Test doubles and a partially installed package still receive a
            # content-derived ID that changes with every trusted action state.
            snapshot_id = _digest("snapshot_", snapshot)
            if isinstance(snapshot.get("snapshot"), dict):
                snapshot["snapshot"]["id"] = snapshot_id
        snapshot = _safe_snapshot(snapshot, root, trusted_actions=True)
        return {"projectId": project_id, "snapshotId": snapshot_id, "map": snapshot}

    @staticmethod
    def _request_sha(project_id: str, action_id: str, expected_snapshot_id: str,
                     expected_state: str, confirmed: bool) -> str:
        return hashlib.sha256(_canonical({
            "projectId": project_id,
            "actionId": action_id,
            "expectedSnapshotId": expected_snapshot_id,
            "expectedState": expected_state,
            "confirmed": confirmed,
        })).hexdigest()

    def _set_learning(self, target: bool, expected_state: str) -> None:
        with runtime.database(self.runtime_home) as db:
            db.execute("BEGIN IMMEDIATE")
            settings = runtime._settings(db)
            current = _state_token({"learning": settings.get("learning", True)})
            if current != expected_state:
                raise MapServiceError("stale_state", "The runtime state has changed")
            if settings.get("learning", True) is target:
                raise MapServiceError("action_unavailable", "The action is no longer available")
            updated = dict(settings)
            updated["learning"] = target
            db.execute(
                "INSERT OR REPLACE INTO settings VALUES(1,?)",
                (json.dumps(updated, sort_keys=True, separators=(",", ":")),),
            )

    def _rollback_agent(self, model: str, expected_state: str) -> None:
        with runtime.database(self.runtime_home) as db:
            db.execute("BEGIN IMMEDIATE")
            agent = runtime._agent(db, model)
            if _state_token(_agent_control_state(agent)) != expected_state:
                raise MapServiceError("stale_state", "The agent state has changed")
            active = next(
                revision for revision in agent["revisions"]
                if revision["id"] == agent["active"]
            )
            if active["parent"] is None:
                raise MapServiceError("action_unavailable", "The action is no longer available")
            parent = next(
                revision for revision in agent["revisions"]
                if revision["id"] == active["parent"]
            )
            agent["blocked"] = sorted(
                set(agent["blocked"]) | (set(active["rules"]) - set(parent["rules"]))
            )
            active["state"] = "rolled_back"
            agent["active"] = agent["stable"] = parent["id"]
            parent["state"] = "stable"
            runtime._put_agent(db, model, agent)
            runtime._update(db, model, "manual_rollback", active["id"])

    def invoke_action(self, *, project_id: str, action_id: str,
                      expected_snapshot_id: str, expected_state: str,
                      idempotency_key: str, confirmed: bool) -> dict:
        self._project(project_id)
        if not isinstance(idempotency_key, str) \
                or not IDEMPOTENCY_KEY.fullmatch(idempotency_key):
            raise MapServiceError("invalid_request", "Invalid idempotency key")
        if type(confirmed) is not bool or not confirmed:
            raise MapServiceError("confirmation_required", "Confirm the action before invoking it")
        for value in (action_id, expected_snapshot_id, expected_state):
            if not isinstance(value, str) or not 1 <= len(value) <= 128:
                raise MapServiceError("invalid_request", "Invalid action precondition")

        request_sha = self._request_sha(
            project_id, action_id, expected_snapshot_id, expected_state, confirmed,
        )
        replay = self.receipts.existing(idempotency_key, request_sha)
        if replay is not None:
            return replay

        before = self.get_snapshot(project_id)
        if before["snapshotId"] != expected_snapshot_id:
            raise MapServiceError("stale_snapshot", "The project snapshot has changed")
        actions = {action.descriptor["id"]: action
                   for action in self._actions(project_id)}
        action = actions.get(action_id)
        if action is None:
            raise MapServiceError("unknown_action", "Unknown action")
        if action.expected_state != expected_state:
            raise MapServiceError("stale_state", "The runtime state has changed")
        if action.descriptor["availability"] != "available":
            raise MapServiceError("action_unavailable", "The action is not available")

        created_at = datetime.fromtimestamp(self.clock(), timezone.utc) \
            .isoformat().replace("+00:00", "Z")
        receipt_id = _digest(
            "receipt_", [project_id, idempotency_key, request_sha], 32,
        )
        reserved = self.receipts.reserve(
            key=idempotency_key, project_id=project_id,
            request_sha256=request_sha, receipt_id=receipt_id,
            action_id=action_id, before_snapshot_id=expected_snapshot_id,
            created_at=created_at,
        )
        if reserved is not None:
            return reserved
        try:
            if action.operation == "pause_learning":
                self._set_learning(False, expected_state)
            elif action.operation == "resume_learning":
                self._set_learning(True, expected_state)
            elif action.operation == "rollback_agent" and action.subject is not None:
                self._rollback_agent(action.subject, expected_state)
            else:  # The internal action table is closed; this is not client input.
                raise MapServiceError("unknown_action", "Unknown action")
        except MapServiceError as error:
            self.receipts.fail(idempotency_key, error)
            raise
        except (OSError, ValueError, sqlite3.Error) as error:
            safe = MapServiceError("action_failed", "The local runtime action failed")
            self.receipts.fail(idempotency_key, safe)
            raise safe from error

        # The mutation committed when the operation returned. Snapshot building
        # is a separate read and must never rewrite that truth as "failed".
        try:
            after = self.get_snapshot(project_id)
        except Exception:
            after = None
        after_snapshot_id = after["snapshotId"] if after is not None else None
        response = {
            "receipt": {
                "schemaVersion": "owi.action-receipt.v1",
                "receiptId": receipt_id,
                "projectId": project_id,
                "actionId": action_id,
                "outcome": "applied",
                "createdAt": created_at,
                "beforeSnapshotId": expected_snapshot_id,
                "afterSnapshotId": after_snapshot_id,
                "snapshotState": "available" if after is not None else "unavailable",
            },
            "snapshot": after,
        }
        try:
            self.receipts.complete(idempotency_key, after_snapshot_id, response)
        except sqlite3.Error as error:
            raise MapServiceError(
                "action_outcome_unknown",
                "The action was applied but its audit receipt could not be finalized; "
                "inspect the latest snapshot",
            ) from error
        return response
