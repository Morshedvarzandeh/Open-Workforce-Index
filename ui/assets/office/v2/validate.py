#!/usr/bin/env python3
"""Validate the dependency-light OWI Office v2 graphics, scenes, and tour."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent
REPO = ROOT.parents[3]
SCENES = REPO / "ui" / "scenes"
TOURS = REPO / "ui" / "tours"
ID_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
SEMVER_RE = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$"
)
SHA256_RE = re.compile(r"^[a-f0-9]{64}$")
PROFILE_IDS = {"phone-portrait", "phone-landscape", "tablet", "desktop"}
TOUR_ANCHORS = [
    "company-hq",
    "github-projects",
    "manager-incoming",
    "task-board",
    "staffing",
    "run-review",
    "results-evidence",
]
LAYER_BANDS = {
    "background": (0, 99),
    "environment": (100, 199),
    "rear-props": (200, 299),
    "characters": (300, 399),
    "front-props": (400, 499),
    "effects": (500, 599),
    "hud": (800, 899),
    "shade": (900, 909),
    "spotlight": (910, 919),
    "coach": (920, 949),
    "toast": (990, 999),
}
MEDIA_SUFFIX = {
    "image/png": ".png",
    "image/webp": ".webp",
    "image/avif": ".avif",
    "image/svg+xml": ".svg",
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
        report.error(f"{path.relative_to(REPO)}: cannot read valid JSON: {exc}")
        return {}
    if not isinstance(value, dict):
        report.error(f"{path.relative_to(REPO)}: root must be an object")
        return {}
    return value


def check_id(value: Any, where: str, report: Report) -> bool:
    if not isinstance(value, str) or not ID_RE.fullmatch(value):
        report.error(f"{where}: expected a lowercase kebab-case ID")
        return False
    return True


def check_semver(value: Any, where: str, report: Report) -> bool:
    if not isinstance(value, str) or not SEMVER_RE.fullmatch(value):
        report.error(f"{where}: expected SemVer")
        return False
    return True


def safe_path(root: Path, value: Any, where: str, report: Report) -> Path | None:
    if not isinstance(value, str) or not value:
        report.error(f"{where}: expected a non-empty relative path")
        return None
    candidate = Path(value)
    if candidate.is_absolute() or "\\" in value or ".." in candidate.parts:
        report.error(f"{where}: path must stay under {root.relative_to(REPO)}")
        return None
    resolved = (root / candidate).resolve()
    if root.resolve() not in resolved.parents and resolved != root.resolve():
        report.error(f"{where}: path escapes {root.relative_to(REPO)}")
        return None
    if (root / candidate).is_symlink():
        report.error(f"{where}: symlinks are not accepted as graphics evidence")
        return None
    return resolved


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
    return width, height, color_type in {4, 6} or b"tRNS" in data


def webp_metadata(path: Path) -> tuple[int, int, bool] | None:
    data = path.read_bytes()
    if len(data) < 25 or data[:4] != b"RIFF" or data[8:12] != b"WEBP":
        return None
    chunk = data[12:16]
    if chunk == b"VP8X" and len(data) >= 30:
        return (
            1 + int.from_bytes(data[24:27], "little"),
            1 + int.from_bytes(data[27:30], "little"),
            bool(data[20] & 0x10),
        )
    if chunk == b"VP8 " and len(data) >= 30 and data[23:26] == b"\x9d\x01\x2a":
        return (
            int.from_bytes(data[26:28], "little") & 0x3FFF,
            int.from_bytes(data[28:30], "little") & 0x3FFF,
            False,
        )
    if chunk == b"VP8L" and len(data) >= 25 and data[20] == 0x2F:
        bits = int.from_bytes(data[21:25], "little")
        return 1 + (bits & 0x3FFF), 1 + ((bits >> 14) & 0x3FFF), bool((bits >> 28) & 1)
    return None


def check_file(
    path: Path | None,
    expected_bytes: Any,
    expected_sha: Any,
    where: str,
    report: Report,
) -> None:
    if path is None:
        return
    if not path.is_file():
        report.error(f"{where}: file does not exist")
        return
    if not isinstance(expected_bytes, int) or expected_bytes < 1:
        report.error(f"{where}.byte_size: expected a positive integer")
    elif path.stat().st_size != expected_bytes:
        report.error(f"{where}.byte_size: declared {expected_bytes}, file is {path.stat().st_size}")
    if not isinstance(expected_sha, str) or not SHA256_RE.fullmatch(expected_sha):
        report.error(f"{where}.sha256: expected a lowercase SHA-256")
    elif sha256(path) != expected_sha:
        report.error(f"{where}.sha256: checksum mismatch")


def check_image(
    path: Path | None,
    media_type: Any,
    width: Any,
    height: Any,
    alpha: Any,
    where: str,
    report: Report,
) -> None:
    if path is None or not path.is_file():
        return
    suffix = MEDIA_SUFFIX.get(media_type)
    if suffix is None:
        report.error(f"{where}.media_type: unsupported type")
        return
    if path.suffix.lower() != suffix:
        report.error(f"{where}: media type does not match file extension")
    if media_type == "image/png":
        actual = png_metadata(path)
    elif media_type == "image/webp":
        actual = webp_metadata(path)
    else:
        report.warn(f"{where}: dimensions for {media_type} are not independently decoded")
        return
    if actual is None:
        report.error(f"{where}: unreadable {media_type} header")
        return
    if actual[:2] != (width, height):
        report.error(f"{where}: declared {width}x{height}, file is {actual[0]}x{actual[1]}")
    if isinstance(alpha, bool) and actual[2] != alpha:
        report.error(f"{where}: declared has_alpha={alpha}, file has_alpha={actual[2]}")


def validate_sources(
    sources: dict[str, Any],
    asset_ids: set[str],
    release: bool,
    report: Report,
) -> set[str]:
    if sources.get("schema_version") != 2:
        report.error("SOURCES.schema_version: expected 2")
    records = sources.get("records")
    if not isinstance(records, list) or not records:
        report.error("SOURCES.records: expected a non-empty array")
        return set()
    ids: set[str] = set()
    outputs: set[str] = set()
    for index, record in enumerate(records):
        where = f"SOURCES.records[{index}]"
        if not isinstance(record, dict):
            report.error(f"{where}: expected an object")
            continue
        source_id = record.get("id")
        if check_id(source_id, f"{where}.id", report):
            if source_id in ids:
                report.error(f"{where}.id: duplicate {source_id!r}")
            ids.add(source_id)
        output_ids = record.get("output_asset_ids")
        if not isinstance(output_ids, list) or not output_ids:
            report.error(f"{where}.output_asset_ids: expected at least one asset")
        else:
            for asset_id in output_ids:
                if asset_id not in asset_ids:
                    report.error(f"{where}.output_asset_ids: unknown asset {asset_id!r}")
                outputs.add(asset_id)
        prompt = record.get("prompt")
        if isinstance(prompt, dict):
            prompt_path = safe_path(ROOT, prompt.get("file"), f"{where}.prompt.file", report)
            check_file(prompt_path, prompt_path.stat().st_size if prompt_path and prompt_path.is_file() else None,
                       prompt.get("sha256"), f"{where}.prompt", report)
        elif prompt is not None:
            report.error(f"{where}.prompt: expected an object or null")
        for input_index, source_file in enumerate(record.get("input_files", [])):
            f_where = f"{where}.input_files[{input_index}]"
            if not isinstance(source_file, dict):
                report.error(f"{f_where}: expected an object")
                continue
            source_path = safe_path(REPO, source_file.get("file"), f"{f_where}.file", report)
            check_file(source_path,
                       source_path.stat().st_size if source_path and source_path.is_file() else None,
                       source_file.get("sha256"), f_where, report)
        for artifact_index, artifact in enumerate(record.get("artifacts", [])):
            a_where = f"{where}.artifacts[{artifact_index}]"
            if not isinstance(artifact, dict):
                report.error(f"{a_where}: expected an object")
                continue
            artifact_path = safe_path(ROOT, artifact.get("file"), f"{a_where}.file", report)
            check_file(artifact_path, artifact.get("byte_size"), artifact.get("sha256"), a_where, report)
        rights = record.get("rights")
        status = rights.get("status") if isinstance(rights, dict) else None
        if status not in {"approved", "pending", "restricted", "unknown"}:
            report.error(f"{where}.rights.status: invalid status")
        elif status != "approved":
            message = f"{where}.rights.status: {status!r} is not release-ready"
            report.error(message) if release else report.warn(message)
        elif not rights.get("owner") or not rights.get("license_expression") \
                or not rights.get("reviewed_by") or not rights.get("reviewed_at"):
            report.error(f"{where}.rights: approved evidence is incomplete")
    missing = asset_ids - outputs
    if missing:
        report.error(f"SOURCES.records: assets without provenance output: {sorted(missing)}")
    return ids


def validate_manifest(
    manifest: dict[str, Any],
    sources: dict[str, Any],
    release: bool,
    report: Report,
) -> set[str]:
    if manifest.get("schema_version") != 2:
        report.error("manifest.schema_version: expected 2")
    if manifest.get("pack_id") != "org.open-workforce-index.office":
        report.error("manifest.pack_id: unexpected pack ID")
    check_semver(manifest.get("pack_version"), "manifest.pack_version", report)
    status = manifest.get("status")
    if status not in {"development", "release-candidate", "released", "retired"}:
        report.error("manifest.status: invalid status")
    if release and status not in {"release-candidate", "released"}:
        report.error("manifest.status: release validation requires release-candidate or released")
    if release and isinstance(manifest.get("pack_version"), str) \
            and "-" in manifest["pack_version"]:
        report.error("manifest.pack_version: release mode rejects prerelease versions")
    for field in ("license_file", "sources_file"):
        path = safe_path(ROOT, manifest.get(field), f"manifest.{field}", report)
        if path is not None and not path.is_file():
            report.error(f"manifest.{field}: referenced file does not exist")
    skins = manifest.get("skins")
    skin_ids: set[str] = set()
    if not isinstance(skins, list) or not skins:
        report.error("manifest.skins: expected a non-empty array")
    else:
        for index, skin in enumerate(skins):
            where = f"manifest.skins[{index}]"
            if not isinstance(skin, dict):
                report.error(f"{where}: expected an object")
                continue
            skin_id = skin.get("id")
            if check_id(skin_id, f"{where}.id", report):
                if skin_id in skin_ids:
                    report.error(f"{where}.id: duplicate {skin_id!r}")
                skin_ids.add(skin_id)
    if manifest.get("default_skin") not in skin_ids:
        report.error("manifest.default_skin: not registered")
    assets = manifest.get("assets")
    if not isinstance(assets, list) or not assets:
        report.error("manifest.assets: expected a non-empty array")
        return set()
    asset_ids: set[str] = set()
    critical_bytes = 0
    runtime_bytes = 0
    for index, asset in enumerate(assets):
        where = f"manifest.assets[{index}]"
        if not isinstance(asset, dict):
            report.error(f"{where}: expected an object")
            continue
        asset_id = asset.get("id")
        if check_id(asset_id, f"{where}.id", report):
            if asset_id in asset_ids:
                report.error(f"{where}.id: duplicate {asset_id!r}")
            asset_ids.add(asset_id)
        check_semver(asset.get("revision"), f"{where}.revision", report)
        if asset.get("brand_status") != "original-unbranded":
            report.error(f"{where}.brand_status: base pack requires original-unbranded")
        decorative = asset.get("decorative")
        alt_text = asset.get("alt_text")
        if not isinstance(decorative, bool) or not isinstance(alt_text, str):
            report.error(f"{where}: decorative and alt_text have invalid types")
        elif decorative and alt_text:
            report.error(f"{where}.alt_text: decorative assets use empty alt text")
        elif not decorative and not alt_text.strip():
            report.error(f"{where}.alt_text: meaningful assets need literal alt text")
        unknown_skins = set(asset.get("skin_ids", [])) - skin_ids
        if unknown_skins:
            report.error(f"{where}.skin_ids: unknown skins {sorted(unknown_skins)}")
        states = asset.get("states")
        if not isinstance(states, list) or not states:
            report.error(f"{where}.states: expected at least one state")
        if asset.get("kind") == "character" and not isinstance(asset.get("character"), dict):
            report.error(f"{where}.character: character metadata is required")
        if asset.get("kind") != "character" and "character" in asset:
            report.error(f"{where}.character: only character assets may declare it")
        rights = asset.get("rights")
        rights_status = rights.get("status") if isinstance(rights, dict) else None
        if rights_status not in {"approved", "pending", "restricted", "unknown"}:
            report.error(f"{where}.rights.status: invalid status")
        elif rights_status != "approved":
            message = f"{where}.rights.status: {rights_status!r} is not release-ready"
            report.error(message) if release else report.warn(message)
        elif not rights.get("expression") or not rights.get("evidence_source_ids"):
            report.error(f"{where}.rights: approved asset needs license and evidence")
        renditions = asset.get("renditions")
        runtime_count = 0
        if not isinstance(renditions, list) or not renditions:
            report.error(f"{where}.renditions: expected at least one rendition")
            continue
        for r_index, rendition in enumerate(renditions):
            r_where = f"{where}.renditions[{r_index}]"
            if not isinstance(rendition, dict):
                report.error(f"{r_where}: expected an object")
                continue
            path = safe_path(ROOT, rendition.get("file"), f"{r_where}.file", report)
            check_file(path, rendition.get("byte_size"), rendition.get("sha256"), r_where, report)
            check_image(path, rendition.get("media_type"), rendition.get("width_px"),
                        rendition.get("height_px"), rendition.get("has_alpha"), r_where, report)
            if rendition.get("purpose") == "runtime":
                runtime_count += 1
                byte_size = rendition.get("byte_size")
                if isinstance(byte_size, int):
                    runtime_bytes += byte_size
                    if rendition.get("loading") == "critical":
                        critical_bytes += byte_size
            elif rendition.get("purpose") != "master":
                report.error(f"{r_where}.purpose: expected master or runtime")
        if not runtime_count:
            report.error(f"{where}.renditions: at least one runtime rendition is required")
    budget = manifest.get("initial_payload_budget_bytes")
    if not isinstance(budget, int) or budget < 1:
        report.error("manifest.initial_payload_budget_bytes: expected a positive integer")
    elif critical_bytes > budget:
        report.error(f"critical runtime art is {critical_bytes} bytes; budget is {budget}")
    if runtime_bytes > 2 * 1024 * 1024:
        report.error(f"runtime art is {runtime_bytes} bytes; v2 pack ceiling is 2 MiB")
    source_ids = validate_sources(sources, asset_ids, release, report)
    for index, asset in enumerate(assets):
        for source_id in asset.get("source_record_ids", []):
            if source_id not in source_ids:
                report.error(f"manifest.assets[{index}].source_record_ids: unknown {source_id!r}")
        rights = asset.get("rights", {})
        for source_id in rights.get("evidence_source_ids", []):
            if source_id not in source_ids:
                report.error(f"manifest.assets[{index}].rights: unknown evidence {source_id!r}")
    return asset_ids


def check_bounds(value: Any, where: str, report: Report) -> None:
    if not isinstance(value, list) or len(value) != 4 \
            or not all(isinstance(x, (int, float)) and 0 <= x <= 1 for x in value):
        report.error(f"{where}: expected normalized [x, y, width, height]")
        return
    if value[0] + value[2] > 1.000001 or value[1] + value[3] > 1.000001:
        report.error(f"{where}: bounds leave the normalized scene")


def validate_scenes(asset_ids: set[str], report: Report) -> set[str]:
    scene_ids: set[str] = set()
    hotspot_anchors: set[str] = set()
    files = sorted(SCENES.glob("*.v1.json"))
    if not files:
        report.error("ui/scenes: no versioned scenes found")
        return hotspot_anchors
    for path in files:
        scene = read_json(path, report)
        where = str(path.relative_to(REPO))
        if scene.get("schema_version") != 1:
            report.error(f"{where}.schema_version: expected 1")
        scene_id = scene.get("scene_id")
        if check_id(scene_id, f"{where}.scene_id", report):
            if scene_id in scene_ids:
                report.error(f"{where}.scene_id: duplicate {scene_id!r}")
            scene_ids.add(scene_id)
        check_semver(scene.get("version"), f"{where}.version", report)
        pack = scene.get("asset_pack")
        if not isinstance(pack, dict) or pack.get("pack_id") != "org.open-workforce-index.office":
            report.error(f"{where}.asset_pack: unexpected pack")
        elif not check_semver(pack.get("minimum_version"), f"{where}.asset_pack.minimum_version", report):
            pass
        profiles = scene.get("layout_profiles")
        profile_ids = {p.get("id") for p in profiles if isinstance(p, dict)} \
            if isinstance(profiles, list) else set()
        if profile_ids != PROFILE_IDS:
            report.error(f"{where}.layout_profiles: expected exactly {sorted(PROFILE_IDS)}")
        layers = scene.get("layers")
        layer_map: dict[str, tuple[int, int]] = {}
        if not isinstance(layers, list):
            report.error(f"{where}.layers: expected an array")
        else:
            for layer in layers:
                if not isinstance(layer, dict):
                    continue
                layer_id = layer.get("id")
                pair = (layer.get("z_min"), layer.get("z_max"))
                if layer_id in layer_map:
                    report.error(f"{where}.layers: duplicate {layer_id!r}")
                layer_map[layer_id] = pair
            if layer_map != LAYER_BANDS:
                report.error(f"{where}.layers: must use the fixed v1 z-index bands")
        instances = scene.get("instances")
        instance_ids: set[str] = set()
        if not isinstance(instances, list) or not instances:
            report.error(f"{where}.instances: expected a non-empty array")
        else:
            for index, instance in enumerate(instances):
                i_where = f"{where}.instances[{index}]"
                if not isinstance(instance, dict):
                    report.error(f"{i_where}: expected an object")
                    continue
                instance_id = instance.get("instance_id")
                if instance_id in instance_ids:
                    report.error(f"{i_where}.instance_id: duplicate {instance_id!r}")
                instance_ids.add(instance_id)
                if instance.get("asset_id") not in asset_ids:
                    report.error(f"{i_where}.asset_id: unknown asset")
                layer_id = instance.get("layer_id")
                z_index = instance.get("z_index")
                if layer_id not in layer_map:
                    report.error(f"{i_where}.layer_id: unknown layer")
                elif not isinstance(z_index, int) or not layer_map[layer_id][0] <= z_index <= layer_map[layer_id][1]:
                    report.error(f"{i_where}.z_index: outside {layer_id!r} band")
                layouts = instance.get("layout_by_profile")
                if not isinstance(layouts, dict) or set(layouts) != PROFILE_IDS:
                    report.error(f"{i_where}.layout_by_profile: expected all four profiles")
                else:
                    for profile_id, layout in layouts.items():
                        if not isinstance(layout, dict):
                            report.error(f"{i_where}.layout_by_profile.{profile_id}: expected an object")
                        else:
                            check_bounds(layout.get("bounds"), f"{i_where}.layout_by_profile.{profile_id}.bounds", report)
        hotspots = scene.get("hotspots")
        if not isinstance(hotspots, list):
            report.error(f"{where}.hotspots: expected an array")
        else:
            hotspot_ids: set[str] = set()
            for index, hotspot in enumerate(hotspots):
                h_where = f"{where}.hotspots[{index}]"
                if not isinstance(hotspot, dict):
                    report.error(f"{h_where}: expected an object")
                    continue
                hotspot_id = hotspot.get("hotspot_id")
                if hotspot_id in hotspot_ids:
                    report.error(f"{h_where}.hotspot_id: duplicate {hotspot_id!r}")
                hotspot_ids.add(hotspot_id)
                anchor = hotspot.get("target_anchor")
                if not check_id(anchor, f"{h_where}.target_anchor", report):
                    continue
                hotspot_anchors.add(anchor)
                bounds_map = hotspot.get("bounds_by_profile")
                if not isinstance(bounds_map, dict) or set(bounds_map) != PROFILE_IDS:
                    report.error(f"{h_where}.bounds_by_profile: expected all four profiles")
                else:
                    for profile_id, bounds in bounds_map.items():
                        check_bounds(bounds, f"{h_where}.bounds_by_profile.{profile_id}", report)
    return hotspot_anchors


def validate_tour(asset_ids: set[str], scene_anchors: set[str], report: Report) -> None:
    path = TOURS / "first-project.v2.json"
    tour = read_json(path, report)
    where = str(path.relative_to(REPO))
    if tour.get("schema_version") != 1 or tour.get("version") != 2:
        report.error(f"{where}: expected schema_version 1 and content version 2")
    if tour.get("tour_id") != "first-project":
        report.error(f"{where}.tour_id: expected 'first-project'")
    steps = tour.get("steps")
    if not isinstance(steps, list) or len(steps) != 7:
        report.error(f"{where}.steps: expected exactly seven steps")
        return
    step_ids: set[str] = set()
    actual_anchors: list[str] = []
    required_fields = {
        "step_id", "view", "target_anchor", "fallback_anchor",
        "coach_asset_id", "title", "body", "placement", "next_label",
    }
    for index, step in enumerate(steps):
        s_where = f"{where}.steps[{index}]"
        if not isinstance(step, dict):
            report.error(f"{s_where}: expected an object")
            continue
        if set(step) != required_fields:
            report.error(f"{s_where}: fields differ from the runtime tour contract")
        step_id = step.get("step_id")
        if check_id(step_id, f"{s_where}.step_id", report):
            if step_id in step_ids:
                report.error(f"{s_where}.step_id: duplicate {step_id!r}")
            step_ids.add(step_id)
        if step.get("view") not in {"office", "manager", "work", "results"}:
            report.error(f"{s_where}.view: invalid view")
        anchor = step.get("target_anchor")
        actual_anchors.append(anchor)
        if anchor not in scene_anchors:
            report.error(f"{s_where}.target_anchor: no scene hotspot for {anchor!r}")
        if step.get("fallback_anchor") not in scene_anchors:
            report.error(f"{s_where}.fallback_anchor: no scene hotspot for fallback")
        if step.get("coach_asset_id") != "character-orbit-dispatcher-idle":
            report.error(f"{s_where}.coach_asset_id: Orbit must coach the v1 tour")
        if step.get("coach_asset_id") not in asset_ids:
            report.error(f"{s_where}.coach_asset_id: unknown asset")
        if step.get("placement") not in {"auto", "center"}:
            report.error(f"{s_where}.placement: expected auto or center")
        for field in ("title", "body", "next_label"):
            if not isinstance(step.get(field), str) or not step[field].strip():
                report.error(f"{s_where}.{field}: expected non-empty text")
    if actual_anchors != TOUR_ANCHORS:
        report.error(f"{where}.steps: anchors/order must be {TOUR_ANCHORS}")


def validate(release: bool) -> Report:
    report = Report()
    for path in (
        ROOT / "manifest.schema.json",
        ROOT / "sources.schema.json",
        SCENES / "scene.schema.json",
        TOURS / "tour.schema.json",
    ):
        read_json(path, report)
    manifest = read_json(ROOT / "manifest.json", report)
    sources = read_json(ROOT / "SOURCES.json", report)
    if not manifest or not sources:
        return report
    asset_ids = validate_manifest(manifest, sources, release, report)
    scene_anchors = validate_scenes(asset_ids, report)
    validate_tour(asset_ids, scene_anchors, report)
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", action="store_true", help="also enforce publication rights")
    arguments = parser.parse_args()
    report = validate(arguments.release)
    for warning in report.warnings:
        print(f"warning: {warning}", file=sys.stderr)
    for error in report.errors:
        print(f"error: {error}", file=sys.stderr)
    if report.errors:
        print(
            f"office v2 graphics invalid: {len(report.errors)} error(s), "
            f"{len(report.warnings)} warning(s)",
            file=sys.stderr,
        )
        return 1
    mode = "release" if arguments.release else "development"
    print(f"office v2 graphics valid ({mode}): {len(report.warnings)} pending-rights warning(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
