#!/usr/bin/env python3
"""End-to-end contract checks for the persisted OWI project workflow."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import os
import sqlite3
import tempfile
import threading
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path
from urllib.parse import quote

from owi_workflow import WorkflowProblem, WorkflowService


TOOLS = Path(__file__).resolve().parent
loader = importlib.machinery.SourceFileLoader(
    "owi_serve_workflow_check", str(TOOLS / "owi-serve")
)
spec = importlib.util.spec_from_loader("owi_serve_workflow_check", loader)
owi_serve = importlib.util.module_from_spec(spec)
loader.exec_module(owi_serve)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"workflow runtime check failed: {message}")


def sqlite_positions(home: Path, project_id: str) -> list[tuple[int]]:
    connection = sqlite3.connect(home / "workflow.sqlite")
    try:
        return connection.execute(
            "SELECT position FROM tasks WHERE project_id=? ORDER BY position",
            (project_id,),
        ).fetchall()
    finally:
        connection.close()


class FakeOwi:
    SKILLS = {
        "skill:text-editing": {"tools": []},
        "skill:python-numerical-implementation": {"tools": ["shell"]},
    }

    def __init__(self) -> None:
        self.runners = {"worker:writer/text": "fake writer"}
        self.outputs = {"writer": "READY: finished result"}
        self.outcomes: list[dict] = []
        self.snapshot_ids: list[str] = []
        self.fail_record = False

    def classify(self, brief: str) -> str:
        return (
            "skill:python-numerical-implementation"
            if "code" in brief.lower()
            else "skill:text-editing"
        )

    @staticmethod
    def clean_checklist(raw) -> list[str]:
        return [str(item).strip() for item in raw if str(item).strip()][:4]

    def resolve_runner(self, _home: Path, model: str):
        return self.runners.get(model)

    def resolve_worker_runner(
        self, _home: Path, worker_id: str, allow_model_fallback: bool = True
    ):
        if worker_id in self.runners:
            return self.runners[worker_id], "worker"
        if not allow_model_fallback:
            return None, None
        model = worker_id.removeprefix("worker:").split("/", 1)[0]
        command = self.runners.get(model)
        return (command, "model_fallback") if command else (None, None)

    @staticmethod
    def plan_parts(_home: Path, _goal: str):
        return None, None

    def owi(self, command: str, *args: str) -> dict:
        require(command == "allocate", "workflow used something except allocator")
        input_path = Path(args[args.index("--input") + 1])
        request = json.loads(input_path.read_text(encoding="utf-8"))
        self.snapshot_ids.append(request["snapshot_id"])
        skill = request["task"]["required_skills"][0]["skill_id"]
        if skill == "skill:text-editing":
            workers = [("worker:writer/text", 12_000),
                       ("worker:backup/text", 19_000)]
        else:
            workers = [("worker:coder/code", 25_000)]
        eligible = [{
            "worker_id": worker,
            "cost": {"expected_accepted_cost_micros": cost},
        } for worker, cost in workers]
        return {"quote": {
            "selected_worker_id": workers[0][0],
            "eligible_candidates": eligible,
            "rejected_candidates": [{
                "worker_id": "worker:blocked/text",
                "reasons": [{"code": "missing_skill"}],
            }],
        }}

    @staticmethod
    def inspection_for(_home: Path, _worker: str, _skill: str) -> dict:
        return {"level": "full", "judge": True, "why": "test"}

    @staticmethod
    def prevention_notes(_home: Path, _skill: str) -> list[str]:
        return []

    @staticmethod
    def build_payload(task: str, _stdin: str, checklist: list[str], _notes) -> str:
        return task + "\n" + "\n".join(checklist)

    def run_command(
        self, _home: Path, model: str, _payload: str, timeout: int = 300
    ) -> tuple[str, str, int]:
        require(timeout == 300, "workflow changed the bounded runner timeout")
        return self.outputs[model], "", 0

    def run_worker_command(
        self, home: Path, worker_id: str, payload: str, timeout: int = 300,
        allow_model_fallback: bool = True,
    ) -> tuple[str, str, int]:
        command, _ = self.resolve_worker_runner(
            home, worker_id, allow_model_fallback=allow_model_fallback
        )
        if not command:
            raise RuntimeError("no exact runner")
        model = worker_id.removeprefix("worker:").split("/", 1)[0]
        return self.run_command(home, model, payload, timeout)

    @staticmethod
    def check_item(item: str, output: str):
        if item.lower().startswith("contains:"):
            needle = item.split(":", 1)[1].strip()
            return "contains", needle.lower() in output.lower(), \
                f'output contains "{needle}"'
        return "judged", None, "needs judgement"

    @staticmethod
    def run_checklist(
        _home: Path, _task: str, output: str, items: list[str],
        maker_model: str, judge_enabled: bool = True
    ) -> dict:
        require(maker_model == "writer", "checker got a browser-supplied maker")
        require(judge_enabled, "explicit criteria were sampled out")
        results = []
        for item in items:
            needle = item.split(":", 1)[1].strip()
            passed = needle.lower() in output.lower()
            results.append({"item": item, "kind": "contains", "pass": passed,
                            "note": f'output contains "{needle}"'})
        return {"items": results,
                "verdict": "accepted" if all(x["pass"] for x in results)
                           else "rejected",
                "judge": None}

    def record_outcome(
        self, _home: Path, worker: str, skill: str, accepted: bool,
        cause=None, **metadata
    ) -> None:
        if self.fail_record:
            raise RuntimeError("private ledger unavailable")
        self.outcomes.append({"worker": worker, "skill": skill,
                              "accepted": accepted, "cause": cause,
                              "metadata": metadata})


def catalog(_home: Path) -> dict:
    return {"snapshot": "snapshot:user-real", "workers": []}


def check_lifecycle_and_persistence() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        fake = FakeOwi()
        workflow = WorkflowService(home, fake, catalog)
        project = workflow.create_project({
            "name": "Release office",
            "goal": "Prepare and implement the release",
            "budgetMicros": 100_000,
            "tasks": [
                {"title": "Write release note", "brief": "Write the note",
                 "skill": "skill:text-editing",
                 "checklist": ["contains:READY"]},
                {"title": "Implement", "brief": "Code the feature",
                 "skill": "skill:python-numerical-implementation"},
            ],
        })
        require(project["planningSource"] == "user",
                "manual task plan was relabelled as AI planning")
        require([task["status"] for task in project["tasks"]] ==
                ["draft", "draft"],
                "project creation skipped the visible draft stage")
        require(project["totals"]["estimatedCostMicros"] is None
                and project["totals"]["selectedSubtotalMicros"] == 0
                and project["totals"]["estimateCoverage"] == {
                    "estimated": 0, "tasks": 2
                }, "missing staffing estimates were encoded as zero")
        require(fake.snapshot_ids == [],
                "project creation silently performed staffing")
        project = workflow.staff_project(project["id"])
        require([task["status"] for task in project["tasks"]] ==
                ["staffed", "setup_required"],
                "tasks were not independently staffed/runner-gated")
        require(project["tasks"][0]["runnerBinding"] == "worker",
                "exact worker runner binding was not preserved")
        require(project["totals"]["estimatedCostMicros"] == 37_000,
                "project estimate did not sum accepted-result costs")
        require(project["totals"]["overBudget"] is False,
                "within-budget plan looks over budget")
        require(fake.snapshot_ids == ["snapshot:user-real"] * 2,
                "allocator did not receive the workspace's current snapshot")
        fake.runners["worker:coder/code"] = "fake coder"
        refreshed = workflow.staff_project(project["id"])
        require(fake.snapshot_ids == ["snapshot:user-real"] * 2,
                "runner refresh recorded a duplicate allocation quote")
        require(refreshed["tasks"][1]["status"] == "staffed",
                "configured runner did not unblock saved staffing")
        del fake.runners["worker:coder/code"]

        # A fresh service reads the same state: this is not browser memory.
        reopened = WorkflowService(home, fake, catalog)
        require(reopened.current()["id"] == project["id"],
                "current project did not survive service restart")
        require((home / "workflow.sqlite").is_file(),
                "workflow state was not persisted")

        first = project["tasks"][0]
        project, completed = reopened.run_task(project["id"], first["id"])
        require(completed["status"] == "accepted",
                "passing checked output was not accepted")
        require(completed["output"] == "READY: finished result",
                "runner output was not stored")
        require(completed["checks"]["verdict"] == "accepted",
                "check report was not persisted with the task")
        require(fake.outcomes[-1]["worker"] == "worker:writer/text",
                "outcome identity did not come from persisted staffing")
        require(fake.outcomes[-1]["metadata"]["task_id"] == first["id"],
                "outcome ledger lost the persisted project task identity")
        require(fake.outcomes[-1]["metadata"]["validation_kind"] ==
                "deterministic"
                and isinstance(fake.outcomes[-1]["metadata"]["latency_ms"], int),
                "automatic check was labelled human or lost measured latency")

        second = refreshed["tasks"][1]
        try:
            reopened.run_task(project["id"], second["id"])
        except WorkflowProblem as error:
            require(error.status == 409 and "no runner" in str(error),
                    "setup-required task failed unclearly")
        else:
            raise SystemExit("workflow runtime check failed: missing runner executed")

        # Editing invalidates the old staffing and re-routes through Rust.
        project, edited = reopened.edit_task(project["id"], second["id"], {
            "brief": "Write the implementation guide",
            "skill": "skill:text-editing",
        })
        require(edited["worker"] is None and edited["status"] == "draft",
                "edited task retained stale staffing or skipped draft")
        project = reopened.staff_project(project["id"])
        edited = next(task for task in project["tasks"]
                      if task["id"] == edited["id"])
        require(edited["worker"] == "worker:writer/text",
                "explicit staff action did not allocate edited task")
        project, needs_review = reopened.run_task(
            project["id"], edited["id"]
        )
        require(needs_review["status"] == "needs_review",
                "task without criteria made an automatic quality claim")
        project, reviewed = reopened.review_task(
            project["id"], edited["id"], {"accepted": True}
        )
        require(reviewed["status"] == "accepted"
                and project["status"] == "completed",
                "human review did not finish the project")
        require(fake.outcomes[-1]["worker"] == "worker:writer/text",
                "review trusted a worker identity from the caller")

        # Rejected/failed work may be retried with the same server-side identity.
        project, added = reopened.add_task(project["id"], {
            "brief": "Write retryable result", "checklist": ["contains:MISSING"]
        })
        require(added["status"] == "draft" and added["worker"] is None,
                "new task skipped the explicit staffing stage")
        project = reopened.staff_project(project["id"])
        added = next(task for task in project["tasks"]
                     if task["id"] == added["id"])
        project, rejected = reopened.run_task(project["id"], added["id"])
        require(rejected["status"] == "needs_review"
                and rejected["checks"]["verdict"] == "rejected",
                "mechanical failure was hidden or blamed automatically")
        project, rejected = reopened.review_task(
            project["id"], added["id"],
            {"accepted": False, "cause": "worker",
             "detail": "owner confirmed the model missed the requirement"},
        )
        require(rejected["status"] == "rejected",
                "owner-confirmed rejection was not recorded")
        fake.outputs["writer"] = "READY and MISSING are now included"
        _, retried = reopened.run_task(project["id"], added["id"])
        require(retried["status"] == "accepted",
                "POST run did not support retry after rejection")

        # Judgement items never trigger an unstaffed checker/model. They stay
        # visibly open for human review, and the two-attempt spend brake holds.
        project, judged = reopened.add_task(project["id"], {
            "brief": "Write something requiring judgement",
            "checklist": ["reads as a professional final answer",
                          "regex:(a+)+$"],
        })
        project = reopened.staff_project(project["id"])
        judged = next(task for task in project["tasks"] if task["id"] == judged["id"])
        project, judged = reopened.run_task(project["id"], judged["id"])
        require(judged["status"] == "needs_review"
                and judged["checks"]["judge"] is None
                and judged["checks"]["items"][0]["pass"] is None
                and judged["checks"]["items"][1]["pass"] is None
                and "not executed" in judged["checks"]["items"][1]["note"],
                "workflow bypassed staffing/privacy to invoke a model judge")
        project, judged = reopened.run_task(project["id"], judged["id"])
        require(judged["attemptCount"] == 2,
                "run attempts were not persisted")
        try:
            reopened.run_task(project["id"], judged["id"])
        except WorkflowProblem as error:
            require(error.status == 409 and "attempt" in str(error),
                    "attempt safety cap did not stop repeated spend")
        else:
            raise SystemExit(
                "workflow runtime check failed: third task attempt executed"
            )
        project, judged = reopened.edit_task(project["id"], judged["id"], {
            "brief": "Edited judgement task"
        })
        require(judged["attemptCount"] == 0 and judged["status"] == "draft",
                "editing the specification did not reset its attempt cap")
        project = reopened.delete_task(project["id"], judged["id"])

        # Failure can also trigger a fresh allocator decision; the stale output
        # is cleared before a possibly different worker receives the task.
        fake.outputs["writer"] = "still does not contain the needle"
        project, restaffable = reopened.add_task(project["id"], {
            "brief": "Write a restaffable result",
            "checklist": ["contains:NEVER"],
        })
        require(project["status"] == "draft"
                and project["totals"]["estimatedCostMicros"] is None,
                "adding a draft left a completed/zero-cost project status")
        project = reopened.staff_project(project["id"])
        restaffable = next(task for task in project["tasks"]
                           if task["id"] == restaffable["id"])
        project, restaffable = reopened.run_task(
            project["id"], restaffable["id"]
        )
        require(restaffable["status"] == "needs_review",
                "mechanical failure skipped owner review")
        project, restaffable = reopened.review_task(
            project["id"], restaffable["id"],
            {"accepted": False, "cause": "worker"},
        )
        require(restaffable["status"] == "rejected",
                "restaff regression setup did not confirm rejection")
        decisions_before = len(fake.snapshot_ids)
        project = reopened.staff_project(project["id"])
        restaffed = next(task for task in project["tasks"]
                         if task["id"] == restaffable["id"])
        require(len(fake.snapshot_ids) == decisions_before + 1
                and restaffed["status"] == "staffed"
                and restaffed["output"] == "",
                "staff after failure did not create a clean allocator decision")

        project = reopened.delete_task(project["id"], restaffed["id"])
        require(all(task["id"] != restaffed["id"] for task in project["tasks"]),
                "editable task was not deleted")
        require([row[0] for row in sqlite_positions(home, project["id"])] ==
                list(range(1, len(project["tasks"]) + 1)),
                "task positions were not compacted after delete")


def check_single_task_fallback() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        fake = FakeOwi()
        workflow = WorkflowService(Path(scratch), fake, catalog)
        project = workflow.create_project({"goal": "Write one clear summary"})
        require(project["planningSource"] ==
                "planner_not_requested_single_task_fallback",
                "opt-in planning fallback was not disclosed")
        require("not requested" in project["planningProblem"],
                "planner opt-in state has no user-facing explanation")
        require(len(project["tasks"]) == 1,
                "workflow invented an unrequested model decomposition")

    with tempfile.TemporaryDirectory() as scratch:
        fake = FakeOwi()
        fake.runners = {"writer": "legacy model-wide command"}
        workflow = WorkflowService(Path(scratch), fake, catalog)
        project = workflow.create_project({
            "goal": "Do not collapse identity",
            "tasks": [{"brief": "Do not collapse identity"}],
        })
        project = workflow.staff_project(project["id"])
        require(project["tasks"][0]["status"] == "setup_required"
                and project["tasks"][0]["runnerBinding"] is None,
                "workflow executed a model-wide command for an exact worker quote")


def check_recovery_and_outcome_failure() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        fake = FakeOwi()
        workflow = WorkflowService(home, fake, catalog)
        project = workflow.create_project({
            "goal": "Write a recoverable result",
            "tasks": [{"brief": "Write a recoverable result"}],
        })
        project = workflow.staff_project(project["id"])
        task = project["tasks"][0]
        connection = sqlite3.connect(home / "workflow.sqlite")
        connection.execute(
            "UPDATE tasks SET status='running', attempt_count=1 WHERE id=?",
            (task["id"],),
        )
        connection.commit()
        connection.close()
        restarted = WorkflowService(home, fake, catalog)
        require(restarted.recover_interrupted_runs() == 1,
                "server restart did not recover interrupted run")
        recovered = restarted.current()["tasks"][0]
        require(recovered["status"] == "run_failed"
                and "server stopped" in recovered["stderr"],
                "interrupted run stayed permanently running")

    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        fake = FakeOwi()
        workflow = WorkflowService(home, fake, catalog)
        project = workflow.create_project({
            "goal": "Write a checked result",
            "tasks": [{"brief": "Write a checked result",
                       "checklist": ["contains:READY"]}],
        })
        project = workflow.staff_project(project["id"])
        fake.fail_record = True
        project, task = workflow.run_task(project["id"], project["tasks"][0]["id"])
        require(task["status"] == "needs_review"
                and task["output"] == "READY: finished result"
                and "outcomeRecordingError" in task["checks"],
                "ledger failure lost output or stranded the task as running")


def check_planner_decomposition_and_budget_gate() -> None:
    class PlanningOwi(FakeOwi):
        @staticmethod
        def plan_parts(_home: Path, _goal: str):
            return "planner-pro", [
                {"summary": "Write the launch note",
                 "skill": "skill:text-editing",
                 "checklist": ["contains:READY"]},
                {"summary": "Code the launch endpoint",
                 "skill": "skill:python-numerical-implementation",
                 "checklist": []},
            ]

    with tempfile.TemporaryDirectory() as scratch:
        fake = PlanningOwi()
        workflow = WorkflowService(Path(scratch), fake, catalog)
        project = workflow.create_project({
            "goal": "Plan and deliver the launch", "budgetMicros": 10_000,
            "usePlanningModel": True,
        })
        require(project["planningSource"] == "model:planner-pro",
                "runnable planner was not used for project decomposition")
        require(project["actualSpendEnforced"] is False
                and project["budgetBasis"] ==
                "forecast_ceiling_expected_accepted_cost",
                "forecast budget was presented as an actual spend cap")
        require([task["skill"] for task in project["tasks"]] == [
            "skill:text-editing", "skill:python-numerical-implementation"
        ], "planner-created specialist tasks were lost")
        require(project["tasks"][0]["checklistSource"] == "planner",
                "planner acceptance criteria were labelled as user input")
        project = workflow.staff_project(project["id"])
        require(project["totals"]["overBudget"] is True,
                "over-budget plan was not reported")
        try:
            workflow.run_task(project["id"], project["tasks"][0]["id"])
        except WorkflowProblem as error:
            require(error.status == 409 and "over budget" in str(error),
                    "server did not enforce the project budget")
        else:
            raise SystemExit(
                "workflow runtime check failed: over-budget task was executed"
            )

    class PrivacySpy(PlanningOwi):
        def __init__(self) -> None:
            super().__init__()
            self.plan_calls = 0

        def plan_parts(self, _home: Path, _goal: str):
            self.plan_calls += 1
            return super().plan_parts(_home, _goal)

    with tempfile.TemporaryDirectory() as scratch:
        fake = PrivacySpy()
        workflow = WorkflowService(Path(scratch), fake, catalog)
        project = workflow.create_project({
            "goal": "1. Analyze the secret\n2. Write the private result",
            "privacy": "secret",
            "usePlanningModel": True,
        })
        require(fake.plan_calls == 0,
                "secret project text was sent to an unstaffed planner")
        require(project["planningSource"].startswith("planner_blocked_privacy")
                and len(project["tasks"]) == 2,
                "privacy-safe deterministic decomposition was not disclosed")

    with tempfile.TemporaryDirectory() as scratch:
        fake = PrivacySpy()
        workflow = WorkflowService(Path(scratch), fake, catalog)
        project = workflow.create_project({
            "goal": "Plan nothing paid", "budgetMicros": 0,
            "usePlanningModel": True,
        })
        require(fake.plan_calls == 0
                and project["planningSource"].startswith("planner_blocked_budget"),
                "zero-budget project still spent money on a planning model")


def check_symlink_rejected() -> None:
    if os.name == "nt" or not hasattr(os, "symlink"):
        return
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        real = root / "real.sqlite"
        real.write_text("do not overwrite", encoding="utf-8")
        home = root / "home"
        home.mkdir()
        (home / "workflow.sqlite").symlink_to(real)
        workflow = WorkflowService(home, FakeOwi(), catalog)
        try:
            workflow.current()
        except WorkflowProblem as error:
            require(error.status == 409 and "symlink" in str(error),
                    "symlink database was not rejected clearly")
        else:
            raise SystemExit(
                "workflow runtime check failed: workflow.sqlite followed symlink"
            )
        require(real.read_text(encoding="utf-8") == "do not overwrite",
                "symlink target was modified")

    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        home = root / "home"
        home.mkdir()
        external = root / "external-runners.json"
        external.write_text('{"worker:writer/text":"unsafe"}', encoding="utf-8")
        (home / "runners.json").symlink_to(external)
        try:
            owi_serve.owi_do.resolve_worker_runner(
                home, "worker:writer/text", allow_model_fallback=False
            )
        except owi_serve.owi_do.OwiSetupError as error:
            require("symlink" in str(error),
                    "runner symlink was not rejected clearly")
        else:
            raise SystemExit(
                "workflow runtime check failed: runners.json followed symlink"
            )

    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        home = root / "home"
        home.mkdir()
        external = root / "external-ledger.sqlite"
        external.write_text("do not overwrite", encoding="utf-8")
        (home / "local.sqlite").symlink_to(external)
        workflow = WorkflowService(home, FakeOwi(), catalog)
        project = workflow.create_project({
            "goal": "Do not redirect ledger",
            "tasks": [{"brief": "Do not redirect ledger"}],
        })
        try:
            workflow.staff_project(project["id"])
        except WorkflowProblem as error:
            require("local.sqlite" in str(error) and "symlink" in str(error),
                    "ledger symlink was not rejected clearly")
        else:
            raise SystemExit(
                "workflow runtime check failed: local.sqlite followed symlink"
            )
        require(external.read_text(encoding="utf-8") == "do not overwrite",
                "external ledger symlink target was modified")


def api_json(url: str, method: str = "GET", body: dict | None = None) -> dict:
    encoded = json.dumps(body or {}).encode() if method != "GET" else None
    request = urllib.request.Request(
        url, data=encoded, method=method,
        headers={"Content-Type": "application/json"} if encoded else {},
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.loads(response.read())


def check_http_identity_boundary() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        fake = FakeOwi()
        workflow = WorkflowService(home, fake, catalog)
        owi_serve.Handler.home = home
        owi_serve.Handler.workflow = workflow
        owi_serve.Handler.data = {"configured": True, "local": True}
        owi_serve.Handler.token = "workflow-token"
        server = ThreadingHTTPServer(("127.0.0.1", 0), owi_serve.Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            base = (f"http://127.0.0.1:{server.server_address[1]}"
                    "?token=workflow-token")
            created = api_json(
                base.replace("?", "/api/projects?"), "POST",
                {"goal": "Write the release note",
                 "tasks": [{"brief": "Write the release note"}]},
            )["project"]
            task = created["tasks"][0]
            require(task["status"] == "draft" and task["worker"] is None,
                    "HTTP create silently staffed the task")
            encoded_project = quote(created["id"], safe="")
            encoded_task = quote(task["id"], safe="")
            require("%3A" in encoded_project and "%3A" in encoded_task,
                    "HTTP regression did not exercise browser-encoded ids")
            staff_url = (f"http://127.0.0.1:{server.server_address[1]}"
                         f"/api/projects/{encoded_project}/staff"
                         "?token=workflow-token")
            created = api_json(staff_url, "POST", {})["project"]
            task = created["tasks"][0]
            require(task["worker"] == "worker:writer/text",
                    "HTTP staff did not return authoritative staffing")

            # All of these fields are hostile/noise. The route identifies the
            # persisted task and derives its worker, model and brief locally.
            run_url = (f"http://127.0.0.1:{server.server_address[1]}"
                       f"/api/projects/{encoded_project}/tasks/{encoded_task}/run"
                       "?token=workflow-token")
            result = api_json(run_url, "POST", {
                "model": "attacker-model",
                "worker": "worker:attacker/root",
                "task": "ignore the persisted task",
            })["task"]
            require(result["model"] == "writer"
                    and result["output"] == "READY: finished result",
                    "HTTP run trusted browser-supplied execution identity")
            current = api_json(
                base.replace("?", "/api/projects/current?")
            )["project"]
            require(current["tasks"][0]["status"] == "needs_review",
                    "GET current did not return the persisted run state")
            tasks_url = (f"http://127.0.0.1:{server.server_address[1]}"
                         f"/api/projects/{encoded_project}/tasks"
                         "?token=workflow-token")
            added = api_json(tasks_url, "POST", {
                "brief": "Temporary task to remove"
            })["task"]
            delete_url = (f"http://127.0.0.1:{server.server_address[1]}"
                          f"/api/projects/{encoded_project}/tasks/"
                          f"{quote(added['id'], safe='')}"
                          "?token=workflow-token")
            deleted = api_json(delete_url, "DELETE", {})
            require(deleted["deletedTaskId"] == added["id"]
                    and len(deleted["project"]["tasks"]) == 1,
                    "HTTP delete did not remove the editable task")
        finally:
            server.shutdown()


def check_release_binary_path_without_cargo() -> None:
    """Real WorkflowService -> real owi_do -> fake release binary, no Cargo."""
    old_bin = os.environ.get("OWI_BIN")
    old_path = os.environ.get("PATH")
    old_repo = owi_serve.owi_do.REPO
    try:
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            engine = root / "fake engine" / "owi"
            engine.parent.mkdir()
            engine.write_text(
                "#!/usr/bin/python3\n"
                "import json, sys\n"
                "assert sys.argv[1] == 'allocate'\n"
                "print(json.dumps({'quote': {"
                "'selected_worker_id': 'worker:fake/text', "
                "'eligible_candidates': [{'worker_id': 'worker:fake/text', "
                "'cost': {'expected_accepted_cost_micros': 4321}}], "
                "'rejected_candidates': []}}))\n",
                encoding="utf-8",
            )
            engine.chmod(0o700)
            os.environ["OWI_BIN"] = str(engine)
            # Prove this path does not accidentally find or invoke Cargo.
            os.environ["PATH"] = ""
            home = root / "home"
            home.mkdir()
            (home / "runners.json").write_text(
                json.dumps({"worker:fake/text": "/usr/bin/true"}),
                encoding="utf-8"
            )
            workflow = WorkflowService(home, owi_serve.owi_do, catalog)
            project = workflow.create_project({
                "goal": "Write through the installed engine",
                "tasks": [{"brief": "Write through the installed engine"}],
            })
            project = workflow.staff_project(project["id"])
            task = project["tasks"][0]
            require(task["worker"] == "worker:fake/text"
                    and task["estimatedCostMicros"] == 4321,
                    "workflow staffing did not use OWI_BIN without Cargo")
            require(task["status"] == "staffed",
                    "release-binary staffing lost runner readiness")
            require(task["runnerBinding"] == "worker",
                    "exact worker runner was collapsed to a model-wide command")

        os.environ.pop("OWI_BIN", None)
        os.environ["PATH"] = ""
        # CI has just built target/debug/owi in the real repository. Point the
        # resolver at an empty location so this branch tests a genuinely
        # missing engine on developer machines and CI alike.
        owi_serve.owi_do.REPO = root / "missing-repository"
        try:
            owi_serve.owi_do.resolve_owi_command()
        except owi_serve.owi_do.OwiSetupError as error:
            require("Install an OWI release binary" in str(error),
                    "missing engine error has no actionable setup guidance")
        else:
            raise SystemExit(
                "workflow runtime check failed: missing engine failed unclearly"
            )
    finally:
        if old_bin is None:
            os.environ.pop("OWI_BIN", None)
        else:
            os.environ["OWI_BIN"] = old_bin
        if old_path is None:
            os.environ.pop("PATH", None)
        else:
            os.environ["PATH"] = old_path
        owi_serve.owi_do.REPO = old_repo


def check_outcome_measurement_provenance() -> None:
    captured = {}
    captured_path = None
    original = owi_serve.owi_do.owi
    try:
        with tempfile.TemporaryDirectory() as scratch:
            home = Path(scratch)

            def capture(_command: str, *args: str):
                nonlocal captured_path
                path = Path(args[args.index("--input") + 1])
                captured_path = path
                captured.update(json.loads(path.read_text(encoding="utf-8")))
                return {}

            owi_serve.owi_do.owi = capture
            owi_serve.owi_do.record_outcome(
                home, "worker:writer/text", "skill:text-editing", True,
                task="checked output", task_id="task:persisted",
                validation_kind="deterministic", latency_ms=17,
            )
        event = captured["event"]
        coverage = event["metadata"]["measurement_coverage"]
        require(event["validation_kind"] == "deterministic"
                and event["latency_ms"] == 17,
                "automatic evidence has false human/latency provenance")
        require(event["actual_cash_micros"] == 0
                and coverage == {"cash": "unknown", "quota": "unknown",
                                 "latency": "measured"},
                "numeric compatibility zero was exposed without unknown coverage")
        require(captured_path is not None and not captured_path.exists(),
                "private outcome payload remained on disk after ingestion")
    finally:
        owi_serve.owi_do.owi = original


def main() -> int:
    check_lifecycle_and_persistence()
    check_single_task_fallback()
    check_recovery_and_outcome_failure()
    check_planner_decomposition_and_budget_gate()
    check_symlink_rejected()
    check_http_identity_boundary()
    check_release_binary_path_without_cargo()
    check_outcome_measurement_provenance()
    print("workflow runtime verified: persisted create, independent staffing, "
          "real-snapshot allocation, runner gate, execution, checks, review, "
          "edit invalidation, retry, planner decomposition, budget enforcement, "
          "symlink rejection, honest fallback, an HTTP identity boundary, and "
          "release-binary staffing without Cargo")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
