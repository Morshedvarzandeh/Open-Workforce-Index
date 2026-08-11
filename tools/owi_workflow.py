#!/usr/bin/env python3
"""Persisted, server-authoritative project workflow for the OWI Office.

The browser is a view/controller only.  It may describe a project and its
tasks, but it never supplies the worker, model, runner command, or output being
reviewed.  Staffing comes from the Rust allocator and execution is resolved
from the local workspace's ``runners.json``.
"""

from __future__ import annotations

import json
import os
import re
import sqlite3
import stat
import subprocess
import tempfile
import time
import uuid
from contextlib import contextmanager
from pathlib import Path
from typing import Callable
from urllib.parse import urlsplit


PRIVACY_LEVELS = {
    "public",
    "private_metadata",
    "confidential_content",
    "secret",
}
PRIVACY_RANK = {
    "public": 0,
    "private_metadata": 1,
    "confidential_content": 2,
    "secret": 3,
}
EDITABLE_TASK_STATES = {
    "draft",
    "staffed",
    "setup_required",
    "unstaffed",
    "rejected",
    "run_failed",
    "needs_review",
}
RUNNABLE_TASK_STATES = {"staffed", "rejected", "run_failed", "needs_review"}
CAUSES = {"worker", "task_spec", "harness", "environment"}
MAX_OUTPUT_CHARS = 100_000
MAX_STDERR_CHARS = 4_000
MAX_ATTEMPTS = 2


class WorkflowProblem(RuntimeError):
    """A request problem safe to return from the local-only HTTP API."""

    def __init__(self, message: str, status: int = 400):
        super().__init__(message)
        self.status = status


def now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def new_id(kind: str) -> str:
    return f"{kind}:{uuid.uuid4().hex}"


def model_for_worker(worker_id: str) -> str:
    if not worker_id.startswith("worker:") or "/" not in worker_id:
        return ""
    return worker_id.removeprefix("worker:").split("/", 1)[0].strip()


def title_for(summary: str) -> str:
    first = summary.strip().splitlines()[0].strip()
    return (first[:77] + "...") if len(first) > 80 else first


class WorkflowService:
    """Project/task lifecycle backed by a private SQLite file."""

    def __init__(
        self,
        home: Path,
        owi_do,
        catalog_loader: Callable[[Path], dict],
    ) -> None:
        self.home = Path(home)
        self.owi_do = owi_do
        self.catalog_loader = catalog_loader
        self.path = self.home / "workflow.sqlite"

    @contextmanager
    def _connect(self):
        self._ensure_safe_database()
        self.home.mkdir(parents=True, exist_ok=True)
        connection = sqlite3.connect(self.path, timeout=10)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA foreign_keys = ON")
        try:
            connection.executescript(
                """
            CREATE TABLE IF NOT EXISTS projects (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              goal TEXT NOT NULL,
              privacy TEXT NOT NULL,
              budget_micros INTEGER,
              planning_source TEXT NOT NULL,
              status TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tasks (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              position INTEGER NOT NULL,
              title TEXT NOT NULL,
              brief TEXT NOT NULL,
              skill TEXT NOT NULL,
              privacy TEXT NOT NULL,
              checklist_json TEXT NOT NULL,
              checklist_source TEXT NOT NULL,
              status TEXT NOT NULL,
              worker TEXT,
              model TEXT,
              quote_json TEXT,
              estimated_cost_micros INTEGER,
              runner_ready INTEGER NOT NULL DEFAULT 0,
              output TEXT,
              stderr TEXT,
              exit_code INTEGER,
              check_json TEXT,
              actual_cost_micros INTEGER,
              attempt_count INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              UNIQUE(project_id, position)
            );
            CREATE TABLE IF NOT EXISTS workflow_events (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              project_id TEXT NOT NULL,
              task_id TEXT,
              kind TEXT NOT NULL,
              payload_json TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS workflow_tasks_project
              ON tasks(project_id, position);
            CREATE TABLE IF NOT EXISTS source_imports (
              source_system TEXT NOT NULL,
              source_repository_id TEXT NOT NULL,
              source_item_type TEXT NOT NULL,
              source_item_id TEXT NOT NULL,
              source_revision TEXT NOT NULL,
              source_digest TEXT NOT NULL,
              source_updated_at TEXT,
              observed_at TEXT NOT NULL,
              api_version TEXT NOT NULL,
              observation_partial INTEGER NOT NULL,
              observation_error TEXT,
              source_repository_name TEXT NOT NULL,
              source_url TEXT NOT NULL,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
              imported_at TEXT NOT NULL,
              PRIMARY KEY (
                source_system, source_repository_id,
                source_item_type, source_item_id
              )
            );
            CREATE INDEX IF NOT EXISTS workflow_source_import_task
              ON source_imports(task_id);
                """
            )
            columns = {
                row[1] for row in connection.execute("PRAGMA table_info(tasks)")
            }
            if "attempt_count" not in columns:
                connection.execute(
                    "ALTER TABLE tasks ADD COLUMN attempt_count INTEGER "
                    "NOT NULL DEFAULT 0"
                )
            import_columns = {
                row[1] for row in connection.execute(
                    "PRAGMA table_info(source_imports)"
                )
            }
            import_additions = {
                "source_updated_at": "TEXT",
                "observed_at": "TEXT NOT NULL DEFAULT 'unknown'",
                "api_version": "TEXT NOT NULL DEFAULT 'unknown'",
                "observation_partial": "INTEGER NOT NULL DEFAULT 0",
                "observation_error": "TEXT",
            }
            for name, declaration in import_additions.items():
                if name not in import_columns:
                    connection.execute(
                        f"ALTER TABLE source_imports ADD COLUMN {name} {declaration}"
                    )
            with connection:
                yield connection
        finally:
            connection.close()

    def _ensure_safe_database(self) -> None:
        """Never let a workspace redirect the private ledger through a symlink."""
        absolute_home = self.home.absolute()
        chain = [absolute_home, *absolute_home.parents]
        for component in reversed(chain):
            if component.is_symlink():
                raise WorkflowProblem(
                    f"workflow path contains a symlink: {component}", 409
                )
        self.home.mkdir(parents=True, exist_ok=True)
        flags = os.O_RDWR | os.O_CREAT
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        try:
            descriptor = os.open(self.path, flags, 0o600)
        except OSError as error:
            raise WorkflowProblem(
                f"workflow database cannot be opened safely: {error}", 409
            ) from error
        try:
            metadata = os.fstat(descriptor)
            if not stat.S_ISREG(metadata.st_mode):
                raise WorkflowProblem(
                    "workflow database must be a regular file", 409
                )
            if hasattr(os, "getuid") and metadata.st_uid != os.getuid():
                raise WorkflowProblem(
                    "workflow database is not owned by this user", 409
                )
            if os.name != "nt":
                os.fchmod(descriptor, 0o600)
        finally:
            os.close(descriptor)

    @staticmethod
    def _json(value, fallback):
        try:
            parsed = json.loads(value) if value else fallback
        except (TypeError, json.JSONDecodeError):
            parsed = fallback
        return parsed

    def _event(
        self,
        connection: sqlite3.Connection,
        project_id: str,
        task_id: str | None,
        kind: str,
        payload: dict | None = None,
    ) -> None:
        connection.execute(
            "INSERT INTO workflow_events "
            "(project_id, task_id, kind, payload_json, created_at) "
            "VALUES (?, ?, ?, ?, ?)",
            (project_id, task_id, kind, json.dumps(payload or {}), now()),
        )

    def _require_safe_local_ledger(self) -> None:
        local = self.home / "local.sqlite"
        if local.is_symlink():
            raise WorkflowProblem("local.sqlite may not be a symlink", 409)
        if local.exists() and not local.is_file():
            raise WorkflowProblem("local.sqlite must be a regular file", 409)

    def _task_dict(self, row: sqlite3.Row) -> dict:
        quote = self._json(row["quote_json"], {})
        runner_command, runner_binding = self.owi_do.resolve_worker_runner(
            self.home, row["worker"] or "", allow_model_fallback=False
        )
        return {
            "id": row["id"],
            "title": row["title"],
            "brief": row["brief"],
            "status": row["status"],
            "skill": row["skill"],
            "privacy": row["privacy"],
            "worker": row["worker"],
            "model": row["model"],
            "estimatedCostMicros": row["estimated_cost_micros"],
            # Provider CLIs currently do not expose a trustworthy receipt.
            "actualCostMicros": row["actual_cost_micros"],
            "runnerReady": bool(runner_command),
            "runnerBinding": runner_binding,
            "attemptCount": row["attempt_count"],
            "maxAttempts": MAX_ATTEMPTS,
            "output": row["output"] or "",
            "stderr": row["stderr"] or "",
            "exitCode": row["exit_code"],
            "checklist": self._json(row["checklist_json"], []),
            "checklistSource": row["checklist_source"],
            "checks": self._json(row["check_json"], None),
            "alternatives": quote.get("alternatives", []),
            "rejections": quote.get("rejections", []),
            "createdAt": row["created_at"],
            "updatedAt": row["updated_at"],
        }

    def _project_dict(
        self, connection: sqlite3.Connection, row: sqlite3.Row
    ) -> dict:
        task_rows = connection.execute(
            "SELECT * FROM tasks WHERE project_id = ? ORDER BY position",
            (row["id"],),
        ).fetchall()
        tasks = [self._task_dict(task) for task in task_rows]
        estimates = [task["estimatedCostMicros"] for task in tasks]
        estimated_count = sum(value is not None for value in estimates)
        selected_subtotal = sum(value for value in estimates if value is not None)
        total = (selected_subtotal
                 if tasks and estimated_count == len(tasks) else None)
        budget = row["budget_micros"]
        planning_source = row["planning_source"]
        planning_problem = None
        if planning_source.startswith("planner_unavailable"):
            planning_problem = (
                "No runnable planning model is configured. Review the fallback "
                "tasks or add tasks manually before running."
            )
        elif planning_source.startswith("planner_failed"):
            planning_problem = (
                "The planning model did not return a valid decomposition. Review "
                "the deterministic fallback tasks before running."
            )
        elif planning_source.startswith("planner_blocked_privacy"):
            planning_problem = (
                "Model planning was disabled for confidential/secret content. "
                "Review the deterministic fallback or add tasks manually."
            )
        elif planning_source.startswith("planner_blocked_budget"):
            planning_problem = (
                "Model planning was not run because this project's task-forecast "
                "budget is zero. Add/edit tasks manually."
            )
        elif planning_source.startswith("planner_not_requested"):
            planning_problem = (
                "Model planning was not requested. Review the deterministic "
                "fallback or add tasks manually."
            )
        elif planning_source.startswith("model:"):
            planning_problem = (
                "The planning model's provider receipt is unavailable and its "
                "usage is not included in the task-execution forecast."
            )
        blocked = sum(
            task["status"] in ("unstaffed", "setup_required", "run_failed")
            for task in tasks
        )
        pending = sum(
            task["status"] not in ("accepted", "rejected") for task in tasks
        )
        return {
            "id": row["id"],
            "name": row["name"],
            "goal": row["goal"],
            "privacy": row["privacy"],
            "budgetMicros": budget,
            "budgetBasis": "forecast_ceiling_expected_accepted_cost",
            "budgetScope": "staffed_task_execution_only",
            "actualSpendEnforced": False,
            "status": row["status"],
            "planningSource": planning_source,
            "planningProblem": planning_problem,
            # Runner CLIs do not currently expose a trustworthy planning receipt.
            "planningCostMicros": None,
            "createdAt": row["created_at"],
            "updatedAt": row["updated_at"],
            "tasks": tasks,
            "totals": {
                "estimatedCostMicros": total,
                "selectedSubtotalMicros": selected_subtotal,
                "estimateCoverage": {
                    "estimated": estimated_count,
                    "tasks": len(tasks),
                },
                "actualCostMicros": None,
                "accepted": sum(task["status"] == "accepted" for task in tasks),
                "rejected": sum(task["status"] == "rejected" for task in tasks),
                "blocked": blocked,
                "pending": pending,
                "overBudget": (
                    None if total is None or budget is None else total > budget
                ),
            },
        }

    def _get_project_row(
        self, connection: sqlite3.Connection, project_id: str
    ) -> sqlite3.Row:
        row = connection.execute(
            "SELECT * FROM projects WHERE id = ?", (project_id,)
        ).fetchone()
        if row is None:
            raise WorkflowProblem("project not found", 404)
        return row

    def _get_task_row(
        self, connection: sqlite3.Connection, project_id: str, task_id: str
    ) -> sqlite3.Row:
        row = connection.execute(
            "SELECT * FROM tasks WHERE id = ? AND project_id = ?",
            (task_id, project_id),
        ).fetchone()
        if row is None:
            raise WorkflowProblem("task not found", 404)
        return row

    def current(self) -> dict | None:
        if self.path.is_symlink():
            raise WorkflowProblem("workflow database may not be a symlink", 409)
        if not self.path.exists():
            return None
        with self._connect() as connection:
            row = connection.execute(
                "SELECT * FROM projects ORDER BY created_at DESC, rowid DESC LIMIT 1"
            ).fetchone()
            return self._project_dict(connection, row) if row else None

    def recover_interrupted_runs(self) -> int:
        """On server startup, turn abandoned `running` rows into retryable facts."""
        if self.path.is_symlink() or not self.path.exists():
            return 0
        with self._connect() as connection:
            rows = connection.execute(
                "SELECT id, project_id FROM tasks WHERE status='running'"
            ).fetchall()
            for row in rows:
                connection.execute(
                    "UPDATE tasks SET status='run_failed', stderr=?, updated_at=? "
                    "WHERE id=?",
                    ("server stopped before this run completed", now(), row["id"]),
                )
                self._event(
                    connection, row["project_id"], row["id"],
                    "task_run_interrupted"
                )
            for project_id in {row["project_id"] for row in rows}:
                self._update_project_status(connection, project_id)
            return len(rows)

    def get_project(self, project_id: str) -> dict:
        with self._connect() as connection:
            return self._project_dict(
                connection, self._get_project_row(connection, project_id)
            )

    def _normalise_task(self, raw, privacy: str) -> dict:
        if isinstance(raw, str):
            raw = {"brief": raw}
        if not isinstance(raw, dict):
            raise WorkflowProblem("each task must be text or an object")
        brief = str(raw.get("brief") or raw.get("summary") or "").strip()
        if not brief:
            raise WorkflowProblem("each task needs a brief")
        if len(brief) > 20_000:
            raise WorkflowProblem("task brief is too long")
        skill = str(raw.get("skill") or self.owi_do.classify(brief))
        if skill not in self.owi_do.SKILLS:
            raise WorkflowProblem(f"unknown task skill: {skill}")
        task_privacy = str(raw.get("privacy") or privacy)
        if task_privacy not in PRIVACY_LEVELS:
            raise WorkflowProblem(f"unknown privacy level: {task_privacy}")
        checklist = self.owi_do.clean_checklist(raw.get("checklist", []))
        return {
            "title": str(raw.get("title") or title_for(brief)).strip()[:120],
            "brief": brief,
            "skill": skill,
            "privacy": task_privacy,
            "checklist": checklist,
            "checklist_source": "user" if checklist else "none",
        }

    @staticmethod
    def _numbered_tasks(goal: str) -> list[dict]:
        items = []
        for line in goal.splitlines():
            match = re.match(r"^\s*(?:[-*]|\d+[.)])\s+(.+?)\s*$", line)
            if match:
                items.append({"brief": match.group(1)})
        return items if len(items) >= 2 else []

    def create_project(self, body: dict) -> dict:
        goal = str(body.get("goal") or "").strip()
        if not goal:
            raise WorkflowProblem("project goal is required")
        if len(goal) > 20_000:
            raise WorkflowProblem("project goal is too long")
        privacy = str(body.get("privacy") or "private_metadata")
        if privacy not in PRIVACY_LEVELS:
            raise WorkflowProblem(f"unknown privacy level: {privacy}")
        budget = body.get("budgetMicros")
        if budget is not None:
            if isinstance(budget, bool) or not isinstance(budget, int) or budget < 0:
                raise WorkflowProblem("budgetMicros must be a non-negative integer")
        supplied = body.get("tasks")
        if supplied is not None and not isinstance(supplied, list):
            raise WorkflowProblem("tasks must be a list")
        use_planner = body.get("usePlanningModel", body.get("usePlanner", False))
        if not isinstance(use_planner, bool):
            raise WorkflowProblem("usePlanningModel must be true or false")
        if supplied:
            raw_tasks = supplied[:100]
            planning_source = "user"
        elif not use_planner:
            numbered = self._numbered_tasks(goal)
            raw_tasks = numbered or [{"brief": goal}]
            shape = "numbered" if numbered else "single_task"
            planning_source = f"planner_not_requested_{shape}_fallback"
        elif budget == 0:
            numbered = self._numbered_tasks(goal)
            raw_tasks = numbered or [{"brief": goal}]
            shape = "numbered" if numbered else "single_task"
            planning_source = f"planner_blocked_budget_{shape}_fallback"
        elif privacy in ("confidential_content", "secret"):
            numbered = self._numbered_tasks(goal)
            raw_tasks = numbered or [{"brief": goal}]
            shape = "numbered" if numbered else "single_task"
            planning_source = f"planner_blocked_privacy_{shape}_fallback"
        else:
            planner, planned = self.owi_do.plan_parts(self.home, goal)
            if planned:
                raw_tasks = [{
                    "brief": part["summary"],
                    "skill": part["skill"],
                    "checklist": part.get("checklist", []),
                    "_checklist_source": "planner",
                } for part in planned[:100]]
                planning_source = f"model:{planner}"
            else:
                numbered = self._numbered_tasks(goal)
                raw_tasks = numbered or [{"brief": goal}]
                reason = "failed" if planner else "unavailable"
                shape = "numbered" if numbered else "single_task"
                planning_source = f"planner_{reason}_{shape}_fallback"
        tasks = [self._normalise_task(raw, privacy) for raw in raw_tasks]
        for task, raw in zip(tasks, raw_tasks):
            if (isinstance(raw, dict)
                    and raw.get("_checklist_source") == "planner"
                    and task["checklist"]):
                task["checklist_source"] = "planner"
        project_id = new_id("project")
        timestamp = now()
        name = str(body.get("name") or title_for(goal)).strip()[:120]
        with self._connect() as connection:
            connection.execute(
                "INSERT INTO projects VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    project_id,
                    name,
                    goal,
                    privacy,
                    budget,
                    planning_source,
                    "draft",
                    timestamp,
                    timestamp,
                ),
            )
            for position, task in enumerate(tasks, start=1):
                task_id = new_id("task")
                connection.execute(
                    "INSERT INTO tasks "
                    "(id, project_id, position, title, brief, skill, privacy, "
                    "checklist_json, checklist_source, status, created_at, updated_at) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    (
                        task_id,
                        project_id,
                        position,
                        task["title"],
                        task["brief"],
                        task["skill"],
                        task["privacy"],
                        json.dumps(task["checklist"]),
                        task["checklist_source"],
                        "draft",
                        timestamp,
                        timestamp,
                    ),
                )
            self._event(connection, project_id, None, "project_created", {
                "taskCount": len(tasks), "planningSource": planning_source
            })
        return self.get_project(project_id)

    def add_task(self, project_id: str, body: dict) -> tuple[dict, dict]:
        with self._connect() as connection:
            project = self._get_project_row(connection, project_id)
            task = self._normalise_task(body, project["privacy"])
            position = connection.execute(
                "SELECT COALESCE(MAX(position), 0) + 1 FROM tasks "
                "WHERE project_id = ?", (project_id,)
            ).fetchone()[0]
            task_id = new_id("task")
            timestamp = now()
            connection.execute(
                "INSERT INTO tasks "
                "(id, project_id, position, title, brief, skill, privacy, "
                "checklist_json, checklist_source, status, created_at, updated_at) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (task_id, project_id, position, task["title"], task["brief"],
                 task["skill"], task["privacy"], json.dumps(task["checklist"]),
                 task["checklist_source"], "draft", timestamp, timestamp),
            )
            self._event(connection, project_id, task_id, "task_created")
            self._update_project_status(connection, project_id)
        project = self.get_project(project_id)
        return project, next(task for task in project["tasks"] if task["id"] == task_id)

    def import_github_item(
        self,
        source: dict,
        project_id: str | None = None,
        privacy: str | None = None,
        skill: str | None = None,
        checklist: list[str] | None = None,
    ) -> tuple[dict, dict, bool]:
        """Atomically bind server-cached GitHub provenance to one draft task.

        This deliberately bypasses planning, allocation and execution.  A repeat
        import of the same stable GitHub repository/item identity returns the
        original local task and never overwrites owner edits.
        """
        required = {
            "repository_id", "repository_name", "repository_private",
            "item_type", "item_id", "revision", "digest", "url", "title",
        }
        if not isinstance(source, dict) or not required.issubset(source):
            raise WorkflowProblem("incomplete server-owned GitHub provenance")
        item_type = str(source["item_type"])
        if item_type not in {"issue", "pull_request", "action_failure"}:
            raise WorkflowProblem("unknown GitHub source item type")
        repository_id = str(source["repository_id"])
        item_id = str(source["item_id"])
        digest = str(source["digest"])
        if (not repository_id.isdigit() or not item_id.isdigit()
                or len(repository_id) > 30 or len(item_id) > 30
                or not re.fullmatch(r"[0-9a-f]{64}", digest)):
            raise WorkflowProblem("invalid stable GitHub source identity")
        source_url = str(source["url"])
        parsed_url = urlsplit(source_url)
        try:
            unsafe_port = parsed_url.port is not None
        except ValueError as error:
            raise WorkflowProblem("invalid canonical GitHub source URL") from error
        if (parsed_url.scheme != "https" or parsed_url.hostname != "github.com"
                or parsed_url.username or parsed_url.password
                or unsafe_port or len(source_url) > 2000):
            raise WorkflowProblem("invalid canonical GitHub source URL")
        title = str(source["title"]).strip()
        if not title or len(title) > 500:
            raise WorkflowProblem("invalid GitHub source title")
        source_observed_at = str(source.get("observed_at") or "").strip()
        if not source_observed_at:
            raise WorkflowProblem(
                "GitHub source has no successful observation to import", 409
            )
        source_key = (
            "github", repository_id, item_type, item_id,
        )
        requested_privacy = str(privacy) if privacy is not None else None
        if requested_privacy is not None and requested_privacy not in PRIVACY_LEVELS:
            raise WorkflowProblem(f"unknown privacy level: {requested_privacy}")
        source_floor = (
            "confidential_content" if bool(source["repository_private"])
            else "public"
        )

        with self._connect() as connection:
            # Serialize the stable source-key check and insert across server
            # threads, not merely inside one Python service instance.
            connection.execute("BEGIN IMMEDIATE")
            existing = connection.execute(
                "SELECT project_id, task_id FROM source_imports WHERE "
                "source_system=? AND source_repository_id=? AND "
                "source_item_type=? AND source_item_id=?",
                source_key,
            ).fetchone()
            if existing is not None:
                task_row = connection.execute(
                    "SELECT * FROM tasks WHERE id=? AND project_id=?",
                    (existing["task_id"], existing["project_id"]),
                ).fetchone()
                if task_row is not None:
                    project_row = self._get_project_row(
                        connection, existing["project_id"]
                    )
                    return (
                        self._project_dict(connection, project_row),
                        self._task_dict(task_row),
                        False,
                    )
                # Compatibility with a prerelease table created before the FK.
                connection.execute(
                    "DELETE FROM source_imports WHERE source_system=? AND "
                    "source_repository_id=? AND source_item_type=? AND "
                    "source_item_id=?", source_key
                )

            timestamp = now()
            if project_id:
                project_row = self._get_project_row(connection, project_id)
                project_privacy = project_row["privacy"]
                if PRIVACY_RANK[project_privacy] < PRIVACY_RANK[source_floor]:
                    raise WorkflowProblem(
                        "a private GitHub item requires a confidential_content "
                        "or secret project", 409
                    )
            else:
                base_privacy = requested_privacy or (
                    "confidential_content" if bool(source["repository_private"])
                    else "private_metadata"
                )
                if PRIVACY_RANK[base_privacy] < PRIVACY_RANK[source_floor]:
                    raise WorkflowProblem(
                        "private GitHub content cannot be imported below "
                        "confidential_content", 409
                    )
                project_id = new_id("project")
                project_name = str(source["repository_name"]).strip()[:120]
                goal = (
                    f"Review and resolve selected work from "
                    f"{source['repository_name']}."
                )
                connection.execute(
                    "INSERT INTO projects VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    (project_id, project_name, goal, base_privacy, None,
                     "github_import", "draft", timestamp, timestamp),
                )
                self._event(connection, project_id, None, "project_created", {
                    "taskCount": 1, "planningSource": "github_import"
                })
                project_row = self._get_project_row(connection, project_id)
                project_privacy = base_privacy

            task_privacy = requested_privacy or project_privacy
            if PRIVACY_RANK[task_privacy] < PRIVACY_RANK[project_privacy]:
                task_privacy = project_privacy
            if PRIVACY_RANK[task_privacy] < PRIVACY_RANK[source_floor]:
                raise WorkflowProblem(
                    "private GitHub content cannot be imported below "
                    "confidential_content", 409
                )
            label = {
                "issue": "Issue",
                "pull_request": "Pull request",
                "action_failure": "Failed Actions run",
            }.get(str(source["item_type"]), "Work item")
            number = source.get("number")
            number_text = f" #{number}" if number is not None else ""
            body_text = str(source.get("body") or "").strip()[:16_000]
            brief = (
                f"{label}{number_text}: {title}\n\n"
                f"Source: {source['url']}\n\n"
                f"{body_text}"
            ).strip()
            raw_task = {"brief": brief, "privacy": task_privacy}
            if skill is not None:
                raw_task["skill"] = skill
            if checklist is not None:
                raw_task["checklist"] = checklist
            task = self._normalise_task(raw_task, project_privacy)
            task["title"] = f"[GitHub {label}{number_text}] {title}"[:120]
            position = connection.execute(
                "SELECT COALESCE(MAX(position), 0) + 1 FROM tasks "
                "WHERE project_id=?", (project_id,)
            ).fetchone()[0]
            task_id = new_id("task")
            connection.execute(
                "INSERT INTO tasks "
                "(id, project_id, position, title, brief, skill, privacy, "
                "checklist_json, checklist_source, status, created_at, updated_at) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'draft', ?, ?)",
                (task_id, project_id, position, task["title"], task["brief"],
                 task["skill"], task["privacy"], json.dumps(task["checklist"]),
                 task["checklist_source"], timestamp, timestamp),
            )
            connection.execute(
                "INSERT INTO source_imports "
                "(source_system,source_repository_id,source_item_type,source_item_id,"
                "source_revision,source_digest,source_updated_at,observed_at,"
                "api_version,observation_partial,observation_error,"
                "source_repository_name,source_url,project_id,task_id,imported_at) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (*source_key, str(source["revision"]), digest,
                 source.get("source_updated_at"),
                 source_observed_at,
                 str(source.get("api_version") or "unknown"),
                 int(bool(source.get("observation_partial"))),
                 source.get("observation_error"),
                 str(source["repository_name"]), source_url,
                 project_id, task_id, timestamp),
            )
            self._event(connection, project_id, task_id, "task_created", {
                "source": "github_import"
            })
            self._update_project_status(connection, project_id)
            project_result = self._project_dict(
                connection, self._get_project_row(connection, project_id)
            )
            task_result = next(
                row for row in project_result["tasks"] if row["id"] == task_id
            )
            return project_result, task_result, True

    def edit_task(
        self, project_id: str, task_id: str, body: dict
    ) -> tuple[dict, dict]:
        with self._connect() as connection:
            project = self._get_project_row(connection, project_id)
            old = self._get_task_row(connection, project_id, task_id)
            if old["status"] not in EDITABLE_TASK_STATES:
                raise WorkflowProblem(
                    f"a task in {old['status']} state cannot be edited", 409
                )
            merged = {
                "title": body.get("title", old["title"]),
                "brief": body.get("brief", old["brief"]),
                "skill": body.get("skill", old["skill"]),
                "privacy": body.get("privacy", old["privacy"]),
                "checklist": body.get(
                    "checklist", self._json(old["checklist_json"], [])
                ),
            }
            task = self._normalise_task(merged, project["privacy"])
            timestamp = now()
            connection.execute(
                "UPDATE tasks SET title=?, brief=?, skill=?, privacy=?, "
                "checklist_json=?, checklist_source=?, status='draft', worker=NULL, "
                "model=NULL, quote_json=NULL, estimated_cost_micros=NULL, "
                "runner_ready=0, output=NULL, stderr=NULL, exit_code=NULL, "
                "check_json=NULL, actual_cost_micros=NULL, attempt_count=0, "
                "updated_at=? WHERE id=?",
                (task["title"], task["brief"], task["skill"], task["privacy"],
                 json.dumps(task["checklist"]), task["checklist_source"],
                 timestamp, task_id),
            )
            self._event(connection, project_id, task_id, "task_edited")
            self._update_project_status(connection, project_id)
        project_result = self.get_project(project_id)
        return project_result, next(
            task for task in project_result["tasks"] if task["id"] == task_id
        )

    def delete_task(self, project_id: str, task_id: str) -> dict:
        with self._connect() as connection:
            self._get_project_row(connection, project_id)
            task = self._get_task_row(connection, project_id, task_id)
            if task["status"] in ("running", "accepted"):
                raise WorkflowProblem(
                    f"a task in {task['status']} state cannot be deleted", 409
                )
            count = connection.execute(
                "SELECT COUNT(*) FROM tasks WHERE project_id=?", (project_id,)
            ).fetchone()[0]
            if count <= 1:
                raise WorkflowProblem(
                    "a project must keep at least one task; edit it instead", 409
                )
            old_position = task["position"]
            connection.execute("DELETE FROM tasks WHERE id=?", (task_id,))
            connection.execute(
                "UPDATE tasks SET position=position-1, updated_at=? "
                "WHERE project_id=? AND position>?",
                (now(), project_id, old_position),
            )
            self._event(connection, project_id, task_id, "task_deleted", {
                "title": task["title"], "position": old_position
            })
            self._update_project_status(connection, project_id)
            return self._project_dict(
                connection, self._get_project_row(connection, project_id)
            )

    def _allocation_request(self, project: sqlite3.Row, task: sqlite3.Row) -> dict:
        catalog = self.catalog_loader(self.home)
        return {
            "decision_id": new_id("decision:office"),
            "snapshot_id": catalog["snapshot"],
            "at_epoch_ms": int(time.time() * 1000),
            "created_at": now(),
            "task": {
                "id": task["id"],
                "summary": task["brief"][:200],
                "repository": project["id"],
                "required_skills": [{
                    "skill_id": task["skill"],
                    "minimum_success_probability": 0.24,
                    "minimum_evidence_count": 0,
                }],
                "required_tools": self.owi_do.SKILLS[task["skill"]]["tools"],
                "privacy": task["privacy"],
                "risk": "low",
                "verification": "deterministic",
                "minimum_success_probability": 0.24,
                "minimum_evidence_count": 0,
                "estimated_input_tokens": 1500 + len(task["brief"]) // 4,
                "estimated_output_tokens": 800,
            },
            "policy": {
                "policy_id": "policy:office-economy-v1",
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
                "default_p95_latency_ms": 30_000,
                "opportunity_micros_per_hour": 0,
                "review_minutes_on_accept": 3.0,
                "review_minutes_on_reject": 25.0,
                "expected_fallback_cash_micros": 20_000,
            },
        }

    def _allocate(self, project: sqlite3.Row, task: sqlite3.Row) -> dict:
        self._require_safe_local_ledger()
        request = self._allocation_request(project, task)
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", prefix="workflow-", dir=self.home,
            encoding="utf-8", delete=False
        ) as handle:
            json.dump(request, handle)
            path = Path(handle.name)
        try:
            return self.owi_do.owi(
                "allocate",
                "--index", str(self.home / "index.sqlite"),
                "--local", str(self.home / "local.sqlite"),
                "--input", str(path),
                "--record",
            )
        finally:
            path.unlink(missing_ok=True)

    def staff_project(
        self, project_id: str, task_ids: set[str] | None = None
    ) -> dict:
        with self._connect() as connection:
            project = self._get_project_row(connection, project_id)
            tasks = connection.execute(
                "SELECT * FROM tasks WHERE project_id=? ORDER BY position",
                (project_id,),
            ).fetchall()
            for task in tasks:
                if task_ids is not None and task["id"] not in task_ids:
                    continue
                if task["status"] == "setup_required":
                    runner_ready = bool(self.owi_do.resolve_worker_runner(
                        self.home, task["worker"] or "",
                        allow_model_fallback=False,
                    )[0])
                    if runner_ready:
                        connection.execute(
                            "UPDATE tasks SET status='staffed', runner_ready=1, "
                            "updated_at=? WHERE id=?", (now(), task["id"])
                        )
                        self._event(
                            connection, project_id, task["id"], "runner_ready",
                            {"model": task["model"]},
                        )
                    continue
                if task["status"] not in (
                    "draft", "unstaffed", "rejected", "run_failed"
                ):
                    continue
                try:
                    result = self._allocate(project, task)
                except (Exception, SystemExit) as error:
                    raise WorkflowProblem(f"staffing failed: {error}", 500) from error
                quote = result.get("quote", {})
                eligible = quote.get("eligible_candidates") or []
                if not eligible:
                    rejections = sorted({
                        reason.get("code", "unknown")
                        for candidate in quote.get("rejected_candidates", [])
                        for reason in candidate.get("reasons", [])
                    })
                    connection.execute(
                        "UPDATE tasks SET status='unstaffed', worker=NULL, model=NULL, "
                        "quote_json=?, estimated_cost_micros=NULL, runner_ready=0, "
                        "output=NULL, stderr=NULL, exit_code=NULL, check_json=NULL, "
                        "actual_cost_micros=NULL, updated_at=? WHERE id=?",
                        (json.dumps({"alternatives": [], "rejections": rejections}),
                         now(), task["id"]),
                    )
                    self._event(connection, project_id, task["id"],
                                "task_unstaffed", {"rejections": rejections})
                    continue
                selected_id = quote.get("selected_worker_id") or eligible[0]["worker_id"]
                selected = next(
                    (candidate for candidate in eligible
                     if candidate["worker_id"] == selected_id), eligible[0]
                )
                worker = selected["worker_id"]
                model = model_for_worker(worker)
                runner_ready = bool(self.owi_do.resolve_worker_runner(
                    self.home, worker, allow_model_fallback=False
                )[0])
                status = "staffed" if runner_ready else "setup_required"
                alternatives = [{
                    "worker": candidate["worker_id"],
                    "estimatedCostMicros": candidate["cost"][
                        "expected_accepted_cost_micros"
                    ],
                    "runnerReady": bool(
                        self.owi_do.resolve_worker_runner(
                            self.home, candidate["worker_id"],
                            allow_model_fallback=False,
                        )[0]
                    ),
                } for candidate in eligible[:4]]
                rejections = sorted({
                    reason.get("code", "unknown")
                    for candidate in quote.get("rejected_candidates", [])
                    for reason in candidate.get("reasons", [])
                })
                cost = selected["cost"]["expected_accepted_cost_micros"]
                connection.execute(
                    "UPDATE tasks SET status=?, worker=?, model=?, quote_json=?, "
                    "estimated_cost_micros=?, runner_ready=?, output=NULL, "
                    "stderr=NULL, exit_code=NULL, check_json=NULL, "
                    "actual_cost_micros=NULL, updated_at=? WHERE id=?",
                    (status, worker, model,
                     json.dumps({"alternatives": alternatives,
                                 "rejections": rejections}),
                     cost, int(runner_ready), now(), task["id"]),
                )
                self._event(connection, project_id, task["id"], "task_staffed", {
                    "worker": worker,
                    "estimatedCostMicros": cost,
                    "runnerReady": runner_ready,
                })
            self._update_project_status(connection, project_id)
            return self._project_dict(
                connection, self._get_project_row(connection, project_id)
            )

    def _update_project_status(
        self, connection: sqlite3.Connection, project_id: str
    ) -> None:
        states = [row[0] for row in connection.execute(
            "SELECT status FROM tasks WHERE project_id=?", (project_id,)
        )]
        if states and all(state == "accepted" for state in states):
            status = "completed"
        elif any(state == "running" for state in states):
            status = "running"
        elif any(state == "needs_review" for state in states):
            status = "needs_review"
        elif any(state == "draft" for state in states):
            status = "draft"
        elif any(state in ("unstaffed", "setup_required") for state in states):
            status = "blocked"
        elif any(state in ("rejected", "run_failed") for state in states):
            status = "attention_needed"
        else:
            status = "staffed"
        connection.execute(
            "UPDATE projects SET status=?, updated_at=? WHERE id=?",
            (status, now(), project_id),
        )

    def _mechanical_checklist(self, output: str, items: list[str]) -> dict:
        """Check locally; judgement never bypasses allocator/privacy/budget."""
        results = []
        for item in items:
            if item.strip().lower().startswith("regex:"):
                kind, verdict, note = (
                    "regex", None,
                    "regex checks require review; they are not executed in the server",
                )
            else:
                kind, verdict, note = self.owi_do.check_item(item, output)
            results.append({
                "item": item, "kind": kind, "pass": verdict, "note": note
            })
        failed = any(item["pass"] is False for item in results)
        undecided = any(item["pass"] is None for item in results)
        verdict = "rejected" if failed else "undecided" if undecided else "accepted"
        return {
            "items": results,
            "verdict": verdict,
            "judge": None,
            "judgementPolicy": "human_required_until_checker_is_staffed",
        }

    def run_task(self, project_id: str, task_id: str) -> tuple[dict, dict]:
        self._require_safe_local_ledger()
        with self._connect() as connection:
            project_row = self._get_project_row(connection, project_id)
            task = self._get_task_row(connection, project_id, task_id)
            if task["attempt_count"] >= MAX_ATTEMPTS:
                raise WorkflowProblem(
                    f"this task reached its {MAX_ATTEMPTS}-attempt safety cap; "
                    "edit the task specification before running it again", 409
                )
            estimate_row = connection.execute(
                "SELECT COUNT(*), COUNT(estimated_cost_micros), "
                "SUM(estimated_cost_micros) FROM tasks WHERE project_id=?",
                (project_id,),
            ).fetchone()
            if estimate_row[0] != estimate_row[1]:
                raise WorkflowProblem(
                    "staff every project task before running work", 409
                )
            estimated_total = estimate_row[2] or 0
            if (project_row["budget_micros"] is not None
                    and estimated_total > project_row["budget_micros"]):
                raise WorkflowProblem(
                    "project is over budget; reduce/re-staff its tasks or create "
                    "a project with a sufficient budget before running", 409
                )
            if task["status"] == "setup_required":
                raise WorkflowProblem(
                    f"no runner is configured for {task['model']}", 409
                )
            if task["status"] == "unstaffed":
                raise WorkflowProblem("the task has no qualified worker", 409)
            if task["status"] not in RUNNABLE_TASK_STATES:
                raise WorkflowProblem(
                    f"a task in {task['status']} state cannot be run", 409
                )
            # Re-resolve the command immediately before execution.  The browser
            # never sees or supplies it, and a removed runner fails closed.
            command, _ = self.owi_do.resolve_worker_runner(
                self.home, task["worker"], allow_model_fallback=False
            )
            if not command:
                connection.execute(
                    "UPDATE tasks SET status='setup_required', runner_ready=0, "
                    "updated_at=? WHERE id=?", (now(), task_id)
                )
                self._update_project_status(connection, project_id)
                raise WorkflowProblem(
                    f"no runner is configured for {task['model']}", 409
                )
            checklist = self._json(task["checklist_json"], [])
            worker = task["worker"]
            inspection = (
                {"level": "mechanical", "judge": False,
                 "why": "mechanical checks run locally; judgement requires review"}
                if checklist else self.owi_do.inspection_for(
                    self.home, worker, task["skill"]
                )
            )
            notes = self.owi_do.prevention_notes(self.home, task["skill"])
            payload = self.owi_do.build_payload(
                task["brief"], "", checklist, notes
            )
            connection.execute(
                "UPDATE tasks SET status='running', "
                "attempt_count=attempt_count+1, updated_at=? WHERE id=?",
                (now(), task_id),
            )
            self._update_project_status(connection, project_id)
            self._event(connection, project_id, task_id, "task_run_started", {
                "worker": worker
            })

        run_started_ns = time.monotonic_ns()
        try:
            stdout, stderr, exit_code = self.owi_do.run_worker_command(
                self.home, task["worker"], payload, timeout=300,
                allow_model_fallback=False,
            )
        except subprocess.TimeoutExpired:
            stdout, stderr, exit_code = "", "timed out after 300s", -1
        except Exception as error:
            stdout, stderr, exit_code = "", f"runner could not start: {error}", -1
        measured_latency_ms = max(
            0, (time.monotonic_ns() - run_started_ns) // 1_000_000
        )

        stdout = (stdout or "")[-MAX_OUTPUT_CHARS:]
        stderr = (stderr or "")[-MAX_STDERR_CHARS:]
        report = None
        if exit_code != 0 or not stdout.strip():
            status = "run_failed"
        elif checklist:
            try:
                report = self._mechanical_checklist(stdout, checklist)
                status = {
                    "accepted": "accepted",
                    # A mechanical failure is evidence, not root-cause proof.
                    # The owner decides whether the worker, task, harness, or
                    # environment caused it before calibration changes.
                    "rejected": "needs_review",
                    "undecided": "needs_review",
                }[report["verdict"]]
            except Exception as error:
                report = {
                    "items": [], "verdict": "undecided", "judge": None,
                    "judgementPolicy": "human_required_until_checker_is_staffed",
                    "verificationError": str(error)[:300],
                }
                status = "needs_review"
        else:
            # No acceptance criteria means no automatic quality claim.
            status = "needs_review"

        if status in ("accepted", "rejected"):
            detail = ""
            if report and status == "rejected":
                detail = "; ".join(
                    f"failed: {item['item']} ({item['note']})"
                    for item in report["items"] if item["pass"] is False
                )
            try:
                self.owi_do.record_outcome(
                    self.home,
                    task["worker"],
                    task["skill"],
                    status == "accepted",
                    None if status == "accepted" else "worker",
                    task=task["brief"],
                    detail=detail,
                    checklist=report["items"] if report else None,
                    inspection=inspection["level"],
                    privacy=task["privacy"],
                    task_id=task["id"],
                    validation_kind="deterministic",
                    latency_ms=measured_latency_ms,
                )
            except (Exception, SystemExit) as error:
                if report is not None:
                    report["outcomeRecordingError"] = str(error)[:300]
                status = "needs_review"

        with self._connect() as connection:
            connection.execute(
                "UPDATE tasks SET status=?, output=?, stderr=?, exit_code=?, "
                "check_json=?, updated_at=? WHERE id=?",
                (status, stdout, stderr, exit_code,
                 json.dumps(report) if report else None, now(), task_id),
            )
            self._event(connection, project_id, task_id, "task_run_finished", {
                "status": status, "exitCode": exit_code
            })
            self._update_project_status(connection, project_id)
            project = self._project_dict(
                connection, self._get_project_row(connection, project_id)
            )
        return project, next(item for item in project["tasks"] if item["id"] == task_id)

    def review_task(
        self, project_id: str, task_id: str, body: dict
    ) -> tuple[dict, dict]:
        self._require_safe_local_ledger()
        if not isinstance(body.get("accepted"), bool):
            raise WorkflowProblem("accepted must be true or false")
        accepted = body["accepted"]
        cause = body.get("cause")
        if not accepted:
            cause = str(cause or "worker")
            if cause not in CAUSES:
                raise WorkflowProblem("unknown rejection cause")
        with self._connect() as connection:
            self._get_project_row(connection, project_id)
            task = self._get_task_row(connection, project_id, task_id)
            if task["status"] != "needs_review":
                raise WorkflowProblem(
                    f"a task in {task['status']} state cannot be reviewed", 409
                )
            if not (task["output"] or "").strip():
                raise WorkflowProblem("there is no model output to review", 409)
            report = self._json(task["check_json"], None)
            self.owi_do.record_outcome(
                self.home,
                task["worker"],
                task["skill"],
                accepted,
                None if accepted else cause,
                task=task["brief"],
                detail=str(body.get("detail") or "")[:500],
                checklist=report.get("items") if isinstance(report, dict) else None,
                inspection="human",
                privacy=task["privacy"],
                task_id=task["id"],
                validation_kind="human",
            )
            status = "accepted" if accepted else "rejected"
            connection.execute(
                "UPDATE tasks SET status=?, updated_at=? WHERE id=?",
                (status, now(), task_id),
            )
            self._event(connection, project_id, task_id, "task_reviewed", {
                "accepted": accepted, **({"cause": cause} if not accepted else {})
            })
            self._update_project_status(connection, project_id)
            project = self._project_dict(
                connection, self._get_project_row(connection, project_id)
            )
        return project, next(item for item in project["tasks"] if item["id"] == task_id)
