#!/usr/bin/env python3
"""Exercise the real WFE with a disposable HTTPS process and Chrome profile.

No provider prompts, existing profiles, private configuration, or running user
processes are used. Chrome and the global TypeScript compiler (tsc) must already be
available; this script installs nothing.
"""

from __future__ import annotations

import argparse
import errno
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TEST_TSCONFIG = ROOT / "web/tests/tsconfig.json"
# Emitted path of web/tests/tools/wfe-files-browser.ts relative to the compiler outDir.
DRIVER_OUTPUT = Path("tests/tools/wfe-files-browser.js")


def compile_driver(destination: Path) -> Path:
    """Type-check and compile the TypeScript CDP driver into a disposable directory."""
    tsc = shutil.which("tsc")
    if tsc is None:
        raise RuntimeError("global tsc is required to compile the browser driver")
    subprocess.run([tsc, "--project", str(TEST_TSCONFIG), "--outDir", str(destination),
                    "--pretty", "false"], check=True)
    driver = destination / DRIVER_OUTPUT
    if not driver.is_file():
        raise RuntimeError("browser driver compilation produced no output")
    return driver


def wait_process(pid: int, seconds: float) -> bool:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            if os.waitpid(pid, os.WNOHANG)[0] == pid:
                return True
        except ChildProcessError:
            return True
        time.sleep(0.05)
    return False


def stop_owned_process(pid: int) -> None:
    if wait_process(pid, 0.1):
        return
    os.kill(pid, signal.SIGTERM)
    if not wait_process(pid, 15):
        os.kill(pid, signal.SIGKILL)
        if not wait_process(pid, 5):
            raise RuntimeError("test-owned Lethetic did not exit")


def launch(binary: Path, workspace: Path, home: Path, enabled: bool, tokenless: bool):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    origin = f"https://127.0.0.1:{port}"
    arguments = [str(binary), "--new-session", "--service", "--wfe-remote-control", origin]
    if enabled:
        arguments += ["--wfe-files", "localonly"]
    if tokenless:
        arguments.append("--wfe-disable-authtoken")
    environment = os.environ.copy()
    environment.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
                       XDG_STATE_HOME=str(home / "state"), XDG_DATA_HOME=str(home / "data"),
                       XDG_CACHE_HOME=str(home / "cache"), TERM="xterm-256color")
    for name in list(environment):
        if any(marker in name.upper() for marker in ("API_KEY", "ACCESS_TOKEN", "AUTH_TOKEN", "OAUTH")):
            environment.pop(name)
    pid, terminal = pty.fork()
    if pid == 0:
        os.chdir(workspace)
        os.execve(binary, arguments, environment)
    output = bytearray()
    acknowledged = False
    private_url = None
    deadline = time.monotonic() + 45
    try:
        while time.monotonic() < deadline:
            if select.select([terminal], [], [], 0.2)[0]:
                try:
                    chunk = os.read(terminal, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise
                if not chunk:
                    break
                output.extend(chunk)
                if len(output) > 1024 * 1024:
                    raise RuntimeError("test startup exceeded its output limit")
                text = output.decode("utf-8", errors="replace")
                match = re.search(r"Private controller URL: (https://[^\s]+)", text)
                if match:
                    private_url = match.group(1)
                if not acknowledged and "Press Enter" in text:
                    os.write(terminal, b"\n")
                    acknowledged = True
                if acknowledged and ((tokenless and "Lethetic WFE listening" in text) or
                                      (not tokenless and private_url is not None)):
                    # The authenticated listener is already bound before its acknowledgement.
                    return pid, terminal, origin, origin if tokenless else private_url
            if os.waitpid(pid, os.WNOHANG)[0] == pid:
                break
        # Startup may contain a private controller URL: never include raw output.
        raise RuntimeError("test-owned Lethetic failed to complete secure bootstrap")
    except BaseException:
        stop_owned_process(pid)
        os.close(terminal)
        raise


def run_case(binary: Path, browser: Path, driver_script: Path, case: Path, enabled: bool,
             tokenless: bool) -> dict:
    workspace = case / "workspace"
    home = case / "home"
    workspace.mkdir(parents=True)
    home.mkdir()
    (workspace / "src").mkdir()
    (workspace / ".git").mkdir()
    (workspace / ".git" / "control").write_text("protected synthetic control", encoding="utf-8")
    (workspace / ".env").write_text("SYNTHETIC_PRIVATE=do-not-share\n", encoding="utf-8")
    (workspace / "config.yml").write_text(
        "server_url: http://127.0.0.1:9\nmodel: offline-browser-test\ncontext_size: 128000\nmodel_servers: []\n",
        encoding="utf-8",
    )
    (workspace / "README.md").write_text("# Disposable browser fixture\nNo provider calls.\n", encoding="utf-8")
    source = 'fn main() {\n    println!("browser-fixture-marker-名字");\n}\n'
    (workspace / "src" / "main.rs").write_text(source, encoding="utf-8")
    hostile = '<script>window.__fileExecuted=true</script>\n<img src="https://invalid.example/leak" onerror="window.__fileExecuted=true">\n'
    (workspace / "hostile.html").write_text(hostile, encoding="utf-8")
    (workspace / "binary.bin").write_bytes(bytes(range(256)))
    (workspace / "large.txt").write_bytes(b"x" * (2 * 1024 * 1024 + 1))
    downloads = case / "downloads"
    downloads.mkdir()
    browser_home = case / "browser-home"
    browser_home.mkdir()
    profile = case / "browser-profile"
    profile.mkdir()
    baseline = {path.relative_to(workspace).as_posix(): path.read_bytes()
                for path in workspace.rglob("*") if path.is_file()}
    pid, terminal, origin, url = launch(binary, workspace, home, enabled, tokenless)
    try:
        configuration = {"browser": str(browser), "profile": str(profile), "browserHome": str(browser_home),
                         "downloads": str(downloads), "origin": origin, "url": url,
                         "enabled": enabled, "screenshot": str(case / "browser.png"),
                         "source": source, "hostile": hostile}
        driver = subprocess.Popen(["node", "--experimental-default-type=module", str(driver_script)],
                                  stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                  text=True, start_new_session=True)
        try:
            stdout, stderr = driver.communicate(json.dumps(configuration), timeout=150)
        except subprocess.TimeoutExpired:
            os.killpg(driver.pid, signal.SIGTERM)
            try:
                driver.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(driver.pid, signal.SIGKILL)
                driver.communicate()
            raise RuntimeError("test-owned browser driver exceeded its deadline") from None
        if driver.returncode:
            # Driver errors are fixed stage labels, not URLs, headers, or browser logs.
            raise RuntimeError("browser driver failed: " + stderr[-2000:])
        report = json.loads(stdout)
        for relative, before in baseline.items():
            if (workspace / relative).read_bytes() != before:
                raise RuntimeError("file viewing changed a fixture source")
        if enabled:
            if (downloads / "main.rs").read_text(encoding="utf-8") != source:
                raise RuntimeError("file download differs from source")
            with zipfile.ZipFile(downloads / "src.zip") as archive:
                names = archive.namelist()
                files = [entry for entry in archive.infolist() if not entry.is_dir()]
                if len(files) != 1 or any(name.startswith("/") or ".." in name.split("/") for name in names):
                    raise RuntimeError("archive paths were not normalized")
                if archive.read(files[0]) != source.encode("utf-8"):
                    raise RuntimeError("archive content differs from source")
                if any(entry.compress_type != zipfile.ZIP_STORED for entry in archive.infolist()):
                    raise RuntimeError("archive was not a Stored ZIP")
        return {"files": enabled, "tokenless": tokenless, **report}
    finally:
        stop_owned_process(pid)
        os.close(terminal)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/lethetic")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    browser = args.browser.resolve(strict=True)
    evidence = Path(tempfile.mkdtemp(prefix="lethetic-wfe-files-acceptance-"))
    print(f"Browser evidence: {evidence}", flush=True)
    driver_script = compile_driver(evidence / "driver")
    reports = []
    for name, enabled, tokenless in [("disabled", False, False), ("authenticated", True, False), ("tokenless", True, True)]:
        reports.append(run_case(binary, browser, driver_script, evidence / name, enabled, tokenless))
        print(f"Passed: {name}", flush=True)
    (evidence / "result.json").write_text(json.dumps(reports, indent=2) + "\n", encoding="utf-8")
    print("Real browser file acceptance passed; no provider prompts sent.", flush=True)


if __name__ == "__main__":
    main()
