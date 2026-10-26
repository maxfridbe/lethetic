#!/usr/bin/env python3
"""Build the embedded remote UI in Cargo's OUT_DIR, without invoking Cargo."""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
from generate_web_asset_manifest import generated

ROOT = Path(__file__).resolve().parents[1]


def build(output: Path) -> None:
    compiler = shutil.which("tsc")
    if not compiler:
        raise ValueError("install TypeScript 7.0.2 globally: npm install --global typescript@7.0.2")
    version = subprocess.check_output([compiler, "--version"], text=True).strip()
    if version != "Version 7.0.2":
        raise ValueError(f"TypeScript 7.0.2 is required; found {version}")
    if Path(compiler).resolve().is_relative_to(ROOT):
        raise ValueError("use a global TypeScript compiler, not a project-local installation")

    spec = importlib.util.spec_from_file_location(
        "verify_web_assets", ROOT / "web/static/build-support/verify-web-assets.py"
    )
    assert spec is not None and spec.loader is not None
    verifier = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = verifier
    spec.loader.exec_module(verifier)
    verifier.validate(require_application=True)

    output.mkdir(parents=True, exist_ok=True)
    destination = output / "web-dist"
    manifest = output / "web-assets.rs"
    if destination.is_symlink() or manifest.is_symlink():
        raise ValueError("refusing symlink build outputs")
    # Publish only after compilation, allowlist, hashes, and imports validate.
    with tempfile.TemporaryDirectory(prefix=".web-build-", dir=output) as temporary:
        stage = Path(temporary) / "dist"
        stage.mkdir()
        subprocess.run([compiler, "--project", str(ROOT / "web/tsconfig.json"),
                        "--outDir", str(stage), "--pretty", "false"], check=True)
        verifier.copy_assets(stage)
        verifier.check_dist(stage)
        contents = generated(stage)
        if destination.exists():
            shutil.rmtree(destination)
        stage.replace(destination)
        manifest.write_bytes(contents)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    build(parser.parse_args().output)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"build_web_assets.py: {error}") from error
