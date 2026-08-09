#!/usr/bin/env python3
"""Focused checks for the production-honest local OWI Office launcher."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import sqlite3
import tempfile
import threading
import urllib.error
import urllib.request
from http.server import ThreadingHTTPServer
from pathlib import Path


TOOLS = Path(__file__).resolve().parent
loader = importlib.machinery.SourceFileLoader("owi_serve_check", str(TOOLS / "owi-serve"))
spec = importlib.util.spec_from_loader("owi_serve_check", loader)
owi_serve = importlib.util.module_from_spec(spec)
loader.exec_module(owi_serve)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"serve launcher check failed: {message}")


def check_no_implicit_demo() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch) / "not-created"
        calls: list[Path] = []
        original = owi_serve.owi_do.bootstrap
        owi_serve.owi_do.bootstrap = lambda target: calls.append(target)
        try:
            state = owi_serve.prepare_workspace(home, demo=False)
            require(not calls, "default start called the demo bootstrap")
            require(not home.exists(), "default start created a workspace")
            require(state["configured"] is False, "empty workspace looks configured")
            require(state["workers"] == [], "empty workspace contains workers")
            require(state["posteriors"] == {}, "empty workspace contains abilities")
            require(state["runnable"] == [], "empty workspace contains runners")
            require("PATH_TO_EXISTING_OWI_WORKSPACE" in
                    state["setup"]["existingCommand"],
                    "setup points back to the known-missing directory")

            owi_serve.prepare_workspace(home, demo=True)
            require(calls == [home], "--demo did not explicitly invoke bootstrap")
        finally:
            owi_serve.owi_do.bootstrap = original


def check_stored_catalog_and_ledger() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        home = Path(scratch)
        index = sqlite3.connect(home / "index.sqlite")
        index.executescript(
            """
            CREATE TABLE snapshots (
              id TEXT, source_revision TEXT, worker_profile_ids_json TEXT,
              created_at TEXT
            );
            CREATE TABLE provider_offerings (
              id TEXT, input_micros_per_million_tokens INTEGER,
              output_micros_per_million_tokens INTEGER
            );
            CREATE TABLE worker_profiles (
              id TEXT, offering_id TEXT, supported_skill_ids_json TEXT,
              tools_json TEXT, privacy_clearance TEXT
            );
            """
        )
        index.execute(
            "INSERT INTO snapshots VALUES (?, ?, ?, ?)",
            ("snapshot:real", "user:real-v1", json.dumps(["worker:mine/text"]),
             "2026-08-09T00:00:00Z"),
        )
        index.executemany(
            "INSERT INTO provider_offerings VALUES (?, ?, ?)",
            [("offering:mine", 123, 456), ("offering:fixture", 999, 999)],
        )
        index.executemany(
            "INSERT INTO worker_profiles VALUES (?, ?, ?, ?, ?)",
            [
                ("worker:mine/text", "offering:mine",
                 json.dumps(["skill:text-editing"]), json.dumps([]),
                 "private_metadata"),
                ("worker:not-in-snapshot/text", "offering:fixture",
                 json.dumps(["skill:text-editing"]), json.dumps([]),
                 "private_metadata"),
            ],
        )
        index.commit()
        index.close()

        catalog = owi_serve.stored_catalog(home)
        require(catalog["snapshot"] == "snapshot:real", "wrong snapshot loaded")
        require([row["id"] for row in catalog["workers"]] ==
                ["worker:mine/text"],
                "office roster was not limited to stored snapshot members")
        require(catalog["workers"][0]["inRate"] == 123,
                "office price did not come from the selected index")

        local = sqlite3.connect(home / "local.sqlite")
        local.execute(
            "CREATE TABLE outcome_events (id TEXT, worker_id TEXT, skill_id TEXT, "
            "accepted INTEGER, metadata_json TEXT, observed_at TEXT)"
        )
        local.executemany(
            "INSERT INTO outcome_events VALUES (?, ?, ?, ?, ?, ?)",
            [
                ("o1", "worker:mine/text", "skill:text-editing", 1, "{}", "1"),
                ("o2", "worker:mine/text", "skill:text-editing", 0,
                 '{"root_cause":"worker"}', "2"),
                ("o3", "worker:mine/text", "skill:text-editing", 0,
                 '{"root_cause":"environment"}', "3"),
            ],
        )
        local.commit()
        local.close()
        feedback = owi_serve.ledger_feedback(home)
        require(feedback["total"] == 3 and feedback["accepted"] == 1,
                "ledger totals are not derived from outcome_events")
        require(feedback["modelRejected"] == 1,
                "environment failure was counted against the model")
        require(feedback["rows"][0]["otherRejected"] == 1,
                "non-model rejection disappeared from the ledger report")


def request_status(request: urllib.request.Request) -> tuple[int, object]:
    try:
        with urllib.request.urlopen(request, timeout=5) as response:
            return response.status, response.headers
    except urllib.error.HTTPError as error:
        return error.code, error.headers


def check_http_gate() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        owi_serve.Handler.home = Path(scratch)
        owi_serve.Handler.data = owi_serve.setup_payload(Path(scratch))
        owi_serve.Handler.token = "launcher-check-token"
        server = ThreadingHTTPServer(("127.0.0.1", 0), owi_serve.Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            base = f"http://127.0.0.1:{server.server_address[1]}"
            status, _ = request_status(urllib.request.Request(f"{base}/api/data"))
            require(status == 401, "localhost API is available without a token")

            status, headers = request_status(urllib.request.Request(
                f"{base}/?token=launcher-check-token"))
            require(status == 200, "tokenized first page was rejected")
            cookie = headers.get("Set-Cookie")
            require(cookie and "owi_token=launcher-check-token" in cookie,
                    "first page did not exchange the token for a cookie")
            cookie_pair = cookie.split(";", 1)[0]

            plain = urllib.request.Request(
                f"{base}/api/run", data=b"{}", method="POST",
                headers={"Cookie": cookie_pair, "Content-Type": "text/plain"},
            )
            status, _ = request_status(plain)
            require(status == 415, "non-JSON write was admitted")

            foreign = urllib.request.Request(
                f"{base}/api/run", data=b"{}", method="POST",
                headers={"Cookie": cookie_pair, "Content-Type": "application/json",
                         "Origin": "https://attacker.example",
                         "Sec-Fetch-Site": "cross-site"},
            )
            status, _ = request_status(foreign)
            require(status == 403, "cross-origin write was admitted")

            same = urllib.request.Request(
                f"{base}/api/run", data=b"{}", method="POST",
                headers={"Cookie": cookie_pair, "Content-Type": "application/json",
                         "Origin": base, "Sec-Fetch-Site": "same-origin"},
            )
            status, _ = request_status(same)
            require(status == 409,
                    "unconfigured office did not stop execution before a runner")
        finally:
            server.shutdown()


def main() -> int:
    check_no_implicit_demo()
    check_stored_catalog_and_ledger()
    check_http_gate()
    print("serve launcher verified: no implicit demo, stored roster/prices, "
          "ledger feedback, token gate, JSON-only same-origin writes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
