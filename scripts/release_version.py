#!/usr/bin/env python3
"""One calendar version for every Lethetic distribution; no third-party packages."""
from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import date, datetime, timezone
import json
import os
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]
MANIFESTS = (
    "Cargo.toml", "lethetic-wasm/Cargo.toml", "gemma-chat/Cargo.toml",
    "native/src-tauri/Cargo.toml", "native/host/Cargo.toml", "native/workspace/Cargo.toml",
    "native/network/Cargo.toml",
)


@dataclass(frozen=True)
class Version:
    text: str

    def __post_init__(self):
        if not re.fullmatch(r"[0-9]{2}\.[0-9]{4}\.[0-9]{2}", self.text):
            raise ValueError("Version must be yy.mmdd.## (for example 26.1024.01)")
        year, month_day, sequence = self.text.split(".")
        date(2000 + int(year), int(month_day[:2]), int(month_day[2:]))
        if int(sequence) == 0:
            raise ValueError("Daily release sequence starts at 01")

    @property
    def semver(self) -> str:
        return ".".join(str(int(part)) for part in self.text.split("."))

    @property
    def tag(self) -> str:
        return "v" + self.text

    @property
    def android_code(self) -> int:
        return int(self.text.replace(".", ""))

    @property
    def macos_build(self) -> str:
        year, month_day, sequence = self.text.split(".")
        return f"{int(year + month_day[:2])}.{int(month_day[2:])}.{int(sequence)}"

    def asset(self, platform: str, extension: str) -> str:
        if platform == "android":
            return f"com.TechHurts.Lethetic_v{self.text}.apk"
        return f"lethetic-{platform}_v{self.text}{extension}"


def current(root: Path = ROOT) -> Version:
    version = Version((root / "VERSION").read_text().strip())
    ref = os.environ.get("GITHUB_REF", "")
    if ref.startswith("refs/tags/") and ref != "refs/tags/" + version.tag:
        raise ValueError(f"Tag {ref} does not match VERSION ({version.tag})")
    override = os.environ.get("LETHETIC_RELEASE_VERSION")
    if override and override not in (version.text, version.tag):
        raise ValueError("LETHETIC_RELEASE_VERSION does not match VERSION")
    return version


def next_version(version: Version, today: date) -> Version:
    prefix = today.strftime("%y.%m%d")
    if not 2000 <= today.year <= 2099:
        raise ValueError("Calendar versions support years 2000–2099")
    if version.text[:7] > prefix:
        raise ValueError("VERSION is ahead of today's UTC date")
    sequence = int(version.text[-2:]) + 1 if version.text[:7] == prefix else 1
    if sequence > 99:
        raise ValueError("Daily release sequence exhausted (99)")
    return Version(f"{prefix}.{sequence:02}")


def generated_files(root: Path, version: Version) -> dict[Path, str]:
    result = {}
    names = set()
    for relative in MANIFESTS:
        path = root / relative
        source = path.read_text()
        names.add(tomllib.loads(source)["package"]["name"])
        source, count = re.subn(r'(\[package\][\s\S]*?\nversion = )"[^"]+"',
                                lambda match: match[1] + json.dumps(version.semver), source, count=1)
        if count != 1:
            raise ValueError(f"Missing package version in {relative}")
        result[path] = source
    for relative in ("Cargo.lock", "native/Cargo.lock"):
        path = root / relative
        def replace(match):
            return match[1] + json.dumps(version.semver) if match[2] in names else match[0]
        result[path] = re.sub(r'(\[\[package\]\]\nname = "([^"]+)"\nversion = )"[^"]+"', replace, path.read_text())
    path = root / "native/src-tauri/tauri.conf.json"
    config = json.loads(path.read_text())
    config["version"] = version.semver
    config["bundle"].setdefault("android", {})["versionCode"] = version.android_code
    config["bundle"].setdefault("macOS", {})["bundleVersion"] = version.macos_build
    result[path] = json.dumps(config, indent=2) + "\n"
    return result


def sync(root: Path, version: Version, check: bool) -> None:
    files = generated_files(root, version)
    stale = [str(path.relative_to(root)) for path, expected in files.items() if path.read_text() != expected]
    if check and stale:
        raise ValueError("Generated versions differ from VERSION; run scripts/release_version.py --sync: " + ", ".join(stale))
    if not check:
        for path, expected in files.items():
            path.write_text(expected)
        (root / "VERSION").write_text(version.text + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--set", metavar="YY.MMDD.NN")
    mode.add_argument("--next", action="store_true", help="Advance today's UTC release counter")
    mode.add_argument("--sync", action="store_true", help="Regenerate package metadata from VERSION")
    mode.add_argument("--check", action="store_true", help="Reject drift in generated package metadata")
    args = parser.parse_args()
    version = Version(args.set) if args.set else current()
    if args.next:
        version = next_version(version, datetime.now(timezone.utc).date())
    if args.set or args.next or args.sync or args.check:
        sync(ROOT, version, check=args.check)
    print(version.text)
