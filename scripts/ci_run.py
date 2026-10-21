#!/usr/bin/env python3
"""Run a check and expose its failure tail in GitHub's check annotations."""
from collections import deque
import os
import subprocess
import sys


def annotation(text: str) -> str:
    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def run(command: list[str]) -> int:
    tail: deque[str] = deque(maxlen=80)
    with subprocess.Popen(
        command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, errors="replace",
    ) as process:
        assert process.stdout is not None
        for line in process.stdout:
            print(line, end="", flush=True)
            tail.append(line[-2000:])
        status = process.wait()
    if status and os.environ.get("GITHUB_ACTIONS") == "true":
        message = f"Check exited with {status}. Last output:\n" + "".join(tail)[-12000:]
        print("::error title=Check failure output::" + annotation(message), flush=True)
    return status if status >= 0 else 128 - status


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit("Usage: ci_run.py COMMAND [ARG ...]")
    sys.exit(run(sys.argv[1:]))
