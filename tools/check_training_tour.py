#!/usr/bin/env python3
"""Dependency-light release checks for the OWI Office training tour."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from html.parser import HTMLParser
from pathlib import Path
from typing import Any

from owi_assets import inline_office_assets


REPO = Path(__file__).resolve().parent.parent
TEMPLATE = REPO / "tools" / "owi_ask_template.html"
TOUR_IDS = {
    "trainingTour",
    "tourLaunch",
    "tourCoach",
    "tourTitle",
    "tourBody",
    "tourBack",
    "tourNext",
    "tourSkip",
    "tourClose",
    "tourProgress",
    "office-tour-data",
}
MUTATING_ENDPOINTS = ("/staff", "/run", "/review")
MUTATING_CALLS = (
    "workflowStaff(",
    "workflowRun(",
    "workflowReview(",
    "workflowCreateProject(",
    "workflowAddTask(",
    "workflowEditTask(",
    "workflowDeleteTask(",
)


class TourPageParser(HTMLParser):
    """Collect the small DOM contract without adding an HTML dependency."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.elements: dict[str, tuple[str, dict[str, str | None]]] = {}
        self.anchors: list[str] = []
        self.text: dict[str, list[str]] = {}
        self.scripts: list[tuple[dict[str, str | None], str]] = []
        self.styles: list[str] = []
        self._stack: list[tuple[str, str | None]] = []
        self._script: tuple[dict[str, str | None], list[str]] | None = None
        self._style: list[str] | None = None

    def handle_starttag(
        self, tag: str, attrs: list[tuple[str, str | None]]
    ) -> None:
        attributes = dict(attrs)
        element_id = attributes.get("id")
        if element_id:
            self.elements[element_id] = (tag, attributes)
            self.text.setdefault(element_id, [])
        anchor = attributes.get("data-tour-id")
        if anchor:
            self.anchors.append(anchor)
        self._stack.append((tag, element_id))
        if tag == "script":
            self._script = (attributes, [])
        elif tag == "style":
            self._style = []

    def handle_startendtag(
        self, tag: str, attrs: list[tuple[str, str | None]]
    ) -> None:
        attributes = dict(attrs)
        element_id = attributes.get("id")
        if element_id:
            self.elements[element_id] = (tag, attributes)
            self.text.setdefault(element_id, [])
        anchor = attributes.get("data-tour-id")
        if anchor:
            self.anchors.append(anchor)

    def handle_endtag(self, tag: str) -> None:
        if tag == "script" and self._script is not None:
            attributes, chunks = self._script
            self.scripts.append((attributes, "".join(chunks)))
            self._script = None
        elif tag == "style" and self._style is not None:
            self.styles.append("".join(self._style))
            self._style = None
        for index in range(len(self._stack) - 1, -1, -1):
            if self._stack[index][0] == tag:
                del self._stack[index:]
                break

    def handle_data(self, data: str) -> None:
        if self._script is not None:
            self._script[1].append(data)
        if self._style is not None:
            self._style.append(data)
        if data.strip():
            for _, element_id in self._stack:
                if element_id:
                    self.text.setdefault(element_id, []).append(data)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"training tour check failed: {message}")


def element_attributes(
    parser: TourPageParser, element_id: str
) -> tuple[str, dict[str, str | None]]:
    require(element_id in parser.elements, f"missing #{element_id}")
    return parser.elements[element_id]


def visible_name(parser: TourPageParser, element_id: str) -> str:
    _, attrs = element_attributes(parser, element_id)
    return " ".join(parser.text.get(element_id, [])).strip() or str(
        attrs.get("aria-label") or attrs.get("title") or ""
    ).strip()


def tour_payload(parser: TourPageParser) -> tuple[Any, list[dict[str, Any]]]:
    matches = [
        source
        for attrs, source in parser.scripts
        if attrs.get("id") == "office-tour-data"
    ]
    require(len(matches) == 1, "expected one #office-tour-data JSON script")
    try:
        payload = json.loads(matches[0])
    except json.JSONDecodeError as error:
        raise SystemExit(
            f"training tour check failed: #office-tour-data is invalid JSON: {error}"
        ) from error
    chapters = payload.get("steps") if isinstance(payload, dict) else None
    require(isinstance(chapters, list), "tour JSON needs a top-level steps array")
    require(all(isinstance(item, dict) for item in chapters),
            "every tour chapter must be an object")
    return payload, chapters


def normalized_anchor(value: Any) -> str | None:
    if not isinstance(value, str) or not value.strip():
        return None
    value = value.strip()
    match = re.fullmatch(
        r"\[data-tour-id\s*=\s*['\"]([^'\"]+)['\"]\]", value
    )
    return match.group(1) if match else value


def flattened_strings(value: Any) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [text for item in value for text in flattened_strings(item)]
    if isinstance(value, dict):
        return [text for item in value.values() for text in flattened_strings(item)]
    return []


def office_asset_ids() -> set[str]:
    ids: set[str] = set()
    manifests = sorted((REPO / "ui" / "assets" / "office").glob("*/manifest.json"))
    require(bool(manifests), "no versioned Office asset manifest was found")
    for manifest in manifests:
        try:
            payload = json.loads(manifest.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise SystemExit(
                f"training tour check failed: invalid asset manifest {manifest}: {error}"
            ) from error
        assets = payload.get("assets", []) if isinstance(payload, dict) else []
        require(isinstance(assets, list), f"{manifest} needs an assets array")
        for asset in assets:
            if isinstance(asset, dict) and isinstance(asset.get("id"), str):
                ids.add(asset["id"])
    return ids


def check_accessibility(parser: TourPageParser) -> None:
    require(TOUR_IDS.issubset(parser.elements), "a required tour surface is missing")
    dialog_candidates = []
    for element_id in ("trainingTour", "tourCoach"):
        _, attrs = element_attributes(parser, element_id)
        if attrs.get("role") == "dialog":
            dialog_candidates.append((element_id, attrs))
    require(len(dialog_candidates) == 1,
            "exactly one tour container must use role=dialog")
    _, dialog = dialog_candidates[0]
    require(dialog.get("aria-modal") == "true", "tour dialog needs aria-modal=true")
    require(dialog.get("aria-labelledby") == "tourTitle",
            "tour dialog must be labelled by #tourTitle")
    require(dialog.get("aria-describedby") == "tourBody",
            "tour dialog must be described by #tourBody")
    _, overlay = element_attributes(parser, "trainingTour")
    require("hidden" in overlay,
            "tour overlay must start hidden to avoid a first-paint modal flash")

    for element_id in (
        "tourLaunch", "tourBack", "tourNext", "tourSkip", "tourClose"
    ):
        tag, attrs = element_attributes(parser, element_id)
        require(tag == "button", f"#{element_id} must be a keyboard-native button")
        require(attrs.get("type") == "button", f"#{element_id} needs type=button")
        require(bool(visible_name(parser, element_id)),
                f"#{element_id} needs an accessible name")
    _, progress = element_attributes(parser, "tourProgress")
    require(
        progress.get("aria-live") in {"polite", "assertive"}
        or progress.get("role") == "status",
        "tour progress must be announced to assistive technology",
    )


def check_css(parser: TourPageParser) -> None:
    css = "\n".join(parser.styles)
    rules = re.findall(r"([^{}]+)\{([^{}]*)\}", css, re.S)
    tour_css = "\n".join(
        body for selector, body in rules if "tour" in selector.lower()
    )
    require(tour_css, "no tour-specific CSS rules were found")

    width_values = re.findall(
        r"(?:min-)?(?:width|inline-size)\s*:[^;{}]*?(\d+(?:\.\d+)?)px",
        tour_css,
        re.I,
    )
    height_values = re.findall(
        r"(?:min-)?(?:height|block-size)\s*:[^;{}]*?(\d+(?:\.\d+)?)px",
        tour_css,
        re.I,
    )
    require(
        any(float(value) >= 44 for value in width_values)
        and any(float(value) >= 44 for value in height_values),
        "tour controls need a declared 44 by 44 pixel touch target",
    )
    for edge in ("top", "right", "bottom", "left"):
        require(f"env(safe-area-inset-{edge})" in css,
                f"tour layout does not protect the {edge} mobile safe area")

    reduced = re.search(
        r"@media\s*\(\s*prefers-reduced-motion\s*:\s*reduce\s*\)",
        css,
        re.I,
    )
    require(bool(reduced), "tour needs a prefers-reduced-motion mode")
    reduced_tail = css[reduced.start():] if reduced else ""
    require(
        re.search(
            r"animation(?:-duration)?\s*:\s*(?:none|0(?:ms|s)?)",
            reduced_tail,
            re.I,
        )
        is not None,
        "reduced-motion mode must remove tour animation",
    )
    require(
        re.search(
            r"transition(?:-duration)?\s*:\s*(?:none|0(?:ms|s)?)",
            reduced_tail,
            re.I,
        )
        is not None,
        "reduced-motion mode must remove tour transitions",
    )


def check_chapters(
    parser: TourPageParser, payload: Any, chapters: list[dict[str, Any]]
) -> None:
    require(len(chapters) == 7, "the complete tour must contain exactly 7 chapters")
    ids: list[str] = []
    anchors: list[str] = []
    fallbacks: list[str] = []
    views: list[str] = []
    coach_assets: list[str] = []
    for position, chapter in enumerate(chapters, start=1):
        required_fields = {
            "step_id", "view", "target_anchor", "fallback_anchor",
            "coach_asset_id", "title", "body", "placement", "next_label",
        }
        missing_fields = sorted(required_fields - set(chapter))
        require(not missing_fields,
                f"step {position} is missing fields: {missing_fields}")
        chapter_id = chapter.get("step_id")
        require(isinstance(chapter_id, str) and bool(chapter_id.strip()),
                f"step {position} needs a stable step_id")
        ids.append(chapter_id.strip())

        anchor = normalized_anchor(chapter.get("target_anchor"))
        require(anchor is not None, f"chapter {chapter_id} needs a primary anchor")
        anchors.append(str(anchor))

        fallback = normalized_anchor(chapter.get("fallback_anchor"))
        require(fallback is not None,
                f"chapter {chapter_id} needs a fallback_anchor")
        fallbacks.append(str(fallback))

        view = chapter.get("view")
        require(isinstance(view, str) and bool(view.strip()),
                f"chapter {chapter_id} needs a view")
        views.append(view.strip())
        for field in (
            "coach_asset_id", "title", "body", "placement", "next_label"
        ):
            require(isinstance(chapter.get(field), str)
                    and bool(chapter[field].strip()),
                    f"chapter {chapter_id} needs non-empty {field}")
        coach_assets.append(chapter["coach_asset_id"].strip())

    require(len(set(ids)) == 7, "tour chapter IDs must be unique")
    require(len(set(anchors)) == 7, "tour primary anchors must be unique")
    require(len(parser.anchors) == len(set(parser.anchors)),
            "data-tour-id anchors in the page must be unique")
    missing = sorted((set(anchors) | set(fallbacks)) - set(parser.anchors))
    require(not missing, f"tour JSON points to missing data-tour-id anchors: {missing}")
    missing_views = sorted(
        view for view in set(views) if f"view-{view}" not in parser.elements
    )
    require(not missing_views, f"tour JSON points to missing views: {missing_views}")
    missing_assets = sorted(set(coach_assets) - office_asset_ids())
    require(not missing_assets,
            f"tour JSON points to unknown coach assets: {missing_assets}")

    tour_copy = " ".join(flattened_strings(payload)).lower()
    for endpoint in MUTATING_ENDPOINTS:
        require(endpoint not in tour_copy,
                f"tour data must not contain the mutating endpoint {endpoint}")
    require(re.search(r"\b(?:unknown|unavailable|not available)\b", tour_copy) is not None,
            "tour must say when an outcome metric is unknown")
    require("evidence" in tour_copy or "measured" in tour_copy,
            "tour must connect outcome metrics to evidence")
    for metric in ("spend", "saving", "water"):
        require(metric in tour_copy, f"tour honesty copy does not mention {metric}")
    require("co₂" in tour_copy or "co2" in tour_copy,
            "tour honesty copy does not mention CO2")
    require(re.search(r"\$\s*\d|\b\d+(?:\.\d+)?\s*%", tour_copy) is None,
            "tour contains an unsupported hard-coded cost or savings claim")


def check_javascript(page: str, parser: TourPageParser) -> None:
    require(page.count("// >>> tour-state >>>") == 1,
            "expected one tour-state opening marker")
    require(page.count("// <<< tour-state <<<") == 1,
            "expected one tour-state closing marker")
    start = page.index("// >>> tour-state >>>")
    end = page.index("// <<< tour-state <<<", start)
    require(start < end, "tour-state markers are reversed")
    state = page[start:end]
    for forbidden in (
        "document", "window", "localStorage", "sessionStorage",
        "XMLHttpRequest", "navigator", "location",
    ):
        require(re.search(rf"\b{re.escape(forbidden)}\b", state) is None,
                f"pure tour-state block depends on browser API {forbidden}")
    require(re.search(r"\bfetch\s*\(", state) is None,
            "pure tour-state block depends on browser API fetch")
    for forbidden in MUTATING_ENDPOINTS + MUTATING_CALLS:
        require(forbidden not in state,
                f"tour state can trigger a mutating workflow path: {forbidden}")
    state_checks = r"""
const verify = (condition, message) => {
  if (!condition) throw new Error(message);
};
const version = "tour-contract-test";
const count = 7;
let value = initialTourState(version);
verify(value.status === "new" && value.step === 0, "initial state");
verify(normalizeTourState(null, version, count).status === "new", "null state");
verify(normalizeTourState({version:"old",status:"completed",step:6},
  version, count).status === "new", "version reset");
verify(normalizeTourState({version,status:"in_progress",step:99},
  version, count).step === 6, "upper clamp");
value = reduceTourState(value, {type:"START"}, version, count);
verify(value.status === "in_progress" && value.step === 0, "start");
value = reduceTourState(value, {type:"GO",step:4}, version, count);
verify(value.status === "in_progress" && value.step === 4, "go");
value = reduceTourState(value, {type:"PAUSE"}, version, count);
verify(value.status === "paused" && value.step === 4, "pause");
value = reduceTourState(value, {type:"RESUME"}, version, count);
verify(value.status === "in_progress" && value.step === 4, "resume");
value = reduceTourState(value, {type:"SKIP"}, version, count);
verify(value.status === "skipped" && value.step === 4, "skip");
value = reduceTourState(value, {type:"REPLAY"}, version, count);
verify(value.status === "in_progress" && value.step === 0, "replay");
value = reduceTourState(value, {type:"COMPLETE"}, version, count);
verify(value.status === "completed" && value.step === 6, "complete");
"""
    reduced = subprocess.run(
        ["node"], input=state + state_checks, text=True,
        capture_output=True, check=False
    )
    require(reduced.returncode == 0,
            "tour state reducer failed first-run/resume/replay/skip checks:\n"
            + reduced.stderr[-1200:])

    application_scripts = [
        source
        for attrs, source in parser.scripts
        if (attrs.get("type") or "").lower()
        not in {"application/json", "application/ld+json"}
    ]
    require(application_scripts, "no application JavaScript was found")
    require("owi-office-tour-v2" in "\n".join(application_scripts),
            "tour runtime must use the versioned owi-office-tour-v2 key")
    for position, source in enumerate(application_scripts, start=1):
        checked = subprocess.run(
            ["node", "--check"], input=source, text=True,
            capture_output=True, check=False
        )
        require(checked.returncode == 0,
                f"application JavaScript block {position} is invalid:\n"
                + checked.stderr[-1200:])

    lines = "\n".join(application_scripts).splitlines()
    tour_line_indexes = [
        index for index, line in enumerate(lines)
        if re.search(r"\b(?:tour[A-Z_]|trainingTour|office-tour)", line)
    ]
    require(tour_line_indexes, "no tour runtime was found in application JavaScript")
    for index in tour_line_indexes:
        nearby = "\n".join(lines[max(0, index - 3):index + 5])
        for forbidden in MUTATING_CALLS:
            require(forbidden not in nearby,
                    f"tour runtime is coupled to mutating call {forbidden}")
        require(
            re.search(r"method\s*:\s*['\"](?:POST|PATCH|DELETE)['\"]", nearby, re.I)
            is None,
            "tour runtime contains a mutating HTTP method",
        )


def check_page_truth(page: str) -> None:
    lowered = page.lower()
    require("actual spend" in lowered, "results lost the actual-spend boundary")
    require("verified savings" in lowered, "results lost the savings boundary")
    require("co₂" in lowered or "co2" in lowered,
            "results lost the environmental boundary")
    require("water" in lowered, "results lost the water boundary")
    require(
        "no measured provider evidence" in lowered
        or "measured evidence" in lowered,
        "environmental metrics no longer disclose their evidence boundary",
    )
    require("68%" not in page and "cost saved" not in lowered,
            "the page contains a hard-coded savings claim")


def main() -> int:
    page = TEMPLATE.read_text(encoding="utf-8")
    rendered = inline_office_assets(page)
    rendered = rendered.replace("__DATA__", "{}").replace("__BUILT__", "test")
    require("__ART_" not in rendered and "__OFFICE_" not in rendered,
            "the rendered tour contains an unresolved asset or layer placeholder")
    parser = TourPageParser()
    parser.feed(rendered)

    check_accessibility(parser)
    check_css(parser)
    payload, chapters = tour_payload(parser)
    check_chapters(parser, payload, chapters)
    check_javascript(rendered, parser)
    check_page_truth(rendered)

    print(
        "training tour verified: 7 truthful chapters, accessible dialog, "
        "resolved anchors and fallbacks, mobile safe areas, reduced motion, "
        "no workflow mutation hooks, and valid JavaScript"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
