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


def extract_body(reply: str, indent: str, name: str = "") -> str:
    # rstrip, never strip: leading whitespace on the first line IS the body's
    # indentation. Removing it made the min-indent shift below see column zero
    # and push every deeper line four spaces further in, breaking the block
    # structure of any reply that was not fenced. Fenced replies already took
    # the rstrip path, so the two shapes of the same answer were normalised
    # differently — one of them wrongly.
    text = reply.rstrip()
    fenced = re.findall(r"```(?:python)?\s*\n(.*?)```", text, re.S)
    if fenced:
        text = max(fenced, key=len).rstrip()
    lines = text.splitlines()
    # Drop a repeated signature: everything through the line that ends the
    # `def ...:` header — but ONLY when the model restated THIS function at
    # column zero.
    #
    # The earlier rule matched any `def` at any indentation, which silently
    # decapitated a nested helper: a body opening with `def _to_float(...)`
    # lost that header and kept its indented block, so the spliced file could
    # not even be imported. Pytest reported "errors during collection", the
    # runner scored it as a failed implementation, and the bench was really
    # measuring which models avoid nested helpers. That is a style, not a
    # capability, and it was being written into the ledger as one.
    for position, line in enumerate(lines):
        stripped = line.lstrip()
        if not stripped.startswith(("def ", "async def ")):
            continue
        if line != stripped:
            break                      # indented: a helper the body needs
        if name and not re.match(rf"(async\s+)?def\s+{re.escape(name)}\s*\(",
                                 stripped):
            break                      # a different function: leave it alone
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
    # The command is positional, and getting that wrong is easy to do and
    # expensive to diagnose: passing `--command 'claude ...'` made subprocess
    # try to execute a file literally named `--command`, and the resulting
    # FileNotFoundError said nothing about the real mistake. Eleven benchmark
    # tasks came back in forty milliseconds each as "adapter produced no
    # body". The blame rule held -- nothing was recorded against the model --
    # but a whole run measured nothing.
    if command[0].startswith("-"):
        print(f"bench_chat_adapter.py takes the command positionally, not as "
              f"a flag.\n  got:      {' '.join(command)}\n"
              f"  meant:    {' '.join(command).replace('--command ', '', 1)}",
              file=sys.stderr)
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
    body = extract_body(completed.stdout, task.get("indent") or "    ",
                        task.get("qualified_name", ""))
    if not body:
        sys.stderr.write("adapter: reply contained no usable body\n")
        return 1
    print(body)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
