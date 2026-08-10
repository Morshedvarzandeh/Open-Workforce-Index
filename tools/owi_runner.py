#!/usr/bin/env python3
"""The runner contract: what a worker is, and whose fault a failure is.

A runner is the user's own command line for a model. owi never holds
credentials — `.owi-quick/runners.json` maps a model name to a command that
already works on this machine, and running a worker means handing that
command the task and reading back what it says.

That sounds like one call to `subprocess.run`. It is not, and the difference
has cost real evidence twice. The rules below were each learned from a
misattributed failure, and they used to live as prose plus three or four
near-copies of the same code — which is exactly how two identical bugs came
to be fixed by hand in two separate places while a third copy kept the bug.
This module is the one implementation. Every caller gets a `RunResult` whose
`blame` says who failed, so attributing a plumbing failure to a model takes a
deliberate misuse rather than a moment's inattention.

The contract:

1. The task travels on stdin, whole. Not as an argument, not as a file path.
2. The worker runs in a fresh scratch directory that inherits nothing.
   Agentic CLIs read standing instructions (CLAUDE.md, AGENTS.md) from every
   directory above them, and they wander whatever repository they land in.
   Started inside this repo a worker reads its own router instructions and
   asks whether to route the task instead of doing it — asked its role from
   .owi-quick/ a model answers "Router", from a scratch directory "Worker".
3. Empty stdout is the ENVIRONMENT's failure, never the worker's. A model
   never replies with literally nothing; empty output means a CLI collision,
   an auth prompt, or a crash that exited zero. Checking a checklist against
   "" would convict a worker who never spoke.
4. A non-zero exit is the ENVIRONMENT's failure. So is a timeout, and so is a
   command that could not be started at all.
5. Only output that actually arrived can be held against a model. Anything
   else is plumbing, and plumbing is not performance.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

# Whose failure it was. There are exactly two answers, and most of this
# module exists to keep the second one honest.
WORKER = "worker"
ENVIRONMENT = "environment"

# Files an agentic CLI reads as standing instructions, discovered by walking
# up from wherever it was invoked.
AGENT_INSTRUCTION_FILES = ("CLAUDE.md", "AGENTS.md")

DEFAULT_TIMEOUT_SECONDS = 600


def instruction_files_above(path: Path) -> list[str]:
    """Standing instructions a worker started in `path` would inherit."""
    directory = Path(path).resolve()
    return [str(parent / name)
            for parent in [directory, *directory.parents]
            for name in AGENT_INSTRUCTION_FILES
            if (parent / name).exists()]


@dataclass(frozen=True)
class RunResult:
    """What came back, and whose fault it is if nothing usable did.

    `blame` is ENVIRONMENT whenever the worker never got a fair chance to
    answer. `output` is non-empty if and only if blame is WORKER: there is no
    state in which a caller holds output it must not check, or lacks output
    it should have checked.
    """

    output: str
    stderr: str
    exit_code: int
    blame: str
    detail: str = ""

    @property
    def usable(self) -> bool:
        """True when a checklist may be run against this output."""
        return self.blame == WORKER

    @property
    def rejected_by_environment(self) -> bool:
        return self.blame == ENVIRONMENT


def run_worker(command: str, payload: str,
               timeout: int = DEFAULT_TIMEOUT_SECONDS) -> RunResult:
    """Run one worker under the contract above.

    Never raises for an ordinary failure: a timeout, a non-zero exit, a
    missing program and silence all come back as ENVIRONMENT results, because
    every caller has to handle them identically and an exception is the one
    shape that invites a caller to forget.
    """
    try:
        with tempfile.TemporaryDirectory(prefix="owi-run-") as scratch:
            completed = subprocess.run(
                command, shell=True, input=payload, text=True,
                capture_output=True, timeout=timeout, cwd=scratch)
    except subprocess.TimeoutExpired:
        return RunResult("", "", -1, ENVIRONMENT,
                         f"runner timed out after {timeout}s")
    except OSError as error:
        return RunResult("", str(error), -1, ENVIRONMENT,
                         f"runner could not be started: {error}")

    stdout, stderr = completed.stdout or "", completed.stderr or ""
    if completed.returncode != 0:
        return RunResult("", stderr, completed.returncode, ENVIRONMENT,
                         f"runner exit {completed.returncode}: {stderr[:300]}")
    if not stdout.strip():
        return RunResult("", stderr, completed.returncode, ENVIRONMENT,
                         "runner produced no output")
    return RunResult(stdout, stderr, completed.returncode, WORKER)


def load_runners(home: Path) -> dict[str, str]:
    """The model → command map, or an empty one. A runners.json that cannot
    be parsed is reported rather than silently treated as "no runners
    configured", which would look exactly like a model nobody can run."""
    path = Path(home) / "runners.json"
    if not path.exists():
        return {}
    try:
        loaded = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise SystemExit(f"{path} is not valid JSON: {error}")
    if not isinstance(loaded, dict):
        raise SystemExit(f"{path} must be an object mapping model to command")
    return {str(model): str(command or "")
            for model, command in loaded.items()}


def resolve(home: Path, model: str, builtins: dict[str, str]) -> str | None:
    """The command for a model: what the user configured, else the built-in
    for a CLI that is actually installed here. Configuration always wins —
    the built-ins are a convenience, never an override."""
    configured = load_runners(home).get(model, "").strip()
    if configured:
        return configured
    command = builtins.get(model)
    if not command:
        return None
    program = command.split()[0]
    return command if shutil.which(program) else None
