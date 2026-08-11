#!/usr/bin/env python3
"""Run a task corpus against one worker and emit verified outcome records.

For each task the runner stubs out the function body, asks an adapter to write
a replacement, restores the file, and runs the corpus's verify command. The
command's exit status is the only thing that decides acceptance — no model,
including the one under test, gets a say in whether its own output passed.

The runner holds no credentials. The `command` adapter shells out to a program
you supply, hands it the task as JSON on stdin, and reads the function body
from stdout. Any CLI that can talk to a model works, and the key stays wherever
you already keep it.

Two built-in adapters exist to prove the harness itself is honest before any
money is spent:

- `oracle` restores the repository's own implementation. Every task must pass.
  If one does not, the harness is broken, not the model.
- `stub` leaves the body unimplemented. Every task must fail. If one passes,
  that task measures nothing and should never have been in the corpus.

Run both before trusting a single real result.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path


@dataclass
class Attempt:
    task_id: str
    accepted: bool
    latency_ms: int
    input_tokens: int
    output_tokens: int
    cash_micros: int
    failure_detail: str
    # Who failed. "worker" means the model wrote a body and the tests
    # rejected it — real evidence. "harness" means no body ever reached the
    # repository: the adapter crashed, timed out, or was never found. The
    # tests never ran, so there is nothing to hold against the model, and a
    # harness attempt is reported but never written as an outcome. Same rule
    # the front door applies to empty stdout: plumbing is not performance.
    blame: str = "worker"


def read_file_at_commit(repo: Path, commit: str, relative_path: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo), "show", f"{commit}:{relative_path}"],
        capture_output=True,
        text=True,
        check=True,
    )
    return completed.stdout


def splice(original: str, start_line: int, end_line: int, replacement: str) -> str:
    """Replace an inclusive 1-indexed line range with `replacement`."""
    lines = original.splitlines(keepends=True)
    body = replacement if replacement.endswith("\n") else replacement + "\n"
    return "".join(lines[: start_line - 1] + [body] + lines[end_line:])


def adapter_oracle(task: dict, repo: Path, commit: str) -> tuple[str, dict]:
    """Return the repository's own implementation of the body."""
    pristine = read_file_at_commit(repo, commit, task["patch"]["file"])
    lines = pristine.splitlines(keepends=True)
    start, end = task["patch"]["start_line"], task["patch"]["end_line"]
    return "".join(lines[start - 1 : end]), {}


def adapter_stub(task: dict, repo: Path, commit: str) -> tuple[str, dict]:
    """Leave the body unimplemented."""
    return task["patch"]["stub"], {}


def adapter_command(
    task: dict, repo: Path, commit: str, command: list[str], timeout: int
) -> tuple[str, dict]:
    """Delegate to an external program: task JSON on stdin, body on stdout.

    Usage metrics are optional. If the program writes a final line of JSON to
    stderr with `input_tokens`, `output_tokens`, or `cash_micros`, those are
    recorded; otherwise they stay zero and the outcome is still valid, just
    less informative about cost.
    """
    payload = json.dumps(
        {
            "task_id": task["task_id"],
            "instruction": task["instruction"],
            "signature": task["signature"],
            "docstring": task["docstring"],
            "source_path": task["source_path"],
            "qualified_name": task["qualified_name"],
            "indent": "    ",
        }
    )
    completed = subprocess.run(
        command,
        input=payload,
        capture_output=True,
        text=True,
        timeout=timeout,
        cwd=repo,
    )
    usage: dict = {}
    for line in reversed(completed.stderr.strip().splitlines()):
        try:
            parsed = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(parsed, dict):
            usage = parsed
        break
    return completed.stdout, usage


def run_task(
    task: dict,
    repo: Path,
    commit: str,
    verify_command: list[str],
    produce_body,
    verify_timeout: int,
) -> Attempt:
    path = repo / task["patch"]["file"]
    original = path.read_text()
    started = time.monotonic()
    usage: dict = {}
    try:
        body, usage = produce_body(task)
        elapsed_ms = int((time.monotonic() - started) * 1000)
        if not body.strip():
            return Attempt(
                task["task_id"], False, elapsed_ms, 0, 0, 0,
                "adapter produced no body", blame="harness"
            )
        path.write_text(
            splice(
                original,
                task["patch"]["start_line"],
                task["patch"]["end_line"],
                body.rstrip("\n"),
            )
        )
        try:
            completed = subprocess.run(
                verify_command,
                cwd=repo,
                capture_output=True,
                text=True,
                timeout=verify_timeout,
            )
            accepted = completed.returncode == 0
            detail = "" if accepted else _tail(completed.stdout + completed.stderr)
        except subprocess.TimeoutExpired:
            accepted, detail = False, "verify command timed out"
    except subprocess.TimeoutExpired:
        elapsed_ms = int((time.monotonic() - started) * 1000)
        return Attempt(task["task_id"], False, elapsed_ms, 0, 0, 0,
                       "adapter timed out", blame="harness")
    except OSError as error:
        # The adapter program does not exist, is not executable, or the
        # working tree could not be written. None of that is the model's
        # doing, and crashing here would lose the whole run's report.
        elapsed_ms = int((time.monotonic() - started) * 1000)
        return Attempt(task["task_id"], False, elapsed_ms, 0, 0, 0,
                       f"adapter could not be started: {error}",
                       blame="harness")
    finally:
        # Always restore, so one failed task cannot corrupt the next.
        path.write_text(original)

    return Attempt(
        task_id=task["task_id"],
        accepted=accepted,
        latency_ms=int((time.monotonic() - started) * 1000),
        input_tokens=int(usage.get("input_tokens", 0)),
        output_tokens=int(usage.get("output_tokens", 0)),
        cash_micros=int(usage.get("cash_micros", 0)),
        failure_detail=detail,
    )


def _tail(text: str, lines: int = 2) -> str:
    return "\n".join(text.strip().splitlines()[-lines:])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", required=True, type=Path)
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--worker-id", required=True)
    parser.add_argument(
        "--adapter", required=True, choices=["oracle", "stub", "command"]
    )
    parser.add_argument(
        "--command",
        help="Program for the `command` adapter; task JSON on stdin, body on stdout.",
    )
    parser.add_argument("--adapter-timeout", type=int, default=300)
    parser.add_argument("--verify-timeout", type=int, default=600)
    parser.add_argument("--observed-at", required=True)
    parser.add_argument(
        "--outcome-dir",
        type=Path,
        help="Write one PrivateOutcomeRecord JSON per task, for `owi outcome`.",
    )
    parser.add_argument("--report", type=Path)
    arguments = parser.parse_args()

    corpus = json.loads(arguments.corpus.read_text())
    repo = arguments.repo.resolve()
    commit = corpus["commit"]

    if arguments.adapter == "command":
        if not arguments.command:
            parser.error("--command is required for the `command` adapter")
        command = arguments.command.split()

        def produce(task: dict):
            return adapter_command(
                task, repo, commit, command, arguments.adapter_timeout
            )

    elif arguments.adapter == "oracle":

        def produce(task: dict):
            return adapter_oracle(task, repo, commit)

    else:

        def produce(task: dict):
            return adapter_stub(task, repo, commit)

    attempts: list[Attempt] = []
    for index, task in enumerate(corpus["tasks"], start=1):
        attempt = run_task(
            task,
            repo,
            commit,
            corpus["verify_command"],
            produce,
            arguments.verify_timeout,
        )
        attempts.append(attempt)
        mark = ("PASS" if attempt.accepted
                else "FAIL" if attempt.blame == "worker" else "HARNESS")
        detail = f"  {attempt.failure_detail}" if attempt.blame != "worker" else ""
        print(
            f"[{index}/{len(corpus['tasks'])}] {mark} {task['qualified_name']} "
            f"({attempt.latency_ms} ms){detail}",
            file=sys.stderr,
        )

    scored = [a for a in attempts if a.blame == "worker"]
    unscored = [a for a in attempts if a.blame != "worker"]
    accepted = sum(1 for attempt in scored if attempt.accepted)
    report = {
        "corpus_id": corpus["corpus_id"],
        "commit": commit,
        "worker_id": arguments.worker_id,
        "adapter": arguments.adapter,
        "skill_id": corpus["skill_id"],
        "attempted": len(attempts),
        "scored": len(scored),
        "unscored_harness_failures": len(unscored),
        "accepted": accepted,
        "pass_rate": accepted / len(scored) if scored else 0.0,
        "total_cash_micros": sum(a.cash_micros for a in attempts),
        "attempts": [asdict(a) for a in attempts],
    }
    if arguments.report:
        arguments.report.write_text(json.dumps(report, indent=2) + "\n")

    if arguments.outcome_dir:
        arguments.outcome_dir.mkdir(parents=True, exist_ok=True)
        for attempt in scored:
            slug = attempt.task_id.replace("/", "_").replace(":", "_")
            record = {
                "event": {
                    "id": f"outcome:{corpus['corpus_id']}:{arguments.worker_id}:{slug}",
                    "task_id": attempt.task_id,
                    "worker_id": arguments.worker_id,
                    "skill_id": corpus["skill_id"],
                    "accepted": attempt.accepted,
                    # The test suite decided this, not a model reviewing itself.
                    "validation_kind": "deterministic",
                    "actual_cash_micros": attempt.cash_micros,
                    "actual_quota_milliunits": 0,
                    "latency_ms": attempt.latency_ms,
                    "observed_at": arguments.observed_at,
                }
            }
            (arguments.outcome_dir / f"{slug}.json").write_text(
                json.dumps(record, indent=2) + "\n"
            )

    print(
        f"\n{arguments.worker_id} via {arguments.adapter}: "
        f"{accepted}/{len(scored)} accepted "
        f"({report['pass_rate']:.1%})",
        file=sys.stderr,
    )
    if unscored:
        print(
            f"{len(unscored)} task(s) never reached the tests and were NOT "
            f"recorded — the harness failed, not the model: "
            f"{unscored[0].failure_detail}",
            file=sys.stderr,
        )
        if not scored:
            print("nothing was measured; fix the adapter and run again",
                  file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
