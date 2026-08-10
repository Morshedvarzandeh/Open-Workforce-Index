#!/usr/bin/env python3
"""Static release checks for the dependency-free OWI Office interface."""

from __future__ import annotations

import re
import subprocess
import sys
from html.parser import HTMLParser
from pathlib import Path

from owi_assets import OFFICE_ASSETS, inline_office_assets


REPO = Path(__file__).resolve().parent.parent
TEMPLATE = REPO / "tools" / "owi_ask_template.html"


class PageParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.ids: list[str] = []
        self.html_language: str | None = None

    def handle_starttag(
        self, tag: str, attrs: list[tuple[str, str | None]]
    ) -> None:
        attributes = dict(attrs)
        if tag == "html":
            self.html_language = attributes.get("lang")
        if attributes.get("id"):
            self.ids.append(str(attributes["id"]))


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"office GUI check failed: {message}")


def main() -> int:
    page = TEMPLATE.read_text(encoding="utf-8")
    rendered = inline_office_assets(page).replace("__DATA__", "{}").replace(
        "__BUILT__", "test"
    )
    parser = PageParser()
    parser.feed(rendered)

    require(parser.html_language == "en", "the document needs lang=en")
    require(len(parser.ids) == len(set(parser.ids)), "HTML ids must be unique")
    required_ids = {
        "view-office",
        "view-work",
        "view-results",
        "crewGrid",
        "officeStatus",
        "skin",
        "projectForm",
        "projectGoal",
        "projectPrivacy",
        "usePlanningModel",
        "projectBoard",
        "taskForm",
        "taskBrief",
        "taskChecklist",
        "taskList",
        "resultTotal",
        "resultAccepted",
        "resultRejected",
        "resultsList",
    }
    require(required_ids.issubset(parser.ids), "a core Office surface is missing")
    require(re.search(r'id="taskList"[^>]*aria-live="polite"', page) is not None,
            "project task results need an aria-live region")
    require("Create project &amp; draft tasks" in page
            and "Staff unassigned tasks" in page
            and "Accept result" in page,
            "create, staff, run and review must be one visible workflow")
    for endpoint in (
        '"/api/projects/current"',
        '"/api/projects"',
        '"/staff"',
        '"/run"',
        '"/review"',
    ):
        require(endpoint in page or endpoint.strip('"') in page,
                f"the live project workflow lost endpoint {endpoint}")
    require('{ method: "POST", body: {} }' in page
            and "identity, task text and runner are server-owned" in page,
            "the browser must not select a model or rewrite a saved task at run time")
    require("estimatedCostMicros" in page and "Exact assigned worker" in page,
            "staffed task cards must show exact identity and estimated cost")
    require("runnerReady" in page and "Execution is blocked" in page,
            "execution must be blocked when the assigned runner is unavailable")
    require("runnerBinding" in page and "exact worker command" in page
            and "model-wide fallback · not runnable" in page,
            "runner binding must distinguish exact worker and model-wide execution")
    require('task.runnerReady === true && task.runnerBinding === "worker"' in page,
            "a model-wide fallback may not make a staffed task runnable")
    require("attemptCount" in page and "maxAttempts" in page
            and "Attempt limit reached" in page,
            "task retries must expose and enforce the returned attempt budget")
    require("task.output" in page and "task.checks" in page,
            "review cards must expose real output and acceptance evidence")
    require("Restaff using new evidence" in page
            and "Retry same assignment" in page,
            "failed work needs both retry and evidence-aware restaff paths")
    require('method: "DELETE"' in page and "window.confirm" in page
            and "Remove task" in page,
            "editable draft tasks need a guarded removal path")
    require('micros !== null && micros !== undefined' in page
            and "estimateCoverage" in page
            and "selectedSubtotalMicros" in page,
            "partial forecasts must never be presented as measured zero")
    require("planningProblem" in page and "budgetWarning" in page
            and "not a verified all-in" in page,
            "planner fallback and task-forecast budget boundaries must be visible")
    require('type="checkbox" id="usePlanningModel"' in page
            and "Optional and unchecked by default" in page
            and "usePlanningModel:" in page,
            "planning-model use must be explicit, optional and sent to the server")
    require('value="secret">Secret' in page,
            "the project privacy selector must expose the backend secret level")
    require("workflowSyncPlanningPrivacy" in page
            and "Model planning is disabled for Confidential and Secret" in page
            and 'privacy === "confidential_content" || privacy === "secret"' in page,
            "sensitive project goals must not be sent to the planning model")
    require("runners.json must bind the exact worker ID" in page
            and "model-only runner keys are not executable" in page,
            "setup must explain the exact worker-ID runner contract")
    require('["accepted", "rejected"].includes(task.status)' in page
            and "await workflowRefreshRuntime()" in page,
            "automatic terminal outcomes must refresh the live Results ledger")
    require("measured roster" not in page.lower()
            and "measured prices" not in page.lower()
            and "stored roster snapshot" in page
            and "price evidence" in page,
            "the UI must describe stored evidence without claiming measurement")
    require("budgetBasis" in page and "budgetScope" in page
            and "actualSpendEnforced" in page,
            "the task-forecast budget may not masquerade as an all-in spend cap")
    require("read-only product preview" in page
            and "pretend a model ran" in page,
            "the static build must not masquerade as a live office")
    require('value="bright">Bright office</option>' in page,
            "the bright office must remain the default skin")
    require('class="character robot"' not in page,
            "crew must come from the injected roster, not fictional static workers")
    require("CREW_PERSONAS" in page and 'asset: "orbit"' in page,
            "the blue bowl robot persona is missing")
    require("worker.id" in page and "crew-model" in page,
            "friendly crew cards must expose exact worker identities")
    require("setup required" in page and "ready to run" in page,
            "catalogued and runnable workers must be distinguished")
    require("Actual spend" in page and "Not recorded by this browser" in page,
            "unmetered cash must not look like measured zero")
    require("Verified savings" in page and "Needs a frozen baseline" in page,
            "savings need an explicit evidence boundary")
    require("CO₂ · water" in page and "No measured provider evidence" in page,
            "environmental unknowns must remain visible")
    require("68%" not in page and "cost saved" not in page.lower(),
            "the interface contains a hard-coded savings claim")
    require("@media (max-width:620px)" in page,
            "the Office needs a compact-screen layout")
    require("prefers-reduced-motion:reduce" in page,
            "the Office needs a reduced-motion mode")
    require(page.count("// >>> decision-math >>>") == 1
            and page.count("// <<< decision-math <<<") == 1,
            "the verified decision-math boundary changed")
    for placeholder, path in OFFICE_ASSETS.items():
        require(path.is_file(), f"the art pack is missing {path.name}")
        require(placeholder not in rendered,
                f"the rendered page retained {placeholder}")
    asset_validation = subprocess.run(
        [sys.executable, str(REPO / "ui/assets/office/v2/validate.py")],
        text=True, capture_output=True, check=False
    )
    require(asset_validation.returncode == 0,
            "the Office art library is invalid:\n"
            + asset_validation.stderr[-1200:])

    scripts = re.findall(r"<script(?: [^>]*)?>(.*?)</script>", rendered, re.S)
    require(len(scripts) == 3,
            "expected tour data, runtime data and one application script")
    checked = subprocess.run(
        ["node", "--check"], input=scripts[-1], text=True,
        capture_output=True, check=False
    )
    require(checked.returncode == 0,
            "application JavaScript is invalid:\n" + checked.stderr[-1200:])

    print("office GUI verified: bright default, versioned game-art library, "
          "real roster identities, truthful unknowns, responsive shell, "
          "valid JavaScript")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
