#!/usr/bin/env python3
"""Fail when an authored source or test file exceeds the physical-line limit."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import os
from pathlib import Path, PurePosixPath
import sys
from typing import Iterable

MAX_LINES = 2_000
WARN_LINES = 1_800

SOURCE_SUFFIXES = {
    ".bash",
    ".c",
    ".cc",
    ".cjs",
    ".cpp",
    ".cs",
    ".css",
    ".go",
    ".h",
    ".hpp",
    ".htm",
    ".html",
    ".java",
    ".js",
    ".json",
    ".jsx",
    ".kt",
    ".kts",
    ".mjs",
    ".php",
    ".proto",
    ".py",
    ".pyi",
    ".rb",
    ".rs",
    ".sh",
    ".sql",
    ".swift",
    ".toml",
    ".ts",
    ".tsx",
    ".yaml",
    ".yml",
}
SOURCE_NAMES = {"Makefile"}
SOURCE_PREFIXES = ("Containerfile", "Dockerfile")
LOCKFILE_NAMES = {
    "Cargo.lock",
    "bun.lock",
    "bun.lockb",
    "deno.lock",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
}
EXCLUDED_DIRECTORY_NAMES = {
    ".git",
    ".claude",
    ".lethetic",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".venv",
    "__pycache__",
    "node_modules",
    "target",
    "vendor",
}
EXCLUDED_PREFIXES = {
    ("web", "dist"),
    ("web", "lib"),
    ("web", "src", "generated"),
}


@dataclass(frozen=True, order=True)
class SourceLineCount:
    relative_path: str
    lines: int


def physical_line_count(data: bytes) -> int:
    if not data:
        return 0
    return data.count(b"\n") + (0 if data.endswith(b"\n") else 1)


def is_source_file(path: Path) -> bool:
    return (
        path.name in SOURCE_NAMES
        or path.name.startswith(SOURCE_PREFIXES)
        or path.suffix.lower() in SOURCE_SUFFIXES
    ) and path.name not in LOCKFILE_NAMES


def is_excluded_relative(relative: PurePosixPath) -> bool:
    parts = relative.parts
    if any(part in EXCLUDED_DIRECTORY_NAMES for part in parts[:-1]):
        return True
    return any(parts[: len(prefix)] == prefix for prefix in EXCLUDED_PREFIXES)


def _raise_walk_error(error: OSError) -> None:
    raise error


def authored_sources(root: Path) -> Iterable[Path]:
    root = root.resolve(strict=True)
    for directory, names, files in os.walk(
        root,
        followlinks=False,
        onerror=_raise_walk_error,
    ):
        directory_path = Path(directory)
        relative_directory = directory_path.relative_to(root)
        names[:] = sorted(
            name
            for name in names
            if name not in EXCLUDED_DIRECTORY_NAMES
            and not (directory_path / name).is_symlink()
            and not is_excluded_relative(
                PurePosixPath(*(relative_directory / name).parts)
            )
        )
        for name in sorted(files):
            path = directory_path / name
            relative = PurePosixPath(*path.relative_to(root).parts)
            if (
                path.is_symlink()
                or is_excluded_relative(relative)
                or not is_source_file(path)
            ):
                continue
            yield path


def scan_sources(root: Path) -> list[SourceLineCount]:
    resolved = root.resolve(strict=True)
    counts = []
    for path in authored_sources(resolved):
        relative = path.relative_to(resolved).as_posix()
        try:
            data = path.read_bytes()
            data.decode("utf-8")
        except (OSError, UnicodeError) as error:
            raise RuntimeError(f"could not read authored source {relative}: {error}") from error
        counts.append(SourceLineCount(relative, physical_line_count(data)))
    return sorted(counts)


def parse_args(arguments: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="repository root (default: parent of scripts/)",
    )
    parser.add_argument("--max-lines", type=int, default=MAX_LINES)
    parser.add_argument("--warn-lines", type=int, default=WARN_LINES)
    args = parser.parse_args(arguments)
    if args.max_lines < 1:
        parser.error("--max-lines must be positive")
    if args.warn_lines < 1 or args.warn_lines > args.max_lines:
        parser.error("--warn-lines must be positive and no greater than --max-lines")
    return args


def main(arguments: list[str]) -> int:
    args = parse_args(arguments)
    try:
        counts = scan_sources(args.root)
    except (OSError, RuntimeError) as error:
        print(f"check_source_lines.py: {error}", file=sys.stderr)
        return 2

    oversized = [item for item in counts if item.lines > args.max_lines]
    warnings = [
        item
        for item in counts
        if args.warn_lines <= item.lines <= args.max_lines
    ]
    for item in warnings:
        print(
            f"source-line warning: {item.relative_path}: {item.lines} lines "
            f"(warning threshold {args.warn_lines})",
            file=sys.stderr,
        )
    for item in oversized:
        print(
            f"source-line limit exceeded: {item.relative_path}: {item.lines} lines "
            f"(maximum {args.max_lines})",
            file=sys.stderr,
        )
    if oversized:
        return 1
    print(
        f"source-line check passed ({len(counts)} authored files; "
        f"maximum {args.max_lines})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
