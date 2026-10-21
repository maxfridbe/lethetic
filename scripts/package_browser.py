#!/usr/bin/env python3
"""Package the self-contained site/SDK with checksums and a readable integration guide."""
from __future__ import annotations

import hashlib
import html
import json
import os
from pathlib import Path
import re
import sys
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def package(stage: Path) -> None:
    # Keep the original SDK URLs stable while sharing the native remote UI tree.
    (stage / "src").mkdir(exist_ok=True)
    for module in (stage / "browser/src").glob("*.js"):
        (stage / "src" / module.name).write_text(f'export * from "../browser/src/{module.name}";\n')
        declaration = module.with_suffix(".d.ts")
        if declaration.exists():
            (stage / "src" / declaration.name).write_text(f'export * from "../browser/src/{module.name}";\n')
    crate = tomllib.loads((ROOT / "lethetic-wasm/Cargo.toml").read_text())
    version = os.environ.get("LETHETIC_RELEASE_VERSION", crate["package"]["version"])
    version = version.removeprefix("browser-v").removeprefix("v")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?", version):
        raise ValueError("release version must be a semantic version")
    (stage / "package.json").write_text(json.dumps({
        "name": "@lethetic/browser", "version": version, "type": "module",
        "description": "Lethetic browser WASM runtime, Pyodide and .NET tools, and embeddable UI",
        "exports": {".": {"types": "./src/sdk.d.ts", "import": "./src/sdk.js"},
                    "./embed": {"types": "./src/embed.d.ts", "import": "./src/embed.js"},
                    "./styles.css": "./styles.css"},
        "files": ["src", "browser", "web", "runtimes", "python", "assets", "styles.css", "browser.css", "integration.md"],
        "private": True,
    }, indent=2) + "\n")
    # A readable, escaped copy needs no Markdown renderer, CDN, or script privileges.
    guide = html.escape((stage / "integration.md").read_text())
    (stage / "integration.html").write_text(
        '<!doctype html><html lang="en"><meta charset="utf-8">'
        '<meta name="viewport" content="width=device-width, initial-scale=1">'
        '<title>Lethetic integration guide</title><link rel="stylesheet" href="./guide.css">'
        '<body><main><a href="./">← Lethetic</a><h1>Integration guide</h1><pre>' + guide + '</pre></main></body></html>')
    checksums = {str(path.relative_to(stage)): hashlib.sha256(path.read_bytes()).hexdigest()
                 for path in sorted(stage.rglob("*")) if path.is_file() and path.name != "manifest.json"}
    (stage / "manifest.json").write_text(json.dumps({"version": version, "sha256": checksums}, indent=2) + "\n")
    archive = ROOT / "target" / "lethetic-browser.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
        for path in sorted(stage.rglob("*")):
            if path.is_file():
                info = zipfile.ZipInfo(str(path.relative_to(stage)), date_time=(2020, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o100644 << 16
                output.writestr(info, path.read_bytes())
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(".zip.sha256").write_text(f"{digest}  {archive.name}\n")


if __name__ == "__main__":
    package(Path(sys.argv[1]))
