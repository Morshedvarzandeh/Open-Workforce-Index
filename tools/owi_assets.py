#!/usr/bin/env python3
"""Embed the versioned OWI Office art pack into a self-contained page."""

from __future__ import annotations

import base64
import json
from pathlib import Path


REPO = Path(__file__).resolve().parent.parent
PACK_V2 = REPO / "ui" / "assets" / "office" / "v2"
RUNTIME = REPO / "ui" / "runtime"
TOURS = REPO / "ui" / "tours"
OFFICE_ASSET_IDS = {
    "__ART_BRIGHT_OFFICE__": "background-bright-office-main",
    "__ART_ORBIT__": "character-orbit-dispatcher-idle",
    "__ART_NOVA__": "character-nova-researcher-idle",
    "__ART_MIRA__": "character-mira-designer-idle",
    "__ART_TRAINING_MAP__": "background-training-map-first-project",
}

OFFICE_LAYERS = {
    "__OFFICE_GAME_CSS__": RUNTIME / "office-game.css",
    "__OFFICE_TOUR_JS__": RUNTIME / "office-tour.js",
}

OFFICE_JSON = {
    "__OFFICE_TOUR_DATA__": TOURS / "first-project.v1.json",
}


def asset_registry() -> dict[str, Path]:
    """Resolve stable UI slots through the pack manifest, like a game client."""
    manifest_path = PACK_V2 / "manifest.json"
    if not manifest_path.is_file():
        raise FileNotFoundError(f"required Office manifest is missing: {manifest_path}")
    manifest = json.loads(manifest_path.read_text())
    by_id = {asset["id"]: asset for asset in manifest.get("assets", [])}
    registry: dict[str, Path] = {}
    for placeholder, asset_id in OFFICE_ASSET_IDS.items():
        asset = by_id.get(asset_id)
        if asset is None:
            raise ValueError(f"Office manifest is missing asset {asset_id}")
        runtime = next((item for item in asset.get("renditions", [])
                        if item.get("purpose") == "runtime"), None)
        if runtime is None:
            raise ValueError(f"Office asset {asset_id} has no runtime rendition")
        registry[placeholder] = PACK_V2 / runtime["file"]
    return registry


OFFICE_ASSETS = asset_registry()


def data_uri(path: Path) -> str:
    if not path.is_file():
        raise FileNotFoundError(f"required Office art is missing: {path}")
    media_type = {".png": "image/png", ".webp": "image/webp"}.get(path.suffix)
    if media_type is None:
        raise ValueError(f"unsupported Office art format: {path.suffix}")
    encoded = base64.b64encode(path.read_bytes()).decode("ascii")
    return f"data:{media_type};base64,{encoded}"


def inline_office_assets(template: str) -> str:
    """Replace every stable art placeholder with the pack's current rendition."""
    result = template
    for placeholder, path in OFFICE_LAYERS.items():
        if placeholder not in result:
            raise ValueError(f"Office template is missing {placeholder}")
        if not path.is_file():
            raise FileNotFoundError(f"required Office layer is missing: {path}")
        result = result.replace(placeholder, path.read_text())
    for placeholder, path in OFFICE_JSON.items():
        if placeholder not in result:
            raise ValueError(f"Office template is missing {placeholder}")
        if not path.is_file():
            raise FileNotFoundError(f"required Office data is missing: {path}")
        # Parse and emit a compact, deterministic payload. Escaping the closing
        # tag keeps a future authored string from terminating its JSON script.
        payload = json.dumps(json.loads(path.read_text()), separators=(",", ":"))
        result = result.replace(placeholder, payload.replace("</", "<\\/"))
    for placeholder, path in OFFICE_ASSETS.items():
        if placeholder not in result:
            raise ValueError(f"Office template is missing {placeholder}")
        result = result.replace(placeholder, data_uri(path))
    leftovers = [key for key in (*OFFICE_ASSETS, *OFFICE_LAYERS, *OFFICE_JSON)
                 if key in result]
    if leftovers:
        raise ValueError(f"Office art placeholders remain: {leftovers}")
    return result
