#!/usr/bin/env python3
"""Exercise clean embedded builds and failed-compiler recovery without Cargo recursion."""

import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import build_web_assets


class EmbeddedWebBuildTests(unittest.TestCase):
    def test_clean_build_is_deterministic_and_failure_keeps_previous_assets(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            build_web_assets.build(output)
            manifest = output / "web-assets.rs"
            self.assertIn('"index.html"', manifest.read_text())
            self.assertIn('"src/app.js"', manifest.read_text())
            self.assertTrue((output / "web-dist/lib/monaco/vs/editor/editor.api.js").is_file())
            self.assertFalse(list(output.rglob("*.ts")))

            def hashes():
                return {str(path.relative_to(output)): hashlib.sha256(path.read_bytes()).hexdigest()
                        for path in output.rglob("*") if path.is_file()}

            first = hashes()
            with patch.object(build_web_assets.subprocess, "run",
                              side_effect=subprocess.CalledProcessError(2, "tsc")):
                with self.assertRaises(subprocess.CalledProcessError):
                    build_web_assets.build(output)
            self.assertEqual(hashes(), first)
            build_web_assets.build(output)
            self.assertEqual(hashes(), first)


if __name__ == "__main__":
    unittest.main()
