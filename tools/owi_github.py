#!/usr/bin/env python3
"""Read-only GitHub Manager adapter for the local OWI Office.

GitHub credentials stay in this server process.  The browser receives only a
bounded repository/work-item projection, and every outbound operation is a GET
to the fixed GitHub REST API host.  Import is a separate, explicit local action
that creates an unassigned OWI draft; it never changes GitHub or invokes a
worker.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import sqlite3
import stat
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path


API_ORIGIN = "https://api.github.com"
API_HOST = "api.github.com"
API_VERSION = "2026-03-10"
AUTH_MODES = {"fine_grained_pat", "github_app_user_token"}
OWNER_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$")
SEGMENT_RE = re.compile(r"^[A-Za-z0-9_.-]+$")
KINDS = ("issue", "pull_request", "action_failure")
MAX_PAGES = 5
MAX_REPOSITORIES = 500
MAX_INSTALLATIONS = 5
MAX_BODY_BYTES = 5_000_000


class GitHubProblem(RuntimeError):
    """A bounded problem safe to return to the local browser."""

    def __init__(self, message: str, status: int = 400, code: str = "github_error",
                 details: dict | None = None):
        super().__init__(message)
        self.status = status
        self.code = code
        self.details = details or {}


def _now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def _safe_segment(value: object, label: str) -> str:
    text = str(value or "")
    if (not text or len(text) > 100 or text in (".", "..")
            or not SEGMENT_RE.fullmatch(text)
            or "/" in text or "\\" in text or "%" in text):
        raise GitHubProblem(f"invalid cached GitHub {label}", 409,
                            "invalid_cached_identity")
    return urllib.parse.quote(text, safe="")


def _safe_web_url(value: object) -> str:
    text = str(value or "")
    parsed = urllib.parse.urlsplit(text)
    try:
        unsafe_port = parsed.port is not None
    except ValueError:
        return ""
    if (parsed.scheme != "https" or parsed.hostname != "github.com"
            or parsed.username or parsed.password or unsafe_port):
        return ""
    return text[:2000]


def _safe_secret_file(path: Path) -> str:
    absolute = Path(os.path.abspath(os.path.expanduser(str(path))))
    for component in (absolute, *absolute.parents):
        try:
            if stat.S_ISLNK(os.lstat(component).st_mode):
                raise GitHubProblem("GitHub token file path may not contain a symlink",
                                    409, "unsafe_token_file")
        except FileNotFoundError:
            if component == absolute:
                raise GitHubProblem("GitHub token file does not exist", 409,
                                    "missing_token_file")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(absolute, flags)
    except OSError as error:
        raise GitHubProblem("GitHub token file cannot be opened safely", 409,
                            "unsafe_token_file") from error
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise GitHubProblem("GitHub token file must be a regular file", 409,
                                "unsafe_token_file")
        if hasattr(os, "getuid") and metadata.st_uid != os.getuid():
            raise GitHubProblem("GitHub token file must be owned by this user", 409,
                                "unsafe_token_file")
        if os.name != "nt" and metadata.st_mode & 0o077:
            raise GitHubProblem("GitHub token file permissions must be 0600",
                                409, "unsafe_token_file")
        try:
            value = os.read(descriptor, 16_385).decode("utf-8").strip()
        except UnicodeError as error:
            raise GitHubProblem("GitHub token file is not valid UTF-8", 409,
                                "invalid_token_file") from error
    finally:
        os.close(descriptor)
    if not value or len(value) > 16_384 or "\x00" in value:
        raise GitHubProblem("GitHub token file is empty or invalid", 409,
                            "invalid_token_file")
    return value


@dataclass(frozen=True)
class GitHubConfig:
    owner: str | None
    token: str | None
    auth_mode: str

    @classmethod
    def load(cls, owner: str | None = None, token_file: Path | None = None,
             auth_mode: str | None = None) -> "GitHubConfig":
        configured_owner = owner or os.environ.get("OWI_GITHUB_OWNER") or None
        if configured_owner and not OWNER_RE.fullmatch(configured_owner):
            raise GitHubProblem("configured GitHub owner is invalid", 409,
                                "invalid_owner")
        env_token = os.environ.get("OWI_GITHUB_TOKEN")
        configured_file = token_file or (
            Path(os.environ["OWI_GITHUB_TOKEN_FILE"])
            if os.environ.get("OWI_GITHUB_TOKEN_FILE") else None
        )
        if env_token and configured_file:
            raise GitHubProblem("configure one GitHub credential source, not two",
                                409, "ambiguous_credentials")
        token = env_token.strip() if env_token is not None else (
            _safe_secret_file(configured_file) if configured_file else None
        )
        if token is not None and (
            not token or len(token) > 16_384
            or any(ord(character) < 33 or ord(character) == 127
                   for character in token)
        ):
            raise GitHubProblem("GitHub token is invalid", 409,
                                "invalid_token")
        if token:
            mode = auth_mode or os.environ.get("OWI_GITHUB_AUTH_MODE") \
                or "fine_grained_pat"
            if mode not in AUTH_MODES:
                raise GitHubProblem("unknown GitHub authentication mode", 409,
                                    "invalid_auth_mode")
        else:
            mode = "public_only" if configured_owner else "unconfigured"
        return cls(configured_owner, token, mode)


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise urllib.error.HTTPError(req.full_url, code, "redirect rejected",
                                     headers, fp)


class GitHubRestClient:
    """Small fixed-host, GET-only REST transport."""

    def __init__(self, token: str | None, timeout: float = 12.0):
        self._token = token
        self.timeout = timeout
        self._opener = urllib.request.build_opener(_NoRedirect())

    @staticmethod
    def _relative_from_next(value: str) -> str:
        parsed = urllib.parse.urlsplit(value)
        try:
            unsafe_port = parsed.port is not None
        except ValueError as error:
            raise GitHubProblem("GitHub pagination target was rejected", 502,
                                "unsafe_pagination") from error
        if (parsed.scheme != "https" or parsed.hostname != API_HOST
                or parsed.username or parsed.password or unsafe_port):
            raise GitHubProblem("GitHub pagination target was rejected", 502,
                                "unsafe_pagination")
        return parsed.path + (("?" + parsed.query) if parsed.query else "")

    @staticmethod
    def _next(headers) -> str | None:
        link = headers.get("Link") or ""
        for part in link.split(","):
            match = re.match(r'\s*<([^>]+)>;\s*rel="([^"]+)"', part)
            if match and match.group(2) == "next":
                return GitHubRestClient._relative_from_next(match.group(1))
        return None

    def get(self, relative: str) -> tuple[object, dict]:
        if not relative.startswith("/") or relative.startswith("//"):
            raise GitHubProblem("invalid GitHub API path", 500,
                                "invalid_api_path")
        parsed = urllib.parse.urlsplit(API_ORIGIN + relative)
        if parsed.scheme != "https" or parsed.hostname != API_HOST:
            raise GitHubProblem("GitHub API host was rejected", 500,
                                "invalid_api_host")
        headers = {
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": "open-workforce-index-local-manager",
        }
        if self._token:
            headers["Authorization"] = f"Bearer {self._token}"
        request = urllib.request.Request(API_ORIGIN + relative, headers=headers,
                                         method="GET")
        try:
            with self._opener.open(request, timeout=self.timeout) as response:
                declared = response.headers.get("Content-Length")
                if declared and int(declared) > MAX_BODY_BYTES:
                    raise GitHubProblem("GitHub response was too large", 502,
                                        "upstream_too_large")
                raw = response.read(MAX_BODY_BYTES + 1)
                if len(raw) > MAX_BODY_BYTES:
                    raise GitHubProblem("GitHub response was too large", 502,
                                        "upstream_too_large")
                payload = json.loads(raw.decode("utf-8"))
                meta = {
                    "remaining": _integer(response.headers.get("X-RateLimit-Remaining")),
                    "resetAt": _epoch_iso(response.headers.get("X-RateLimit-Reset")),
                    "next": self._next(response.headers),
                }
                return payload, meta
        except GitHubProblem:
            raise
        except urllib.error.HTTPError as error:
            retry = _integer(error.headers.get("Retry-After")) if error.headers else None
            reset = _epoch_iso(error.headers.get("X-RateLimit-Reset")) \
                if error.headers else None
            remaining = _integer(error.headers.get("X-RateLimit-Remaining")) \
                if error.headers else None
            if error.code == 401:
                status, code = 401, "auth_failed"
            elif error.code == 429 or (error.code == 403 and
                                       (retry is not None or remaining == 0)):
                status, code = 429, "rate_limited"
            elif error.code == 403:
                status, code = 403, "permission_denied"
            elif error.code == 404:
                status, code = 404, "upstream_not_found"
            else:
                status, code = 502, "upstream_http_error"
            raise GitHubProblem("GitHub could not complete this read-only request",
                                status, code,
                                {"retryAfterSeconds": retry, "resetAt": reset}) from error
        except (OSError, TimeoutError, UnicodeError, json.JSONDecodeError, ValueError) as error:
            raise GitHubProblem("GitHub returned no usable response", 502,
                                "upstream_unavailable") from error

    def pages(self, relative: str, array_key: str | None = None,
              max_pages: int = MAX_PAGES) -> tuple[list, dict]:
        values: list = []
        seen: set[str] = set()
        next_path: str | None = relative
        last_meta = {"remaining": None, "resetAt": None, "next": None}
        pages = 0
        while next_path and pages < max_pages:
            if next_path in seen:
                raise GitHubProblem("GitHub pagination cycle was rejected", 502,
                                    "pagination_cycle")
            seen.add(next_path)
            payload, last_meta = self.get(next_path)
            page = payload.get(array_key) if array_key and isinstance(payload, dict) \
                else payload
            if not isinstance(page, list):
                raise GitHubProblem("GitHub response shape was not recognized", 502,
                                    "upstream_shape")
            values.extend(page)
            next_path = last_meta.get("next")
            pages += 1
        return values, {
            "remaining": last_meta.get("remaining"),
            "resetAt": last_meta.get("resetAt"),
            "truncated": bool(next_path),
            "nextCursor": None,
            "pages": pages,
        }


def _integer(value) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _epoch_iso(value) -> str | None:
    number = _integer(value)
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(number)) \
        if number is not None else None


class GitHubManager:
    """Catalog, manual sync cache and explicit local draft importer."""

    def __init__(self, home: Path, config: GitHubConfig,
                 rest: GitHubRestClient | None = None):
        self.home = Path(home)
        self.path = self.home / "github-manager.sqlite"
        self.config = config
        self.rest = rest or GitHubRestClient(config.token)
        self._database_lock = threading.RLock()
        self._import_lock = threading.Lock()
        # Cached identities are not authority after a restart or credential
        # scope change.  Each process must rediscover its allowed catalog.
        self._catalog_ids: set[str] = set()
        self._credential_verified = False

    def _ensure_safe_database(self) -> None:
        absolute_home = self.home.absolute()
        for component in (absolute_home, *absolute_home.parents):
            if component.is_symlink():
                raise GitHubProblem("GitHub Manager path contains a symlink", 409,
                                    "unsafe_storage")
        self.home.mkdir(parents=True, exist_ok=True)
        flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
        try:
            descriptor = os.open(self.path, flags, 0o600)
        except OSError as error:
            raise GitHubProblem("GitHub Manager storage is unsafe", 409,
                                "unsafe_storage") from error
        try:
            metadata = os.fstat(descriptor)
            if not stat.S_ISREG(metadata.st_mode):
                raise GitHubProblem("GitHub Manager storage must be a regular file",
                                    409, "unsafe_storage")
            if hasattr(os, "getuid") and metadata.st_uid != os.getuid():
                raise GitHubProblem("GitHub Manager storage has the wrong owner",
                                    409, "unsafe_storage")
            if os.name != "nt":
                os.fchmod(descriptor, 0o600)
        finally:
            os.close(descriptor)

    @contextmanager
    def _connect(self):
        with self._database_lock:
            self._ensure_safe_database()
            connection = sqlite3.connect(self.path, timeout=10)
            connection.row_factory = sqlite3.Row
            try:
                connection.executescript("""
                CREATE TABLE IF NOT EXISTS repositories (
                  id TEXT PRIMARY KEY, owner TEXT NOT NULL, name TEXT NOT NULL,
                  full_name TEXT NOT NULL, private INTEGER NOT NULL,
                  archived INTEGER NOT NULL, default_branch TEXT NOT NULL,
                  web_url TEXT NOT NULL, updated_at TEXT, selected INTEGER NOT NULL,
                  catalog_active INTEGER NOT NULL DEFAULT 1,
                  last_sync_at TEXT, sync_partial INTEGER,
                  catalog_seen_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS sync_state (
                  repository_id TEXT NOT NULL, kind TEXT NOT NULL,
                  coverage TEXT NOT NULL, observed_at TEXT, error_code TEXT,
                  item_count INTEGER, truncated INTEGER NOT NULL DEFAULT 0,
                  PRIMARY KEY(repository_id, kind)
                );
                CREATE TABLE IF NOT EXISTS work_items (
                  id TEXT PRIMARY KEY, repository_id TEXT NOT NULL,
                  source_type TEXT NOT NULL, source_id TEXT NOT NULL,
                  source_number INTEGER, title TEXT NOT NULL, body TEXT NOT NULL,
                  web_url TEXT NOT NULL, state TEXT NOT NULL,
                  labels_json TEXT NOT NULL, updated_at TEXT,
                  revision TEXT NOT NULL, digest TEXT NOT NULL,
                  observed_at TEXT,
                  active INTEGER NOT NULL, imported_project_id TEXT,
                  imported_task_id TEXT, imported_at TEXT,
                  UNIQUE(repository_id, source_type, source_id)
                );
                CREATE INDEX IF NOT EXISTS github_items_repository
                  ON work_items(repository_id, source_type, active);
                CREATE TABLE IF NOT EXISTS manager_meta (
                  key TEXT PRIMARY KEY, value TEXT NOT NULL
                );
                """)
                repository_columns = {
                    row[1] for row in connection.execute(
                        "PRAGMA table_info(repositories)"
                    )
                }
                if "catalog_active" not in repository_columns:
                    connection.execute(
                        "ALTER TABLE repositories ADD COLUMN catalog_active "
                        "INTEGER NOT NULL DEFAULT 0"
                    )
                item_columns = {
                    row[1] for row in connection.execute(
                        "PRAGMA table_info(work_items)"
                    )
                }
                if "observed_at" not in item_columns:
                    connection.execute(
                        "ALTER TABLE work_items ADD COLUMN observed_at TEXT"
                    )
                with connection:
                    yield connection
            finally:
                connection.close()

    def _record_rate(self, meta: dict) -> None:
        values = {"rate_remaining": meta.get("remaining"),
                  "rate_reset_at": meta.get("resetAt")}
        with self._connect() as connection:
            for key, value in values.items():
                if value is not None:
                    connection.execute(
                        "INSERT INTO manager_meta VALUES (?, ?) ON CONFLICT(key) "
                        "DO UPDATE SET value=excluded.value", (key, str(value))
                    )

    @staticmethod
    def _repo(raw: dict) -> dict | None:
        repo_id = raw.get("id")
        owner = (raw.get("owner") or {}).get("login")
        name = raw.get("name")
        if not isinstance(repo_id, int) or not isinstance(owner, str) \
                or not isinstance(name, str):
            return None
        try:
            _safe_segment(owner, "owner")
            _safe_segment(name, "repository")
        except GitHubProblem:
            return None
        return {
            "id": str(repo_id), "owner": owner, "name": name,
            "fullName": str(raw.get("full_name") or f"{owner}/{name}")[:260],
            "private": bool(raw.get("private")),
            "archived": bool(raw.get("archived")),
            "defaultBranch": str(raw.get("default_branch") or "")[:200],
            "url": _safe_web_url(raw.get("html_url")),
            "updatedAt": raw.get("updated_at"), "selected": False,
        }

    def list_repositories(self) -> dict:
        # Discovery is the authority boundary for this process.  Revoke the
        # previous in-memory catalog before any request so a failed refresh
        # cannot leave old private scope usable.
        self._catalog_ids.clear()
        self._credential_verified = False
        if self.config.auth_mode == "unconfigured":
            return {"repositories": [], "source": "unconfigured",
                    "truncated": False, "nextPage": None, "nextCursor": None,
                    "rateLimit": {"remaining": None, "resetAt": None}}
        if self.config.auth_mode == "github_app_user_token":
            installs, meta = self.rest.pages("/user/installations?per_page=100",
                                             "installations")
            rows, truncated = [], bool(meta["truncated"])
            if len(installs) > MAX_INSTALLATIONS:
                truncated = True
            for installation in installs[:MAX_INSTALLATIONS]:
                if len(rows) >= MAX_REPOSITORIES:
                    truncated = True
                    break
                installation_id = installation.get("id")
                if not isinstance(installation_id, int):
                    continue
                found, part = self.rest.pages(
                    f"/user/installations/{installation_id}/repositories?per_page=100",
                    "repositories",
                )
                rows.extend(found)
                truncated = truncated or bool(part["truncated"])
                meta = part
            source = "github_app_installations"
        elif self.config.token:
            rows, meta = self.rest.pages(
                "/user/repos?affiliation=owner,collaborator,organization_member"
                "&sort=updated&per_page=100"
            )
            truncated = bool(meta["truncated"])
            source = "authenticated_user"
        else:
            owner = _safe_segment(self.config.owner, "owner")
            rows, meta = self.rest.pages(
                f"/users/{owner}/repos?type=owner&sort=updated&per_page=100"
            )
            truncated = bool(meta["truncated"])
            source = "configured_public_owner"
        repositories = []
        seen = set()
        for raw in rows:
            repository = self._repo(raw) if isinstance(raw, dict) else None
            if repository and repository["id"] not in seen:
                seen.add(repository["id"])
                repositories.append(repository)
                if len(repositories) >= MAX_REPOSITORIES:
                    truncated = truncated or len(rows) > len(repositories)
                    break
        observed = _now()
        with self._connect() as connection:
            connection.execute("UPDATE repositories SET catalog_active=0")
            selected = connection.execute(
                "SELECT id FROM repositories WHERE selected=1"
            ).fetchone()
            selected_id = selected[0] if selected else None
            for repo in repositories:
                repo["selected"] = repo["id"] == selected_id
                connection.execute(
                    "INSERT INTO repositories "
                    "(id,owner,name,full_name,private,archived,default_branch,web_url,"
                    "updated_at,selected,catalog_active,catalog_seen_at) "
                    "VALUES (?,?,?,?,?,?,?,?,?,?,1,?) "
                    "ON CONFLICT(id) DO UPDATE SET owner=excluded.owner,name=excluded.name,"
                    "full_name=excluded.full_name,private=excluded.private,"
                    "archived=excluded.archived,default_branch=excluded.default_branch,"
                    "web_url=excluded.web_url,updated_at=excluded.updated_at,"
                    "catalog_active=1,catalog_seen_at=excluded.catalog_seen_at",
                    (repo["id"], repo["owner"], repo["name"], repo["fullName"],
                     int(repo["private"]), int(repo["archived"]),
                     repo["defaultBranch"], repo["url"], repo["updatedAt"],
                     int(repo["selected"]), observed),
                )
        self._record_rate(meta)
        self._catalog_ids = {repo["id"] for repo in repositories}
        self._credential_verified = bool(self.config.token)
        return {"repositories": repositories, "source": source,
                "truncated": truncated, "nextPage": None, "nextCursor": None,
                "rateLimit": {"remaining": meta.get("remaining"),
                              "resetAt": meta.get("resetAt")}}

    @staticmethod
    def _row_repo(row) -> dict:
        return {"id": row["id"], "owner": row["owner"], "name": row["name"],
                "fullName": row["full_name"], "private": bool(row["private"]),
                "archived": bool(row["archived"]),
                "defaultBranch": row["default_branch"], "url": row["web_url"],
                "updatedAt": row["updated_at"], "selected": bool(row["selected"])}

    def _known_repository(self, repository_id: str) -> tuple[sqlite3.Row, dict]:
        if (not str(repository_id).isdigit()
                or str(repository_id) not in self._catalog_ids):
            raise GitHubProblem("repository was not discovered by this server",
                                404, "repository_not_found")
        with self._connect() as connection:
            row = connection.execute(
                "SELECT * FROM repositories WHERE id=? AND catalog_active=1 "
                "AND (? OR private=0)",
                (str(repository_id), int(bool(self.config.token)))
            ).fetchone()
        if row is None:
            raise GitHubProblem("repository was not discovered by this server",
                                404, "repository_not_found")
        return row, self._row_repo(row)

    @staticmethod
    def _item(repository_id: str, kind: str, raw: dict) -> dict | None:
        source_id = raw.get("id")
        if not isinstance(source_id, int):
            return None
        if kind == "action_failure":
            title = raw.get("name") or raw.get("display_title") or "Failed workflow"
            number = raw.get("run_number")
            state = str(raw.get("conclusion") or "failure")
            revision = str(raw.get("updated_at") or raw.get("head_sha") or source_id)
            body = str(raw.get("display_title") or "")
        else:
            title = raw.get("title") or "Untitled GitHub item"
            number = raw.get("number")
            state = str(raw.get("state") or "open")
            revision = str(raw.get("updated_at") or source_id)
            body = str(raw.get("body") or "")
        labels = [str(label.get("name"))[:100] for label in raw.get("labels", [])
                  if isinstance(label, dict) and label.get("name")][:50]
        canonical = {"id": source_id, "kind": kind, "revision": revision,
                     "title": str(title), "body": body, "state": state,
                     "number": number}
        digest = hashlib.sha256(json.dumps(
            canonical, sort_keys=True, separators=(",", ":")
        ).encode()).hexdigest()
        return {
            "id": f"github:{repository_id}:{kind}:{source_id}",
            "repositoryId": repository_id, "sourceType": kind,
            "sourceId": str(source_id),
            "sourceNumber": number if isinstance(number, int) else None,
            "title": str(title).strip()[:500], "body": body[:16_000],
            "url": _safe_web_url(raw.get("html_url")), "state": state[:50],
            "labels": labels, "updatedAt": raw.get("updated_at"),
            "revision": revision[:500], "digest": digest,
        }

    def sync(self, repository_id: str) -> dict:
        row, repository = self._known_repository(repository_id)
        owner = _safe_segment(row["owner"], "owner")
        name = _safe_segment(row["name"], "repository")
        specs = {
            "issue": (f"/repos/{owner}/{name}/issues?state=open&per_page=100", None),
            "pull_request": (f"/repos/{owner}/{name}/pulls?state=open&per_page=100", None),
            "action_failure": (
                f"/repos/{owner}/{name}/actions/runs?status=failure&per_page=100",
                "workflow_runs",
            ),
        }
        observed = _now()
        coverage = {}
        problems: list[dict] = []
        partial = False
        last_meta = {"remaining": None, "resetAt": None}
        for kind, (path, key) in specs.items():
            try:
                raw_items, meta = self.rest.pages(path, key)
                last_meta = meta
                if kind == "issue":
                    raw_items = [item for item in raw_items
                                 if isinstance(item, dict)
                                 and "pull_request" not in item]
                items = [item for raw in raw_items
                         if isinstance(raw, dict)
                         if (item := self._item(str(repository_id), kind, raw))]
                self._store_sync(str(repository_id), kind, items, observed,
                                 bool(meta.get("truncated")), None)
                truncated = bool(meta.get("truncated"))
                coverage[kind] = {
                                  "status": "partial" if truncated else "observed",
                                  "count": None if truncated else len(items),
                                  "loadedCount": len(items),
                                  "observedAt": observed,
                                  "truncated": truncated}
                partial = partial or truncated
            except GitHubProblem as error:
                partial = True
                problems.append({"kind": kind, "code": error.code,
                                 **error.details})
                if error.code == "auth_failed":
                    self._credential_verified = False
                    self._catalog_ids.clear()
                    self._store_sync(str(repository_id), kind, None, observed,
                                     False, "auth_failed")
                    with self._connect() as connection:
                        connection.execute(
                            "UPDATE repositories SET catalog_active=0,selected=0"
                        )
                    raise GitHubProblem(
                        "GitHub credential is no longer authorized", 401,
                        "auth_failed"
                    ) from error
                if error.code == "rate_limited":
                    last_meta = {"remaining": 0,
                                 "resetAt": error.details.get("resetAt")}
                self._store_sync(str(repository_id), kind, None, observed, False,
                                 error.code)
                coverage[kind] = {"status": "unknown", "count": None,
                                  "loadedCount": None,
                                  "observedAt": None, "truncated": False,
                                  "error": error.code,
                                  **error.details}
                if error.code == "rate_limited":
                    for remaining_kind in KINDS[KINDS.index(kind) + 1:]:
                        self._store_sync(str(repository_id), remaining_kind,
                                         None, observed, False, "rate_limited")
                        coverage[remaining_kind] = {
                            "status": "unknown", "count": None,
                            "loadedCount": None, "observedAt": None,
                            "truncated": False, "error": "rate_limited",
                            **error.details,
                        }
                    break
        observed_kinds = sum(value["status"] in ("observed", "partial")
                             for value in coverage.values())
        successful_at = observed if observed_kinds else row["last_sync_at"]
        with self._connect() as connection:
            connection.execute("UPDATE repositories SET selected=0")
            if observed_kinds:
                connection.execute(
                    "UPDATE repositories SET selected=1,last_sync_at=?,sync_partial=? "
                    "WHERE id=?", (observed, int(partial), str(repository_id))
                )
            else:
                connection.execute(
                    "UPDATE repositories SET selected=1,sync_partial=1 WHERE id=?",
                    (str(repository_id),),
                )
        self._record_rate(last_meta)
        repository["selected"] = True
        result = self.work_items(str(repository_id))
        outcome = "complete" if not partial else (
            "failed" if observed_kinds == 0 else "partial"
        )
        warning = None if outcome == "complete" else {
            "code": problems[0]["code"] if problems else "truncated",
            "message": "GitHub observation is incomplete; unknown values are not zero.",
            "problems": problems,
        }
        return {"repository": repository, "outcome": outcome,
                "warning": warning, "partial": partial, "stale": partial,
                "sync": {"attemptedAt": observed, "syncedAt": successful_at,
                         "partial": partial,
                         "coverage": coverage,
                         "openIssues": coverage["issue"]["count"],
                         "openPullRequests": coverage["pull_request"]["count"],
                         "failedCi": coverage["action_failure"]["count"],
                         "failedActionRuns": coverage["action_failure"]["count"],
                         "rateLimit": {"remaining": last_meta.get("remaining"),
                                       "resetAt": last_meta.get("resetAt")}},
                "workItems": result["workItems"]}

    def _store_sync(self, repository_id: str, kind: str, items: list[dict] | None,
                    observed: str, truncated: bool, error_code: str | None) -> None:
        with self._connect() as connection:
            if items is not None:
                # Fetched items become the current active set. On truncation we
                # retain unseen cache rows inactive (never delete or present
                # them as freshly observed).
                connection.execute(
                    "UPDATE work_items SET active=0 WHERE repository_id=? "
                    "AND source_type=?", (repository_id, kind)
                )
                for item in items:
                    connection.execute(
                        "INSERT INTO work_items "
                        "(id,repository_id,source_type,source_id,source_number,title,"
                        "body,web_url,state,labels_json,updated_at,revision,digest,"
                        "observed_at,active) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,1) "
                        "ON CONFLICT(id) DO UPDATE "
                        "SET source_number=excluded.source_number,title=excluded.title,"
                        "body=excluded.body,web_url=excluded.web_url,state=excluded.state,"
                        "labels_json=excluded.labels_json,updated_at=excluded.updated_at,"
                        "revision=excluded.revision,digest=excluded.digest,"
                        "observed_at=excluded.observed_at,active=1",
                        (item["id"], repository_id, kind, item["sourceId"],
                         item["sourceNumber"], item["title"], item["body"],
                         item["url"], item["state"], json.dumps(item["labels"]),
                         item["updatedAt"], item["revision"], item["digest"],
                         observed),
                    )
                if not truncated:
                    connection.execute(
                        "DELETE FROM work_items WHERE repository_id=? AND source_type=? "
                        "AND active=0 AND imported_task_id IS NULL",
                        (repository_id, kind)
                    )
            connection.execute(
                "INSERT INTO sync_state VALUES (?,?,?,?,?,?,?) ON CONFLICT"
                "(repository_id,kind) DO UPDATE SET coverage=excluded.coverage,"
                "observed_at=CASE WHEN excluded.coverage='unknown' THEN "
                "sync_state.observed_at ELSE excluded.observed_at END,"
                "error_code=excluded.error_code,"
                "item_count=excluded.item_count,truncated=excluded.truncated",
                (repository_id, kind,
                 ("partial" if truncated else "observed")
                 if items is not None else "unknown",
                 observed if items is not None else None, error_code,
                 len(items) if items is not None and not truncated else None,
                 int(truncated)),
            )

    def _workflow_imports(self, repository_id: str) -> dict[str, dict]:
        workflow_path = self.home / "workflow.sqlite"
        if workflow_path.is_symlink() or not workflow_path.is_file():
            return {}
        try:
            connection = sqlite3.connect(
                workflow_path.resolve().as_uri() + "?mode=ro", uri=True, timeout=5
            )
            connection.row_factory = sqlite3.Row
            rows = connection.execute(
                "SELECT i.source_item_type,i.source_item_id,i.project_id,i.task_id,"
                "t.status FROM source_imports i LEFT JOIN tasks t ON t.id=i.task_id "
                "WHERE i.source_system='github' AND i.source_repository_id=?",
                (repository_id,),
            ).fetchall()
            connection.close()
        except sqlite3.Error:
            return {}
        return {f"{row['source_item_type']}:{row['source_item_id']}": dict(row)
                for row in rows}

    @staticmethod
    def _lane(status: str | None) -> tuple[str, bool]:
        return {
            "draft": ("draft", False), "staffed": ("staffing", False),
            "setup_required": ("blocked", True), "unstaffed": ("blocked", True),
            "running": ("running", False), "needs_review": ("review", True),
            "accepted": ("done", False), "rejected": ("blocked", True),
            "run_failed": ("blocked", True),
        }.get(status, ("blocked", True))

    def work_items(self, repository_id: str | None = None) -> dict:
        if self.config.auth_mode == "unconfigured" or not self._catalog_ids:
            return {"repository": None, "workItems": [],
                    "summary": self._unknown_summary(), "coverage": {},
                    "partial": False, "stale": True}
        with self._connect() as connection:
            if repository_id is None:
                repo_row = connection.execute(
                    "SELECT * FROM repositories WHERE selected=1 AND "
                    "catalog_active=1 AND (? OR private=0)",
                    (int(bool(self.config.token)),),
                ).fetchone()
            else:
                if not str(repository_id).isdigit():
                    raise GitHubProblem("repository was not discovered", 404,
                                        "repository_not_found")
                repo_row = connection.execute(
                    "SELECT * FROM repositories WHERE id=? AND catalog_active=1 "
                    "AND (? OR private=0)",
                    (str(repository_id), int(bool(self.config.token))),
                ).fetchone()
            if repo_row is None:
                return {"repository": None, "workItems": [],
                        "summary": self._unknown_summary(), "coverage": {},
                        "partial": False, "stale": True}
            repository_id = repo_row["id"]
            rows = connection.execute(
                "SELECT * FROM work_items WHERE repository_id=? AND "
                "(active=1 OR imported_task_id IS NOT NULL) "
                "ORDER BY updated_at DESC,id", (repository_id,)
            ).fetchall()
            coverage_rows = connection.execute(
                "SELECT * FROM sync_state WHERE repository_id=?", (repository_id,)
            ).fetchall()
        imports = self._workflow_imports(repository_id)
        items = []
        for row in rows:
            imported = imports.get(f"{row['source_type']}:{row['source_id']}")
            status = imported.get("status") if imported else None
            lane, waiting = self._lane(status) if imported else ("incoming", False)
            items.append({
                "id": row["id"], "repositoryId": repository_id,
                "sourceType": row["source_type"],
                "sourceNumber": row["source_number"], "title": row["title"],
                "url": row["web_url"], "state": row["state"],
                "labels": json.loads(row["labels_json"] or "[]"),
                "updatedAt": row["updated_at"], "lane": lane,
                "sourceActive": bool(row["active"]),
                "waitingForOwner": waiting, "selectedForWork": bool(imported),
                "importedProjectId": imported.get("project_id") if imported else None,
                "importedTaskId": imported.get("task_id") if imported else None,
            })
        coverage = {row["kind"]: {
            "status": row["coverage"], "count": row["item_count"],
            "observedAt": row["observed_at"], "truncated": bool(row["truncated"]),
            "error": row["error_code"],
        } for row in coverage_rows}
        for item in items:
            source_coverage = coverage.get(item["sourceType"])
            item["observedAt"] = next(
                (row["observed_at"] for row in rows if row["id"] == item["id"]),
                None,
            )
            coverage_status = source_coverage.get("status") if source_coverage else None
            item["stale"] = (
                not item["sourceActive"]
                or coverage_status == "unknown" or coverage_status is None
            )
        def measured(kind):
            value = coverage.get(kind)
            return value["count"] if value and value["status"] == "observed" else None
        summary = {
            "openIssues": measured("issue"),
            "openPullRequests": measured("pull_request"),
            "failedCi": measured("action_failure"),
            "failedActionRuns": measured("action_failure"),
            "selectedWork": sum(item["selectedForWork"] for item in items),
            "waitingForOwner": sum(item["waitingForOwner"] for item in items),
        }
        partial = (len(coverage) != len(KINDS)
                   or any(value["status"] != "observed"
                          for value in coverage.values()))
        return {"repository": self._row_repo(repo_row), "workItems": items,
                "summary": summary, "coverage": coverage,
                "partial": partial, "stale": partial}

    @staticmethod
    def _unknown_summary() -> dict:
        return {"openIssues": None, "openPullRequests": None, "failedCi": None,
                "selectedWork": None, "waitingForOwner": None}

    def status(self) -> dict:
        items = self.work_items()
        with self._connect() as connection:
            meta = {row["key"]: row["value"] for row in connection.execute(
                "SELECT key,value FROM manager_meta"
            )}
            selected = connection.execute(
                "SELECT last_sync_at,sync_partial FROM repositories WHERE selected=1 "
                "AND catalog_active=1 AND (? OR private=0)",
                (int(bool(self.config.token)),),
            ).fetchone()
        configured = self.config.auth_mode != "unconfigured"
        return {
            "version": "v1", "authMode": self.config.auth_mode,
            "configured": configured, "readOnly": True,
            "githubWriteEnabled": False,
            "credentialConfigured": bool(self.config.token),
            "credentialVerified": self._credential_verified,
            "privateRepositoriesConfigured": bool(self.config.token),
            "privateRepositoriesAvailable": bool(
                self.config.token and self._credential_verified
            ),
            "configurationMessage": (
                "private repositories unavailable; configure a server-side token"
                if self.config.auth_mode == "public_only" else None
            ),
            "selectedRepository": items["repository"],
            "lastSyncAt": selected["last_sync_at"] if selected else None,
            "stale": items.get("stale", True),
            "partial": items.get("partial", False),
            "summary": items["summary"], "coverage": items["coverage"],
            "rateLimit": {"remaining": _integer(meta.get("rate_remaining")),
                          "resetAt": meta.get("rate_reset_at")},
        }

    def import_item(self, item_id: str, body: dict, workflow) -> dict:
        if not isinstance(body, dict):
            raise GitHubProblem("import body must be an object", 400,
                                "invalid_import")
        allowed = {"projectId", "privacy", "skill", "checklist"}
        if set(body) - allowed:
            raise GitHubProblem("import body contains unknown fields", 400,
                                "invalid_import")
        if (body.get("projectId") is not None
                and not isinstance(body.get("projectId"), str)):
            raise GitHubProblem("projectId must be a string", 400,
                                "invalid_import")
        if (body.get("privacy") is not None
                and not isinstance(body.get("privacy"), str)):
            raise GitHubProblem("privacy must be a string", 400,
                                "invalid_import")
        if (body.get("skill") is not None
                and not isinstance(body.get("skill"), str)):
            raise GitHubProblem("skill must be a string", 400,
                                "invalid_import")
        checklist = body.get("checklist")
        if (checklist is not None and
                (not isinstance(checklist, list)
                 or any(not isinstance(item, str) for item in checklist))):
            raise GitHubProblem("checklist must be an array of strings", 400,
                                "invalid_import")
        with self._connect() as connection:
            item = connection.execute(
                "SELECT w.*,r.full_name,r.private,w.observed_at AS source_observed_at,"
                "s.truncated AS source_truncated,s.coverage AS source_coverage,"
                "s.error_code AS source_error_code "
                "FROM work_items w JOIN repositories r ON r.id=w.repository_id "
                "AND r.catalog_active=1 "
                "LEFT JOIN sync_state s ON s.repository_id=w.repository_id "
                "AND s.kind=w.source_type WHERE w.id=? AND "
                "(w.active=1 OR w.imported_task_id IS NOT NULL) "
                "AND (? OR r.private=0)",
                (item_id, int(bool(self.config.token))),
            ).fetchone()
        if item is None:
            raise GitHubProblem("work item was not found in the server cache", 404,
                                "work_item_not_found")
        if item["repository_id"] not in self._catalog_ids:
            raise GitHubProblem("work item repository is not in the current catalog",
                                404, "work_item_not_found")
        source = {
            "repository_id": item["repository_id"],
            "repository_name": item["full_name"],
            "repository_private": bool(item["private"]),
            "item_type": item["source_type"], "item_id": item["source_id"],
            "number": item["source_number"], "revision": item["revision"],
            "digest": item["digest"], "url": item["web_url"],
            "title": item["title"], "body": item["body"],
            "source_updated_at": item["updated_at"],
            "observed_at": item["source_observed_at"],
            "api_version": API_VERSION,
            "observation_partial": (
                bool(item["source_truncated"])
                or item["source_coverage"] != "observed"
            ),
            "observation_error": item["source_error_code"],
        }
        with self._import_lock:
            project, task, created = workflow.import_github_item(
                source, project_id=body.get("projectId"),
                privacy=body.get("privacy"), skill=body.get("skill"),
                checklist=body.get("checklist"),
            )
            with self._connect() as connection:
                connection.execute(
                    "UPDATE work_items SET imported_project_id=?,imported_task_id=?,"
                    "imported_at=COALESCE(imported_at,?) WHERE id=?",
                    (project["id"], task["id"], _now(), item_id),
                )
        if (task["status"] != "draft" or task["worker"] is not None
                or task["model"] is not None or task["estimatedCostMicros"] is not None
                or task["attemptCount"] != 0):
            raise GitHubProblem("imported task violated draft-only invariants", 500,
                                "draft_invariant")
        return {"project": project, "task": task,
                "import": {"id": item_id, "created": created,
                           "sourceType": item["source_type"],
                           "sourceUrl": item["web_url"]}}
