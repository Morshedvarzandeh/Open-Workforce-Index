#!/usr/bin/env python3
"""Bridge a chat CLI to run_bench's body-on-stdout contract.

    python3 tools/run_bench.py ... --adapter command \
      --command "python3 tools/bench_chat_adapter.py claude --model <id> -p"

run_bench hands the adapter a task as JSON on stdin and inserts stdout as the
function body, verbatim. A chat CLI is the wrong shape for that twice over:
agentic CLIs try to go visit the repository instead of answering ("I can't
reach the docs from here"), and chat models wrap code in fences and prose.
This shim closes the gap without touching the harness contract:

- the prompt states that NO files exist and everything needed is inline, so
  there is nothing to wander off to;
- the reply is normalised into a bare body: code fences stripped, a repeated
  `def` line dropped, indentation raised to the requested base.

The wrapped command is everything after the script name. It runs from a
neutral temporary directory so an agentic CLI has no repository to inherit.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile

PROMPT = """You are completing exactly one Python function. There are NO files
and NO repository — everything you need is below. Do not ask questions.

Reply with ONLY the function body: indented Python statements. No `def` line,
no code fences, no explanation before or after.

SIGNATURE:
{signature}

DOCSTRING:
\"\"\"{docstring}\"\"\"

INSTRUCTION:
{instruction}"""


def extract_body(reply: str, indent: str) -> str:
    text = reply.strip()
    fenced = re.findall(r"```(?:python)?\s*\n(.*?)```", text, re.S)
    if fenced:
        text = max(fenced, key=len).rstrip()
    lines = text.splitlines()
    # Drop a repeated signature: everything through the first line that ends
    # the `def ...:` header.
    for position, line in enumerate(lines):
        if line.lstrip().startswith(("def ", "async def ")):
            for end in range(position, len(lines)):
                if lines[end].rstrip().endswith(":"):
                    lines = lines[end + 1:]
                    break
            break
    body = [line for line in lines]
    while body and not body[0].strip():
        body.pop(0)
    while body and not body[-1].strip():
        body.pop()
    if not body:
        return ""
    filled = [line for line in body if line.strip()]
    current = min(len(line) - len(line.lstrip()) for line in filled)
    shift = indent if current == 0 else ""
    return "\n".join((shift + line) if line.strip() else "" for line in body)


def main() -> int:
    command = sys.argv[1:]
    if not command:
        print("usage: bench_chat_adapter.py <command...>", file=sys.stderr)
        return 2
    task = json.load(sys.stdin)
    prompt = PROMPT.format(signature=task["signature"],
                           docstring=task["docstring"],
                           instruction=task["instruction"])
    with tempfile.TemporaryDirectory() as neutral:
        completed = subprocess.run(command, input=prompt, text=True,
                                   capture_output=True, cwd=neutral)
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr[-1000:])
        return completed.returncode
    body = extract_body(completed.stdout, task.get("indent", "    "))
    if not body:
        sys.stderr.write("adapter: reply contained no usable body\n")
        return 1
    print(body)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
