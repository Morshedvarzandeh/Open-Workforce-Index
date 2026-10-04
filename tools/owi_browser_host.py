"""Length-bounded Native Messaging host for the generic OWI project map."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import struct
import sys
import tempfile

from owi_browser_map import MapServiceError, ProjectMapService
import owi_platform


MAX_MESSAGE_BYTES = 1_000_000
REQUEST_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:@-]{0,127}\Z")
PROJECT_CONFIG_SCHEMA = "owi.browser-projects.v1"
PROJECT_CONFIG_NAME = "browser-projects.json"
NATIVE_HOST_NAME = "org.openworkforceindex.project_map"
FIREFOX_EXTENSION_ID = "project-map@openworkforceindex.org"
CHROMIUM_EXTENSION_ID = re.compile(r"[a-p]{32}\Z")
CHROMIUM_ORIGIN = re.compile(r"chrome-extension://[a-p]{32}/?\Z")


class _DuplicateField(ValueError):
    pass


def _object_without_duplicates(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise _DuplicateField("Duplicate JSON field")
        result[key] = value
    return result


def _reject_constant(_value):
    raise ValueError("Non-finite JSON number")


def decode_json(body: bytes):
    try:
        text = body.decode("utf-8")
        return json.loads(
            text, object_pairs_hook=_object_without_duplicates,
            parse_constant=_reject_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError,
            RecursionError) as error:
        raise MapServiceError("invalid_json", "Expected one valid JSON object") from error


def _validate_id(value) -> str:
    if not isinstance(value, str) or not REQUEST_ID.fullmatch(value):
        raise MapServiceError("invalid_request", "Invalid request ID")
    return value


def _exact_fields(request: dict, expected: set[str]) -> None:
    if set(request) != expected:
        raise MapServiceError("invalid_request", "Unexpected or missing request fields")


def dispatch(service: ProjectMapService, request) -> dict:
    """Validate one untrusted request and return a versioned response envelope."""
    request_id = request.get("id") if isinstance(request, dict) else None
    try:
        if not isinstance(request, dict):
            raise MapServiceError("invalid_request", "Expected a JSON object")
        request_id = _validate_id(request.get("id"))
        message_type = request.get("type")
        if type(message_type) is not str:
            raise MapServiceError("invalid_request", "Invalid request type")

        if message_type == "list_projects":
            _exact_fields(request, {"id", "type"})
            result = service.list_projects()
        elif message_type == "get_snapshot":
            _exact_fields(request, {"id", "type", "projectId"})
            if type(request["projectId"]) is not str:
                raise MapServiceError("invalid_request", "Invalid project ID")
            result = service.get_snapshot(request["projectId"])
        elif message_type == "invoke_action":
            _exact_fields(request, {
                "id", "type", "projectId", "actionId", "expectedSnapshotId",
                "expectedState", "idempotencyKey", "confirmed",
            })
            for key in ("projectId", "actionId", "expectedSnapshotId",
                        "expectedState", "idempotencyKey"):
                if type(request[key]) is not str:
                    raise MapServiceError("invalid_request", "Invalid action request")
            if type(request["confirmed"]) is not bool:
                raise MapServiceError("invalid_request", "Invalid confirmation")
            result = service.invoke_action(
                project_id=request["projectId"],
                action_id=request["actionId"],
                expected_snapshot_id=request["expectedSnapshotId"],
                expected_state=request["expectedState"],
                idempotency_key=request["idempotencyKey"],
                confirmed=request["confirmed"],
            )
        else:
            raise MapServiceError("unknown_request", "Unknown request type")
        return {
            "schemaVersion": "owi.native-project-map.v1",
            "id": request_id,
            "ok": True,
            "result": result,
        }
    except MapServiceError as error:
        return error_response(request_id, error.code, str(error))
    except Exception:
        # Native responses must not disclose roots, database details, or other
        # local exception text to the browser client.
        return error_response(request_id, "internal_error", "The local host failed")


def error_response(request_id, code: str, message: str) -> dict:
    safe_id = request_id if isinstance(request_id, str) \
        and REQUEST_ID.fullmatch(request_id) else None
    return {
        "schemaVersion": "owi.native-project-map.v1",
        "id": safe_id,
        "ok": False,
        "error": {"code": code, "message": message},
    }


def _encode(payload: dict) -> bytes:
    return json.dumps(
        payload, ensure_ascii=False, allow_nan=False, sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def write_message(stream, payload: dict, max_bytes: int = MAX_MESSAGE_BYTES) -> None:
    body = _encode(payload)
    if len(body) > max_bytes:
        body = _encode(error_response(
            payload.get("id"), "response_too_large",
            "The project snapshot exceeds the native-message limit",
        ))
    if len(body) > max_bytes:
        raise ValueError("Native-message limit is too small for an error response")
    stream.write(struct.pack("=I", len(body)))
    stream.write(body)
    stream.flush()


def _read_exact(stream, length: int) -> bytes:
    chunks = []
    remaining = length
    while remaining:
        chunk = stream.read(remaining)
        if not chunk:
            break
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def serve_native_messages(service: ProjectMapService, input_stream, output_stream,
                          *, max_bytes: int = MAX_MESSAGE_BYTES) -> int:
    """Serve messages until clean EOF; framing failures receive one last error."""
    while True:
        header = _read_exact(input_stream, 4)
        if not header:
            return 0
        if len(header) != 4:
            write_message(
                output_stream,
                error_response(None, "invalid_frame", "Truncated native-message header"),
                max_bytes,
            )
            return 1
        length = struct.unpack("=I", header)[0]
        if length == 0:
            write_message(
                output_stream,
                error_response(None, "invalid_frame", "Empty native message"),
                max_bytes,
            )
            continue
        if length > max_bytes:
            write_message(
                output_stream,
                error_response(None, "message_too_large", "Native message exceeds the size limit"),
                max_bytes,
            )
            return 1
        body = _read_exact(input_stream, length)
        if len(body) != length:
            write_message(
                output_stream,
                error_response(None, "invalid_frame", "Truncated native message"),
                max_bytes,
            )
            return 1
        try:
            request = decode_json(body)
        except MapServiceError as error:
            response = error_response(None, error.code, str(error))
        else:
            response = dispatch(service, request)
        write_message(output_stream, response, max_bytes)


def _project_argument(value: str) -> tuple[str, Path]:
    project_id, separator, root = value.partition("=")
    if not separator or not project_id or not root:
        raise argparse.ArgumentTypeError("use PROJECT_ID=/trusted/project/root")
    return project_id, Path(root)


def project_config_path(home: Path) -> Path:
    return Path(home) / PROJECT_CONFIG_NAME


def _validated_projects(projects) -> dict[str, Path]:
    if not isinstance(projects, list) or not 1 <= len(projects) <= 64:
        raise ValueError("Configure between one and 64 projects")
    result = {}
    for item in projects:
        if not isinstance(item, dict) or set(item) != {"id", "root"}:
            raise ValueError("Every configured project needs exactly id and root")
        project_id = item["id"]
        root = item["root"]
        if not isinstance(project_id, str) or not REQUEST_ID.fullmatch(project_id):
            raise ValueError("Project IDs must be opaque letters, numbers, '_' or '-'")
        if not isinstance(root, str) or not Path(root).is_absolute():
            raise ValueError("Configured project roots must be absolute paths")
        path = Path(root).resolve()
        if not path.is_dir() or path.parent == path:
            raise ValueError("Every configured project root must be a directory")
        if project_id in result:
            raise ValueError("Duplicate project ID")
        result[project_id] = path
    return result


def load_project_config(home: Path, *, required: bool = True) -> dict[str, Path]:
    """Load the owner-managed registry used by browser-launched hosts."""
    path = project_config_path(home)
    try:
        body = path.read_bytes()
    except FileNotFoundError:
        if required:
            raise ValueError(
                f"No browser projects are configured; run {Path(sys.argv[0]).name} "
                "--configure-project PROJECT_ID=/absolute/project/root"
            ) from None
        return {}
    if len(body) > 256 * 1024:
        raise ValueError("The browser project registry is too large")
    try:
        value = decode_json(body)
    except MapServiceError as error:
        raise ValueError("The browser project registry is not valid JSON") from error
    if not isinstance(value, dict) or set(value) != {"schemaVersion", "projects"}:
        raise ValueError("The browser project registry has unexpected fields")
    if value["schemaVersion"] != PROJECT_CONFIG_SCHEMA:
        raise ValueError("The browser project registry has an unsupported version")
    return _validated_projects(value["projects"])


def write_project_config(home: Path, projects: dict[str, Path]) -> Path:
    """Atomically write the local registry without exposing it to the client."""
    normalized = _validated_projects([
        {"id": project_id, "root": str(Path(root).resolve())}
        for project_id, root in sorted(projects.items())
    ])
    home = Path(home)
    home.mkdir(parents=True, exist_ok=True, mode=0o700)
    payload = {
        "schemaVersion": PROJECT_CONFIG_SCHEMA,
        "projects": [
            {"id": project_id, "root": str(root)}
            for project_id, root in sorted(normalized.items())
        ],
    }
    temporary_name = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", dir=home, prefix=".browser-projects-",
            suffix=".tmp", delete=False,
        ) as output:
            temporary_name = output.name
            json.dump(payload, output, ensure_ascii=True, sort_keys=True, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        Path(temporary_name).chmod(0o600)
        os.replace(temporary_name, project_config_path(home))
    finally:
        if temporary_name is not None:
            try:
                Path(temporary_name).unlink()
            except FileNotFoundError:
                pass
    return project_config_path(home)


def native_host_manifest(executable: Path, browser: str,
                         extension_id: str | None = None) -> dict:
    """Build one least-privilege browser registration manifest."""
    executable = Path(executable).resolve()
    if not executable.is_file() or not executable.is_absolute():
        raise ValueError("The native host executable must be an absolute file")
    manifest = {
        "name": NATIVE_HOST_NAME,
        "description": "Local Open Workforce Index project-map host",
        "path": str(executable),
        "type": "stdio",
    }
    if browser == "firefox":
        addon_id = extension_id or FIREFOX_EXTENSION_ID
        if addon_id != FIREFOX_EXTENSION_ID:
            raise ValueError("The Firefox extension ID does not match this package")
        manifest["allowed_extensions"] = [addon_id]
    elif browser in {"chrome", "chromium", "edge"}:
        if not isinstance(extension_id, str) \
                or not CHROMIUM_EXTENSION_ID.fullmatch(extension_id):
            raise ValueError("Chromium installation requires its 32-character extension ID")
        manifest["allowed_origins"] = [f"chrome-extension://{extension_id}/"]
    else:
        raise ValueError("Unsupported browser")
    return manifest


def native_manifest_directory(browser: str, *, user_home: Path | None = None,
                              platform_name: str | None = None) -> Path:
    """Return the documented per-user manifest directory on Unix platforms."""
    user_home = Path.home() if user_home is None else Path(user_home)
    platform_name = sys.platform if platform_name is None else platform_name
    if platform_name == "linux":
        directories = {
            "chrome": user_home / ".config/google-chrome/NativeMessagingHosts",
            "chromium": user_home / ".config/chromium/NativeMessagingHosts",
            "edge": user_home / ".config/microsoft-edge/NativeMessagingHosts",
            "firefox": user_home / ".mozilla/native-messaging-hosts",
        }
    elif platform_name == "darwin":
        support = user_home / "Library/Application Support"
        directories = {
            "chrome": support / "Google/Chrome/NativeMessagingHosts",
            "chromium": support / "Chromium/NativeMessagingHosts",
            "edge": support / "Microsoft Edge/NativeMessagingHosts",
            "firefox": support / "Mozilla/NativeMessagingHosts",
        }
    else:
        raise ValueError("Automatic native-host registration currently supports Linux and macOS")
    try:
        return directories[browser]
    except KeyError as error:
        raise ValueError("Unsupported browser") from error


def install_native_manifest(executable: Path, browser: str,
                            extension_id: str | None = None, *,
                            directory: Path | None = None) -> Path:
    manifest = native_host_manifest(executable, browser, extension_id)
    target_directory = native_manifest_directory(browser) if directory is None \
        else Path(directory)
    target_directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    target = target_directory / f"{NATIVE_HOST_NAME}.json"
    temporary_name = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", dir=target_directory,
            prefix=f".{NATIVE_HOST_NAME}-", suffix=".tmp", delete=False,
        ) as output:
            temporary_name = output.name
            json.dump(manifest, output, ensure_ascii=True, sort_keys=True, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        Path(temporary_name).chmod(0o600)
        os.replace(temporary_name, target)
    finally:
        if temporary_name is not None:
            try:
                Path(temporary_name).unlink()
            except FileNotFoundError:
                pass
    return target


def _native_launch_arguments(argv: list[str]) -> bool:
    """Recognize, then ignore, the browser-supplied launch metadata."""
    if not argv:
        return True
    if len(argv) in (1, 2) and CHROMIUM_ORIGIN.fullmatch(argv[0]):
        return len(argv) == 1 or argv[1].startswith("--parent-window=")
    return len(argv) == 2 and Path(argv[0]).is_absolute() \
        and argv[1] == FIREFOX_EXTENSION_ID


def main(argv=None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--project", action="append", type=_project_argument,
        metavar="PROJECT_ID=ROOT",
        help="temporary trusted mapping for a manual host session",
    )
    parser.add_argument("--home", type=Path, default=owi_platform.default_home())
    management = parser.add_mutually_exclusive_group()
    management.add_argument(
        "--configure-project", type=_project_argument, metavar="PROJECT_ID=ROOT",
        help="add or update a project in the owner-only browser registry",
    )
    management.add_argument(
        "--remove-project", metavar="PROJECT_ID",
        help="remove a project from the owner-only browser registry",
    )
    management.add_argument(
        "--list-configured-projects", action="store_true",
        help="print the trusted browser registry and exit",
    )
    management.add_argument(
        "--install-native-host", choices=("chrome", "chromium", "edge", "firefox"),
        metavar="BROWSER", help="register this host for one browser on Linux or macOS",
    )
    parser.add_argument(
        "--extension-id",
        help="32-character unpacked/installed extension ID for Chromium browsers",
    )
    parser.add_argument(
        "--max-message-bytes", type=int, default=MAX_MESSAGE_BYTES,
        help="maximum request or response body (1024..1000000 bytes)",
    )
    browser_launch = _native_launch_arguments(argv)
    arguments = parser.parse_args([] if browser_launch else argv)
    if not 1024 <= arguments.max_message_bytes <= MAX_MESSAGE_BYTES:
        parser.error("--max-message-bytes must be between 1024 and 1000000")
    if arguments.extension_id and not arguments.install_native_host:
        parser.error("--extension-id requires --install-native-host")
    try:
        if arguments.configure_project:
            projects = load_project_config(arguments.home, required=False)
            project_id, root = arguments.configure_project
            projects[project_id] = root.resolve()
            path = write_project_config(arguments.home, projects)
            print(json.dumps({"configured": project_id, "registry": str(path)}))
            return 0
        if arguments.remove_project:
            projects = load_project_config(arguments.home)
            if arguments.remove_project not in projects:
                raise ValueError("Unknown configured project ID")
            del projects[arguments.remove_project]
            if not projects:
                project_config_path(arguments.home).unlink()
            else:
                write_project_config(arguments.home, projects)
            print(json.dumps({"removed": arguments.remove_project}))
            return 0
        if arguments.list_configured_projects:
            projects = load_project_config(arguments.home)
            print(json.dumps({
                "schemaVersion": PROJECT_CONFIG_SCHEMA,
                "projects": [{"id": key, "root": str(value)}
                             for key, value in sorted(projects.items())],
            }, indent=2))
            return 0
        if arguments.install_native_host:
            # Browsers execute exactly this no-argument entry point later.
            load_project_config(arguments.home)
            executable = Path(sys.argv[0]).resolve()
            path = install_native_manifest(
                executable, arguments.install_native_host, arguments.extension_id,
            )
            print(json.dumps({"browser": arguments.install_native_host,
                              "manifest": str(path)}))
            return 0
        if arguments.project:
            projects = {}
            for project_id, root in arguments.project:
                if project_id in projects:
                    parser.error("duplicate project ID")
                projects[project_id] = root
        else:
            projects = load_project_config(arguments.home)
        service = ProjectMapService(projects, arguments.home)
    except ValueError as error:
        parser.error(str(error))
    return serve_native_messages(
        service, sys.stdin.buffer, sys.stdout.buffer,
        max_bytes=arguments.max_message_bytes,
    )


if __name__ == "__main__":
    raise SystemExit(main())
