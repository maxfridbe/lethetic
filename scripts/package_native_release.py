#!/usr/bin/env python3
"""Collect the five native installers from Actions artifacts for a tagged release."""
from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import shutil

INSTALLERS = {
    "lethetic-Windows": (".exe",),
    "lethetic-macOS": (".dmg",),
    "lethetic-Linux": (".deb", ".AppImage"),
    "lethetic-Android": (".apk",),
}


def package(source: Path, destination: Path) -> list[Path]:
    selected: list[tuple[Path, str]] = []
    for artifact, extensions in INSTALLERS.items():
        for extension in extensions:
            files = sorted((source / artifact).rglob(f"*{extension}"))
            if len(files) != 1 or not files[0].is_file() or files[0].stat().st_size == 0:
                raise ValueError(f"Expected one nonempty {extension} installer in {artifact}; found {len(files)}")
            # Tauri desktop filenames already include their version/architecture.
            name = "lethetic-android-arm64-debug.apk" if extension == ".apk" else files[0].name
            selected.append((files[0], name))
    if len({name for _, name in selected}) != len(selected):
        raise ValueError("Native installer filenames collide")
    destination.mkdir(parents=True, exist_ok=True)
    if any(destination.iterdir()):
        raise ValueError("Release output directory must be empty")
    result = []
    for source_file, name in selected:
        output = destination / name
        shutil.copyfile(source_file, output)
        with output.open("rb") as stream:
            checksum = hashlib.file_digest(stream, "sha256").hexdigest()
        output.with_name(name + ".sha256").write_text(f"{checksum}  {name}\n", encoding="utf-8")
        result.append(output)
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    for installer in package(args.source, args.destination):
        print(installer.name)
