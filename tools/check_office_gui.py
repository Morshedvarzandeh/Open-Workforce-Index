#!/usr/bin/env python3
"""Static release checks for the dependency-free OWI Office interface."""

from __future__ import annotations

import re
import subprocess
import sys
from html.parser import HTMLParser
from pathlib import Path

from owi_assets import OFFICE_ASSETS


REPO = Path(__file__).resolve().parent.parent
TEMPLATE = REPO / "tools" / "owi_ask_template.html"


class PageParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.ids: list[str] = []
        self.html_language: str | None = None

    def handle_starttag(
        self, tag: str, attrs: list[tuple[str, str | None]]
    ) -> None:
        attributes = dict(attrs)
        if tag == "html":
            self.html_language = attributes.get("lang")
        if attributes.get("id"):
            self.ids.append(str(attributes["id"]))


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"office GUI check failed: {message}")


def main() -> int:
    page = TEMPLATE.read_text(encoding="utf-8")
    rendered = page.replace("__DATA__", "{}").replace("__BUILT__", "test")
    parser = PageParser()
    parser.feed(rendered)

    require(parser.html_language == "en", "the document needs lang=en")
    require(len(parser.ids) == len(set(parser.ids)), "HTML ids must be unique")
    required_ids = {
        "view-office",
        "view-work",
        "view-results",
        "crewGrid",
        "officeStatus",
        "skin",
        "form",
        "q",
        "checks",
        "answer",
        "resultTotal",
        "resultAccepted",
        "resultRejected",
        "resultsList",
    }
    require(required_ids.issubset(parser.ids), "a core Office surface is missing")
    require('id="answer" aria-live="polite"' in page,
            "task results need an aria-live region")
    require('value="bright">Bright office</option>' in page,
            "the bright office must remain the default skin")
    require('class="character robot"' not in page,
            "crew must come from the injected roster, not fictional static workers")
    require("CREW_PERSONAS" in page and 'asset: "orbit"' in page,
            "the blue bowl robot persona is missing")
    require("worker.id" in page and "crew-model" in page,
            "friendly crew cards must expose exact worker identities")
    require("setup required" in page and "ready to run" in page,
            "catalogued and runnable workers must be distinguished")
    require("Actual spend" in page and "Not recorded by this browser" in page,
            "unmetered cash must not look like measured zero")
    require("Verified savings" in page and "Needs a frozen baseline" in page,
            "savings need an explicit evidence boundary")
    require("CO₂ · water" in page and "No measured provider evidence" in page,
            "environmental unknowns must remain visible")
    require("68%" not in page and "cost saved" not in page.lower(),
            "the interface contains a hard-coded savings claim")
    require("@media (max-width:620px)" in page,
            "the Office needs a compact-screen layout")
    require("prefers-reduced-motion:reduce" in page,
            "the Office needs a reduced-motion mode")
    require(page.count("// >>> decision-math >>>") == 1
            and page.count("// <<< decision-math <<<") == 1,
            "the verified decision-math boundary changed")
    for placeholder, path in OFFICE_ASSETS.items():
        require(placeholder in page, f"the template lost {placeholder}")
        require(path.is_file(), f"the art pack is missing {path.name}")
    asset_validation = subprocess.run(
        [sys.executable, str(REPO / "ui/assets/office/v1/validate.py")],
        text=True, capture_output=True, check=False
    )
    require(asset_validation.returncode == 0,
            "the Office art library is invalid:\n"
            + asset_validation.stderr[-1200:])

    scripts = re.findall(r"<script(?: [^>]*)?>(.*?)</script>", rendered, re.S)
    require(len(scripts) == 2, "expected one data script and one application script")
    checked = subprocess.run(
        ["node", "--check"], input=scripts[1], text=True,
        capture_output=True, check=False
    )
    require(checked.returncode == 0,
            "application JavaScript is invalid:\n" + checked.stderr[-1200:])

    print("office GUI verified: bright default, versioned game-art library, "
          "real roster identities, truthful unknowns, responsive shell, "
          "valid JavaScript")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
