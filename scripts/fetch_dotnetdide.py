#!/usr/bin/env python3
"""Install the checksum-pinned GitHub runtime release into a staged browser build."""
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import sys
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def install(stage: Path) -> None:
    pin = json.loads((ROOT / "browser/dotnetdide-release.json").read_text())
    version = pin["version"]
    filename = f"dotnetdide-{version}.zip"
    cache = ROOT / "target/runtime-cache" / filename
    cache.parent.mkdir(parents=True, exist_ok=True)
    valid = lambda: cache.is_file() and hashlib.sha256(cache.read_bytes()).hexdigest() == pin["sha256"]
    if not valid():
        url = f"https://github.com/maxfridbe/dotnetdide/releases/download/{version}/{filename}"
        temporary = cache.with_suffix(".download")
        try:
            with urllib.request.urlopen(url, timeout=120) as response, temporary.open("wb") as output:
                shutil.copyfileobj(response, output)
            if hashlib.sha256(temporary.read_bytes()).hexdigest() != pin["sha256"]:
                raise ValueError("dotnetdide release checksum mismatch")
            temporary.replace(cache)
        finally:
            temporary.unlink(missing_ok=True)
    destination = stage / "runtimes/dotnetdide"
    destination.mkdir(parents=True, exist_ok=True)
    selected = {"dotnetdide.js", "dotnetdide.worker.js", "network-policy.js", "llm.md", "LICENSE", "NOTICE", "release.json"}
    with zipfile.ZipFile(cache) as archive:
        metadata = json.loads(archive.read("release.json"))
        if metadata["version"] != version:
            raise ValueError("dotnetdide release version mismatch")
        for member in archive.infolist():
            path = PurePosixPath(member.filename)
            if path.is_absolute() or ".." in path.parts:
                raise ValueError("Unsafe release archive path")
            relative = member.filename.removeprefix("wwwroot/")
            if member.is_dir() or not (relative in selected or relative.startswith(("_framework/", "docs/runtime-"))):
                continue
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(archive.read(member))
    for required in selected | {"_framework/dotnet.js"}:
        if not (destination / required).is_file():
            raise ValueError(f"Release is missing {required}")
    (destination / "source.json").write_text(json.dumps(pin, indent=2) + "\n")
    print(f"Bundled dotnetdide {version} (verified SHA-256)")


if __name__ == "__main__":
    install(Path(sys.argv[1]))
