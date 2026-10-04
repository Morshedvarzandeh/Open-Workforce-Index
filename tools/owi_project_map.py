"""Build a private, repository-neutral project map from local Git evidence.

The checked-in ``.owi/map.json`` file is descriptive data.  In particular, it
cannot define commands or actions.  Writable actions belong to trusted host
adapters and the core deliberately emits an empty action list.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import heapq
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import sys
import tempfile
import time
from typing import Any


DECLARATION_SCHEMA = "owi.project-map.v1"
SNAPSHOT_SCHEMA = "owi.system-map.v1"
DECLARATION_ADAPTER_VERSION = "1"
GIT_ADAPTER_VERSION = "1"
MAX_UNTRACKED_BYTES = 16 * 1024 * 1024
MAX_GIT_STDOUT_BYTES = 512_000
MAX_GIT_STDERR_BYTES = 64_000
MAX_GIT_INPUT_BYTES = 256_000
GIT_TIMEOUT_SECONDS = 15
MAX_DISCOVERED_FILES = 4096
MAX_DISCOVERED_PATH_BYTES = 192_000
MAX_DISCOVERY_ENTRIES = 8192
MAX_OBSERVED_FILES = 768
MAX_OBSERVED_PATH_BYTES = 96_000
MAX_FILTER_DRIVERS = 64
COMPONENT_KINDS = frozenset(
    {
        "surface", "api", "router", "agent", "runtime", "trigger",
        "ledger", "human", "module",
    }
)

_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,127}\Z")
_WINDOWS_ABSOLUTE = re.compile(r"[A-Za-z]:[/\\]")
_GLOB_CHARACTER = re.compile(r"[*?\[\]{}]")
_FILTER_DRIVER = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}\Z")


class MapValidationError(ValueError):
    """A project-map declaration does not satisfy the v1 contract."""


class _ObservationLimit(MapValidationError):
    """A bounded repository adapter could not make a complete observation."""

    def __init__(self, reason: str, message: str):
        super().__init__(message)
        self.reason = reason


def _object_without_duplicates(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise MapValidationError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def _reject_json_constant(value):
    raise MapValidationError(f"non-finite JSON number: {value}")


def _strict_object(
    value: Any,
    where: str,
    *,
    required: set[str],
    optional: set[str] = frozenset(),
) -> dict[str, Any]:
    if type(value) is not dict:
        raise MapValidationError(f"{where} must be an object")
    keys = set(value)
    missing = sorted(required - keys)
    unknown = sorted(keys - required - optional)
    if missing:
        raise MapValidationError(f"{where} is missing: {', '.join(missing)}")
    if unknown:
        raise MapValidationError(f"{where} has unknown fields: {', '.join(unknown)}")
    return value


def _text(value: Any, where: str, *, maximum: int = 500, empty: bool = False) -> str:
    if not isinstance(value, str) or (not empty and not value.strip()):
        raise MapValidationError(f"{where} must be a non-empty string")
    if len(value) > maximum or any(ord(character) < 32 for character in value):
        raise MapValidationError(f"{where} is too long or contains control characters")
    return value


def _identifier(value: Any, where: str) -> str:
    if not isinstance(value, str) or not _ID.fullmatch(value):
        raise MapValidationError(f"{where} must be a stable identifier")
    return value


def _ownership_path(value: Any, where: str) -> str:
    """Validate an exact path or a trailing-slash directory prefix."""
    if not isinstance(value, str) or not value or len(value) > 1024:
        raise MapValidationError(f"{where} must be a repository-relative path")
    if any(ord(character) < 32 for character in value):
        raise MapValidationError(f"{where} contains control characters")
    if "\\" in value or _WINDOWS_ABSOLUTE.match(value) or _GLOB_CHARACTER.search(value):
        raise MapValidationError(f"{where} must be a normalized path without globs")
    prefix = value.endswith("/")
    body = value[:-1] if prefix else value
    path = PurePosixPath(body)
    if (
        not body
        or body.startswith(("/", "~"))
        or path.is_absolute()
        or any(part in ("", ".", "..") for part in body.split("/"))
        or path.as_posix() != body
        or body == ".git"
        or body.startswith(".git/")
    ):
        raise MapValidationError(f"{where} must stay inside the repository")
    return body + ("/" if prefix else "")


def validate_declaration(value: Any) -> dict[str, Any]:
    """Validate and normalize one strict ``owi.project-map.v1`` declaration."""
    root = _strict_object(
        value,
        "map",
        required={"schemaVersion", "project", "lanes", "components", "relations"},
    )
    if root["schemaVersion"] != DECLARATION_SCHEMA:
        raise MapValidationError(f"unsupported map schemaVersion: {root['schemaVersion']!r}")

    project_value = _strict_object(
        root["project"], "project", required={"id", "name"}, optional={"description"}
    )
    project = {
        "id": _identifier(project_value["id"], "project.id"),
        "name": _text(project_value["name"], "project.name", maximum=160),
        "description": _text(
            project_value.get("description", ""),
            "project.description",
            maximum=1000,
            empty=True,
        ),
    }

    lanes_value = root["lanes"]
    components_value = root["components"]
    relations_value = root["relations"]
    if type(lanes_value) is not list or not lanes_value:
        raise MapValidationError("lanes must be a non-empty array")
    if type(components_value) is not list:
        raise MapValidationError("components must be an array")
    if type(relations_value) is not list:
        raise MapValidationError("relations must be an array")

    identifiers: dict[str, str] = {project["id"]: "project.id"}

    def unique(identifier: str, where: str) -> None:
        previous = identifiers.get(identifier)
        if previous is not None:
            raise MapValidationError(f"duplicate id {identifier!r} at {where}; first used at {previous}")
        identifiers[identifier] = where

    lanes = []
    lane_ids = set()
    for index, raw in enumerate(lanes_value):
        where = f"lanes[{index}]"
        item = _strict_object(
            raw, where, required={"id", "name"}, optional={"description", "order"}
        )
        identifier = _identifier(item["id"], where + ".id")
        unique(identifier, where + ".id")
        lane_ids.add(identifier)
        order = item.get("order", index)
        if type(order) is not int or order < 0:
            raise MapValidationError(where + ".order must be a non-negative integer")
        lanes.append(
            {
                "id": identifier,
                "name": _text(item["name"], where + ".name", maximum=160),
                "description": _text(
                    item.get("description", ""),
                    where + ".description",
                    maximum=1000,
                    empty=True,
                ),
                "order": order,
            }
        )

    components = []
    component_ids = set()
    for index, raw in enumerate(components_value):
        where = f"components[{index}]"
        item = _strict_object(
            raw,
            where,
            required={"id", "name", "kind", "laneId", "lifecycle", "owns"},
            optional={"description", "facts"},
        )
        identifier = _identifier(item["id"], where + ".id")
        unique(identifier, where + ".id")
        component_ids.add(identifier)
        lane_id = _identifier(item["laneId"], where + ".laneId")
        if lane_id not in lane_ids:
            raise MapValidationError(f"{where}.laneId refers to unknown lane {lane_id!r}")
        lifecycle = item["lifecycle"]
        if lifecycle not in ("implemented", "declared", "planned"):
            raise MapValidationError(
                where + ".lifecycle must be implemented, declared, or planned"
            )
        kind = item["kind"]
        if kind not in COMPONENT_KINDS:
            raise MapValidationError(
                where + ".kind must be surface, api, router, agent, runtime, "
                "trigger, ledger, human, or module"
            )
        facts_value = item.get("facts", [])
        if type(facts_value) is not list or len(facts_value) > 32:
            raise MapValidationError(where + ".facts must be an array of at most 32 strings")
        facts = [
            _text(fact, f"{where}.facts[{fact_index}]", maximum=240)
            for fact_index, fact in enumerate(facts_value)
        ]
        if len(facts) != len(set(facts)):
            raise MapValidationError(where + ".facts contains a duplicate")
        owns_value = item["owns"]
        if type(owns_value) is not list:
            raise MapValidationError(where + ".owns must be an array")
        owns = [_ownership_path(path, f"{where}.owns[{owner_index}]")
                for owner_index, path in enumerate(owns_value)]
        if len(owns) != len(set(owns)):
            raise MapValidationError(where + ".owns contains a duplicate path")
        components.append(
            {
                "id": identifier,
                "name": _text(item["name"], where + ".name", maximum=160),
                "description": _text(
                    item.get("description", ""),
                    where + ".description",
                    maximum=1000,
                    empty=True,
                ),
                "kind": kind,
                "laneId": lane_id,
                "lifecycle": lifecycle,
                "owns": owns,
                "facts": facts,
            }
        )

    relations = []
    for index, raw in enumerate(relations_value):
        where = f"relations[{index}]"
        item = _strict_object(
            raw, where, required={"id", "from", "to", "type"}, optional={"label"}
        )
        identifier = _identifier(item["id"], where + ".id")
        unique(identifier, where + ".id")
        source = _identifier(item["from"], where + ".from")
        target = _identifier(item["to"], where + ".to")
        if source not in component_ids:
            raise MapValidationError(f"{where}.from refers to unknown component {source!r}")
        if target not in component_ids:
            raise MapValidationError(f"{where}.to refers to unknown component {target!r}")
        relation_type = _identifier(item["type"], where + ".type")
        relations.append(
            {
                "id": identifier,
                "from": source,
                "to": target,
                "type": relation_type,
                "label": _text(
                    item.get("label", relation_type),
                    where + ".label",
                    maximum=160,
                ),
            }
        )

    return {
        "schemaVersion": DECLARATION_SCHEMA,
        "project": project,
        "lanes": lanes,
        "components": components,
        "relations": relations,
    }


def _git_environment() -> dict[str, str]:
    environment = os.environ.copy()
    environment.update({
        "LC_ALL": "C",
        "GIT_OPTIONAL_LOCKS": "0",
        "GIT_PAGER": "cat",
        "GIT_NO_LAZY_FETCH": "1",
        "GIT_TERMINAL_PROMPT": "0",
        "GCM_INTERACTIVE": "Never",
    })
    return environment


def _run_git(root: Path, *arguments: str, input_bytes: bytes | None = None) -> subprocess.CompletedProcess:
    """Run a read-only Git command without ever buffering unbounded output.

    Regular temporary files keep a verbose or hostile repository from filling
    Python pipe buffers.  Only the configured prefixes are read back into
    memory.  Callers must treat ``output_limited`` and ``timed_out`` as an
    incomplete observation, never as a partial successful result.
    """
    if input_bytes is not None and len(input_bytes) > MAX_GIT_INPUT_BYTES:
        raise _ObservationLimit("git-input-limit", "Git observation input exceeds the safe limit")
    command = ["git", "--no-pager", "-c", "core.fsmonitor=false", *arguments]
    with tempfile.TemporaryFile() as standard_input, \
            tempfile.TemporaryFile() as standard_output, \
            tempfile.TemporaryFile() as standard_error:
        if input_bytes is not None:
            standard_input.write(input_bytes)
            standard_input.seek(0)
            input_stream = standard_input
        else:
            input_stream = subprocess.DEVNULL
        process = subprocess.Popen(
            command,
            cwd=root,
            stdin=input_stream,
            stdout=standard_output,
            stderr=standard_error,
            env=_git_environment(),
        )
        deadline = time.monotonic() + GIT_TIMEOUT_SECONDS
        output_limited = False
        timed_out = False
        while process.poll() is None:
            if (
                os.fstat(standard_output.fileno()).st_size > MAX_GIT_STDOUT_BYTES
                or os.fstat(standard_error.fileno()).st_size > MAX_GIT_STDERR_BYTES
            ):
                output_limited = True
                process.kill()
                break
            if time.monotonic() >= deadline:
                timed_out = True
                process.kill()
                break
            time.sleep(0.002)
        process.wait()
        stdout_size = os.fstat(standard_output.fileno()).st_size
        stderr_size = os.fstat(standard_error.fileno()).st_size
        output_limited = output_limited or stdout_size > MAX_GIT_STDOUT_BYTES \
            or stderr_size > MAX_GIT_STDERR_BYTES
        standard_output.seek(0)
        standard_error.seek(0)
        stdout = standard_output.read(MAX_GIT_STDOUT_BYTES)
        stderr = standard_error.read(MAX_GIT_STDERR_BYTES)
    result = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
    result.output_limited = output_limited
    result.timed_out = timed_out
    return result


def _git_result_reason(result: subprocess.CompletedProcess, default: str) -> str:
    if getattr(result, "output_limited", False):
        return "git-output-limit"
    if getattr(result, "timed_out", False):
        return "git-timeout"
    return default


def _nul_records(value: bytes):
    """Yield NUL-delimited views without materializing a list of every field."""
    start = 0
    while start < len(value):
        end = value.find(b"\0", start)
        if end < 0:
            yield value[start:]
            return
        if end > start:
            yield value[start:end]
        start = end + 1


def _repository_files(root: Path) -> list[str]:
    result = _run_git(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard")
    if getattr(result, "output_limited", False):
        raise _ObservationLimit(
            "git-output-limit", "Repository discovery exceeds the Git output limit"
        )
    if getattr(result, "timed_out", False):
        raise _ObservationLimit("git-timeout", "Repository discovery timed out")
    if result.returncode == 0:
        names = set()
        path_bytes = 0
        records = 0
        for raw in _nul_records(result.stdout):
            records += 1
            path_bytes += len(raw)
            if records > MAX_DISCOVERED_FILES:
                raise _ObservationLimit(
                    "repository-file-limit", "Repository discovery exceeds the file limit"
                )
            if path_bytes > MAX_DISCOVERED_PATH_BYTES:
                raise _ObservationLimit(
                    "repository-path-limit", "Repository discovery exceeds the path-byte limit"
                )
            path = _decode_git_path(raw)
            if not _safe_observed_path(path):
                raise _ObservationLimit(
                    "unsafe-git-path", "Repository discovery encountered an unsafe path"
                )
            names.add(path)
        return sorted(names)

    names = set()
    path_bytes = 0
    entries_seen = 0
    pending = [root]
    try:
        while pending:
            directory = pending.pop()
            entries = []
            with os.scandir(directory) as iterator:
                for entry in iterator:
                    if entry.name == ".git":
                        continue
                    entries_seen += 1
                    if entries_seen > MAX_DISCOVERY_ENTRIES:
                        raise _ObservationLimit(
                            "repository-entry-limit",
                            "Filesystem discovery exceeds the entry limit",
                        )
                    entries.append((entry.name, entry.is_dir(follow_symlinks=False)))
            for name, is_directory in sorted(entries, reverse=True):
                candidate = directory / name
                if is_directory:
                    pending.append(candidate)
                    continue
                path = candidate.relative_to(root).as_posix()
                encoded_length = len(path.encode("utf-8", errors="surrogateescape"))
                if len(names) + 1 > MAX_DISCOVERED_FILES:
                    raise _ObservationLimit(
                        "repository-file-limit", "Filesystem discovery exceeds the file limit"
                    )
                if path_bytes + encoded_length > MAX_DISCOVERED_PATH_BYTES:
                    raise _ObservationLimit(
                        "repository-path-limit",
                        "Filesystem discovery exceeds the path-byte limit",
                    )
                if not _safe_observed_path(path):
                    raise _ObservationLimit(
                        "unsafe-git-path", "Filesystem discovery encountered an unsafe path"
                    )
                if path not in names:
                    names.add(path)
                    path_bytes += encoded_length
    except OSError as error:
        raise MapValidationError("Cannot complete bounded repository discovery") from error
    return sorted(names)


def _slug(value: str, fallback: str) -> str:
    result = re.sub(r"[^a-z0-9]+", "-", value.casefold()).strip("-")
    if not result:
        result = fallback + "-" + hashlib.sha256(value.encode("utf-8")).hexdigest()[:8]
    return result[:80]


def discover_declaration(project_root: str | os.PathLike[str]) -> dict[str, Any]:
    """Create a deterministic one-lane map from repository top-level paths."""
    root = Path(project_root).resolve()
    files = _repository_files(root)
    groups: dict[str, list[str]] = {}
    root_files = []
    for path in files:
        if "/" in path:
            groups.setdefault(path.split("/", 1)[0], [])
        else:
            root_files.append(path)

    used_ids = {"lane-repository"}
    components = []

    def component_id(name: str) -> str:
        base = "component-" + _slug(name, "path")
        candidate = base
        if candidate in used_ids:
            candidate = base[:105] + "-" + hashlib.sha256(name.encode("utf-8")).hexdigest()[:8]
        suffix = 2
        while candidate in used_ids:
            candidate = f"{base[:118]}-{suffix}"
            suffix += 1
        used_ids.add(candidate)
        return candidate

    if root_files:
        components.append(
            {
                "id": component_id("root"),
                "name": "Root files",
                "description": "Files stored at the repository root.",
                "kind": "module",
                "laneId": "lane-repository",
                "lifecycle": "implemented",
                "owns": sorted(root_files),
            }
        )
    for top_level in sorted(groups):
        components.append(
            {
                "id": component_id(top_level),
                "name": top_level,
                "description": "",
                "kind": "module",
                "laneId": "lane-repository",
                "lifecycle": "implemented",
                "owns": [top_level + "/"],
            }
        )
    if not components:
        components.append(
            {
                "id": component_id("root"),
                "name": "Repository",
                "description": "No repository files were discovered.",
                "kind": "module",
                "laneId": "lane-repository",
                "lifecycle": "declared",
                "owns": [],
            }
        )

    name = root.name or "Project"
    return validate_declaration(
        {
            "schemaVersion": DECLARATION_SCHEMA,
            "project": {"id": "project-" + _slug(name, "project"), "name": name},
            "lanes": [{"id": "lane-repository", "name": "Repository", "order": 0}],
            "components": components,
            "relations": [],
        }
    )


def load_declaration(project_root: str | os.PathLike[str]) -> dict[str, Any]:
    """Load ``.owi/map.json`` or use deterministic discovery when it is absent."""
    root = Path(project_root).resolve()
    path = root / ".owi" / "map.json"
    if not path.exists():
        return discover_declaration(root)
    if path.is_symlink() or not path.is_file():
        raise MapValidationError(".owi/map.json must be a regular file")
    try:
        if path.stat().st_size > 1024 * 1024:
            raise MapValidationError(".owi/map.json is too large")
        value = json.loads(
            path.read_text(encoding="utf-8"),
            object_pairs_hook=_object_without_duplicates,
            parse_constant=_reject_json_constant,
        )
    except (OSError, UnicodeError, json.JSONDecodeError, MapValidationError) as error:
        raise MapValidationError(f"cannot read .owi/map.json: {error}") from error
    return validate_declaration(value)


def _decode_git_path(value: bytes) -> str:
    return value.decode("utf-8", errors="surrogateescape")


def _safe_observed_path(value: str) -> bool:
    if (
        not value
        or "\0" in value
        or "\\" in value
        or _WINDOWS_ABSOLUTE.match(value)
        or any(ord(character) < 32 or 0xD800 <= ord(character) <= 0xDFFF
               for character in value)
    ):
        return False
    path = PurePosixPath(value)
    return (
        not path.is_absolute()
        and path.as_posix() == value
        and all(part not in ("", ".", "..") for part in value.split("/"))
        and value != ".git"
        and not value.startswith(".git/")
    )


def _status_name(code: str) -> str:
    if code == "??":
        return "untracked"
    if "U" in code or code in ("AA", "DD"):
        return "unmerged"
    if "D" in code:
        return "deleted"
    if "A" in code:
        return "added"
    if "R" in code:
        return "renamed"
    if "C" in code:
        return "copied"
    if "T" in code:
        return "type-changed"
    return "modified"


def _untracked_counts(root: Path, path: str) -> tuple[str, int | None, int | None]:
    candidate = root / path
    try:
        details = candidate.lstat()
        if stat.S_ISLNK(details.st_mode) or not stat.S_ISREG(details.st_mode):
            return "unavailable", None, None
        if details.st_size > MAX_UNTRACKED_BYTES:
            return "unavailable", None, None
        resolved = candidate.resolve(strict=True)
        try:
            resolved.relative_to(root.resolve())
        except ValueError:
            return "unavailable", None, None
        additions = 0
        total = 0
        last = b""
        binary_window = bytearray()
        with candidate.open("rb") as stream:
            while True:
                chunk = stream.read(64 * 1024)
                if not chunk:
                    break
                total += len(chunk)
                if total > MAX_UNTRACKED_BYTES:
                    return "unavailable", None, None
                if len(binary_window) < 8000:
                    binary_window.extend(chunk[:8000 - len(binary_window)])
                    if b"\0" in binary_window:
                        return "binary", None, None
                additions += chunk.count(b"\n")
                last = chunk[-1:]
    except OSError:
        return "unavailable", None, None
    additions += 1 if total and last != b"\n" else 0
    return "text", additions, 0


def _numstat(
    root: Path, base: str, safe_filters: list[str]
) -> dict[str, tuple[str, int | None, int | None]] | None:
    result = _run_git(
        root,
        *safe_filters,
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=all",
        "--numstat",
        "-z",
        "--no-renames",
        base,
        "--",
    )
    if getattr(result, "output_limited", False):
        raise _ObservationLimit("git-output-limit", "Git numstat exceeds the output limit")
    if getattr(result, "timed_out", False):
        raise _ObservationLimit("git-timeout", "Git numstat timed out")
    if result.returncode != 0:
        return None
    values = {}
    records = 0
    path_bytes = 0
    for record in _nul_records(result.stdout):
        records += 1
        if records > MAX_OBSERVED_FILES:
            raise _ObservationLimit("git-file-limit", "Git numstat exceeds the file limit")
        fields = record.split(b"\t", 2)
        if len(fields) != 3:
            raise _ObservationLimit("git-output-invalid", "Git numstat is incomplete")
        additions_raw, deletions_raw, path_raw = fields
        path_bytes += len(path_raw)
        if path_bytes > MAX_OBSERVED_PATH_BYTES:
            raise _ObservationLimit("git-path-limit", "Git numstat exceeds the path-byte limit")
        path = _decode_git_path(path_raw)
        if not _safe_observed_path(path):
            raise _ObservationLimit("unsafe-git-path", "Git numstat contains an unsafe path")
        if path in values:
            raise _ObservationLimit("git-output-invalid", "Git numstat repeats a path")
        if additions_raw == b"-" or deletions_raw == b"-":
            values[path] = ("binary", None, None)
            continue
        try:
            values[path] = ("text", int(additions_raw), int(deletions_raw))
        except ValueError:
            values[path] = ("unavailable", None, None)
    return values


def _safe_filter_arguments(root: Path) -> list[str] | None:
    """Disable every clean filter selected by repository attributes.

    A local repository is data, not an execution authority. Git normally runs
    configured clean/process filters while comparing a worktree. We first ask
    Git which driver each repository path selects, then override those drivers
    to a no-op before status or diff is allowed to inspect contents.
    """
    files = _repository_files(root)
    if not files:
        return []
    encoded = b"\0".join(
        path.encode("utf-8", errors="surrogateescape") for path in files
    ) + b"\0"
    result = _run_git(root, "check-attr", "-z", "--stdin", "filter", input_bytes=encoded)
    if getattr(result, "output_limited", False):
        raise _ObservationLimit("git-output-limit", "Git attribute output exceeds the limit")
    if getattr(result, "timed_out", False):
        raise _ObservationLimit("git-timeout", "Git attribute inspection timed out")
    if result.returncode != 0:
        return None
    drivers = set()
    fields = []
    rows = 0
    for field in _nul_records(result.stdout):
        fields.append(field)
        if len(fields) < 3:
            continue
        rows += 1
        _path, attribute, raw_value = fields
        fields = []
        if attribute != b"filter" or raw_value in (b"unspecified", b"unset"):
            continue
        try:
            driver = raw_value.decode("ascii")
        except UnicodeDecodeError:
            return None
        if not _FILTER_DRIVER.fullmatch(driver):
            return None
        drivers.add(driver)
        if len(drivers) > MAX_FILTER_DRIVERS:
            raise _ObservationLimit(
                "git-filter-limit", "Repository selects too many Git filter drivers"
            )
    if fields or rows != len(files):
        return None
    arguments = []
    for driver in sorted(drivers):
        arguments.extend((
            "-c", f"filter.{driver}.clean=",
            "-c", f"filter.{driver}.smudge=",
            "-c", f"filter.{driver}.process=",
            "-c", f"filter.{driver}.required=false",
        ))
    return arguments


def git_changes(project_root: str | os.PathLike[str]) -> dict[str, Any]:
    """Observe working-tree changes and Git-style line counts without hooks."""
    root = Path(project_root).resolve()

    def unavailable(reason: str, revision: str | None = None) -> dict[str, Any]:
        return {
            "comparison": {
                "kind": "git-working-tree",
                "base": "HEAD" if revision else "empty-tree",
                "revision": revision,
                "state": "unavailable",
                "reason": reason,
            },
            "files": [],
        }

    inside = _run_git(root, "rev-parse", "--is-inside-work-tree")
    bounded_reason = _git_result_reason(inside, "not-a-git-working-tree")
    if bounded_reason != "not-a-git-working-tree":
        return unavailable(bounded_reason)
    if inside.returncode != 0 or inside.stdout.strip() != b"true":
        result = unavailable("not-a-git-working-tree")
        result["comparison"]["base"] = "HEAD"
        return result

    revision_result = _run_git(root, "rev-parse", "--verify", "HEAD")
    revision_reason = _git_result_reason(revision_result, "")
    if revision_reason:
        return unavailable(revision_reason)
    if revision_result.returncode == 0:
        revision = revision_result.stdout.decode("ascii", errors="replace").strip()
        base = "HEAD"
    else:
        revision = None
        empty_tree = _run_git(root, "hash-object", "-t", "tree", "--stdin", input_bytes=b"")
        empty_reason = _git_result_reason(empty_tree, "")
        if empty_reason:
            return unavailable(empty_reason)
        if empty_tree.returncode != 0:
            return unavailable("git-comparison-failed")
        base = empty_tree.stdout.decode("ascii", errors="replace").strip()

    try:
        safe_filters = _safe_filter_arguments(root)
    except _ObservationLimit as error:
        return unavailable(error.reason, revision)
    if safe_filters is None:
        return unavailable("unsafe-git-filter-configuration", revision)
    status_result = _run_git(
        root,
        *safe_filters,
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=all",
        "--no-renames",
    )
    status_reason = _git_result_reason(status_result, "")
    if status_reason:
        return unavailable(status_reason, revision)
    if status_result.returncode != 0:
        return unavailable("git-comparison-failed", revision)

    statuses: dict[str, str] = {}
    records = 0
    path_bytes = 0
    for record in _nul_records(status_result.stdout):
        if len(record) < 4 or record[2:3] != b" ":
            return unavailable("git-output-invalid", revision)
        records += 1
        if records > MAX_OBSERVED_FILES:
            return unavailable("git-file-limit", revision)
        code = record[:2].decode("ascii", errors="replace")
        raw_path = record[3:]
        path_bytes += len(raw_path)
        if path_bytes > MAX_OBSERVED_PATH_BYTES:
            return unavailable("git-path-limit", revision)
        path = _decode_git_path(raw_path)
        if not _safe_observed_path(path):
            return unavailable("unsafe-git-path", revision)
        if path in statuses:
            return unavailable("git-output-invalid", revision)
        statuses[path] = code
    del status_result

    try:
        statistics = _numstat(root, base, safe_filters)
    except _ObservationLimit as error:
        return unavailable(error.reason, revision)
    if statistics is None:
        return unavailable("git-comparison-failed", revision)

    files = []
    for path in sorted(statuses):
        code = statuses[path]
        if code == "??":
            kind, additions, deletions = _untracked_counts(root, path)
        else:
            kind, additions, deletions = statistics.get(
                path, ("unavailable", None, None)
            )
        files.append(
            {
                "id": "file-" + hashlib.sha256(
                    path.encode("utf-8", errors="surrogateescape")
                ).hexdigest()[:16],
                "path": path,
                "status": _status_name(code),
                "gitStatus": code,
                "kind": kind,
                "additions": additions,
                "deletions": deletions,
            }
        )

    return {
        "comparison": {
            "kind": "git-working-tree",
            "base": "HEAD" if revision else "empty-tree",
            "revision": revision,
            "state": "available",
            "reason": None,
        },
        "files": files,
    }


def _owns(path: str, source_ref: str) -> bool:
    if source_ref.endswith("/"):
        return path.startswith(source_ref)
    return path == source_ref


def _impact_paths(
    direct: set[str], relations: list[dict[str, Any]]
) -> dict[str, tuple[str, ...]]:
    adjacency: dict[str, set[str]] = {}
    for relation in relations:
        adjacency.setdefault(relation["from"], set()).add(relation["to"])
    best: dict[str, tuple[str, ...]] = {}
    pending: list[tuple[int, tuple[str, ...], str]] = []
    for source in sorted(direct):
        path = (source,)
        best[source] = path
        heapq.heappush(pending, (1, path, source))
    while pending:
        _, path, current = heapq.heappop(pending)
        if best.get(current) != path:
            continue
        for target in sorted(adjacency.get(current, ())):
            if target in path:
                continue
            candidate = path + (target,)
            previous = best.get(target)
            if previous is None or (len(candidate), candidate) < (len(previous), previous):
                best[target] = candidate
                heapq.heappush(pending, (len(candidate), candidate, target))
    return best


def _validate_comparison(value: Any) -> dict[str, Any]:
    item = _strict_object(value, "comparison result", required={"comparison", "files"})
    metadata = _strict_object(
        item["comparison"],
        "comparison",
        required={"kind", "base", "revision", "state", "reason"},
    )
    if metadata["kind"] != "git-working-tree":
        raise MapValidationError("comparison.kind must be git-working-tree")
    if metadata["state"] not in ("available", "unavailable"):
        raise MapValidationError("comparison.state must be available or unavailable")
    if type(item["files"]) is not list:
        raise MapValidationError("comparison files must be an array")
    files = []
    seen = set()
    for index, raw in enumerate(item["files"]):
        where = f"comparison.files[{index}]"
        file_item = _strict_object(
            raw,
            where,
            required={"id", "path", "status", "gitStatus", "kind", "additions", "deletions"},
        )
        path = file_item["path"]
        if not isinstance(path, str) or not _safe_observed_path(path):
            raise MapValidationError(where + ".path must stay inside the repository")
        if path in seen:
            raise MapValidationError(where + ".path is duplicated")
        seen.add(path)
        kind = file_item["kind"]
        if kind not in ("text", "binary", "unavailable"):
            raise MapValidationError(where + ".kind is invalid")
        additions = file_item["additions"]
        deletions = file_item["deletions"]
        if kind == "text":
            if type(additions) is not int or additions < 0 or type(deletions) is not int or deletions < 0:
                raise MapValidationError(where + " text counts must be non-negative integers")
        elif additions is not None or deletions is not None:
            raise MapValidationError(where + " binary/unavailable counts must be null")
        files.append(copy.deepcopy(file_item))
    return {"comparison": copy.deepcopy(metadata), "files": sorted(files, key=lambda row: row["path"])}


def compute_snapshot_id(snapshot: dict[str, Any]) -> str:
    """Return the content ID; callers may reseal after adding trusted evidence/actions."""
    value = copy.deepcopy(snapshot)
    metadata = value.get("snapshot")
    if type(metadata) is not dict:
        raise MapValidationError("snapshot metadata is missing")
    metadata["id"] = None
    encoded = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode("utf-8")
    return "sha256:" + hashlib.sha256(encoded).hexdigest()


def seal_snapshot(snapshot: dict[str, Any]) -> dict[str, Any]:
    """Copy a snapshot and set its content-derived immutable ID."""
    result = copy.deepcopy(snapshot)
    result["snapshot"]["id"] = compute_snapshot_id(result)
    return result


def build_snapshot(
    project_root: str | os.PathLike[str],
    declaration: dict[str, Any] | None = None,
    comparison: dict[str, Any] | None = None,
    *,
    generated_at: str | None = None,
) -> dict[str, Any]:
    """Combine a declaration and Git observation into ``owi.system-map.v1``."""
    root = Path(project_root).resolve()
    declaration = load_declaration(root) if declaration is None else validate_declaration(declaration)
    observed = _validate_comparison(git_changes(root) if comparison is None else comparison)
    comparison_available = observed["comparison"]["state"] == "available"

    files = []
    direct_files: dict[str, list[dict[str, Any]]] = {
        component["id"]: [] for component in declaration["components"]
    }
    for observed_file in observed["files"]:
        owners = sorted(
            component["id"]
            for component in declaration["components"]
            if any(_owns(observed_file["path"], source_ref) for source_ref in component["owns"])
        )
        item = {**observed_file, "componentIds": owners}
        files.append(item)
        for owner in owners:
            direct_files[owner].append(item)

    direct = {identifier for identifier, rows in direct_files.items() if rows}
    paths = _impact_paths(direct, declaration["relations"]) if comparison_available else {}
    nodes = []
    for component in declaration["components"]:
        identifier = component["id"]
        owned_changes = direct_files[identifier]
        if not comparison_available:
            impact_state = "unavailable"
            impact_paths = []
        elif owned_changes:
            impact_state = "changed"
            impact_paths = []
        elif identifier in paths:
            impact_state = "affected"
            impact_paths = [list(paths[identifier])]
        else:
            impact_state = "unchanged"
            impact_paths = []

        all_text = bool(owned_changes) and all(item["kind"] == "text" for item in owned_changes)
        additions = sum(item["additions"] for item in owned_changes) if all_text else None
        deletions = sum(item["deletions"] for item in owned_changes) if all_text else None
        nodes.append(
            {
                "id": identifier,
                "name": component["name"],
                "description": component["description"],
                "kind": component["kind"],
                "facts": copy.deepcopy(component["facts"]),
                "laneId": component["laneId"],
                "lifecycle": component["lifecycle"],
                "sourceRefs": [
                    {
                        "path": source_ref[:-1] if source_ref.endswith("/") else source_ref,
                        "kind": "prefix" if source_ref.endswith("/") else "exact",
                    }
                    for source_ref in component["owns"]
                ],
                "impact": {
                    "state": impact_state,
                    "direct": bool(owned_changes) if comparison_available else False,
                    "paths": impact_paths,
                },
                "churn": {
                    "additions": additions,
                    "deletions": deletions,
                    "changedFiles": len(owned_changes),
                    "textFiles": sum(item["kind"] == "text" for item in owned_changes),
                    "binaryFiles": sum(item["kind"] == "binary" for item in owned_changes),
                    "unavailableFiles": sum(
                        item["kind"] == "unavailable" for item in owned_changes
                    ),
                },
            }
        )

    unmapped = [item["path"] for item in files if not item["componentIds"]]
    evidence_state = "measured" if comparison_available else "unavailable"
    evidence = [
        {
            "id": "evidence-git-working-tree",
            "type": "change-comparison",
            "claimState": evidence_state,
            "basis": "git-status-and-numstat",
            "subjectId": declaration["project"]["id"],
            "value": {
                "changedFiles": len(files),
                "unmappedFiles": len(unmapped),
                "textFiles": sum(item["kind"] == "text" for item in files),
                "binaryFiles": sum(item["kind"] == "binary" for item in files),
                "unavailableFiles": sum(item["kind"] == "unavailable" for item in files),
            },
        }
    ]
    raw_snapshot = {
        "schemaVersion": SNAPSHOT_SCHEMA,
        "snapshot": {
            "id": None,
            "projectId": declaration["project"]["id"],
            "projectName": declaration["project"]["name"],
            "projectDescription": declaration["project"]["description"],
            "generatedAt": generated_at,
            "comparison": observed["comparison"],
            "adapters": [
                {"id": "project-declaration", "version": DECLARATION_ADAPTER_VERSION},
                {"id": "git-working-tree", "version": GIT_ADAPTER_VERSION},
            ],
        },
        "lanes": copy.deepcopy(declaration["lanes"]),
        "nodes": nodes,
        "edges": copy.deepcopy(declaration["relations"]),
        "files": files,
        "unmappedFiles": unmapped,
        "evidence": evidence,
        "actions": [],
    }
    return seal_snapshot(raw_snapshot)


# Explicit aliases make the adapter vocabulary convenient without adding a
# second implementation or broadening the declaration contract.
discover_project = discover_declaration
collect_git_changes = git_changes


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Build a private OWI project-map snapshot")
    parser.add_argument("project", nargs="?", default=".", help="selected local project")
    parser.add_argument("--generated-at", help="trusted ISO-8601 observation time")
    parser.add_argument("--compact", action="store_true", help="write compact JSON")
    arguments = parser.parse_args(argv)
    try:
        snapshot = build_snapshot(arguments.project, generated_at=arguments.generated_at)
    except (MapValidationError, OSError, subprocess.SubprocessError) as error:
        print(f"project map: {error}", file=sys.stderr)
        return 2
    if arguments.compact:
        print(json.dumps(snapshot, sort_keys=True, separators=(",", ":"), ensure_ascii=True))
    else:
        print(json.dumps(snapshot, sort_keys=True, indent=2, ensure_ascii=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
