#!/usr/bin/env python3
"""Run workspace unit and integration tests without live model credentials.

Targets named `live` remain explicit opt-in tests. Container/image-dependent
cases retain their existing #[ignore] annotations.
"""
import json
import os
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main() -> None:
    # Tests can persist cleanup records. Keep them away from the user's state,
    # and cap parallel cases so short lifecycle deadlines are not CPU-starved.
    with tempfile.TemporaryDirectory(prefix="lethetic-rust-tests-") as state:
        env = dict(os.environ, XDG_STATE_HOME=state)
        env.setdefault("RUST_TEST_THREADS", "4")
        env.setdefault("CARGO_BUILD_JOBS", "2")
        run_tests(env)


def run_tests(env: dict[str, str]) -> None:
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version=1", "--locked"],
        cwd=ROOT, text=True,
    ))
    subprocess.run(
        ["cargo", "test", "--workspace", "--lib", "--bins", "--locked"],
        cwd=ROOT, check=True, env=env,
    )
    for package in metadata["packages"]:
        if package["id"] not in metadata["workspace_members"]:
            continue
        tests = [target["name"] for target in package["targets"]
                 if "test" in target["kind"] and target["name"] != "live"]
        if tests:
            command = ["cargo", "test", "--locked", "--package", package["name"]]
            for name in tests:
                command.extend(["--test", name])
            subprocess.run(command, cwd=ROOT, check=True, env=env)


if __name__ == "__main__":
    main()
