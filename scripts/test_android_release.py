#!/usr/bin/env python3
"""Regression checks for release APK validation and publication boundaries."""
import base64
from contextlib import chdir
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import android_release
import package_native_release


class AndroidReleaseTests(unittest.TestCase):
    def test_debuggable_manifest_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "debuggable"):
            android_release.check_manifest("package: name='app'\napplication-debuggable\n")
        android_release.check_manifest("package: name='app'\napplication: label='Lethetic'\n")
        with self.assertRaisesRegex(ValueError, "manifest"):
            android_release.check_manifest("")

    def test_debug_certificate_and_unverified_signature_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "debug signing"):
            android_release.check_certificate("Signer #1 certificate DN: CN=Android Debug\ncertificate SHA-256 digest: abc")
        with self.assertRaisesRegex(ValueError, "verified signer"):
            android_release.check_certificate("")
        android_release.check_certificate("Signer #1 certificate DN: CN=Lethetic\nSigner #1 certificate SHA-256 digest: abc")

    def test_missing_credentials_fail_without_echoing_values(self):
        with patch.dict(os.environ, {"ANDROID_KEYSTORE_PASSWORD": "secret-value"}, clear=True):
            with self.assertRaises(ValueError) as error:
                android_release.signing_key()
            self.assertIn("ANDROID_KEYSTORE_BASE64", str(error.exception))
            self.assertNotIn("secret-value", str(error.exception))

    def test_keystore_decode(self):
        values = dict.fromkeys(android_release.SIGNING_VARIABLES, "test")
        values["ANDROID_KEYSTORE_BASE64"] = base64.b64encode(b"keystore").decode() + "\n"
        with patch.dict(os.environ, values, clear=True):
            self.assertEqual(android_release.signing_key(), b"keystore")

    def test_tag_updates_native_version_without_dropping_config(self):
        with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
            android_release.CONFIG.parent.mkdir(parents=True)
            android_release.CONFIG.write_text(json.dumps({"version": "0.1.0", "identifier": "unchanged"}))
            android_release.set_version("browser-v0.9.0")
            self.assertEqual(json.loads(android_release.CONFIG.read_text()), {"version": "0.9.0", "identifier": "unchanged"})
        for bad in ("master", "v1.2", "v01.2.3", "../1.2.3"):
            with self.assertRaises(ValueError):
                android_release.release_version(bad)

    def test_publication_rejects_debug_or_unsigned_apks(self):
        for name in ("app-debug.apk", "com.TechHurts.Lethetic_v0.9.0-unsigned.apk"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                self.installers(root, name)
                with self.assertRaisesRegex(ValueError, "release-signed"):
                    package_native_release.package(root, root / "out")
                self.assertFalse((root / "out").exists())

    def test_complete_release_preserves_name_and_checksums(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.installers(root, "com.TechHurts.Lethetic_v0.9.0.apk")
            files = package_native_release.package(root, root / "out")
            self.assertEqual(len(files), 5)
            self.assertIn("com.TechHurts.Lethetic_v0.9.0.apk", [p.name for p in files])
            for file in files:
                expected = hashlib.sha256(file.read_bytes()).hexdigest() + "  " + file.name + "\n"
                self.assertEqual(file.with_name(file.name + ".sha256").read_text(), expected)

    def test_signing_failure_never_stages_an_apk_and_cleans_keystore(self):
        values = dict.fromkeys(android_release.SIGNING_VARIABLES, "test")
        values.update(ANDROID_HOME="/sdk", ANDROID_KEYSTORE_BASE64=base64.b64encode(b"key").decode())
        temporary_keys = []

        def run(command, **_):
            if command[1] == "sign":
                keystore = command[command.index("--ks") + 1]
                self.assertEqual(keystore.read_bytes(), b"key")
                self.assertEqual(keystore.stat().st_mode & 0o777, 0o600)
                temporary_keys.append(keystore)
                raise RuntimeError("Signing failed")

        with tempfile.TemporaryDirectory() as tmp, chdir(tmp), patch.dict(os.environ, values, clear=True):
            apk = Path("src-tauri/gen/android/app/build/outputs/apk/arm64/release/app-release-unsigned.apk")
            apk.parent.mkdir(parents=True)
            apk.write_bytes(b"input")
            android_release.CONFIG.write_text('{"version":"0.9.0"}')
            with patch.object(android_release.subprocess, "check_output", return_value="package: name='app'"), patch.object(android_release.subprocess, "run", side_effect=run):
                with self.assertRaisesRegex(RuntimeError, "Signing failed"):
                    android_release.package(True)
            self.assertFalse(list(Path("android-release").glob("*.apk")))
            self.assertEqual(len(temporary_keys), 1)
            self.assertFalse(temporary_keys[0].exists())

    @staticmethod
    def installers(root, apk):
        for artifact, extensions in package_native_release.INSTALLERS.items():
            directory = root / artifact / "nested"
            directory.mkdir(parents=True)
            for extension in extensions:
                (directory / (apk if extension == ".apk" else "installer" + extension)).write_bytes(b"fixture")


if __name__ == "__main__":
    unittest.main()
