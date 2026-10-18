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
import subprocess
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[1]


class Handler(SimpleHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        if self.path == "/pyodide/pyodide.mjs":
            body = b'''export async function loadPyodide() {
              let stdout;
              return {setStdout(o){stdout=o.batched},setStderr(){},setStdin(){},
                async loadPackagesFromImports(){},async runPythonAsync(code){
                  if(code.includes("while True")){while(true){}}
                  if(code.includes("raise"))throw new Error("fixture Python error");
                  stdout("42"); return undefined;
                }};
            }'''
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
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return
        data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        messages = data["messages"]
        if messages[-1]["role"] == "tool":
            content = "Python says: " + messages[-1]["content"].strip()
        else:
            content = "Connected" if "single word Connected" in messages[-1]["content"] else "Hello from the browser test\n\n```python\nprint('private snippet')\n```"
        if messages[-1]["role"] == "user" and messages[-1]["content"] == "remember earlier":
            content = "Context restored" if any(m.get("content") == "hello" for m in messages[:-1]) else "Missing context"
        if data.get("tools") and messages[-1]["role"] == "user":
            message = {"role": "assistant", "content": None, "tool_calls": [{
                "id": "python-1", "type": "function",
                "function": {"name": "python", "arguments": json.dumps({"code": "print(6 * 7)"})},
            }]}
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
            subprocess.run(["node", "--experimental-default-type=module",
                            str(temp / "tests/browser/tests/browser-driver.js")],
                           input=json.dumps(config), text=True, check=True, timeout=300)
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
