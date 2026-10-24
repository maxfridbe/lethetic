#!/usr/bin/env python3
"""Validate release APKs and optionally sign them with the persistent CI key.

Run from native/. Signing secrets are read only from environment variables.
"""
from __future__ import annotations

import argparse
import base64
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import zipfile

from release_version import current

SIGNING_VARIABLES = (
    "ANDROID_KEYSTORE_BASE64", "ANDROID_KEYSTORE_PASSWORD",
    "ANDROID_KEY_ALIAS", "ANDROID_KEY_PASSWORD",
)


def signing_key() -> bytes:
    missing = [name for name in SIGNING_VARIABLES if not os.environ.get(name)]
    if missing:
        raise ValueError("Release signing requires GitHub Actions secrets: " + ", ".join(missing))
    key = base64.b64decode("".join(os.environ["ANDROID_KEYSTORE_BASE64"].split()), validate=True)
    if not key:
        raise ValueError("Android signing keystore is empty")
    return key


def check_manifest(badging: str) -> None:
    if not badging.startswith("package:"):
        raise ValueError("Could not inspect APK manifest")
    if "application-debuggable" in badging:
        raise ValueError("Refusing a debuggable APK")


def check_certificate(certificates: str) -> None:
    if "Android Debug" in certificates:
        raise ValueError("Refusing an Android debug signing certificate")
    if "certificate SHA-256 digest:" not in certificates:
        raise ValueError("APK has no verified signer certificate")


def check_icons(apk: Path) -> None:
    # aapt renames resources in optimized APKs. Inspect compiled XML and PNG
    # dimensions rather than source filenames, which do not survive packaging.
    adaptive = False
    sizes = set()
    with zipfile.ZipFile(apk) as archive:
        for name in archive.namelist():
            if not name.startswith("res/"):
                continue
            if name.endswith(".xml"):
                data = archive.read(name)
                adaptive |= any(all(tag.encode(encoding) in data for tag in ("adaptive-icon", "monochrome"))
                                for encoding in ("utf-8", "utf-16-le"))
            elif name.endswith(".png"):
                with archive.open(name) as image:
                    header = image.read(24)
                if header[:8] == b"\x89PNG\r\n\x1a\n" and header[12:16] == b"IHDR":
                    width, height = int.from_bytes(header[16:20], "big"), int.from_bytes(header[20:24], "big")
                    if width == height:
                        sizes.add(width)
    if not adaptive or not {108, 162, 216, 324, 432}.issubset(sizes):
        raise ValueError("APK is missing Lethetic adaptive/themed icons; copy src-tauri/icons/android into the generated Android res directory after init")


def package(sign: bool) -> Path:
    key_bytes = signing_key() if sign else None
    root = Path("src-tauri/gen/android/app/build/outputs/apk")
    apks = sorted(p for p in root.rglob("*.apk") if "release" in p.relative_to(root).parts)
    if len(apks) != 1:
        raise ValueError(f"Expected one release APK; found {len(apks)}")
    sdk = Path(os.environ["ANDROID_HOME"]) / "build-tools" / "36.0.0"
    badging = subprocess.check_output([sdk / "aapt", "dump", "badging", apks[0]], text=True)
    check_manifest(badging)
    check_icons(apks[0])
    destination = Path("android-release")
    destination.mkdir(exist_ok=True)
    if any(destination.iterdir()):
        raise ValueError("Android release output directory must be empty")
    name = current().asset("android", ".apk")
    if not sign:
        name = name.removesuffix(".apk") + "-unsigned.apk"
    output = destination / name
    # Keep keystore and intermediate APK outside artifacts/caches, including on failure.
    with tempfile.TemporaryDirectory(prefix="lethetic-android-") as temporary:
        temporary = Path(temporary)
        aligned = temporary / "aligned.apk"
        subprocess.run([sdk / "zipalign", "-P", "16", "-f", "4", apks[0], aligned], check=True)
        if sign:
            keystore = temporary / "release.jks"
            keystore.write_bytes(key_bytes)
            keystore.chmod(0o600)
            subprocess.run([
                sdk / "apksigner", "sign", "--ks", keystore,
                "--ks-key-alias", os.environ["ANDROID_KEY_ALIAS"],
                "--ks-pass", "env:ANDROID_KEYSTORE_PASSWORD",
                "--key-pass", "env:ANDROID_KEY_PASSWORD",
                "--v4-signing-enabled", "false", aligned,
            ], check=True)
            certificates = subprocess.check_output([
                sdk / "apksigner", "verify", "--verbose", "--print-certs", aligned,
            ], text=True)
            check_certificate(certificates)
        subprocess.run([sdk / "zipalign", "-c", "-P", "16", "4", aligned], check=True)
        shutil.copyfile(aligned, output)
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sign", action="store_true", help="Require release credentials and sign")
    args = parser.parse_args()
    print(package(args.sign))
