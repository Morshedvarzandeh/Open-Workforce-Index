#!/usr/bin/env python3
"""Embed the versioned OWI Office art pack into a self-contained page."""

from __future__ import annotations

import base64
from pathlib import Path


REPO = Path(__file__).resolve().parent.parent
PACK = REPO / "ui" / "assets" / "office" / "v1"
OFFICE_ASSETS = {
    "__ART_BRIGHT_OFFICE__": PACK / "backgrounds" / "bright-office.webp",
    "__ART_ORBIT__": PACK / "characters" / "orbit.webp",
    "__ART_NOVA__": PACK / "characters" / "nova.webp",
    "__ART_MIRA__": PACK / "characters" / "mira.webp",
}


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
    for placeholder, path in OFFICE_ASSETS.items():
        if placeholder not in result:
            raise ValueError(f"Office template is missing {placeholder}")
        result = result.replace(placeholder, data_uri(path))
    leftovers = [key for key in OFFICE_ASSETS if key in result]
    if leftovers:
        raise ValueError(f"Office art placeholders remain: {leftovers}")
    return result
