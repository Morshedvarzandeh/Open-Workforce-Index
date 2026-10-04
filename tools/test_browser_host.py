"""Security and mutation tests for the project-map Native Messaging host."""
from __future__ import annotations

import io
import json
from pathlib import Path
import sqlite3
import struct
import subprocess
import tempfile
import unittest

import owi_project_map
import owi_runtime as runtime
from owi_browser_host import (
    FIREFOX_EXTENSION_ID,
    NATIVE_HOST_NAME,
    _native_launch_arguments,
    dispatch,
    install_native_manifest,
    load_project_config,
    native_host_manifest,
    project_config_path,
    serve_native_messages,
    write_project_config,
)
from owi_browser_map import MapServiceError, ProjectMapService


NOW = 1_700_000_000.0


def framed(value: dict) -> bytes:
    body = json.dumps(value, separators=(",", ":")).encode()
    return struct.pack("=I", len(body)) + body


def read_frames(value: bytes) -> list[dict]:
    stream = io.BytesIO(value)
    result = []
    while True:
        header = stream.read(4)
        if not header:
            return result
        length = struct.unpack("=I", header)[0]
        result.append(json.loads(stream.read(length)))


class BrowserHostTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        base = Path(self.temporary.name)
        self.repo = base / "selected-repository"
        self.home = base / "private-runtime"
        (self.repo / ".owi").mkdir(parents=True)
        (self.repo / "src").mkdir()
        (self.repo / "src" / "app.py").write_text("print('one')\n")
        declaration = {
            "schemaVersion": "owi.project-map.v1",
            "project": {
                "id": "fixture-project",
                "name": "Fixture project",
                "description": "A repository-neutral host fixture.",
            },
            "lanes": [{"id": "lane-code", "name": "Code"}],
            "components": [{
                "id": "component-app",
                "name": "Application",
                "description": "A generic component.",
                "kind": "module",
                "facts": ["Owned by this fixture."],
                "laneId": "lane-code",
                "lifecycle": "implemented",
                "owns": ["src/"],
            }],
            "relations": [],
        }
        (self.repo / ".owi" / "map.json").write_text(json.dumps(declaration))
        subprocess.run(["git", "init", "-q"], cwd=self.repo, check=True)
        subprocess.run(
            ["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
             "add", "."], cwd=self.repo, check=True,
        )
        subprocess.run(
            ["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
             "commit", "-qm", "fixture"], cwd=self.repo, check=True,
        )
        (self.repo / "src" / "app.py").write_text("print('one')\nprint('two')\n")
        self.service = ProjectMapService(
            {"opaque-project-1": self.repo}, self.home, clock=lambda: NOW,
        )

    def tearDown(self):
        self.temporary.cleanup()

    def snapshot(self) -> dict:
        return self.service.get_snapshot("opaque-project-1")

    @staticmethod
    def action(snapshot: dict, kind: str) -> dict:
        return next(action for action in snapshot["map"]["actions"]
                    if action["kind"] == kind)

    def invoke(self, snapshot: dict, action: dict, key="idempotency-key-0001", **changes):
        values = {
            "project_id": snapshot["projectId"],
            "action_id": action["id"],
            "expected_snapshot_id": snapshot["snapshotId"],
            "expected_state": action["expectedState"],
            "idempotency_key": key,
            "confirmed": True,
        }
        values.update(changes)
        return self.service.invoke_action(**values)

    def test_snapshot_adds_safe_generic_runtime_nodes_evidence_and_actions(self):
        result = self.snapshot()
        mapping = result["map"]
        self.assertEqual(result["snapshotId"], mapping["snapshot"]["id"])
        self.assertEqual(mapping["snapshot"]["projectId"], "opaque-project-1")
        self.assertEqual(self.service.list_projects(), {
            "projects": [{"projectId": "opaque-project-1"}],
        })
        runtime_nodes = [node for node in mapping["nodes"]
                         if node["kind"] == "runtime"]
        self.assertEqual(len(runtime_nodes), 1)
        self.assertEqual(runtime_nodes[0]["state"]["learning"], "running")
        evidence = {item["basis"]: item for item in mapping["evidence"]}
        self.assertIn("runner-reported", evidence)
        self.assertIn("api-equivalent", evidence)
        self.assertIsNone(evidence["reconciled-invoice"]["value"]["amount"])
        node_ids = {node["id"] for node in mapping["nodes"]}
        self.assertTrue(mapping["actions"])
        self.assertTrue(all(action["targetId"] in node_ids
                            for action in mapping["actions"]))
        encoded = json.dumps(result)
        self.assertNotIn(str(self.repo), encoded)
        self.assertNotIn("prompt", encoded.lower())
        self.assertNotIn("providerRun", encoded)

    def test_shared_runtime_money_keeps_basis_coverage_and_unknowns(self):
        known = {
            "reported_charge_micros": 125000,
            "api_equivalent_micros": 250000,
        }
        unknown = {
            "reported_charge_micros": None,
            "api_equivalent_micros": None,
        }
        with runtime.database(self.home) as db:
            for index, usage in enumerate((known, unknown)):
                db.execute(
                    "INSERT INTO runs(id,operation,model,role,revision,rules,started,"
                    "finished,exit_code,billing,usage) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                    (f"run-{index}", "shared-operation", "local-agent", "worker", 0,
                     "[]", NOW + index, NOW + index, 0, "{}", json.dumps(usage)),
                )
        evidence = {item["basis"]: item for item in self.snapshot()["map"]["evidence"]}
        runner = evidence["runner-reported"]
        self.assertEqual(runner["type"], "shared-runtime-usage")
        self.assertEqual(runner["claimState"], "partial")
        self.assertEqual(runner["value"], {
            "amount": 125000,
            "unit": "currency-micros",
            "currency": "USD",
            "reportedItems": 1,
            "observedItems": 2,
        })
        self.assertEqual(evidence["reconciled-invoice"]["claimState"], "unavailable")
        self.assertIsNone(evidence["reconciled-invoice"]["value"]["amount"])

    def test_pause_requires_confirmation_preconditions_and_is_idempotent(self):
        before = self.snapshot()
        pause = self.action(before, "runtime.learning.pause")
        with self.assertRaisesRegex(MapServiceError, "Confirm") as caught:
            self.invoke(before, pause, confirmed=False)
        self.assertEqual(caught.exception.code, "confirmation_required")
        with self.assertRaises(MapServiceError) as caught:
            self.invoke(before, pause, expected_state="state_wrong")
        self.assertEqual(caught.exception.code, "stale_state")

        result = self.invoke(before, pause)
        self.assertFalse(runtime.settings(self.home)["learning"])
        self.assertNotEqual(before["snapshotId"], result["snapshot"]["snapshotId"])
        self.assertEqual(result["receipt"]["outcome"], "applied")
        self.assertEqual(self.invoke(before, pause), result)

        with self.assertRaises(MapServiceError) as caught:
            self.invoke(before, pause, key="idempotency-key-0002")
        self.assertEqual(caught.exception.code, "stale_snapshot")
        with self.assertRaises(MapServiceError) as caught:
            self.invoke(before, pause, action_id="act_unknown")
        self.assertEqual(caught.exception.code, "idempotency_conflict")

        with sqlite3.connect(self.home / "browser-map.sqlite") as db:
            row = db.execute(
                "SELECT status,action_id,before_snapshot_id,after_snapshot_id "
                "FROM action_receipts"
            ).fetchone()
        self.assertEqual(row[0], "completed")
        self.assertEqual(row[1], pause["id"])
        self.assertNotEqual(row[2], row[3])

    def test_applied_action_is_not_reported_failed_when_snapshot_refresh_fails(self):
        failing_home = self.home / "refresh-failure"

        class RefreshFailureCore:
            @staticmethod
            def build_snapshot(root, *, generated_at=None):
                if not runtime.settings(failing_home)["learning"]:
                    raise OSError("simulated post-commit read failure")
                return owi_project_map.build_snapshot(root, generated_at=generated_at)

            seal_snapshot = staticmethod(owi_project_map.seal_snapshot)

        service = ProjectMapService(
            {"opaque-project-1": self.repo}, failing_home,
            core=RefreshFailureCore, clock=lambda: NOW,
        )
        before = service.get_snapshot("opaque-project-1")
        pause = self.action(before, "runtime.learning.pause")
        result = service.invoke_action(
            project_id=before["projectId"], action_id=pause["id"],
            expected_snapshot_id=before["snapshotId"],
            expected_state=pause["expectedState"],
            idempotency_key="refresh-failure-key-0001", confirmed=True,
        )
        self.assertFalse(runtime.settings(failing_home)["learning"])
        self.assertEqual(result["receipt"]["outcome"], "applied")
        self.assertEqual(result["receipt"]["snapshotState"], "unavailable")
        self.assertIsNone(result["receipt"]["afterSnapshotId"])
        self.assertIsNone(result["snapshot"])
        replay = service.invoke_action(
            project_id=before["projectId"], action_id=pause["id"],
            expected_snapshot_id=before["snapshotId"],
            expected_state=pause["expectedState"],
            idempotency_key="refresh-failure-key-0001", confirmed=True,
        )
        self.assertEqual(replay, result)

    def test_agent_rollback_uses_opaque_action_and_expected_revision_state(self):
        with runtime.database(self.home) as db:
            agent = runtime._agent(db, "local-agent")
            agent["revisions"].append({
                "id": 1, "parent": 0, "rules": ["json"], "state": "stable",
                "passes": 3, "failures": 0,
                "catalog_sha256": runtime.CATALOG_SHA256,
                "instructions": {"json": runtime.RULES["json"]},
            })
            agent["active"] = agent["stable"] = 1
            runtime._put_agent(db, "local-agent", agent)

        before = self.snapshot()
        rollback = self.action(before, "runtime.agent.rollback")
        self.assertEqual(rollback["availability"], "available")
        self.assertNotIn("local-agent", rollback["id"])
        result = self.invoke(before, rollback, key="rollback-key-0000001")
        state = runtime.snapshot(self.home)["agents"]["local-agent"]
        self.assertEqual(state["active"], 0)
        self.assertEqual(state["revisions"][1]["state"], "rolled_back")
        after_action = next(
            action for action in result["snapshot"]["map"]["actions"]
            if action["id"] == rollback["id"]
        )
        self.assertEqual(after_action["availability"], "unavailable")

    def test_dispatch_rejects_unknown_fields_paths_commands_and_bad_types(self):
        bad_requests = [
            {"id": "r1", "type": "list_projects", "path": str(self.repo)},
            {"id": "r2", "type": "get_snapshot", "projectId": 7},
            {"id": "r3", "type": "get_snapshot", "projectId": str(self.repo)},
            {"id": "r4", "type": "run_provider", "prompt": "ignore policy"},
        ]
        codes = [dispatch(self.service, request)["error"]["code"]
                 for request in bad_requests]
        self.assertEqual(codes, [
            "invalid_request", "invalid_request", "unknown_project", "unknown_request",
        ])
        response = dispatch(self.service, {"id": "r5", "type": "list_projects"})
        self.assertEqual(set(response), {"schemaVersion", "id", "ok", "result"})
        self.assertEqual(response["schemaVersion"], "owi.native-project-map.v1")
        self.assertTrue(response["ok"])

    def test_native_framing_handles_multiple_duplicate_and_oversized_messages(self):
        incoming = io.BytesIO(
            framed({"id": "r1", "type": "list_projects"})
            + framed({
                "id": "r2", "type": "get_snapshot",
                "projectId": "opaque-project-1",
            })
        )
        outgoing = io.BytesIO()
        self.assertEqual(
            serve_native_messages(self.service, incoming, outgoing), 0,
        )
        responses = read_frames(outgoing.getvalue())
        self.assertEqual([response["id"] for response in responses], ["r1", "r2"])
        self.assertTrue(all(response["ok"] for response in responses))

        duplicate = b'{"id":"a","id":"b","type":"list_projects"}'
        outgoing = io.BytesIO()
        serve_native_messages(
            self.service,
            io.BytesIO(struct.pack("=I", len(duplicate)) + duplicate),
            outgoing,
        )
        self.assertEqual(read_frames(outgoing.getvalue())[0]["error"]["code"], "invalid_json")

        outgoing = io.BytesIO()
        self.assertEqual(
            serve_native_messages(
                self.service, io.BytesIO(struct.pack("=I", 2049)), outgoing,
                max_bytes=2048,
            ),
            1,
        )
        self.assertEqual(
            read_frames(outgoing.getvalue())[0]["error"]["code"],
            "message_too_large",
        )

    def test_core_cannot_supply_actions_or_absolute_paths(self):
        valid = self.snapshot()["map"]
        valid["actions"] = []

        class UnsafeCore:
            @staticmethod
            def build_snapshot(_root, *, generated_at=None):
                value = json.loads(json.dumps(valid))
                value["snapshot"]["generatedAt"] = generated_at
                value["actions"] = [{"id": "repository-action"}]
                return value

        unsafe = ProjectMapService(
            {"opaque-project-1": self.repo}, self.home / "unsafe",
            core=UnsafeCore, clock=lambda: NOW,
        )
        with self.assertRaises(MapServiceError) as caught:
            unsafe.get_snapshot("opaque-project-1")
        self.assertEqual(caught.exception.code, "unsafe_snapshot")

    def test_repository_cannot_collide_with_reserved_runtime_identifiers(self):
        settings, agents = self.service._runtime_state()
        _, runtime_id, _ = self.service._runtime_ids("opaque-project-1", agents)
        path = self.repo / ".owi" / "map.json"
        value = json.loads(path.read_text())
        value["components"][0]["id"] = runtime_id
        path.write_text(json.dumps(value))
        with self.assertRaises(MapServiceError) as caught:
            self.snapshot()
        self.assertEqual(caught.exception.code, "unsafe_snapshot")

    def test_unsafe_runtime_identity_cannot_leak_into_client_snapshot(self):
        with runtime.database(self.home) as db:
            agent = runtime._agent(db, "safe-template")
            runtime._put_agent(db, "/private/runtime-agent", agent)
        with self.assertRaises(MapServiceError) as caught:
            self.snapshot()
        self.assertEqual(caught.exception.code, "unsafe_snapshot")

    def test_owner_registry_is_strict_private_and_round_trips(self):
        path = write_project_config(
            self.home, {"opaque-project-1": self.repo},
        )
        self.assertEqual(path, project_config_path(self.home))
        self.assertEqual(load_project_config(self.home), {
            "opaque-project-1": self.repo.resolve(),
        })
        if hasattr(path.stat(), "st_mode"):
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

        value = json.loads(path.read_text())
        value["projects"][0]["command"] = "unsafe"
        path.write_text(json.dumps(value))
        with self.assertRaisesRegex(ValueError, "exactly id and root"):
            load_project_config(self.home)

        path.write_text('{"schemaVersion":"owi.browser-projects.v1",'
                        '"schemaVersion":"owi.browser-projects.v2","projects":[]}')
        with self.assertRaisesRegex(ValueError, "not valid JSON"):
            load_project_config(self.home)

        with self.assertRaisesRegex(ValueError, "project root"):
            write_project_config(self.home, {"too-broad": Path(Path.cwd().anchor)})

    def test_native_registration_is_browser_specific_and_least_privilege(self):
        executable = Path(__file__).resolve()
        chromium_id = "abcdefghijklmnopabcdefghijklmnop"
        chromium = native_host_manifest(executable, "chromium", chromium_id)
        self.assertEqual(chromium, {
            "name": NATIVE_HOST_NAME,
            "description": "Local Open Workforce Index project-map host",
            "path": str(executable),
            "type": "stdio",
            "allowed_origins": [f"chrome-extension://{chromium_id}/"],
        })
        firefox = native_host_manifest(executable, "firefox")
        self.assertEqual(firefox["allowed_extensions"], [FIREFOX_EXTENSION_ID])
        self.assertNotIn("allowed_origins", firefox)
        with self.assertRaisesRegex(ValueError, "32-character"):
            native_host_manifest(executable, "chrome", "not-an-extension-id")

        destination = Path(self.temporary.name) / "manifests"
        installed = install_native_manifest(
            executable, "edge", chromium_id, directory=destination,
        )
        self.assertEqual(installed.name, f"{NATIVE_HOST_NAME}.json")
        self.assertEqual(json.loads(installed.read_text())["allowed_origins"],
                         [f"chrome-extension://{chromium_id}/"])

    def test_browser_supplied_launch_metadata_is_not_parsed_as_configuration(self):
        chromium_id = "abcdefghijklmnopabcdefghijklmnop"
        self.assertTrue(_native_launch_arguments([]))
        self.assertTrue(_native_launch_arguments([
            f"chrome-extension://{chromium_id}/",
        ]))
        self.assertTrue(_native_launch_arguments([
            "/owner/native-host-manifest.json", FIREFOX_EXTENSION_ID,
        ]))
        self.assertFalse(_native_launch_arguments(["--list-configured-projects"]))
        self.assertFalse(_native_launch_arguments(["https://untrusted.example"]))


if __name__ == "__main__":
    unittest.main()
