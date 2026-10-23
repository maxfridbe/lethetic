#!/usr/bin/env python3
"""Run static browser/WASM acceptance tests with disposable Chrome and a mock provider."""
from __future__ import annotations

import argparse
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import tarfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]


class Handler(SimpleHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        if self.path == "/v1/models":
            body = json.dumps({"data": [{"id": "test", "name": "Test model"},
                                        {"id": "deepseek/test", "name": "DeepSeek Test"},
                                        {"id": "other", "name": "Other model"}]}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/pyodide/pyodide-lock.json":
            body = b'{"packages":{}}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/pyodide/pyodide.mjs":
            body = (ROOT / "browser/tests/fixtures/pyodide.mjs").read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", "text/javascript")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path.startswith("/lethetic/"):
            self.path = self.path.removeprefix("/lethetic")
            super().do_GET()
        else:
            self.send_error(404)

    def do_POST(self):
        if self.path == "/python-relay":
            payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if self.headers.get("Authorization") != "Bearer python-test-relay-token-123456789":
                self.send_error(401)
                return
            import base64
            body = json.dumps({"status": 200, "headers": {"Content-Type": "application/json"},
                               "bodyBase64": base64.b64encode(json.dumps(payload).encode()).decode()}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return
        data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        messages = data["messages"]
        if messages[-1]["role"] == "tool":
            content = ("C# says: " if messages[-1].get("tool_call_id", "").startswith("dotnet") else "Python says: ") + messages[-1]["content"].strip()
        else:
            content = "Connected" if "single word Connected" in messages[-1]["content"] else "Hello from the browser test\n\n```python\nprint('private snippet')\n```"
        if messages[-1]["role"] == "user" and messages[-1]["content"] == "remember earlier":
            content = "Context restored" if any(m.get("content") == "hello" for m in messages[:-1]) else "Missing context"
        if messages and "You are compacting one section" in str(messages[0].get("content", "")):
            content = "Continue the Python file task. Files in outputs include edited data.csv and uploaded binary files. Keep the session and files."
        if data.get("tools") and messages[-1]["role"] == "user":
            snippets = {
                "read uploads": "# read-uploaded-files\nfrom pathlib import Path\nprint('uploaded: ' + Path('outputs/upload.csv').read_text().strip() + ' / ' + Path('outputs/raw.bin').read_bytes().hex())",
                "create files": "# create-session-files\nfrom pathlib import Path\nimport base64\nPath('outputs/empty').mkdir(parents=True, exist_ok=True)\nPath('outputs/data.csv').write_text('value\\n42\\n')\nPath('outputs/chart.png').write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII='))\nprint('files created')",
                "read file": "# read-session-file\nfrom pathlib import Path\nprint('read: ' + Path('outputs/data.csv').read_text())",
                "restore files": "# restore-session-file\nfrom pathlib import Path\nprint('restored: ' + Path('outputs/data.csv').read_text())",
                "delete file": "# delete-session-file\nfrom pathlib import Path\nPath('outputs/data.csv').unlink()\nprint('file deleted')",
                "list files": "# list-session-files\nfrom pathlib import Path\nprint('workspace empty: ' + str(not list(Path('.').iterdir())).lower())",
            }
            code = snippets.get(messages[-1]["content"], "print(6 * 7)")
            message = {"role": "assistant", "content": None, "tool_calls": [{
                "id": "python-1", "type": "function",
                "function": {"name": "python", "arguments": json.dumps({"code": code})},
            }]}
            if data["tools"][0]["function"]["name"] == "dotnet":
                code = 'File.WriteAllText("Program.cs", "Console.WriteLine(42);"); var built = await Dotnet.Run("Program.cs"); Console.WriteLine("dotnet files ready");'
                message["tool_calls"][0]["id"] = "dotnet-1"
                message["tool_calls"][0]["function"] = {"name": "dotnet", "arguments": json.dumps({"code": code})}
                assert "Network access: disabled" in messages[0]["content"]
            reason = "tool_calls"
        else:
            message, reason = {"role": "assistant", "content": content}, "stop"
        body = json.dumps({"choices": [{"message": message, "finish_reason": reason}],
                           "usage": {"prompt_tokens": 1200, "completion_tokens": 64,
                                     "total_tokens": 1264, "prompt_cache_hit_tokens": 1024}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pyodide", action="store_true", help="also exercise real Pyodide from its CDN")
    args = parser.parse_args()
    browser = os.environ.get("CHROME_BIN") or shutil.which("google-chrome") or shutil.which("chromium")
    if not browser:
        raise SystemExit("Set CHROME_BIN to a Chrome or Chromium executable")
    if not (ROOT / "target/browser/browser/wasm/lethetic_wasm_bg.wasm").is_file():
        raise SystemExit("Run ./build-browser.sh first")
    with tempfile.TemporaryDirectory(prefix="lethetic-browser-") as temporary:
        temp = Path(temporary)
        subprocess.run(["tsc", "--project", str(ROOT / "browser/tests/tsconfig.json"),
                        "--outDir", str(temp / "tests")], check=True)
        server = ThreadingHTTPServer(("127.0.0.1", 0),
                                    partial(Handler, directory=str(ROOT / "target/browser")))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        origin = f"http://127.0.0.1:{server.server_port}"
        try:
            config = {"browser": browser, "profile": str(temp / "profile"),
                      "origin": origin, "pyodide": args.pyodide,
                      "screenshot": str(ROOT / "target/browser-preview.png")}
            result = subprocess.run(["node", "--experimental-default-type=module",
                                     str(temp / "tests/browser/tests/browser-driver.js")],
                                    input=json.dumps(config), text=True, capture_output=True, timeout=480)
            sys.stdout.write(result.stdout)
            sys.stderr.write(result.stderr)
            if result.returncode and os.environ.get("GITHUB_ACTIONS"):
                lines = result.stderr.splitlines()
                failure = next((line for line in lines if line.startswith("Error:")), "Browser test failed")
                caller = next((line.strip() for line in reversed(lines) if "browser-driver.js:" in line), "")
                detail = (failure + " " + caller)[:3000].replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
                print(f"::error title=Browser acceptance::{detail}", flush=True)
            result.check_returncode()
            with tarfile.open(ROOT / "target/browser-preview-session.tar.gz", "r:gz") as archive:
                assert archive.extractfile("workspace/reports/analysis.py").read() == b"print('private snippet')"
                session = archive.extractfile("session.json").read()
                assert b"secret-never-persisted" not in session and b"updated-tab-key" not in session
                assert "apiKey" not in json.loads(session)["connection"]
                assert archive.getmember("diagnostics.json").isfile()
            print("Downloaded session debug archive verified with Python tarfile.")
            with sqlite3.connect(f"file:{ROOT / 'target/browser-preview-session.db'}?mode=ro", uri=True) as database:
                assert database.execute("PRAGMA integrity_check").fetchone() == ("ok",)
                assert database.execute("PRAGMA foreign_key_check").fetchall() == []
                assert database.execute("PRAGMA user_version").fetchone() == (2,)
                assert database.execute("SELECT content FROM files WHERE path=?", ("reports/analysis.py",)).fetchone() == (b"print('private snippet')",)
                assert database.execute("SELECT COUNT(*) FROM sessions").fetchone() == (1,)
                assert database.execute("SELECT COUNT(*) FROM messages").fetchone()[0] > 0
                assert not database.execute("SELECT name FROM sqlite_schema WHERE name='content_records'").fetchall()
                connection = database.execute("SELECT name,model,extra_json FROM connections").fetchone()
                assert connection and "apiKey" not in json.loads(connection[2])
            print("Browser Turso export verified with Python SQLite integrity_check and binary file read.")

            with zipfile.ZipFile(ROOT / "target/browser-preview-files.zip") as archive:
                assert archive.testzip() is None
                assert archive.read("outputs/raw.bin") == bytes([0, 255, 1])
                assert archive.read("outputs/upload.csv") == b"a,b\n1,2\n"
                assert archive.getinfo("outputs/empty/").is_dir()
            print("Downloaded session ZIP verified with Python zipfile.")
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
