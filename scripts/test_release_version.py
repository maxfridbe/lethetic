#!/usr/bin/env python3
"""Calendar/version consistency checks across all release targets."""
from datetime import date
import json
import os
from pathlib import Path
import shutil
import tempfile
import tarfile
import unittest
from unittest.mock import patch
import zipfile

import package_browser
import package_cli_release
import release_version as release


class ReleaseVersionTests(unittest.TestCase):
    def test_calendar_validation(self):
        for value in ("26.1024.01", "28.0229.99", "26.0102.01"):
            self.assertEqual(release.Version(value).text, value)
        for value in ("0.8.0", "26.1024.1", "26.1024.00", "26.0230.01", "26.1301.01", "26.1024.100"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                release.Version(value)

    def test_platform_versions_and_names(self):
        version = release.Version("26.0102.03")
        self.assertEqual(version.semver, "26.102.3")
        self.assertEqual(version.android_code, 26010203)
        self.assertEqual(version.macos_build, "2601.2.3")
        self.assertEqual(version.asset("android", ".apk"), "com.TechHurts.Lethetic_v26.0102.03.apk")
        self.assertEqual(version.asset("web-wasm", ".zip"), "lethetic-web-wasm_v26.0102.03.zip")

    def test_daily_sequence_and_rollover(self):
        version = release.Version("26.1024.01")
        self.assertEqual(release.next_version(version, date(2026, 10, 24)).text, "26.1024.02")
        self.assertEqual(release.next_version(version, date(2026, 10, 25)).text, "26.1025.01")
        self.assertEqual(release.next_version(release.Version("26.1231.99"), date(2027, 1, 1)).text, "27.0101.01")
        for old, today in (("26.1024.99", date(2026, 10, 24)), ("26.1024.01", date(2026, 10, 23))):
            with self.assertRaises(ValueError):
                release.next_version(release.Version(old), today)

    def test_tag_cannot_override_central_version(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "VERSION").write_text("26.1024.01\n")
            with patch.dict(os.environ, {"GITHUB_REF": "refs/tags/v26.1024.01"}, clear=True):
                self.assertEqual(release.current(root).tag, "v26.1024.01")
            for value in ("refs/tags/browser-v0.8.0", "refs/tags/v26.1024.02"):
                with patch.dict(os.environ, {"GITHUB_REF": value}, clear=True), self.assertRaises(ValueError):
                    release.current(root)

    def test_sync_all_packages_and_locks_and_detect_drift(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            files = [*release.MANIFESTS, "Cargo.lock", "native/Cargo.lock", "native/src-tauri/tauri.conf.json"]
            for relative in files:
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(release.ROOT / relative, target)
            version = release.Version("27.0102.02")
            release.sync(root, version, check=False)
            release.sync(root, version, check=True)
            for relative in release.MANIFESTS:
                self.assertEqual(release.tomllib.loads((root / relative).read_text())["package"]["version"], "27.102.2")
            config = json.loads((root / "native/src-tauri/tauri.conf.json").read_text())
            self.assertEqual(config["bundle"]["android"]["versionCode"], 27010202)
            self.assertEqual(config["bundle"]["macOS"]["bundleVersion"], "2701.2.2")
            path = root / "lethetic-wasm/Cargo.toml"
            path.write_text(path.read_text().replace('version = "27.102.2"', 'version = "0.1.0"', 1))
            with self.assertRaisesRegex(ValueError, "differ from VERSION"):
                release.sync(root, version, check=True)

    def test_web_and_cli_packages_share_version_and_named_archives(self):
        version = release.Version("26.0102.03")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "VERSION").write_text(version.text + "\n")
            stage = root / "target/browser"
            (stage / "browser/src").mkdir(parents=True)
            (stage / "browser/src/sdk.js").write_text("export const example = 1;\n")
            (stage / "integration.md").write_text("Example guide")
            (stage / "index.html").write_text('<span id="splash-version" aria-label="App version">v{{LETHETIC_VERSION}}</span>')
            with patch.object(package_browser, "ROOT", root), patch.object(package_browser, "current", return_value=version):
                package_browser.package(stage)
            self.assertEqual(json.loads((stage / "manifest.json").read_text())["version"], version.text)
            self.assertEqual(json.loads((stage / "package.json").read_text())["version"], version.semver)
            with zipfile.ZipFile(root / "target" / version.asset("web-wasm", ".zip")) as archive:
                self.assertEqual(archive.read("VERSION").decode().strip(), version.text)
                self.assertIn(">v" + version.text + "</span>", archive.read("index.html").decode())
            (root / "target/release").mkdir()
            for binary in ("lethetic", "lethetic-runtime"):
                (root / "target/release" / binary).write_bytes(b"fixture")
            with patch.object(package_cli_release, "ROOT", root), patch.object(package_cli_release, "current", return_value=version):
                archive_path = package_cli_release.package()
            self.assertEqual(archive_path.name, "lethetic-cli-linux-x86_64_v26.0102.03.tar.gz")
            with tarfile.open(archive_path) as archive:
                self.assertEqual(set(archive.getnames()), {"lethetic", "lethetic-runtime", "VERSION"})


if __name__ == "__main__":
    unittest.main()
