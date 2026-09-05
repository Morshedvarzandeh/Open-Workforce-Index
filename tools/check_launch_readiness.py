#!/usr/bin/env python3
"""HTTP regressions for first launch, saved projects and active model runs.

Uses temporary workspaces and deterministic test doubles. No provider calls,
credentials, downloads or Rust installation are needed.
"""

from __future__ import annotations

import json
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from http.server import ThreadingHTTPServer
from pathlib import Path
from urllib.parse import quote
from unittest.mock import patch

from check_workflow_runtime import FakeOwi, catalog, owi_serve
from owi_workflow import WorkflowService


class LaunchChecks(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.home = Path(self.scratch.name) / "workspace"
        self.fake = FakeOwi()
        self.workflow = WorkflowService(self.home, self.fake, catalog)
        handler = owi_serve.Handler
        handler.home, handler.workflow = self.home, self.workflow
        handler.data = owi_serve.setup_payload(self.home)
        handler.token = "launch-regression-token"
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        self.scratch.cleanup()

    def request(self, path, method="GET", body=None, token=True, timeout=2):
        headers = {"X-OWI-Token": "launch-regression-token"} if token else {}
        data = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            data = json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data,
                                     headers=headers, method=method)
        try:
            response = urllib.request.urlopen(req, timeout=timeout)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            raw = response.read().decode()
            payload = (json.loads(raw) if "application/json" in
                       response.headers.get("Content-Type", "") else raw)
            return response.status, payload, response.headers

    def api(self, path, method="GET", body=None, status=200, **kwargs):
        code, payload, _ = self.request(path, method, body, **kwargs)
        self.assertEqual(code, status, payload)
        return payload

    def create(self, name):
        return self.api("/api/projects", "POST", {
            "name": name, "goal": "Write a useful release note",
        })["project"]

    def test_first_launch_drafts_survive_without_an_engine(self):
        self.assertEqual(self.api("/api/projects")["projects"], [])
        self.assertFalse(self.home.exists())
        with patch.object(self.fake, "plan_parts", side_effect=AssertionError("paid planner called")):
            first = self.create("First project")
            second = self.create("Second project")
            for key in ("usePlanner", "usePlanningModel"):
                self.api("/api/projects", "POST", {"goal": "Write it", key: True}, status=409)
        path = "/api/projects/" + quote(first["id"], safe="")
        task = self.api(path + "/tasks", "POST", {"brief": "Write another note"})["task"]
        task_path = path + "/tasks/" + quote(task["id"], safe="")
        changed = self.api(task_path, "PATCH", {"brief": "Write the revised note"})["task"]
        self.assertEqual(changed["brief"], "Write the revised note")
        self.api(task_path, "DELETE")
        self.api(path + "/staff", "POST", {}, status=409)
        self.api(task_path + "/run", "POST", {}, status=409)
        # A fresh service represents reopening the same workspace after restart.
        owi_serve.Handler.workflow = WorkflowService(self.home, self.fake, catalog)
        saved = self.api("/api/projects")["projects"]
        self.assertEqual([p["id"] for p in saved], [second["id"], first["id"]])
        self.assertEqual(saved[1]["taskCount"], 1)
        self.assertNotIn("goal", saved[1])
        self.assertEqual(self.api(path)["project"]["name"], "First project")
        self.assertEqual(self.fake.snapshot_ids, [])
        self.assertEqual(self.fake.outcomes, [])
        self.assertFalse((self.home / "index.sqlite").exists())
        self.assertFalse((self.home / "local.sqlite").exists())
        self.assertEqual((self.home / "workflow.sqlite").stat().st_mode & 0o777, 0o600)

    def test_reload_preserves_drafts_and_never_seeds_or_executes(self):
        project = self.create("Keep my draft")
        owi_serve.Handler.data = {"local": True, "configured": True}
        with patch.object(owi_serve.owi_do, "bootstrap", side_effect=AssertionError("seeded")), \
                patch.object(owi_serve.owi_do, "owi", side_effect=AssertionError("engine called")):
            state = self.api("/api/workspace/reload", "POST", {})
        self.assertFalse(state["configured"])
        self.assertEqual(self.api("/api/projects/current")["project"]["id"], project["id"])
        self.assertEqual(self.api("/api/workspace")["runnerCount"], 0)
        self.api("/api/workspace/reload", "POST", {"home": "/other"}, status=400)
        self.api("/api/workspace/reload", "POST", {}, status=401, token=False)

    def test_read_endpoints_and_duplicate_run_during_slow_worker(self):
        owi_serve.Handler.data = {"local": True, "configured": True}
        project = self.create("Slow worker")
        path = "/api/projects/" + quote(project["id"], safe="")
        project = self.api(path + "/staff", "POST", {})["project"]
        run_path = path + "/tasks/" + quote(project["tasks"][0]["id"], safe="") + "/run"
        started, release = threading.Event(), threading.Event()

        def slow(*_args, **_kwargs):
            started.set()
            if not release.wait(5):
                raise RuntimeError("test did not release runner")
            return "A completed result", "", 0

        with patch.object(self.fake, "run_worker_command", side_effect=slow), \
                ThreadPoolExecutor(max_workers=1) as pool:
            future = pool.submit(self.api, run_path, "POST", {}, timeout=8)
            try:
                self.assertTrue(started.wait(2))
                self.api("/api/data", timeout=0.5)
                self.api("/", timeout=0.5)
                running = self.api(path, timeout=0.5)["project"]["tasks"][0]
                self.assertEqual(running["status"], "running")
                self.api(run_path, "POST", {}, status=409, timeout=0.5)
                self.api("/api/workspace/reload", "POST", {}, status=409, timeout=0.5)
            finally:
                release.set()
            result = future.result(timeout=2)["task"]
        self.assertEqual(result["status"], "needs_review")
        self.assertEqual(result["attemptCount"], 1)

    def test_setup_help_and_private_history_are_token_gated(self):
        code, body, headers = self.request("/", token=False)
        self.assertEqual(code, 401)
        self.assertIn("Open your private Office", body)
        self.assertEqual(headers["Referrer-Policy"], "no-referrer")
        self.assertEqual(headers["X-Frame-Options"], "DENY")
        self.assertIn("Start using OWI Office", self.api("/help/setup"))
        for path in ("/api/projects", "/api/projects/current", "/api/workspace", "/help/setup"):
            self.api(path, status=401, token=False)
        self.api("/api/projects/project%3Amissing", status=404)


if __name__ == "__main__":
    unittest.main(verbosity=2)
