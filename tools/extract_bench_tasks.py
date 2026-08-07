#!/usr/bin/env python3
"""Extract a deterministic task corpus from a Python repository.

One task is: *this documented function has had its body removed; restore an
implementation that makes the test suite pass.* The signature and docstring
stay, so the task measures implementation ability against a stated spec rather
than the ability to guess an unstated one.

Every emitted task is validated twice before it is allowed into the corpus:

1. With the body stubbed out, the verify command must **fail**. A task whose
   tests still pass measures nothing — the function is not covered.
2. With the original body restored, the verify command must **pass**. This
   proves the failure in step 1 is caused by the removed body and not by a
   pre-existing breakage, a missing dependency, or a flaky suite.

A task that fails either check is reported and discarded. The corpus is only
worth what its weakest task is, and an uncovered function silently scored as
"the model failed" would corrupt every estimate derived from it.

The corpus is language-agnostic on purpose: a task is a file, a line range, a
replacement, and a shell command that decides pass or fail. Only this
extractor knows Python.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

STUB_BODY = '    raise NotImplementedError("body removed for benchmark task")'


@dataclass
class Candidate:
    """A function whose body can be removed to make a task."""

    task_id: str
    source_path: str
    qualified_name: str
    signature: str
    docstring: str
    body_start_line: int  # 1-indexed, inclusive; first line after the docstring
    body_end_line: int  # 1-indexed, inclusive
    reference_body: str


@dataclass
class Rejected:
    task_id: str
    reason: str
    detail: str = ""


@dataclass
class ExtractionReport:
    tasks: list[dict] = field(default_factory=list)
    rejected: list[Rejected] = field(default_factory=list)


def function_candidates(source_path: Path, repo_root: Path) -> list[Candidate]:
    """Find every function whose body sits below a docstring."""
    text = source_path.read_text()
    lines = text.splitlines()
    tree = ast.parse(text, filename=str(source_path))
    relative = source_path.relative_to(repo_root).as_posix()
    candidates: list[Candidate] = []

    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if not node.body:
            continue

        first = node.body[0]
        docstring = ast.get_docstring(node)
        # Without a docstring the task would require guessing the contract,
        # which measures telepathy rather than implementation.
        if docstring is None or len(node.body) < 2:
            continue

        body_start = first.end_lineno + 1
        body_end = node.end_lineno
        if body_start > body_end:
            continue

        signature_end = first.lineno - 1
        signature = "\n".join(lines[node.lineno - 1 : signature_end])
        reference_body = "\n".join(lines[body_start - 1 : body_end])
        if not reference_body.strip():
            continue

        candidates.append(
            Candidate(
                task_id=f"task:{relative}:{node.name}",
                source_path=relative,
                qualified_name=node.name,
                signature=signature,
                docstring=docstring,
                body_start_line=body_start,
                body_end_line=body_end,
                reference_body=reference_body,
            )
        )
    return candidates


def apply_stub(repo_root: Path, candidate: Candidate) -> str:
    """Replace the candidate's body with a stub. Returns the original text."""
    path = repo_root / candidate.source_path
    original = path.read_text()
    lines = original.splitlines(keepends=True)
    stubbed = (
        lines[: candidate.body_start_line - 1]
        + [STUB_BODY + "\n"]
        + lines[candidate.body_end_line :]
    )
    path.write_text("".join(stubbed))
    return original


def run_verify(repo_root: Path, command: list[str]) -> tuple[bool, str]:
    try:
        completed = subprocess.run(
            command,
            cwd=repo_root,
            capture_output=True,
            text=True,
            timeout=600,
        )
    except subprocess.TimeoutExpired:
        return False, "verify command timed out"
    tail = (completed.stdout + completed.stderr).strip().splitlines()
    return completed.returncode == 0, "\n".join(tail[-3:])


def extract(
    repo_root: Path,
    source_globs: list[str],
    verify_command: list[str],
    skill_id: str,
) -> ExtractionReport:
    report = ExtractionReport()

    baseline_passed, baseline_detail = run_verify(repo_root, verify_command)
    if not baseline_passed:
        raise SystemExit(
            "the repository's own test suite must pass before tasks can be "
            f"extracted from it:\n{baseline_detail}"
        )

    candidates: list[Candidate] = []
    for pattern in source_globs:
        for source_path in sorted(repo_root.glob(pattern)):
            candidates.extend(function_candidates(source_path, repo_root))

    for candidate in candidates:
        path = repo_root / candidate.source_path
        original = apply_stub(repo_root, candidate)
        try:
            stub_passed, stub_detail = run_verify(repo_root, verify_command)
        finally:
            path.write_text(original)

        if stub_passed:
            report.rejected.append(
                Rejected(
                    candidate.task_id,
                    "not_covered",
                    "the suite still passes with the body removed",
                )
            )
            continue

        restored_passed, restored_detail = run_verify(repo_root, verify_command)
        if not restored_passed:
            report.rejected.append(
                Rejected(
                    candidate.task_id,
                    "restore_failed",
                    f"suite does not pass after restoring: {restored_detail}",
                )
            )
            continue

        report.tasks.append(
            {
                "task_id": candidate.task_id,
                "skill_id": skill_id,
                "source_path": candidate.source_path,
                "qualified_name": candidate.qualified_name,
                "instruction": (
                    f"Implement the body of `{candidate.qualified_name}` in "
                    f"`{candidate.source_path}` so the repository's test suite "
                    "passes. The signature and docstring below state the "
                    "contract; do not change them, and do not modify tests."
                ),
                "signature": candidate.signature,
                "docstring": candidate.docstring,
                "patch": {
                    "file": candidate.source_path,
                    "start_line": candidate.body_start_line,
                    "end_line": candidate.body_end_line,
                    "stub": STUB_BODY,
                },
                "verify_command": verify_command,
                "reference_body_sha256": hashlib.sha256(
                    candidate.reference_body.encode()
                ).hexdigest(),
                "stub_failure_detail": stub_detail,
            }
        )

    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument(
        "--source-glob",
        action="append",
        required=True,
        help="Glob of source files to mine, relative to --repo. Repeatable.",
    )
    parser.add_argument(
        "--verify-command",
        required=True,
        help="Shell-free command that decides pass/fail, space separated.",
    )
    parser.add_argument("--skill-id", required=True)
    parser.add_argument("--corpus-id", required=True)
    parser.add_argument("--repo-url", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    repo_root = arguments.repo.resolve()
    verify_command = arguments.verify_command.split()

    report = extract(
        repo_root,
        arguments.source_glob,
        verify_command,
        arguments.skill_id,
    )

    corpus = {
        "corpus_id": arguments.corpus_id,
        "repository_url": arguments.repo_url,
        "commit": arguments.commit,
        "skill_id": arguments.skill_id,
        "verify_command": verify_command,
        "extractor_version": "extract-bench-tasks@1",
        "task_count": len(report.tasks),
        "tasks": report.tasks,
        "rejected": [
            {"task_id": r.task_id, "reason": r.reason, "detail": r.detail}
            for r in report.rejected
        ],
    }
    arguments.output.write_text(json.dumps(corpus, indent=2) + "\n")

    print(f"accepted {len(report.tasks)} tasks", file=sys.stderr)
    for rejected in report.rejected:
        print(f"  rejected {rejected.task_id}: {rejected.reason}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
