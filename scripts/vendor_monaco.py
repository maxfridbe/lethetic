#!/usr/bin/env python3
"""Vendor Monaco's browser-native ESM directly, without Node or a bundler.

Normal builds never run this acquisition step. An explicit --fetch downloads the
pinned archive; --archive and --check operate entirely offline.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import posixpath
import re
import sys
import tarfile
import urllib.parse
import urllib.request

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
LIB = ROOT / "web/lib"
VERSION = "0.56.0"
ARCHIVE_URL = f"https://registry.npmjs.org/monaco-editor/-/monaco-editor-{VERSION}.tgz"
ARCHIVE_SHA256 = "b74bc4437205c194b779b0f21e5e7fcd3b4e9acbf3f7c8732a545d2059fb7412"
ARCHIVE_INTEGRITY = "sha512-sXboRm3BeBeLm938eaiyLMe0OxzfXIlZvbv4ir/jVgQy1zDhWjgmny0WoN45fuDKhCCQsYMbBJrv/A6jd8aCUg=="
ENTRYPOINTS = (
    "vs/editor/editor.api.js",
    "vs/basic-languages/monaco.contribution.js",
    "vs/editor/contrib/find/browser/findController.js",
    "vs/editor/editor.worker.js",
)
STYLESHEETS = (
    "vs/base/browser/ui/codicons/codicon/codicon.css",
    "vs/base/browser/ui/codicons/codicon/codicon-modifiers.css",
)
CSS_IMPORT = re.compile(r"(?m)^import (['\"])([^'\"\n]+\.css)\1;\r?\n?")
CSS_URL = re.compile(r"url\(\s*(?:(['\"])(.*?)\1|([^)'\"]*?))\s*\)", re.DOTALL)
CSS_AT_IMPORT = re.compile(r"@import\s+(['\"])([^'\"]+)\1\s*;")
EXTERNAL_DIFF = "vs/editor/common/diff/externalLinesDiffComputer.js"
WORKER_SERVICE = "vs/editor/standalone/browser/services/standaloneWebWorkerService.js"


def verifier():
    path = ROOT / "web/static/build-support/verify-web-assets.py"
    spec = importlib.util.spec_from_file_location("lethetic_asset_verifier", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def resolve(importer: str, target: str) -> str:
    if target.startswith("/") or ":" in target or "\\" in target:
        raise ValueError(f"unsupported upstream asset reference in {importer}: {target}")
    path = posixpath.normpath(posixpath.join(posixpath.dirname(importer), target))
    if path == ".." or path.startswith("../"):
        raise ValueError("upstream reference escapes Monaco's ESM tree")
    return path


def relative(importer: str, target: str) -> str:
    path = posixpath.relpath(target, posixpath.dirname(importer) or ".")
    return path if path.startswith("../") else "./" + path


def prepare(archive: bytes) -> dict[str, bytes]:
    if digest(archive) != ARCHIVE_SHA256:
        raise ValueError("Monaco archive SHA-256 mismatch")
    actual_integrity = "sha512-" + base64.b64encode(hashlib.sha512(archive).digest()).decode()
    if actual_integrity != ARCHIVE_INTEGRITY:
        raise ValueError("Monaco archive upstream integrity mismatch")
    v = verifier()
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as package:
        members = {m.name.removeprefix("package/esm/"): m for m in package.getmembers()
                   if m.name.startswith("package/esm/") and m.isfile()}
        prepared: dict[str, bytes] = {}
        css_order: list[str] = []
        patches = []
        visiting = set()

        def upstream(path: str) -> bytes:
            member = members.get(path)
            if member is None or member.size > 8 * 1024 * 1024:
                raise ValueError(f"unavailable or oversized upstream module: {path}")
            return package.extractfile(member).read()

        def visit_css(path: str):
            if path in prepared:
                return
            source = upstream(path).decode("utf-8")
            # A quoted CSS @import is made explicit so the existing verifier
            # follows it just like every other stylesheet dependency.
            source = CSS_AT_IMPORT.sub(lambda m: f'@import url("{m.group(2)}");', source)

            def rewrite_url(match):
                value = (match.group(2) if match.group(1) else match.group(3)).strip()
                if value.startswith("data:"):
                    header, payload = value.split(",", 1)
                    media = header.split(";", 1)[0]
                    extensions = {"data:image/svg+xml": "svg", "data:image/png": "png",
                                  "data:image/gif": "gif"}
                    if media not in extensions:
                        raise ValueError(f"unsupported embedded Monaco asset: {media}")
                    data = (base64.b64decode(payload, validate=True) if ";base64" in header
                            else urllib.parse.unquote_to_bytes(payload))
                    if len(data) > 1024 * 1024:
                        raise ValueError("oversized embedded Monaco asset")
                    target = posixpath.join(posixpath.dirname(path),
                                           f"embedded-{digest(data)[:24]}.{extensions[media]}")
                    prepared[target] = data
                else:
                    target = resolve(path, value)
                    if target.endswith(".css"):
                        visit_css(target)
                    elif target not in prepared:
                        prepared[target] = upstream(target)
                return f'url("{relative(path, target)}")'

            source = CSS_URL.sub(rewrite_url, source)
            prepared[path] = source.encode("utf-8")
            css_order.append(path)
            if prepared[path] != upstream(path):
                patches.append({"path": path, "upstream_sha256": digest(upstream(path)),
                                "changes": ["canonical local stylesheet/embedded-image URLs"]})

        def visit_js(path: str):
            if path in prepared or path in visiting:
                return
            visiting.add(path)
            original = upstream(path)
            source = original.decode("utf-8")
            changes = []
            if path == EXTERNAL_DIFF:
                old = "externalModulePromise = import(/* webpackIgnore: true */ /* @vite-ignore */ `${url}`);"
                if source.count(old) != 1:
                    raise ValueError("external diff loader changed upstream")
                source = source.replace(old, "externalModulePromise = Promise.reject(new Error('External diff modules are disabled in Lethetic'));", 1)
                changes.append("disable optional runtime external diff/package import")
            if path == WORKER_SERVICE:
                old = "return super._createWorker(descriptor);"
                if source.count(old) != 1:
                    raise ValueError("standalone worker fallback changed upstream")
                source = source.replace(old, "throw new Error('Lethetic requires an explicit same-origin module worker');", 1)
                changes.append("reject missing explicit worker hook instead of fallback")
            css_imports = CSS_IMPORT.findall(source)
            references = v.module_references(source, path)
            for ref in references:
                target = resolve(path, ref.specifier)
                if target.endswith(".css"):
                    if not any(specifier == ref.specifier for _, specifier in css_imports):
                        raise ValueError(f"unsupported bound CSS import in {path}")
                    visit_css(target)
                elif target.endswith(".js"):
                    visit_js(target)
                else:
                    raise ValueError(f"unsupported module import in {path}: {ref.specifier}")
            if css_imports:
                source = CSS_IMPORT.sub("/* Stylesheet supplied by monaco.css. */\n", source)
                changes.append("move side-effect CSS imports to the linked stylesheet")
            prepared[path] = source.encode("utf-8")
            if changes:
                patches.append({"path": path, "upstream_sha256": digest(original), "changes": changes})
            visiting.remove(path)

        for stylesheet in STYLESHEETS:
            visit_css(stylesheet)
        for entry in ENTRYPOINTS:
            visit_js(entry)
        prepared["monaco.css"] = ("/* Generated by scripts/vendor_monaco.py. */\n" +
            "".join(f'@import url("./{path}");\n' for path in css_order)).encode()
        prepared["vs/editor/editor.api.d.ts"] = upstream("vs/editor/editor.api.d.ts")
        provenance = {"version": VERSION, "archive": ARCHIVE_URL,
                      "archive_sha256": ARCHIVE_SHA256, "entrypoints": ENTRYPOINTS,
                      "stylesheets": STYLESHEETS,
                      "preparation": "scripts/vendor_monaco.py (Python standard library only)",
                      "patches": sorted(patches, key=lambda p: p["path"])}
        prepared["provenance.json"] = (json.dumps(provenance, indent=2) + "\n").encode()
        result = {"monaco/" + path: data for path, data in prepared.items()}
        license_member = package.getmember("package/LICENSE")
        if not license_member.isfile():
            raise ValueError("Monaco license is not a regular archive member")
        result["licenses/MONACO-MIT.txt"] = package.extractfile(license_member).read()
        return result


def install(files: dict[str, bytes], check: bool, refresh_verified: bool = False):
    v = verifier()
    v.walk_regular(LIB)
    if refresh_verified:
        v.check_vendor()
    manifest_path = LIB / "vendor-manifest.json"
    manifest = json.loads(manifest_path.read_text())
    old = next((d for d in manifest["dependencies"] if d["name"] == "Monaco Editor"), None)
    existing = {p.relative_to(LIB).as_posix() for p in (LIB / "monaco").rglob("*") if p.is_file()}
    expected = {name for name in files if name.startswith("monaco/")}
    if existing - expected:
        raise ValueError("unexpected existing Monaco files; inspect them before refreshing")
    if old and {f["path"] for f in old["files"]} != set(files) and not refresh_verified:
        raise ValueError("Monaco selection changed; review the old manifest before replacing it")
    recorded = {f["path"] for f in old["files"]} if old else set()
    for name, data in sorted(files.items()):
        path = LIB / name
        if path.exists():
            if path.is_symlink():
                raise ValueError(f"refusing to overwrite symlink: {name}")
            if path.read_bytes() != data:
                if not refresh_verified or name not in recorded:
                    raise ValueError(f"refusing to overwrite changed vendored file: {name}")
                path.write_bytes(data)
        elif check:
            raise ValueError(f"missing vendored file: {name}")
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            path.chmod(0o644)
    dependency = {
        "name": "Monaco Editor", "version": VERSION, "license": "MIT",
        "source_archive": ARCHIVE_URL, "source_archive_sha256": ARCHIVE_SHA256,
        "upstream_integrity": ARCHIVE_INTEGRITY,
        "selection": "browser-native ESM editor/basic languages/find/worker graph; Python-prepared CSS and explicit offline loader guards",
        "files": [{"path": name, "sha256": digest(data), "size_bytes": len(data)} for name, data in sorted(files.items())],
    }
    if check:
        if old != dependency:
            raise ValueError("Monaco manifest does not match reproducible preparation")
        v.check_vendor()
        print(f"Monaco {VERSION}: {len(files)} directly vendored files match the pinned archive/preparation")
        return
    manifest["dependencies"] = [d for d in manifest["dependencies"] if d["name"] != "Monaco Editor"] + [dependency]
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    records = {f["path"]: f["sha256"] for d in manifest["dependencies"] for f in d["files"]}
    (LIB / "SHA256SUMS").write_text("".join(f"{records[path]}  {path}\n" for path in sorted(records)))
    asset_path = ROOT / "web/static/build-support/dist-assets.json"
    assets = json.loads(asset_path.read_text())
    selected = [name for name in files if not name.endswith(".d.ts") and name != "monaco/provenance.json"]
    assets["vendor_assets"] = sorted(set(assets["vendor_assets"]) | set(selected))
    asset_path.write_text(json.dumps(assets, indent=2) + "\n")
    v.check_vendor()
    print(f"Vendored Monaco {VERSION}: {len(files)} files, {sum(map(len, files.values()))} bytes; no Node or bundler")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--archive", type=Path)
    source.add_argument("--fetch", action="store_true", help="explicitly download the pinned HTTPS archive")
    parser.add_argument("--check", action="store_true", help="compare prepared bytes without writing")
    parser.add_argument("--refresh-verified", action="store_true", help="explicitly refresh a reviewed selection after verifying every existing manifest hash")
    args = parser.parse_args()
    if args.check and args.refresh_verified:
        parser.error("--check conflicts with --refresh-verified")
    if args.fetch:
        with urllib.request.urlopen(ARCHIVE_URL, timeout=60) as response:
            archive = response.read(64 * 1024 * 1024 + 1)
    else:
        if args.archive.stat().st_size > 64 * 1024 * 1024:
            raise ValueError("archive exceeds the acquisition bound")
        archive = args.archive.read_bytes()
    if len(archive) > 64 * 1024 * 1024:
        raise ValueError("archive exceeds the acquisition bound")
    install(prepare(archive), args.check, args.refresh_verified)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError) as error:
        raise SystemExit(f"vendor_monaco: {error}")
