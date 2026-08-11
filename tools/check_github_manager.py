#!/usr/bin/env python3
"""Focused no-clone/security/lifecycle checks for GitHub Manager v1."""

from __future__ import annotations

import json
import importlib.machinery
import importlib.util
import os
import sqlite3
import tempfile
import threading
import urllib.error
import urllib.parse
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path

from owi_github import GitHubConfig, GitHubManager, GitHubProblem, \
    GitHubRestClient
from owi_workflow import WorkflowProblem, WorkflowService

TOOLS = Path(__file__).resolve().parent
_loader = importlib.machinery.SourceFileLoader(
    "owi_serve_github_check", str(TOOLS / "owi-serve")
)
_spec = importlib.util.spec_from_loader("owi_serve_github_check", _loader)
owi_serve = importlib.util.module_from_spec(_spec)
_loader.exec_module(owi_serve)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"github manager check failed: {message}")


class FakeRest:
    def __init__(self, private: bool = False):
        self.private = private
        self.calls: list[str] = []
        self.fail_kind: str | None = None
        self.fail_code = "permission_denied"
        self.truncate = False
        self.omit_kind: str | None = None

    def pages(self, path: str, array_key=None, max_pages=5):
        self.calls.append(path)
        meta = {"remaining": 47, "resetAt": "2026-08-11T02:00:00Z",
                "truncated": self.truncate, "nextCursor": None, "pages": 1}
        if self.fail_kind and self.fail_kind in path:
            status = (429 if self.fail_code == "rate_limited" else
                      401 if self.fail_code == "auth_failed" else 403)
            raise GitHubProblem("permission unavailable", status,
                                self.fail_code,
                                {"resetAt": "2026-08-11T02:00:00Z"}
                                if status == 429 else None)
        if self.omit_kind and self.omit_kind in path:
            return [], meta
        if path.startswith("/user/repos") or path.startswith("/users/"):
            return [{
                "id": 101, "name": "Open-Workforce-Index",
                "full_name": "Morshedvarzandeh/Open-Workforce-Index",
                "owner": {"login": "Morshedvarzandeh"},
                "private": self.private, "archived": False,
                "default_branch": "main", "updated_at": "2026-08-11T00:00:00Z",
                "html_url": "https://github.com/Morshedvarzandeh/Open-Workforce-Index",
            }], meta
        issue = {
            "id": 201, "number": 7, "title": "Build manager",
            "body": "Implement the read-only manager.", "state": "open",
            "updated_at": "2026-08-11T00:10:00Z", "labels": [{"name": "feature"}],
            "html_url": "https://github.com/Morshedvarzandeh/Open-Workforce-Index/issues/7",
        }
        pr_in_issues = dict(issue, id=202, number=8, title="Draft manager PR",
                            pull_request={})
        pull = dict(pr_in_issues,
                    html_url="https://github.com/Morshedvarzandeh/Open-Workforce-Index/pull/8")
        run = {
            "id": 301, "run_number": 33, "name": "CI",
            "display_title": "Tests failed", "conclusion": "failure",
            "updated_at": "2026-08-11T00:20:00Z", "head_sha": "abc",
            "html_url": "https://github.com/Morshedvarzandeh/Open-Workforce-Index/actions/runs/301",
        }
        if "/issues?" in path:
            return [issue, pr_in_issues], meta
        if "/pulls?" in path:
            return [pull], meta
        if "/actions/runs?" in path:
            return [run], meta
        raise AssertionError(f"unexpected GET path: {path}")


class FakeOwi:
    SKILLS = {"skill:text-editing": {"tools": []}}

    @staticmethod
    def classify(_brief: str) -> str:
        return "skill:text-editing"

    @staticmethod
    def clean_checklist(raw) -> list[str]:
        return [str(item).strip() for item in raw if str(item).strip()][:4]

    @staticmethod
    def resolve_worker_runner(*_args, **_kwargs):
        return None, None

    def __getattr__(self, name):
        if name in {"owi", "plan_parts", "run_worker_command", "record_outcome"}:
            raise AssertionError(f"GitHub import attempted forbidden operation: {name}")
        raise AttributeError(name)


def workflow(home: Path) -> WorkflowService:
    return WorkflowService(home, FakeOwi(), lambda _home: {
        "snapshot": "unused", "workers": []
    })


def check_public_sync_and_import() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        rest = FakeRest()
        manager = GitHubManager(
            home, GitHubConfig("Morshedvarzandeh", None, "public_only"), rest
        )
        before = manager.status()
        require(before["authMode"] == "public_only"
                and before["summary"]["openIssues"] is None,
                "unsynced public mode invented measured zero")
        catalog = manager.list_repositories()
        require(len(catalog["repositories"]) == 1
                and catalog["nextPage"] is None
                and catalog["source"] == "configured_public_owner",
                "server-configured public repository catalog is wrong")
        calls_before_forgery = len(rest.calls)
        try:
            manager.sync("999")
        except GitHubProblem as error:
            require(error.status == 404, "forged repository was not rejected")
        else:
            raise SystemExit("github manager check failed: forged repo synced")
        require(len(rest.calls) == calls_before_forgery,
                "forged repository triggered an outbound request")

        synced = manager.sync("101")
        require(synced["sync"]["openIssues"] == 1
                and synced["sync"]["openPullRequests"] == 1
                and synced["sync"]["failedCi"] == 1,
                "issues, PRs and recent failed runs were not distinct")
        require(len(synced["workItems"]) == 3,
                "issue endpoint PR was duplicated")
        require(not any(fragment in path for path in rest.calls
                        for fragment in ("/contents", "/archive", "/git/")),
                "manager used repository content/clone-style endpoints")
        item = next(row for row in synced["workItems"]
                    if row["sourceType"] == "issue")
        service = workflow(home)
        imported = manager.import_item(item["id"], {}, service)
        task = imported["task"]
        require(imported["import"]["created"] is True
                and task["status"] == "draft" and task["worker"] is None
                and task["model"] is None
                and task["estimatedCostMicros"] is None
                and task["actualCostMicros"] is None
                and task["attemptCount"] == 0,
                "explicit import was not a zero-spend unassigned draft")
        repeated = manager.import_item(item["id"], {}, service)
        require(not repeated["import"]["created"]
                and repeated["task"]["id"] == task["id"],
                "repeat import created a duplicate task")
        service.edit_task(imported["project"]["id"], task["id"], {
            "brief": "Owner edited this local draft"
        })
        unchanged = manager.import_item(item["id"], {}, service)
        require(unchanged["task"]["brief"] == "Owner edited this local draft",
                "repeat import overwrote the owner's local edit")

        database = (home / "github-manager.sqlite").read_bytes()
        require((home / "github-manager.sqlite").stat().st_mode & 0o777 == 0o600,
                "manager cache is not owner-only")
        require(b"Bearer" not in database,
                "manager database appears to contain credentials")
        connection = sqlite3.connect(home / "workflow.sqlite")
        provenance = connection.execute(
            "SELECT api_version,observed_at,source_digest,observation_partial "
            "FROM source_imports"
        ).fetchone()
        connection.close()
        require(provenance and provenance[0] != "unknown"
                and provenance[1] != "unknown" and len(provenance[2]) == 64
                and provenance[3] == 0,
                "workflow provenance did not bind the observed GitHub revision")


def check_delete_reimport_and_concurrency() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        manager = GitHubManager(
            home, GitHubConfig("Morshedvarzandeh", None, "public_only"), FakeRest()
        )
        manager.list_repositories()
        item = manager.sync("101")["workItems"][0]
        service = workflow(home)
        project = service.create_project({"goal": "Existing local project"})
        first = manager.import_item(item["id"], {"projectId": project["id"]}, service)
        service.delete_task(project["id"], first["task"]["id"])
        second = manager.import_item(item["id"], {"projectId": project["id"]}, service)
        require(second["import"]["created"]
                and second["task"]["id"] != first["task"]["id"],
                "deleted imported task left stranded provenance")

        other = next(row for row in manager.work_items("101")["workItems"]
                     if row["sourceType"] == "pull_request")
        results: list[dict] = []
        failures: list[Exception] = []
        def invoke():
            try:
                results.append(manager.import_item(other["id"], {}, service))
            except Exception as error:
                failures.append(error)
        threads = [threading.Thread(target=invoke) for _ in range(2)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        require(not failures and len({row["task"]["id"] for row in results}) == 1
                and sorted(row["import"]["created"] for row in results) == [False, True],
                "concurrent import was not idempotent")


def check_private_floor_and_partial_truth() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        rest = FakeRest(private=True)
        token = "TOKEN_SENTINEL_MUST_NOT_PERSIST"
        manager = GitHubManager(
            home, GitHubConfig(None, token, "fine_grained_pat"), rest
        )
        manager.list_repositories()
        complete = manager.sync("101")
        require(complete["outcome"] == "complete",
                "complete observation was not labelled complete")
        rest.fail_kind = "/actions/"
        synced = manager.sync("101")
        require(synced["partial"] and synced["sync"]["failedCi"] is None
                and synced["sync"]["coverage"]["action_failure"]["status"] == "unknown",
                "missing Actions permission was encoded as zero/fresh")
        require(synced["outcome"] == "partial"
                and synced["warning"]["code"] == "permission_denied"
                and any(row["sourceType"] == "action_failure" and row["stale"]
                        for row in synced["workItems"]),
                "partial refresh looked complete or made retained rows fresh")
        retained_action = next(row for row in synced["workItems"]
                               if row["sourceType"] == "action_failure")
        action_import = manager.import_item(retained_action["id"], {}, workflow(home))
        connection = sqlite3.connect(home / "workflow.sqlite")
        action_provenance = connection.execute(
            "SELECT observed_at,observation_partial,observation_error FROM "
            "source_imports WHERE task_id=?", (action_import["task"]["id"],)
        ).fetchone()
        connection.close()
        require(action_provenance == (
                    complete["sync"]["coverage"]["action_failure"]["observedAt"],
                    1, "permission_denied"),
                "retained failed-run import falsified its observation time/error")
        rest.fail_kind = None
        rest.omit_kind = "/actions/"
        removed = manager.sync("101")
        removed_action = next(row for row in removed["workItems"]
                              if row["id"] == retained_action["id"])
        require(not removed_action["sourceActive"] and removed_action["stale"],
                "removed imported source was presented as current")
        rest.omit_kind = None
        item = next(row for row in synced["workItems"]
                    if row["sourceType"] == "issue")
        service = workflow(home)
        imported = manager.import_item(item["id"], {}, service)
        require(imported["project"]["privacy"] == "confidential_content"
                and imported["task"]["privacy"] == "confidential_content",
                "private repository content was downgraded")
        public_project = service.create_project({
            "goal": "Public destination", "privacy": "public"
        })
        other = next(row for row in synced["workItems"]
                     if row["sourceType"] == "pull_request")
        try:
            manager.import_item(other["id"], {"projectId": public_project["id"]},
                                service)
        except WorkflowProblem as error:
            require(error.status == 409, "private downgrade error is unclear")
        else:
            raise SystemExit("github manager check failed: private item downgraded")
        require(token.encode() not in (home / "github-manager.sqlite").read_bytes()
                and token.encode() not in (home / "workflow.sqlite").read_bytes(),
                "server credential leaked into private databases")

        # Current-process discovery, not a persisted token flag, is authority.
        revoked = FakeRest(private=True)
        revoked.fail_kind = "/user/repos"
        replacement = GitHubManager(
            home, GitHubConfig(None, "different-invalid-token", "fine_grained_pat"),
            revoked,
        )
        require(replacement.status()["selectedRepository"] is None
                and replacement.status()["credentialVerified"] is False,
                "new/unverified credential inherited cached private authority")
        try:
            replacement.list_repositories()
        except GitHubProblem:
            pass
        else:
            raise SystemExit("github manager check failed: revoked token verified")
        calls = len(revoked.calls)
        try:
            replacement.sync("101")
        except GitHubProblem:
            pass
        else:
            raise SystemExit("github manager check failed: revoked scope synced cache")
        require(len(revoked.calls) == calls,
                "revoked cached repository caused an outbound request")

        # A true rate limit stops the endpoint sequence and is machine-readable.
        rest.fail_kind = "/issues?"
        rest.fail_code = "rate_limited"
        before_calls = len(rest.calls)
        limited = manager.sync("101")
        new_calls = rest.calls[before_calls:]
        require(len(new_calls) == 1 and limited["outcome"] == "failed"
                and limited["warning"]["code"] == "rate_limited"
                and limited["sync"]["syncedAt"] == complete["sync"]["syncedAt"]
                and "attemptedAt" in limited["sync"],
                "rate limit retried/continued or looked like an observation")
        rest.fail_kind = None
        rest.truncate = True
        truncated = manager.sync("101")
        require(truncated["outcome"] == "partial"
                and all(value["status"] == "partial"
                        for value in truncated["sync"]["coverage"].values()),
                "successful truncated reads were labelled failed/complete")
        rest.truncate = False
        rest.fail_kind = "/user/repos"
        try:
            manager.list_repositories()
        except GitHubProblem:
            pass
        else:
            raise SystemExit("github manager check failed: failed refresh succeeded")
        calls = len(rest.calls)
        try:
            manager.sync("101")
        except GitHubProblem:
            pass
        else:
            raise SystemExit("github manager check failed: stale scope survived refresh")
        require(len(rest.calls) == calls,
                "failed same-process refresh retained repository authority")


def check_transport_guards_and_token_file() -> None:
    bad_next = [
        "http://api.github.com/x", "https://localhost/x",
        "https://127.0.0.1/x", "https://user@api.github.com/x",
        "https://api.github.com:443/x", "https://attacker.example/x",
    ]
    for value in bad_next:
        try:
            GitHubRestClient._relative_from_next(value)
        except GitHubProblem:
            pass
        else:
            raise SystemExit("github manager check failed: unsafe pagination admitted")
    with tempfile.TemporaryDirectory() as scratch:
        token_file = Path(scratch) / "token"
        token_file.write_text("secret", encoding="utf-8")
        os.chmod(token_file, 0o600)
        config = GitHubConfig.load(owner="Morshedvarzandeh",
                                   token_file=token_file)
        require(config.token == "secret" and config.auth_mode == "fine_grained_pat",
                "secure token file was not loaded server-side")
        os.chmod(token_file, 0o644)
        try:
            GitHubConfig.load(owner="Morshedvarzandeh", token_file=token_file)
        except GitHubProblem:
            pass
        else:
            raise SystemExit("github manager check failed: loose token mode admitted")
    previous = os.environ.get("OWI_GITHUB_TOKEN")
    os.environ["OWI_GITHUB_TOKEN"] = "bad\nheader"
    try:
        try:
            GitHubConfig.load(owner="Morshedvarzandeh")
        except GitHubProblem as error:
            require(error.code == "invalid_token",
                    "malformed environment token failed unclearly")
        else:
            raise SystemExit("github manager check failed: control token admitted")
    finally:
        if previous is None:
            os.environ.pop("OWI_GITHUB_TOKEN", None)
        else:
            os.environ["OWI_GITHUB_TOKEN"] = previous


def check_same_process_token_revocation() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        rest = FakeRest(private=True)
        manager = GitHubManager(
            home, GitHubConfig(None, "once-valid", "fine_grained_pat"), rest
        )
        manager.list_repositories()
        complete = manager.sync("101")
        cached_item = complete["workItems"][0]["id"]
        require(manager.status()["credentialVerified"],
                "successful authenticated catalog was not verified")
        rest.fail_kind = "/issues?"
        rest.fail_code = "auth_failed"
        before = len(rest.calls)
        try:
            manager.sync("101")
        except GitHubProblem as error:
            require(error.status == 401 and error.code == "auth_failed",
                    "revoked credential failed unclearly")
        else:
            raise SystemExit("github manager check failed: revoked sync returned cache")
        require(len(rest.calls) - before == 1,
                "auth failure continued to other GitHub endpoints")
        status = manager.status()
        require(not status["credentialVerified"]
                and status["selectedRepository"] is None
                and status["summary"]["openIssues"] is None,
                "auth failure left cached private data browser-readable")
        try:
            manager.import_item(cached_item, {}, workflow(home))
        except GitHubProblem as error:
            require(error.status == 404,
                    "revoked cached import failed with the wrong boundary")
        else:
            raise SystemExit("github manager check failed: revoked item imported")


def check_versioned_http_contract() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        manager = GitHubManager(
            home, GitHubConfig("Morshedvarzandeh", None, "public_only"), FakeRest()
        )
        owi_serve.Handler.home = home
        owi_serve.Handler.data = owi_serve.setup_payload(home)
        owi_serve.Handler.token = "github-http-check"
        owi_serve.Handler.workflow = workflow(home)
        owi_serve.Handler.github = manager
        server = ThreadingHTTPServer(("127.0.0.1", 0), owi_serve.Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        base = f"http://127.0.0.1:{server.server_address[1]}"
        def call(path: str, method: str = "GET", body: dict | None = None):
            data = json.dumps(body).encode() if body is not None else None
            request = urllib.request.Request(
                base + path, data=data, method=method,
                headers={"X-OWI-Token": "github-http-check",
                         **({"Content-Type": "application/json"}
                            if body is not None else {})},
            )
            with urllib.request.urlopen(request, timeout=5) as response:
                return response.status, json.load(response)
        try:
            status, payload = call("/api/v1/github/status")
            require(status == 200 and payload["version"] == "v1",
                    "versioned status route is not wired")
            _, catalog = call("/api/v1/github/repositories")
            require(catalog["repositories"][0]["id"] == "101",
                    "repository route did not use server discovery")
            _, synced = call("/api/v1/github/repositories/101/sync", "POST", {})
            item_id = synced["workItems"][0]["id"]
            route_id = urllib.parse.quote(item_id, safe="")
            _, imported = call(
                f"/api/v1/github/work-items/{route_id}/import", "POST", {}
            )
            require(imported["task"]["status"] == "draft",
                    "versioned import route did not create a local draft")
            _, work = call("/api/v1/github/work-items?repositoryId=101")
            require(any(item["selectedForWork"] for item in work["workItems"]),
                    "work-item route lost imported state")
            import_url = f"{base}/api/v1/github/work-items/{route_id}/import"
            def rejected(raw: bytes | None) -> int:
                request = urllib.request.Request(
                    import_url, data=raw, method="POST",
                    headers={"X-OWI-Token": "github-http-check",
                             "Content-Type": "application/json"},
                )
                try:
                    urllib.request.urlopen(request, timeout=5)
                except urllib.error.HTTPError as error:
                    return error.code
                return 200
            require(rejected(None) == 400 and rejected(b"[") == 400
                    and rejected(b"[]") == 400
                    and rejected(b'{"unexpected":true}') == 400,
                    "empty/malformed/non-object/unknown import body was admitted")
        finally:
            server.shutdown()


def main() -> int:
    check_public_sync_and_import()
    check_delete_reimport_and_concurrency()
    check_private_floor_and_partial_truth()
    check_transport_guards_and_token_file()
    check_same_process_token_revocation()
    check_versioned_http_contract()
    print("github manager verified: fixed-host GET-only catalog/sync, distinct "
          "issues/PRs/failed runs, partial truth, secure credentials/storage, "
          "private-content floor, and idempotent zero-spend draft import")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
