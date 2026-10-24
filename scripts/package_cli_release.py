#!/usr/bin/env python3
"""Package the Linux CLI and isolated-runtime helper under the central version."""
import hashlib
from pathlib import Path
import tarfile

from release_version import ROOT, current


def package() -> Path:
    version = current()
    destination = ROOT / "target/cli-release"
    destination.mkdir(exist_ok=True)
    archive = destination / version.asset("cli-linux-x86_64", ".tar.gz")
    binaries = [ROOT / "target/release" / name for name in ("lethetic", "lethetic-runtime")]
    if not all(path.is_file() for path in binaries):
        raise ValueError("Build both lethetic and lethetic-runtime before packaging")
    with tarfile.open(archive, "w:gz") as output:
        for path in [*binaries, ROOT / "VERSION"]:
            output.add(path, arcname=path.name)
    with archive.open("rb") as stream:
        checksum = hashlib.file_digest(stream, "sha256").hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(f"{checksum}  {archive.name}\n")
    return archive


if __name__ == "__main__":
    print(package())
