#!/usr/bin/env python3
"""Self-tests for check_source_lines.py."""

from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
from pathlib import Path
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("check_source_lines.py")
SPEC = importlib.util.spec_from_file_location("check_source_lines", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CHECKER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CHECKER
SPEC.loader.exec_module(CHECKER)


def write_lines(path: Path, count: int, *, final_newline: bool = True) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    ending = "\n" if final_newline else ""
    path.write_text(("line\n" * max(0, count - 1)) + ("line" + ending if count else ""))


class SourceLineCheckerTests(unittest.TestCase):
    def test_physical_line_count_handles_empty_and_unterminated_files(self) -> None:
        self.assertEqual(CHECKER.physical_line_count(b""), 0)
        self.assertEqual(CHECKER.physical_line_count(b"one\n"), 1)
        self.assertEqual(CHECKER.physical_line_count(b"one\ntwo"), 2)
        self.assertEqual(CHECKER.physical_line_count(b"\n\n"), 2)

    def test_scan_includes_untracked_sources_and_has_no_main_exemption(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_lines(root / "src" / "main.rs", 2_001)
            write_lines(root / "untracked" / "active.ts", 17, final_newline=False)
            write_lines(root / "ignore" / "still-authored.py", 19)
            counts = {
                item.relative_path: item.lines
                for item in CHECKER.scan_sources(root)
            }
        self.assertEqual(counts["src/main.rs"], 2_001)
        self.assertEqual(counts["untracked/active.ts"], 17)
        self.assertEqual(counts["ignore/still-authored.py"], 19)

    def test_scan_excludes_generated_vendor_build_runtime_and_lock_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            excluded = [
                ".git/hooks/check.py",
                ".claude/worktrees/example/src/lib.rs",
                ".lethetic/state.py",
                "target/debug/build.rs",
                "node_modules/pkg/index.js",
                "vendor/pkg/lib.rs",
                "web/dist/app.js",
                "web/lib/vendor.js",
                "web/src/generated/contracts.ts",
                "Cargo.lock",
                "package-lock.json",
            ]
            for relative in excluded:
                write_lines(root / relative, 2_100)
            write_lines(root / "src" / "lib.rs", 12)
            counts = CHECKER.scan_sources(root)
        self.assertEqual(counts, [CHECKER.SourceLineCount("src/lib.rs", 12)])

    def test_source_names_cover_extensionless_build_inputs(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_lines(root / "Makefile", 3)
            write_lines(root / "Containerfile.runtime", 4)
            write_lines(root / "Dockerfile", 5)
            write_lines(root / "README.md", 6)
            counts = {
                item.relative_path: item.lines
                for item in CHECKER.scan_sources(root)
            }
        self.assertEqual(
            counts,
            {"Containerfile.runtime": 4, "Dockerfile": 5, "Makefile": 3},
        )

    def test_main_warns_at_threshold_and_fails_only_above_maximum(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_lines(root / "warning.rs", 1_800)
            write_lines(root / "allowed.rs", 2_000)
            stdout = io.StringIO()
            stderr = io.StringIO()
            with redirect_stdout(stdout), redirect_stderr(stderr):
                result = CHECKER.main(["--root", str(root)])
            self.assertEqual(result, 0)
            self.assertIn("warning.rs: 1800 lines", stderr.getvalue())
            self.assertIn("allowed.rs: 2000 lines", stderr.getvalue())
            self.assertIn("source-line check passed", stdout.getvalue())

            write_lines(root / "too-large.rs", 2_001)
            stdout = io.StringIO()
            stderr = io.StringIO()
            with redirect_stdout(stdout), redirect_stderr(stderr):
                result = CHECKER.main(["--root", str(root)])
            self.assertEqual(result, 1)
            self.assertIn("too-large.rs: 2001 lines", stderr.getvalue())
            self.assertNotIn("source-line check passed", stdout.getvalue())

    def test_scan_rejects_non_utf8_authored_source(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "bad.rs"
            path.write_bytes(b"\xff\n")
            with self.assertRaisesRegex(RuntimeError, "could not read authored source bad.rs"):
                CHECKER.scan_sources(root)


if __name__ == "__main__":
    unittest.main()
