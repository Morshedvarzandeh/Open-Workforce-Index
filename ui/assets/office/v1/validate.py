#!/usr/bin/env python3
"""Validate the OWI office v1 art-pack contract without third-party packages."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import sys
from datetime import datetime
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent
ID_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
SEMVER_RE = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)
SHA256_RE = re.compile(r"^[a-f0-9]{64}$")
KINDS = {
    "background",
    "character",
    "prop",
    "effect",
    "ui-decoration",
    "skin-overlay",
}
LICENSE_STATES = {"approved", "pending", "restricted", "unknown"}
MEDIA_EXTENSIONS = {
    "image/png": ".png",
    "image/webp": ".webp",
    "image/avif": ".avif",
    "image/svg+xml": ".svg",
}
KIND_ROOTS = {
    "background": {"backgrounds"},
    "character": {"characters"},
    "prop": {"props"},
    "effect": {"skins"},
    "ui-decoration": {"skins"},
    "skin-overlay": {"skins"},
}


class Report:
    def __init__(self) -> None:
        self.errors: list[str] = []
        self.warnings: list[str] = []

    def error(self, message: str) -> None:
        self.errors.append(message)

    def warn(self, message: str) -> None:
        self.warnings.append(message)


def read_json(path: Path, report: Report) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        report.error(f"{path.name}: cannot read valid JSON: {exc}")
        return {}
    if not isinstance(value, dict):
        report.error(f"{path.name}: root must be an object")
        return {}
    return value


def check_id(value: Any, where: str, report: Report) -> bool:
    if not isinstance(value, str) or not ID_RE.fullmatch(value):
        report.error(f"{where}: must be a lowercase kebab-case ID")
        return False
    return True


def check_semver(value: Any, where: str, report: Report) -> bool:
    if not isinstance(value, str) or not SEMVER_RE.fullmatch(value):
        report.error(f"{where}: must be a SemVer value")
        return False
    return True


def safe_path(value: Any, where: str, report: Report) -> Path | None:
    if not isinstance(value, str) or not value:
        report.error(f"{where}: must be a non-empty relative path")
        return None
    candidate = Path(value)
    if candidate.is_absolute() or "\\" in value or ".." in candidate.parts:
        report.error(f"{where}: path must stay inside the asset pack")
        return None
    resolved = (ROOT / candidate).resolve()
    if ROOT not in resolved.parents and resolved != ROOT:
        report.error(f"{where}: path escapes the asset pack")
        return None
    return resolved


def parse_time(value: Any, where: str, report: Report) -> None:
    if value is None:
        return
    if not isinstance(value, str):
        report.error(f"{where}: must be an ISO-8601 timestamp or null")
        return
    try:
        datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        report.error(f"{where}: invalid ISO-8601 timestamp")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def png_metadata(path: Path) -> tuple[int, int, bool] | None:
    data = path.read_bytes()
    if len(data) < 26 or data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        return None
    width, height = struct.unpack(">II", data[16:24])
    color_type = data[25]
    has_alpha = color_type in {4, 6} or b"tRNS" in data
    return width, height, has_alpha


def webp_metadata(path: Path) -> tuple[int, int, bool | None] | None:
    data = path.read_bytes()
    if len(data) < 30 or data[:4] != b"RIFF" or data[8:12] != b"WEBP":
        return None
    chunk = data[12:16]
    if chunk == b"VP8X" and len(data) >= 30:
        has_alpha = bool(data[20] & 0x10)
        width = 1 + int.from_bytes(data[24:27], "little")
        height = 1 + int.from_bytes(data[27:30], "little")
        return width, height, has_alpha
    if chunk == b"VP8 " and len(data) >= 30 and data[23:26] == b"\x9d\x01\x2a":
        width = int.from_bytes(data[26:28], "little") & 0x3FFF
        height = int.from_bytes(data[28:30], "little") & 0x3FFF
        return width, height, False
    if chunk == b"VP8L" and len(data) >= 25 and data[20] == 0x2F:
        bits = int.from_bytes(data[21:25], "little")
        width = 1 + (bits & 0x3FFF)
        height = 1 + ((bits >> 14) & 0x3FFF)
        has_alpha = bool((bits >> 28) & 1)
        return width, height, has_alpha
    return None


def svg_dimensions(path: Path) -> tuple[int, int] | None:
    head = path.read_text(encoding="utf-8", errors="replace")[:16384]
    svg = re.search(r"<svg\b([^>]*)>", head, flags=re.IGNORECASE | re.DOTALL)
    if not svg:
        return None
    attrs = svg.group(1)

    def number(name: str) -> float | None:
        match = re.search(
            rf"\b{name}\s*=\s*['\"]\s*([0-9]+(?:\.[0-9]+)?)",
            attrs,
            flags=re.IGNORECASE,
        )
        return float(match.group(1)) if match else None

    width = number("width")
    height = number("height")
    if width is None or height is None:
        view_box = re.search(
            r"\bviewBox\s*=\s*['\"]\s*[-0-9.]+\s+[-0-9.]+\s+"
            r"([0-9.]+)\s+([0-9.]+)",
            attrs,
            flags=re.IGNORECASE,
        )
        if view_box:
            width, height = float(view_box.group(1)), float(view_box.group(2))
    if width is None or height is None or not width.is_integer() or not height.is_integer():
        return None
    return int(width), int(height)


def check_dimensions(
    path: Path,
    media_type: str,
    width: Any,
    height: Any,
    has_alpha: Any,
    where: str,
    report: Report,
) -> None:
    actual: tuple[int, int] | None = None
    alpha: bool | None = None
    try:
        if media_type == "image/png":
            png = png_metadata(path)
            if png:
                actual = png[:2]
                alpha = png[2]
        elif media_type == "image/webp":
            webp = webp_metadata(path)
            if webp:
                actual = webp[:2]
                alpha = webp[2]
        elif media_type == "image/svg+xml":
            actual = svg_dimensions(path)
    except OSError as exc:
        report.error(f"{where}: cannot inspect image metadata: {exc}")
        return

    if media_type == "image/avif":
        report.warn(f"{where}: AVIF dimensions are declared but not independently decoded")
        return
    if actual is None:
        report.error(f"{where}: file header does not match {media_type} or dimensions are unreadable")
        return
    if actual != (width, height):
        report.error(
            f"{where}: declared dimensions {width}x{height} do not match "
            f"file {actual[0]}x{actual[1]}"
        )
    if alpha is not None and isinstance(has_alpha, bool) and alpha != has_alpha:
        report.error(f"{where}: declared has_alpha={has_alpha} does not match PNG data")


def validate_skins(manifest: dict[str, Any], report: Report) -> set[str]:
    skins = manifest.get("skins")
    if not isinstance(skins, list) or not skins:
        report.error("manifest.skins: must be a non-empty array")
        return set()
    skin_ids: set[str] = set()
    parents: dict[str, str | None] = {}
    for index, skin in enumerate(skins):
        where = f"manifest.skins[{index}]"
        if not isinstance(skin, dict):
            report.error(f"{where}: must be an object")
            continue
        skin_id = skin.get("id")
        if check_id(skin_id, f"{where}.id", report):
            if skin_id in skin_ids:
                report.error(f"{where}.id: duplicate skin ID {skin_id!r}")
            skin_ids.add(skin_id)
            parent = skin.get("inherits")
            if parent is not None and not check_id(parent, f"{where}.inherits", report):
                parent = None
            parents[skin_id] = parent
        for field in ("title", "description"):
            if not isinstance(skin.get(field), str) or not skin[field]:
                report.error(f"{where}.{field}: must be a non-empty string")
        if skin.get("status") not in {"base", "experimental", "available", "retired"}:
            report.error(f"{where}.status: invalid skin status")
    for child, parent in parents.items():
        if parent is not None and parent not in skin_ids:
            report.error(f"skin {child!r}: inherits unknown skin {parent!r}")
        visited: set[str] = set()
        cursor: str | None = child
        while cursor is not None and cursor in parents:
            if cursor in visited:
                report.error(f"skin {child!r}: inheritance cycle detected")
                break
            visited.add(cursor)
            cursor = parents[cursor]
    default_skin = manifest.get("default_skin")
    if default_skin not in skin_ids:
        report.error("manifest.default_skin: must reference a declared skin")
    return skin_ids


def validate_sources(
    sources: dict[str, Any],
    manifest: dict[str, Any],
    release: bool,
    report: Report,
) -> dict[str, dict[str, Any]]:
    if sources.get("schema_version") != 1:
        report.error("SOURCES.schema_version: expected 1")
    if sources.get("pack_id") != manifest.get("pack_id"):
        report.error("SOURCES.pack_id: must match manifest.pack_id")
    records = sources.get("records")
    if not isinstance(records, list):
        report.error("SOURCES.records: must be an array")
        return {}

    by_id: dict[str, dict[str, Any]] = {}
    for index, record in enumerate(records):
        where = f"SOURCES.records[{index}]"
        if not isinstance(record, dict):
            report.error(f"{where}: must be an object")
            continue
        record_id = record.get("id")
        if check_id(record_id, f"{where}.id", report):
            if record_id in by_id:
                report.error(f"{where}.id: duplicate source record {record_id!r}")
            else:
                by_id[record_id] = record
        if record.get("kind") not in {
            "project-authored",
            "generative-tool",
            "commissioned",
            "third-party",
            "derivative",
            "unknown",
        }:
            report.error(f"{where}.kind: invalid source kind")
        if not isinstance(record.get("description"), str) or not record["description"]:
            report.error(f"{where}.description: must be a non-empty string")
        parse_time(record.get("created_at"), f"{where}.created_at", report)
        outputs = record.get("output_asset_ids")
        if not isinstance(outputs, list) or not outputs:
            report.error(f"{where}.output_asset_ids: must be a non-empty array")
        else:
            for output_index, asset_id in enumerate(outputs):
                check_id(asset_id, f"{where}.output_asset_ids[{output_index}]", report)

        prompt = record.get("prompt")
        if prompt is not None:
            if not isinstance(prompt, dict):
                report.error(f"{where}.prompt: must be an object or null")
            else:
                prompt_text = prompt.get("text")
                prompt_file = prompt.get("file")
                prompt_digest = prompt.get("sha256")
                if not prompt_text and not prompt_file:
                    report.error(f"{where}.prompt: text or file is required")
                material: bytes | None = None
                if isinstance(prompt_text, str) and prompt_text:
                    material = prompt_text.encode("utf-8")
                elif prompt_file:
                    prompt_path = safe_path(prompt_file, f"{where}.prompt.file", report)
                    if prompt_path:
                        try:
                            prompt_path.relative_to(ROOT / "sources")
                        except ValueError:
                            report.error(f"{where}.prompt.file: prompt files belong under sources/")
                        if not prompt_path.is_file():
                            report.error(f"{where}.prompt.file: file does not exist")
                        else:
                            material = prompt_path.read_bytes()
                if prompt_digest is not None and not (
                    isinstance(prompt_digest, str) and SHA256_RE.fullmatch(prompt_digest)
                ):
                    report.error(f"{where}.prompt.sha256: must be a lowercase SHA-256 or null")
                if material is not None and prompt_digest != hashlib.sha256(material).hexdigest():
                    report.error(f"{where}.prompt.sha256: digest does not match prompt material")

        artifacts = record.get("artifacts")
        if not isinstance(artifacts, list) or not artifacts:
            report.error(f"{where}.artifacts: must be a non-empty array")
        else:
            artifact_files: set[str] = set()
            for artifact_index, artifact in enumerate(artifacts):
                a_where = f"{where}.artifacts[{artifact_index}]"
                if not isinstance(artifact, dict):
                    report.error(f"{a_where}: must be an object")
                    continue
                if artifact.get("role") not in {
                    "generated-master",
                    "processed-master",
                    "editable-source",
                    "intermediate",
                    "runtime-output",
                }:
                    report.error(f"{a_where}.role: invalid artifact role")
                artifact_file = artifact.get("file")
                artifact_path = safe_path(artifact_file, f"{a_where}.file", report)
                if isinstance(artifact_file, str):
                    if artifact_file in artifact_files:
                        report.error(f"{a_where}.file: duplicate artifact file")
                    artifact_files.add(artifact_file)
                media_type = artifact.get("media_type")
                if media_type not in MEDIA_EXTENSIONS:
                    report.error(f"{a_where}.media_type: unsupported image type")
                elif (
                    isinstance(artifact_file, str)
                    and Path(artifact_file).suffix.lower() != MEDIA_EXTENSIONS[media_type]
                ):
                    report.error(f"{a_where}: media_type does not match file extension")
                width = artifact.get("width_px")
                height = artifact.get("height_px")
                has_alpha = artifact.get("has_alpha")
                byte_size = artifact.get("byte_size")
                digest = artifact.get("sha256")
                if not isinstance(width, int) or width < 1:
                    report.error(f"{a_where}.width_px: expected a positive integer")
                if not isinstance(height, int) or height < 1:
                    report.error(f"{a_where}.height_px: expected a positive integer")
                if not isinstance(has_alpha, bool):
                    report.error(f"{a_where}.has_alpha: expected a boolean")
                if not isinstance(byte_size, int) or byte_size < 1:
                    report.error(f"{a_where}.byte_size: expected a positive integer")
                if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
                    report.error(f"{a_where}.sha256: expected a lowercase SHA-256")
                if artifact_path is None:
                    continue
                if not artifact_path.is_file():
                    report.error(f"{a_where}.file: file does not exist")
                    continue
                if isinstance(byte_size, int) and artifact_path.stat().st_size != byte_size:
                    report.error(f"{a_where}.byte_size: does not match file")
                if isinstance(digest, str) and SHA256_RE.fullmatch(digest):
                    if sha256(artifact_path) != digest:
                        report.error(f"{a_where}.sha256: digest does not match file")
                if (
                    media_type in MEDIA_EXTENSIONS
                    and isinstance(width, int)
                    and isinstance(height, int)
                    and isinstance(has_alpha, bool)
                ):
                    check_dimensions(
                        artifact_path,
                        media_type,
                        width,
                        height,
                        has_alpha,
                        a_where,
                        report,
                    )

        rights = record.get("rights")
        if not isinstance(rights, dict):
            report.error(f"{where}.rights: must be an object")
            continue
        rights_status = rights.get("status")
        if rights_status not in LICENSE_STATES:
            report.error(f"{where}.rights.status: invalid rights status")
        elif rights_status != "approved":
            message = f"{where}.rights.status: {rights_status!r} is not release-ready"
            report.error(message) if release else report.warn(message)
        parse_time(rights.get("reviewed_at"), f"{where}.rights.reviewed_at", report)
        if release:
            for field in ("owner", "license_expression", "reviewed_by", "reviewed_at"):
                if not rights.get(field):
                    report.error(f"{where}.rights.{field}: required for release")
    return by_id


def validate_placement(value: Any, where: str, report: Report) -> None:
    if not isinstance(value, dict):
        report.error(f"{where}: must be an object")
        return
    for field in ("scene", "slot"):
        check_id(value.get(field), f"{where}.{field}", report)
    if value.get("layer") not in {
        "background",
        "room",
        "behind-character",
        "character",
        "front-of-character",
        "overlay",
    }:
        report.error(f"{where}.layer: invalid layer")
    if value.get("anchor") not in {
        "top-left",
        "top-center",
        "top-right",
        "center-left",
        "center",
        "center-right",
        "bottom-left",
        "bottom-center",
        "bottom-right",
    }:
        report.error(f"{where}.anchor: invalid anchor")
    focal = value.get("focal_point")
    if not (
        isinstance(focal, list)
        and len(focal) == 2
        and all(isinstance(number, (int, float)) and 0 <= number <= 1 for number in focal)
    ):
        report.error(f"{where}.focal_point: expected two normalized numbers")
    if value.get("fit") not in {"contain", "cover", "fill", "none"}:
        report.error(f"{where}.fit: invalid fit")
    z_index = value.get("z_index")
    if not isinstance(z_index, int) or not -1000 <= z_index <= 1000:
        report.error(f"{where}.z_index: expected an integer from -1000 to 1000")


def validate_assets(
    manifest: dict[str, Any],
    skins: set[str],
    sources: dict[str, dict[str, Any]],
    release: bool,
    report: Report,
) -> int:
    assets = manifest.get("assets")
    if not isinstance(assets, list):
        report.error("manifest.assets: must be an array")
        return 0
    if not assets:
        message = "manifest.assets: pack contains no published artwork yet"
        report.error(message) if release else report.warn(message)

    asset_ids: set[str] = set()
    file_paths: set[str] = set()
    rendition_count = 0
    for index, asset in enumerate(assets):
        where = f"manifest.assets[{index}]"
        if not isinstance(asset, dict):
            report.error(f"{where}: must be an object")
            continue
        asset_id = asset.get("id")
        if check_id(asset_id, f"{where}.id", report):
            if asset_id in asset_ids:
                report.error(f"{where}.id: duplicate asset ID {asset_id!r}")
            asset_ids.add(asset_id)
        check_semver(asset.get("revision"), f"{where}.revision", report)
        kind = asset.get("kind")
        if kind not in KINDS:
            report.error(f"{where}.kind: invalid asset kind")
        if not isinstance(asset.get("title"), str) or not asset["title"]:
            report.error(f"{where}.title: must be a non-empty string")
        decorative = asset.get("decorative")
        alt_text = asset.get("alt_text")
        if not isinstance(decorative, bool) or not isinstance(alt_text, str):
            report.error(f"{where}: decorative must be boolean and alt_text must be string")
        elif decorative and alt_text:
            report.error(f"{where}.alt_text: decorative assets must use empty alt text")
        elif not decorative and not alt_text.strip():
            report.error(f"{where}.alt_text: meaningful assets require literal alt text")
        if asset.get("brand_status") != "original-unbranded":
            report.error(f"{where}.brand_status: base pack accepts only original-unbranded art")
        embedded_text = asset.get("embedded_text")
        if not isinstance(embedded_text, list) or not all(
            isinstance(text, str) and text for text in embedded_text
        ):
            report.error(f"{where}.embedded_text: must be an array of non-empty strings")

        asset_skins = asset.get("skin_ids")
        if not isinstance(asset_skins, list) or not asset_skins:
            report.error(f"{where}.skin_ids: must be a non-empty array")
        else:
            unknown_skins = set(asset_skins) - skins
            if unknown_skins:
                report.error(f"{where}.skin_ids: unknown skins {sorted(unknown_skins)}")
        validate_placement(asset.get("placement"), f"{where}.placement", report)

        character = asset.get("character")
        if kind == "character":
            if not isinstance(character, dict):
                report.error(f"{where}.character: required for character assets")
            else:
                check_id(character.get("character_id"), f"{where}.character.character_id", report)
                check_id(character.get("office_role"), f"{where}.character.office_role", report)
                for field in ("pose", "expression", "state"):
                    check_id(character.get(field), f"{where}.character.{field}", report)
                character_type = character.get("character_type")
                age_class = character.get("age_class")
                if character_type not in {"human", "robot", "other-fictional"}:
                    report.error(f"{where}.character.character_type: invalid character type")
                elif character_type == "human" and age_class != "adult":
                    report.error(f"{where}.character.age_class: human characters must be adults")
                elif character_type != "human" and age_class != "not-applicable":
                    report.error(
                        f"{where}.character.age_class: non-human characters use not-applicable"
                    )
                if character.get("look_direction") not in {
                    "left",
                    "right",
                    "forward",
                    "away",
                    "not-applicable",
                }:
                    report.error(f"{where}.character.look_direction: invalid direction")
        elif character is not None:
            report.error(f"{where}.character: only character assets may declare character metadata")

        source_ids = asset.get("source_record_ids")
        if not isinstance(source_ids, list) or not source_ids:
            report.error(f"{where}.source_record_ids: at least one provenance record is required")
        else:
            for source_id in source_ids:
                record = sources.get(source_id)
                if record is None:
                    report.error(f"{where}.source_record_ids: unknown source {source_id!r}")
                elif asset_id not in record.get("output_asset_ids", []):
                    report.error(
                        f"{where}.source_record_ids: source {source_id!r} does not list "
                        f"asset {asset_id!r} as output"
                    )

        license_data = asset.get("license")
        if not isinstance(license_data, dict):
            report.error(f"{where}.license: must be an object")
        else:
            license_status = license_data.get("status")
            if license_status not in LICENSE_STATES:
                report.error(f"{where}.license.status: invalid license status")
            elif license_status != "approved":
                message = f"{where}.license.status: {license_status!r} is not release-ready"
                report.error(message) if release else report.warn(message)
            expression = license_data.get("expression")
            if license_status == "approved" and not expression:
                report.error(f"{where}.license.expression: approved assets need a license")
            evidence_ids = license_data.get("evidence_source_ids")
            if not isinstance(evidence_ids, list):
                report.error(f"{where}.license.evidence_source_ids: must be an array")
            else:
                for evidence_id in evidence_ids:
                    if evidence_id not in sources:
                        report.error(
                            f"{where}.license.evidence_source_ids: unknown source {evidence_id!r}"
                        )
                if release and not evidence_ids:
                    report.error(f"{where}.license.evidence_source_ids: required for release")

        renditions = asset.get("renditions")
        if not isinstance(renditions, list) or not renditions:
            report.error(f"{where}.renditions: at least one rendition is required")
            continue
        rendition_ids: set[str] = set()
        runtime_renditions = 0
        for rendition_index, rendition in enumerate(renditions):
            rendition_count += 1
            r_where = f"{where}.renditions[{rendition_index}]"
            if not isinstance(rendition, dict):
                report.error(f"{r_where}: must be an object")
                continue
            rendition_id = rendition.get("id")
            if check_id(rendition_id, f"{r_where}.id", report):
                if rendition_id in rendition_ids:
                    report.error(f"{r_where}.id: duplicate rendition ID {rendition_id!r}")
                rendition_ids.add(rendition_id)
            purpose = rendition.get("purpose")
            if purpose not in {"master", "runtime"}:
                report.error(f"{r_where}.purpose: expected master or runtime")
            elif purpose == "runtime":
                runtime_renditions += 1
            file_value = rendition.get("file")
            image_path = safe_path(file_value, f"{r_where}.file", report)
            if isinstance(file_value, str):
                if file_value in file_paths:
                    report.error(f"{r_where}.file: a file may belong to only one rendition")
                file_paths.add(file_value)
                if kind in KIND_ROOTS and Path(file_value).parts:
                    if Path(file_value).parts[0] not in KIND_ROOTS[kind]:
                        report.error(
                            f"{r_where}.file: {kind} files belong under "
                            f"{sorted(KIND_ROOTS[kind])}"
                        )
                provenance_files = {
                    (artifact.get("file"), artifact.get("role"))
                    for source_id in source_ids
                    if source_id in sources
                    for artifact in sources[source_id].get("artifacts", [])
                    if isinstance(artifact, dict)
                } if isinstance(source_ids, list) else set()
                allowed_roles = (
                    {"runtime-output"}
                    if purpose == "runtime"
                    else {
                        "generated-master",
                        "processed-master",
                        "editable-source",
                        "intermediate",
                    }
                )
                if not any(
                    artifact_file == file_value and artifact_role in allowed_roles
                    for artifact_file, artifact_role in provenance_files
                ):
                    report.error(
                        f"{r_where}.file: no referenced source record lists this {purpose} output"
                    )
            media_type = rendition.get("media_type")
            if media_type not in MEDIA_EXTENSIONS:
                report.error(f"{r_where}.media_type: unsupported runtime image type")
            elif isinstance(file_value, str) and Path(file_value).suffix.lower() != MEDIA_EXTENSIONS[media_type]:
                report.error(f"{r_where}: media_type does not match file extension")
            width = rendition.get("width_px")
            height = rendition.get("height_px")
            if not isinstance(width, int) or width < 1:
                report.error(f"{r_where}.width_px: expected a positive integer")
            if not isinstance(height, int) or height < 1:
                report.error(f"{r_where}.height_px: expected a positive integer")
            ratio = rendition.get("pixel_ratio")
            if not isinstance(ratio, (int, float)) or ratio <= 0:
                report.error(f"{r_where}.pixel_ratio: expected a positive number")
            has_alpha = rendition.get("has_alpha")
            if not isinstance(has_alpha, bool):
                report.error(f"{r_where}.has_alpha: expected a boolean")
            expected_bytes = rendition.get("byte_size")
            if not isinstance(expected_bytes, int) or expected_bytes < 1:
                report.error(f"{r_where}.byte_size: expected a positive integer")
            expected_digest = rendition.get("sha256")
            if not isinstance(expected_digest, str) or not SHA256_RE.fullmatch(expected_digest):
                report.error(f"{r_where}.sha256: expected a lowercase SHA-256")

            if image_path is None:
                continue
            if not image_path.is_file():
                report.error(f"{r_where}.file: file does not exist")
                continue
            if isinstance(expected_bytes, int) and image_path.stat().st_size != expected_bytes:
                report.error(
                    f"{r_where}.byte_size: declared {expected_bytes}, "
                    f"file is {image_path.stat().st_size}"
                )
            if isinstance(expected_digest, str) and SHA256_RE.fullmatch(expected_digest):
                actual_digest = sha256(image_path)
                if actual_digest != expected_digest:
                    report.error(f"{r_where}.sha256: digest does not match file")
            if (
                media_type in MEDIA_EXTENSIONS
                and isinstance(width, int)
                and isinstance(height, int)
                and isinstance(has_alpha, bool)
            ):
                check_dimensions(
                    image_path,
                    media_type,
                    width,
                    height,
                    has_alpha,
                    r_where,
                    report,
                )

        if runtime_renditions == 0:
            report.error(f"{where}.renditions: at least one runtime rendition is required")

    for source_id, record in sources.items():
        for output_id in record.get("output_asset_ids", []):
            if output_id not in asset_ids:
                report.error(
                    f"SOURCES record {source_id!r}: output asset {output_id!r} is not in manifest"
                )
    return rendition_count


def validate(release: bool) -> tuple[Report, int, int]:
    report = Report()
    manifest = read_json(ROOT / "manifest.json", report)
    sources = read_json(ROOT / "SOURCES.json", report)
    if not manifest or not sources:
        return report, 0, 0

    if manifest.get("schema_version") != 1:
        report.error("manifest.schema_version: expected 1")
    if manifest.get("pack_id") != "org.open-workforce-index.office":
        report.error("manifest.pack_id: unexpected stable pack ID")
    check_semver(manifest.get("pack_version"), "manifest.pack_version", report)
    if manifest.get("status") not in {
        "development",
        "release-candidate",
        "released",
        "retired",
    }:
        report.error("manifest.status: invalid pack status")
    if release:
        if manifest.get("status") not in {"release-candidate", "released"}:
            report.error("manifest.status: release validation requires release-candidate or released")
        if isinstance(manifest.get("pack_version"), str) and "-" in manifest["pack_version"]:
            report.error("manifest.pack_version: release validation rejects prerelease versions")

    for field in ("license_file", "sources_file"):
        referenced = safe_path(manifest.get(field), f"manifest.{field}", report)
        if referenced is not None and not referenced.is_file():
            report.error(f"manifest.{field}: referenced file does not exist")

    design_space = manifest.get("design_space")
    if not isinstance(design_space, dict):
        report.error("manifest.design_space: must be an object")
    else:
        for field in ("width_px", "height_px"):
            if not isinstance(design_space.get(field), int) or design_space[field] < 1:
                report.error(f"manifest.design_space.{field}: expected a positive integer")
        safe_area = design_space.get("safe_area")
        if not isinstance(safe_area, dict):
            report.error("manifest.design_space.safe_area: must be an object")
        else:
            for edge in ("top", "right", "bottom", "left"):
                value = safe_area.get(edge)
                if not isinstance(value, (int, float)) or not 0 <= value <= 1:
                    report.error(
                        f"manifest.design_space.safe_area.{edge}: expected a normalized number"
                    )

    skins = validate_skins(manifest, report)
    source_by_id = validate_sources(sources, manifest, release, report)
    rendition_count = validate_assets(manifest, skins, source_by_id, release, report)
    asset_count = len(manifest.get("assets", [])) if isinstance(manifest.get("assets"), list) else 0
    return report, asset_count, rendition_count


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--release",
        action="store_true",
        help="apply publication rules in addition to the development contract",
    )
    args = parser.parse_args()
    report, asset_count, rendition_count = validate(args.release)
    for warning in report.warnings:
        print(f"warning: {warning}", file=sys.stderr)
    for error in report.errors:
        print(f"error: {error}", file=sys.stderr)
    if report.errors:
        print(
            f"office asset pack invalid: {len(report.errors)} error(s), "
            f"{len(report.warnings)} warning(s)",
            file=sys.stderr,
        )
        return 1
    mode = "release" if args.release else "development"
    print(
        f"office asset pack valid ({mode}): {asset_count} asset(s), "
        f"{rendition_count} rendition(s), {len(report.warnings)} warning(s)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
