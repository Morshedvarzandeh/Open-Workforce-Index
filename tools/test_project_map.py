"""Contract tests for the generic, local-only project map."""
from __future__ import annotations

import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

import owi_project_map as project_map


def declaration() -> dict:
    return {
        "schemaVersion": "owi.project-map.v1",
        "project": {"id": "project-example", "name": "Example"},
        "lanes": [
            {"id": "lane-core", "name": "Core", "order": 0},
            {"id": "lane-interface", "name": "Interface", "order": 1},
        ],
        "components": [
            {
                "id": "component-core",
                "name": "Core",
                "kind": "module",
                "facts": ["Owns the core implementation."],
                "laneId": "lane-core",
                "lifecycle": "implemented",
                "owns": ["src/"],
            },
            {
                "id": "component-api",
                "name": "API",
                "kind": "api",
                "laneId": "lane-interface",
                "lifecycle": "declared",
                "owns": ["api/"],
            },
            {
                "id": "component-ui",
                "name": "UI",
                "kind": "surface",
                "laneId": "lane-interface",
                "lifecycle": "planned",
                "owns": ["ui/"],
            },
            {
                "id": "component-independent",
                "name": "Independent",
                "kind": "module",
                "laneId": "lane-core",
                "lifecycle": "implemented",
                "owns": ["independent/"],
            },
        ],
        "relations": [
            {
                "id": "relation-core-api",
                "from": "component-core",
                "to": "component-api",
                "type": "feeds",
            },
            {
                "id": "relation-api-ui",
                "from": "component-api",
                "to": "component-ui",
                "type": "feeds",
            },
        ],
    }


class ProjectMapTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.git("init", "-q")
        self.git("config", "user.email", "tests@example.invalid")
        self.git("config", "user.name", "Project Map Tests")

    def tearDown(self):
        self.temporary.cleanup()

    def git(self, *arguments: str) -> str:
        result = subprocess.run(
            ["git", *arguments],
            cwd=self.root,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
        return result.stdout.strip()

    def write(self, path: str, value: str | bytes) -> Path:
        destination = self.root / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(value, bytes):
            destination.write_bytes(value)
        else:
            destination.write_text(value, encoding="utf-8")
        return destination

    def commit_all(self) -> None:
        self.git("add", "--all")
        self.git("commit", "-qm", "fixture")

    def node(self, snapshot: dict, identifier: str) -> dict:
        return next(node for node in snapshot["nodes"] if node["id"] == identifier)

    def test_git_subprocesses_cannot_lazy_fetch_or_prompt(self):
        environment = project_map._git_environment()
        self.assertEqual(environment["GIT_NO_LAZY_FETCH"], "1")
        self.assertEqual(environment["GIT_TERMINAL_PROMPT"], "0")
        self.assertEqual(environment["GCM_INTERACTIVE"], "Never")

    def test_git_capture_is_bounded_before_materialization(self):
        self.write("a-very-long-untracked-file-name.txt", "content\n")
        with mock.patch.object(project_map, "MAX_GIT_STDOUT_BYTES", 8):
            result = project_map._run_git(
                self.root, "ls-files", "-z", "--cached", "--others", "--exclude-standard"
            )
        self.assertTrue(result.output_limited)
        self.assertLessEqual(len(result.stdout), 8)

    def test_changed_file_limit_is_explicit_and_never_partial(self):
        self.write("one.txt", "one\n")
        self.write("two.txt", "two\n")
        with mock.patch.object(project_map, "MAX_OBSERVED_FILES", 1):
            changes = project_map.git_changes(self.root)
        self.assertEqual(changes["comparison"]["state"], "unavailable")
        self.assertEqual(changes["comparison"]["reason"], "git-file-limit")
        self.assertEqual(changes["files"], [])

    def test_auto_discovery_fails_closed_at_file_limit(self):
        self.write("one.txt", "one\n")
        self.write("two.txt", "two\n")
        with mock.patch.object(project_map, "MAX_DISCOVERED_FILES", 1), \
                self.assertRaisesRegex(project_map.MapValidationError, "file limit"):
            project_map.discover_declaration(self.root)

    def test_strict_declaration_rejects_unsafe_paths_and_executable_fields(self):
        unsafe = [
            "/etc/passwd",
            "../secret",
            "src/../secret",
            "C:/secret",
            "src\\file.py",
            "src/*.py",
            ".git/config",
            "./src",
            "src//file.py",
            "~/secret",
        ]
        for path in unsafe:
            value = declaration()
            value["components"][0]["owns"] = [path]
            with self.subTest(path=path), self.assertRaises(project_map.MapValidationError):
                project_map.validate_declaration(value)

        for field, payload in [
            ("actions", [{"command": "echo unsafe"}]),
            ("commands", ["echo unsafe"]),
            ("providerRequest", {"model": "anything"}),
        ]:
            value = declaration()
            value[field] = payload
            with self.subTest(field=field), self.assertRaisesRegex(
                project_map.MapValidationError, "unknown fields"
            ):
                project_map.validate_declaration(value)

        value = declaration()
        value["components"][0]["command"] = "echo unsafe"
        with self.assertRaisesRegex(project_map.MapValidationError, "unknown fields"):
            project_map.validate_declaration(value)

        value = declaration()
        value["components"][0]["kind"] = "unsupported-device"
        with self.assertRaisesRegex(project_map.MapValidationError, "kind must be"):
            project_map.validate_declaration(value)

    def test_duplicate_and_dangling_ids_are_rejected(self):
        duplicate = declaration()
        duplicate["components"][1]["id"] = duplicate["components"][0]["id"]
        with self.assertRaisesRegex(project_map.MapValidationError, "duplicate id"):
            project_map.validate_declaration(duplicate)

        duplicate_relation = declaration()
        duplicate_relation["relations"][1]["id"] = duplicate_relation["relations"][0]["id"]
        with self.assertRaisesRegex(project_map.MapValidationError, "duplicate id"):
            project_map.validate_declaration(duplicate_relation)

        dangling = declaration()
        dangling["relations"][0]["to"] = "component-missing"
        with self.assertRaisesRegex(project_map.MapValidationError, "unknown component"):
            project_map.validate_declaration(dangling)

        dangling_lane = declaration()
        dangling_lane["components"][0]["laneId"] = "lane-missing"
        with self.assertRaisesRegex(project_map.MapValidationError, "unknown lane"):
            project_map.validate_declaration(dangling_lane)

    def test_unknown_schema_and_unknown_nested_fields_are_rejected(self):
        value = declaration()
        value["schemaVersion"] = "owi.project-map.v2"
        with self.assertRaisesRegex(project_map.MapValidationError, "unsupported"):
            project_map.validate_declaration(value)
        value = declaration()
        value["project"]["remote"] = "https://example.invalid/private.git"
        with self.assertRaisesRegex(project_map.MapValidationError, "unknown fields"):
            project_map.validate_declaration(value)

    def test_checked_in_json_rejects_duplicate_fields_and_non_finite_numbers(self):
        path = self.write(
            ".owi/map.json",
            '{"schemaVersion":"owi.project-map.v1",'
            '"schemaVersion":"owi.project-map.v2","project":{},'
            '"lanes":[],"components":[],"relations":[]}',
        )
        with self.assertRaisesRegex(project_map.MapValidationError, "duplicate JSON field"):
            project_map.load_declaration(self.root)
        path.write_text(
            '{"schemaVersion":"owi.project-map.v1",'
            '"project":{"id":"project-x","name":"Example"},'
            '"lanes":[{"id":"lane-x","name":"Lane","order":NaN}],'
            '"components":[],"relations":[]}',
            encoding="utf-8",
        )
        with self.assertRaisesRegex(project_map.MapValidationError, "non-finite JSON number"):
            project_map.load_declaration(self.root)

    def test_direct_churn_and_directional_affected_paths_remain_separate(self):
        self.write("src/core.txt", "one\n")
        self.write("api/contract.txt", "stable\n")
        self.write("ui/page.txt", "stable\n")
        self.write("independent/value.txt", "stable\n")
        self.commit_all()
        self.write("src/core.txt", "one\ntwo\nthree\n")

        snapshot = project_map.build_snapshot(self.root, declaration())
        core = self.node(snapshot, "component-core")
        api = self.node(snapshot, "component-api")
        ui = self.node(snapshot, "component-ui")
        independent = self.node(snapshot, "component-independent")

        self.assertEqual(core["lifecycle"], "implemented")
        self.assertEqual(core["kind"], "module")
        self.assertEqual(core["facts"], ["Owns the core implementation."])
        self.assertEqual(core["impact"], {"state": "changed", "direct": True, "paths": []})
        self.assertEqual((core["churn"]["additions"], core["churn"]["deletions"]), (2, 0))

        self.assertEqual(api["lifecycle"], "declared")
        self.assertEqual(api["impact"]["state"], "affected")
        self.assertEqual(api["impact"]["paths"], [["component-core", "component-api"]])
        self.assertEqual((api["churn"]["additions"], api["churn"]["deletions"]), (None, None))

        self.assertEqual(ui["lifecycle"], "planned")
        self.assertEqual(
            ui["impact"]["paths"],
            [["component-core", "component-api", "component-ui"]],
        )
        self.assertEqual(ui["churn"]["changedFiles"], 0)
        self.assertEqual(independent["impact"]["state"], "unchanged")
        self.assertEqual(snapshot["actions"], [])
        self.assertEqual(snapshot["schemaVersion"], "owi.system-map.v1")

    def test_unmapped_changed_files_do_not_change_a_component(self):
        self.write("src/core.txt", "stable\n")
        self.write("notes.txt", "old\n")
        self.commit_all()
        self.write("notes.txt", "old\nnew\n")

        snapshot = project_map.build_snapshot(self.root, declaration())
        self.assertEqual(snapshot["unmappedFiles"], ["notes.txt"])
        note = next(item for item in snapshot["files"] if item["path"] == "notes.txt")
        self.assertEqual(note["componentIds"], [])
        self.assertTrue(all(node["impact"]["state"] == "unchanged" for node in snapshot["nodes"]))
        self.assertEqual(snapshot["evidence"][0]["value"]["unmappedFiles"], 1)

    def test_text_and_binary_untracked_files_have_honest_counts(self):
        self.write("README.md", "fixture\n")
        self.write("src/tracked.bin", b"old\x00bytes")
        self.commit_all()
        self.write("src/tracked.bin", b"new\x00binary\x01")
        self.write("src/untracked.txt", "alpha\nbeta")
        self.write("src/untracked.bin", b"alpha\x00beta")

        changes = project_map.git_changes(self.root)
        files = {item["path"]: item for item in changes["files"]}
        self.assertEqual(changes["comparison"]["state"], "available")
        self.assertEqual(files["src/untracked.txt"]["kind"], "text")
        self.assertEqual(
            (files["src/untracked.txt"]["additions"], files["src/untracked.txt"]["deletions"]),
            (2, 0),
        )
        for name in ("src/tracked.bin", "src/untracked.bin"):
            self.assertEqual(files[name]["kind"], "binary")
            self.assertIsNone(files[name]["additions"])
            self.assertIsNone(files[name]["deletions"])

        snapshot = project_map.build_snapshot(self.root, declaration(), changes)
        core = self.node(snapshot, "component-core")
        self.assertEqual(core["impact"]["state"], "changed")
        self.assertIsNone(core["churn"]["additions"])
        self.assertEqual(core["churn"]["binaryFiles"], 2)
        self.assertEqual(core["churn"]["textFiles"], 1)

    def test_git_observation_does_not_execute_repository_diff_or_clean_drivers(self):
        marker = self.root.parent / f"{self.root.name}-filter-ran"
        command = f"touch {marker}; cat"
        self.git("config", "filter.untrusted.clean", command)
        self.git("config", "filter.untrusted.required", "true")
        self.git("config", "diff.untrusted.textconv", command)
        self.write(".gitattributes", "*.txt filter=untrusted diff=untrusted\n")
        self.write("src/filtered.txt", "one\n")
        self.commit_all()
        marker.unlink(missing_ok=True)
        self.write("src/filtered.txt", "one\ntwo\n")

        changes = project_map.git_changes(self.root)

        self.assertFalse(marker.exists())
        filtered = next(item for item in changes["files"]
                        if item["path"] == "src/filtered.txt")
        self.assertEqual(filtered["kind"], "text")
        self.assertEqual((filtered["additions"], filtered["deletions"]), (1, 0))

    def test_auto_discovery_and_snapshot_are_deterministic(self):
        self.write("README.md", "hello\n")
        self.write("zeta/z.txt", "z\n")
        self.write("alpha/a.txt", "a\n")
        self.commit_all()
        self.write("alpha/a.txt", "a\nchanged\n")

        first_declaration = project_map.discover_declaration(self.root)
        second_declaration = project_map.load_declaration(self.root)
        self.assertEqual(first_declaration, second_declaration)
        self.assertEqual(
            [component["name"] for component in first_declaration["components"]],
            ["Root files", "alpha", "zeta"],
        )

        first = project_map.build_snapshot(self.root)
        second = project_map.build_snapshot(self.root)
        self.assertEqual(first, second)
        self.assertRegex(first["snapshot"]["id"], r"^sha256:[0-9a-f]{64}$")
        self.assertEqual(project_map.compute_snapshot_id(first), first["snapshot"]["id"])
        self.assertFalse(any(str(self.root) in json.dumps(first) for _ in [0]))

    def test_checked_in_declaration_is_loaded_and_output_can_be_resealed(self):
        value = declaration()
        path = self.root / ".owi" / "map.json"
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps(value), encoding="utf-8")
        self.assertEqual(project_map.load_declaration(self.root)["project"]["id"], "project-example")

        snapshot = project_map.build_snapshot(self.root, generated_at="2026-10-04T00:00:00Z")
        original_id = snapshot["snapshot"]["id"]
        augmented = copy.deepcopy(snapshot)
        augmented["actions"].append({"id": "trusted-action-from-host"})
        resealed = project_map.seal_snapshot(augmented)
        self.assertNotEqual(resealed["snapshot"]["id"], original_id)
        self.assertEqual(resealed["snapshot"]["id"], project_map.compute_snapshot_id(resealed))

    def test_non_git_directory_reports_unavailable_impact(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            snapshot = project_map.build_snapshot(root, declaration())
        self.assertEqual(snapshot["snapshot"]["comparison"]["state"], "unavailable")
        self.assertTrue(all(node["impact"]["state"] == "unavailable" for node in snapshot["nodes"]))
        self.assertEqual(snapshot["evidence"][0]["claimState"], "unavailable")


if __name__ == "__main__":
    unittest.main()
