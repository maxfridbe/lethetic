#!/usr/bin/env python3
"""Dependency-free validation and asset copying for build-web.sh."""

from __future__ import annotations

import argparse
import hashlib
from html.parser import HTMLParser
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import sys
from typing import Callable, NamedTuple

SCRIPT = Path(__file__).absolute()
ROOT = SCRIPT.parents[3]
WEB = ROOT / "web"
LIB = WEB / "lib"
STATIC = WEB / "static"
SRC = WEB / "src"
CONFIG = WEB / "tsconfig.json"
ASSET_LIST = SCRIPT.with_name("dist-assets.json")
SUPPORT_FILES = {
    "check-pipeline.sh",
    "dist-assets.json",
    "verify-web-assets.py",
}
HEX_SHA256 = re.compile(r"[0-9a-f]{64}\Z")
REGEX_PREFIX_KEYWORDS = {
    "await",
    "case",
    "delete",
    "do",
    "else",
    "in",
    "instanceof",
    "new",
    "of",
    "return",
    "throw",
    "typeof",
    "void",
    "yield",
}
CONTROL_HEADER_KEYWORDS = {"catch", "for", "if", "switch", "while", "with"}
PUNCTUATORS = tuple(
    sorted(
        {
            "===",
            "!==",
            ">>>",
            "**=",
            "&&=",
            "||=",
            "??=",
            "=>",
            "==",
            "!=",
            "<=",
            ">=",
            "++",
            "--",
            "&&",
            "||",
            "??",
            "**",
            "<<",
            ">>",
            "+=",
            "-=",
            "*=",
            "/=",
            "%=",
            "&=",
            "|=",
            "^=",
            "?.",
            "...",
            ">>>=",
        },
        key=lambda item: (-len(item), item),
    )
)


class ValidationError(Exception):
    pass


def fail(message: str) -> None:
    raise ValidationError(message)


class JavaScriptToken(NamedTuple):
    kind: str
    value: str
    offset: int


class ModuleReference(NamedTuple):
    specifier: str
    offset: int
    dynamic: bool


class HTMLReference(NamedTuple):
    kind: str
    value: str
    line: int


class JavaScriptLexer:
    """Small lexical scanner for imports, not a JavaScript parser.

    It deliberately understands JavaScript lexical contexts instead of erasing
    comments with regular expressions. In particular, comment-looking text in
    strings, templates, and regular expressions cannot hide a later import.
    """

    def __init__(self, source: str, label: str) -> None:
        self.source = source
        self.label = label
        self.index = 0
        self.tokens: list[JavaScriptToken] = []

    def tokenize(self) -> list[JavaScriptToken]:
        if self.source.startswith("#!"):
            newline = self.source.find("\n")
            self.index = len(self.source) if newline < 0 else newline + 1
        self._scan_code(stop_on_closing_brace=False)
        return self.tokens

    def _error(self, message: str, offset: int | None = None) -> None:
        where = self.index if offset is None else offset
        line = self.source.count("\n", 0, where) + 1
        previous_newline = self.source.rfind("\n", 0, where)
        column = where - previous_newline
        fail(f"{message} in {self.label}:{line}:{column}")

    def _scan_string(self) -> tuple[str, int]:
        start = self.index
        quote = self.source[start]
        cursor = start + 1
        while cursor < len(self.source):
            character = self.source[cursor]
            if character == "\\":
                if cursor + 1 >= len(self.source):
                    self._error("unterminated JavaScript string literal", start)
                cursor += 2
                continue
            if character == quote:
                return self.source[start + 1 : cursor], cursor + 1
            if character in "\r\n  ":
                self._error("unterminated JavaScript string literal", start)
            cursor += 1
        self._error("unterminated JavaScript string literal", start)
        raise AssertionError("unreachable")

    def _scan_regular_expression(self) -> int | None:
        cursor = self.index + 1
        in_character_class = False
        while cursor < len(self.source):
            character = self.source[cursor]
            if character in "\r\n  ":
                return None
            if character == "\\":
                cursor += 2
                continue
            if character == "[":
                in_character_class = True
            elif character == "]":
                in_character_class = False
            elif character == "/" and not in_character_class:
                cursor += 1
                while cursor < len(self.source) and (
                    self.source[cursor].isalnum()
                    or self.source[cursor] in "_$"
                    or ord(self.source[cursor]) >= 128
                ):
                    cursor += 1
                return cursor
            cursor += 1
        return None

    def _scan_template(self) -> None:
        start = self.index
        self.index += 1
        while self.index < len(self.source):
            character = self.source[self.index]
            if character == "\\":
                if self.index + 1 >= len(self.source):
                    self._error("unterminated JavaScript template literal", start)
                self.index += 2
            elif character == "`":
                self.index += 1
                return
            elif character == "$" and self.source.startswith("${", self.index):
                self.index += 2
                self._scan_code(stop_on_closing_brace=True)
            else:
                self.index += 1
        self._error("unterminated JavaScript template literal", start)

    def _scan_code(self, stop_on_closing_brace: bool) -> None:
        can_start_regex = True
        brace_depth = 0
        parenthesized_controls: list[bool] = []
        pending_control = False

        while self.index < len(self.source):
            character = self.source[self.index]
            if character.isspace():
                self.index += 1
                continue

            if self.source.startswith("//", self.index):
                newline = self.source.find("\n", self.index + 2)
                self.index = len(self.source) if newline < 0 else newline + 1
                continue
            if self.source.startswith("/*", self.index):
                end = self.source.find("*/", self.index + 2)
                if end < 0:
                    self._error("unterminated JavaScript block comment")
                self.index = end + 2
                continue

            offset = self.index
            if character in "'\"":
                value, self.index = self._scan_string()
                self.tokens.append(JavaScriptToken("string", value, offset))
                can_start_regex = False
                pending_control = False
                continue

            if character == "`":
                self.tokens.append(JavaScriptToken("template", "", offset))
                self._scan_template()
                can_start_regex = False
                pending_control = False
                continue

            if character.isalpha() or character in "_$" or ord(character) >= 128:
                self.index += 1
                while self.index < len(self.source):
                    current = self.source[self.index]
                    if not (
                        current.isalnum()
                        or current in "_$"
                        or ord(current) >= 128
                    ):
                        break
                    self.index += 1
                value = self.source[offset : self.index]
                self.tokens.append(JavaScriptToken("identifier", value, offset))
                pending_control = value in CONTROL_HEADER_KEYWORDS
                can_start_regex = value in REGEX_PREFIX_KEYWORDS
                continue

            if character.isdigit() or (
                character == "."
                and self.index + 1 < len(self.source)
                and self.source[self.index + 1].isdigit()
            ):
                self.index += 1
                while self.index < len(self.source) and (
                    self.source[self.index].isalnum()
                    or self.source[self.index] in "._"
                ):
                    self.index += 1
                self.tokens.append(
                    JavaScriptToken("number", self.source[offset : self.index], offset)
                )
                can_start_regex = False
                pending_control = False
                continue

            if character == "/" and can_start_regex:
                end = self._scan_regular_expression()
                if end is not None:
                    self.tokens.append(
                        JavaScriptToken("regex", self.source[offset:end], offset)
                    )
                    self.index = end
                    can_start_regex = False
                    pending_control = False
                    continue

            punctuator = next(
                (
                    item
                    for item in PUNCTUATORS
                    if self.source.startswith(item, self.index)
                ),
                character,
            )
            self.tokens.append(JavaScriptToken("punctuator", punctuator, offset))
            self.index += len(punctuator)

            if punctuator == "{":
                brace_depth += 1
                can_start_regex = True
            elif punctuator == "}":
                if stop_on_closing_brace and brace_depth == 0:
                    return
                if brace_depth > 0:
                    brace_depth -= 1
                can_start_regex = False
            elif punctuator == "(":
                parenthesized_controls.append(pending_control)
                can_start_regex = True
            elif punctuator == ")":
                was_control = (
                    parenthesized_controls.pop() if parenthesized_controls else False
                )
                can_start_regex = was_control
            elif punctuator in ("]", "++", "--", ".", "?."):
                can_start_regex = False
            else:
                can_start_regex = True
            pending_control = False

        if stop_on_closing_brace:
            self._error("unterminated JavaScript template expression")


def _module_syntax_error(
    source: str, label: str, token: JavaScriptToken, message: str
) -> None:
    line = source.count("\n", 0, token.offset) + 1
    previous_newline = source.rfind("\n", 0, token.offset)
    column = token.offset - previous_newline
    fail(f"{message} in {label}:{line}:{column}")


def module_references(source: str, label: str) -> list[ModuleReference]:
    tokens = JavaScriptLexer(source, label).tokenize()
    references: list[ModuleReference] = []

    for index, token in enumerate(tokens):
        if token.kind != "identifier":
            continue

        if token.value == "import":
            previous = tokens[index - 1] if index > 0 else None
            if previous is not None and previous.value in (".", "?.", "#"):
                continue
            if index + 1 >= len(tokens):
                _module_syntax_error(source, label, token, "incomplete module import")
            following = tokens[index + 1]
            if following.value in (".", ":"):
                # `import.meta` and an object/grammar property named `import`
                # are not module declarations.
                continue
            if following.value == "(":
                if index + 2 >= len(tokens) or tokens[index + 2].kind != "string":
                    _module_syntax_error(
                        source,
                        label,
                        token,
                        "dynamic import must use a string literal",
                    )
                literal = tokens[index + 2]
                if index + 3 >= len(tokens) or tokens[index + 3].value not in (")", ","):
                    _module_syntax_error(
                        source,
                        label,
                        token,
                        "dynamic import must use only a string literal as its first argument",
                    )
                references.append(ModuleReference(literal.value, literal.offset, True))
                continue
            if following.kind == "string":
                references.append(
                    ModuleReference(following.value, following.offset, False)
                )
                continue

            depths = {"(": 0, "[": 0, "{": 0}
            closing = {")": "(", "]": "[", "}": "{"}
            found_from = False
            for candidate_index in range(index + 1, len(tokens)):
                candidate = tokens[candidate_index]
                at_top = not any(depths.values())
                if candidate.value == ";" and at_top:
                    break
                if (
                    candidate.kind == "identifier"
                    and candidate.value == "from"
                    and at_top
                ):
                    if (
                        candidate_index + 1 >= len(tokens)
                        or tokens[candidate_index + 1].kind != "string"
                    ):
                        _module_syntax_error(
                            source,
                            label,
                            candidate,
                            "static import from must be followed by a string literal",
                        )
                    literal = tokens[candidate_index + 1]
                    references.append(
                        ModuleReference(literal.value, literal.offset, False)
                    )
                    found_from = True
                    break
                if candidate.value in depths:
                    depths[candidate.value] += 1
                elif candidate.value in closing:
                    opener = closing[candidate.value]
                    if depths[opener] > 0:
                        depths[opener] -= 1
            if not found_from:
                _module_syntax_error(
                    source, label, token, "could not parse static module import"
                )

        elif token.value == "export":
            cursor = index + 1
            if cursor < len(tokens) and tokens[cursor].value == "type":
                cursor += 1
            if cursor >= len(tokens):
                continue
            start = tokens[cursor]
            if start.value == "*":
                cursor += 1
                if cursor < len(tokens) and tokens[cursor].value == "as":
                    cursor += 2
                if (
                    cursor + 1 >= len(tokens)
                    or tokens[cursor].value != "from"
                    or tokens[cursor + 1].kind != "string"
                ):
                    _module_syntax_error(
                        source,
                        label,
                        token,
                        "export-from must use a string literal",
                    )
                literal = tokens[cursor + 1]
                references.append(ModuleReference(literal.value, literal.offset, False))
            elif start.value == "{":
                depth = 1
                cursor += 1
                while cursor < len(tokens) and depth > 0:
                    if tokens[cursor].value == "{":
                        depth += 1
                    elif tokens[cursor].value == "}":
                        depth -= 1
                    cursor += 1
                if depth != 0:
                    _module_syntax_error(
                        source, label, token, "unterminated export clause"
                    )
                if cursor < len(tokens) and tokens[cursor].value == "from":
                    if (
                        cursor + 1 >= len(tokens)
                        or tokens[cursor + 1].kind != "string"
                    ):
                        _module_syntax_error(
                            source,
                            label,
                            tokens[cursor],
                            "export-from must use a string literal",
                        )
                    literal = tokens[cursor + 1]
                    references.append(
                        ModuleReference(literal.value, literal.offset, False)
                    )

    return references


def load_json(path: Path) -> object:
    def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
        value: dict[str, object] = {}
        for key, item in pairs:
            if key in value:
                fail(f"{path.relative_to(ROOT)} has duplicate key {key!r}")
            value[key] = item
        return value

    try:
        return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_object)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"could not parse {path.relative_to(ROOT)}: {error}")


def safe_relative(value: object, label: str) -> str:
    if not isinstance(value, str) or not value:
        fail(f"{label} must be a non-empty string")
    if "\\" in value or value.startswith("/") or value.endswith("/") or "//" in value:
        fail(f"{label} is not a canonical relative path: {value!r}")
    parts = PurePosixPath(value).parts
    if any(part in ("", ".", "..") for part in parts):
        fail(f"{label} is not a canonical relative path: {value!r}")
    return value


def walk_regular(root: Path) -> tuple[set[str], set[str]]:
    if root.is_symlink():
        fail(f"symlink is not allowed: {root.relative_to(ROOT)}")
    if not root.is_dir():
        fail(f"required directory is missing: {root.relative_to(ROOT)}")
    files: set[str] = set()
    directories: set[str] = set()
    for directory, names, filenames in os.walk(root, followlinks=False):
        base = Path(directory)
        for name in names:
            path = base / name
            mode = path.lstat().st_mode
            if stat.S_ISLNK(mode):
                fail(f"symlink is not allowed: {path.relative_to(ROOT)}")
            if not stat.S_ISDIR(mode):
                fail(f"non-directory entry is not allowed: {path.relative_to(ROOT)}")
            directories.add(path.relative_to(root).as_posix())
        for name in filenames:
            path = base / name
            mode = path.lstat().st_mode
            if stat.S_ISLNK(mode):
                fail(f"symlink is not allowed: {path.relative_to(ROOT)}")
            if not stat.S_ISREG(mode):
                fail(f"non-regular file is not allowed: {path.relative_to(ROOT)}")
            files.add(path.relative_to(root).as_posix())
    return files, directories


def app_sources() -> list[Path]:
    return sorted(
        path
        for path in SRC.rglob("*")
        if path.is_file()
        and (path.suffix == ".ts" or path.suffix == ".tsx")
        and path.relative_to(SRC).parts[0] != "generated"
        and not path.name.endswith(".d.ts")
    )


def check_config() -> None:
    value = load_json(CONFIG)
    if not isinstance(value, dict):
        fail("web/tsconfig.json must contain an object")
    compiler = value.get("compilerOptions")
    required: dict[str, object] = {
        "target": "ES2022",
        "module": "ES2022",
        "moduleResolution": "Bundler",
        "jsx": "react",
        "jsxFactory": "h",
        "rootDir": ".",
        "outDir": "dist",
        "strict": True,
        "noUncheckedIndexedAccess": True,
        "exactOptionalPropertyTypes": True,
        "noEmitOnError": True,
        "isolatedModules": True,
        "verbatimModuleSyntax": True,
        "allowJs": False,
        "allowImportingTsExtensions": False,
        "rewriteRelativeImportExtensions": False,
        "declaration": False,
        "sourceMap": False,
        "incremental": False,
        "skipLibCheck": False,
    }
    if not isinstance(compiler, dict):
        fail("web/tsconfig.json compilerOptions must be an object")
    for key, expected in required.items():
        if compiler.get(key) != expected:
            fail(f"web/tsconfig.json compilerOptions.{key} must be {expected!r}")
    if compiler.get("lib") != ["ES2022", "DOM", "DOM.Iterable"]:
        fail("web/tsconfig.json must use only ES2022, DOM, and DOM.Iterable libraries")
    if compiler.get("types") != []:
        fail("web/tsconfig.json compilerOptions.types must be empty")
    if value.get("include") != ["src/**/*.ts", "src/**/*.tsx"]:
        fail("web/tsconfig.json has an unexpected include list")


def check_forbidden_package_files() -> None:
    forbidden = {
        "package.json",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        "deno.lock",
        "node_modules",
    }
    for directory, names, filenames in os.walk(WEB, followlinks=False):
        for name in [*names, *filenames]:
            if name in forbidden:
                fail(f"package-manager artifact is forbidden: {(Path(directory) / name).relative_to(ROOT)}")


def check_vendor() -> set[str]:
    manifest = load_json(LIB / "vendor-manifest.json")
    if not isinstance(manifest, dict) or set(manifest) != {"schema_version", "dependencies"}:
        fail("web/lib/vendor-manifest.json has an unexpected top-level shape")
    if manifest["schema_version"] != 1 or not isinstance(manifest["dependencies"], list):
        fail("web/lib/vendor-manifest.json has an unsupported schema")

    records: dict[str, tuple[str, int]] = {}
    for dependency_index, dependency in enumerate(manifest["dependencies"]):
        if not isinstance(dependency, dict) or not isinstance(dependency.get("files"), list):
            fail(f"vendor dependency {dependency_index} has an invalid files list")
        archive_hash = dependency.get("source_archive_sha256")
        if not isinstance(archive_hash, str) or not HEX_SHA256.fullmatch(archive_hash):
            fail(f"vendor dependency {dependency_index} has an invalid archive hash")
        for field in ("name", "version", "license", "source_archive", "selection"):
            if not isinstance(dependency.get(field), str) or not dependency[field]:
                fail(f"vendor dependency {dependency_index} has invalid {field}")
        for file_index, entry in enumerate(dependency["files"]):
            label = f"vendor dependency {dependency_index} file {file_index}"
            if not isinstance(entry, dict) or set(entry) != {"path", "sha256", "size_bytes"}:
                fail(f"{label} has an unexpected shape")
            relative = safe_relative(entry["path"], f"{label} path")
            digest = entry["sha256"]
            size = entry["size_bytes"]
            if relative in records:
                fail(f"duplicate vendored path: {relative}")
            if not isinstance(digest, str) or not HEX_SHA256.fullmatch(digest):
                fail(f"{label} has an invalid SHA-256")
            if isinstance(size, bool) or not isinstance(size, int) or size < 0:
                fail(f"{label} has an invalid size")
            records[relative] = (digest, size)

    actual, _ = walk_regular(LIB)
    expected = set(records) | {"README.md", "SHA256SUMS", "vendor-manifest.json"}
    if actual != expected:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        fail(f"unexpected web/lib contents (missing={missing}, extra={extra})")

    checksum_text = "".join(f"{records[path][0]}  {path}\n" for path in sorted(records))
    try:
        if (LIB / "SHA256SUMS").read_text(encoding="ascii") != checksum_text:
            fail("web/lib/SHA256SUMS does not exactly match vendor-manifest.json")
    except (OSError, UnicodeError) as error:
        fail(f"could not read web/lib/SHA256SUMS: {error}")

    for relative, (expected_hash, expected_size) in records.items():
        path = LIB / relative
        metadata = path.stat()
        if stat.S_IMODE(metadata.st_mode) != 0o644:
            fail(f"vendored file must have mode 0644: web/lib/{relative}")
        if metadata.st_size != expected_size:
            fail(f"vendored size mismatch: web/lib/{relative}")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != expected_hash:
            fail(f"vendored SHA-256 mismatch: web/lib/{relative}")
    return set(records)


def emitted_application_assets() -> set[str]:
    return {
        path.relative_to(WEB).with_suffix(".js").as_posix()
        for path in SRC.rglob("*")
        if path.is_file()
        and path.suffix in (".ts", ".tsx")
        and not path.name.endswith(".d.ts")
    }


def asset_lists(
    vendor_files: set[str], application_present: bool
) -> tuple[list[str], list[str], list[str]]:
    value = load_json(ASSET_LIST)
    expected_keys = {
        "schema_version",
        "application_assets",
        "static_assets",
        "vendor_assets",
    }
    if not isinstance(value, dict) or set(value) != expected_keys:
        fail("dist-assets.json has an unexpected shape")
    if value["schema_version"] != 2:
        fail("dist-assets.json has an unsupported schema")
    application_values = value["application_assets"]
    static_values = value["static_assets"]
    vendor_values = value["vendor_assets"]
    if (
        not isinstance(application_values, list)
        or not isinstance(static_values, list)
        or not isinstance(vendor_values, list)
    ):
        fail("dist-assets.json asset lists must be arrays")
    application_assets = [
        safe_relative(item, "application asset") for item in application_values
    ]
    static_assets = [safe_relative(item, "static asset") for item in static_values]
    vendor_assets = [safe_relative(item, "vendor asset") for item in vendor_values]
    lists = (application_assets, static_assets, vendor_assets)
    if any(len(set(assets)) != len(assets) for assets in lists):
        fail("dist-assets.json contains a duplicate asset")
    if any(assets != sorted(assets) for assets in lists):
        fail("dist-assets.json asset lists must be sorted")
    all_assets = [path for assets in lists for path in assets]
    if len(set(all_assets)) != len(all_assets):
        fail("dist-assets.json contains a cross-category path collision")
    if any(path.startswith("build-support/") or path == "build-support" for path in static_assets):
        fail("build-support files cannot be distribution assets")
    missing_vendor = set(vendor_assets) - vendor_files
    if missing_vendor:
        fail(f"distribution vendor assets are not manifest entries: {sorted(missing_vendor)}")

    emitted = emitted_application_assets()
    if set(application_assets) != emitted:
        fail(
            "application asset manifest does not match TypeScript outputs "
            f"(missing={sorted(emitted - set(application_assets))}, "
            f"extra={sorted(set(application_assets) - emitted)})"
        )

    actual_static, _ = walk_regular(STATIC)
    support = {path for path in actual_static if path.startswith("build-support/")}
    expected_support = {f"build-support/{path}" for path in SUPPORT_FILES}
    if support != expected_support:
        fail(
            "unexpected build-support contents "
            f"(missing={sorted(expected_support - support)}, extra={sorted(support - expected_support)})"
        )
    app_static = actual_static - support
    unexpected = app_static - set(static_assets)
    if unexpected:
        fail(f"unexpected web/static assets: {sorted(unexpected)}")
    if application_present and app_static != set(static_assets):
        fail(f"application static assets are incomplete: {sorted(set(static_assets) - app_static)}")
    return application_assets, static_assets, vendor_assets


def read_utf8(path: Path, label: str) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        fail(f"could not read {label}: {error}")


def canonical_relative_reference(importer: str, target: str) -> str:
    base_parts = importer.split("/")[:-1]
    target_parts = target.split("/")
    common = 0
    while (
        common < len(base_parts)
        and common < len(target_parts)
        and base_parts[common] == target_parts[common]
    ):
        common += 1
    parts = [".."] * (len(base_parts) - common) + target_parts[common:]
    relative = "/".join(parts)
    return relative if relative.startswith("../") else f"./{relative}"


def resolve_web_reference(importer: str, specifier: str, kind: str) -> str:
    if not specifier.startswith(("./", "../")):
        fail(
            f"{kind} must use an explicit relative path in {importer}: "
            f"{specifier!r}"
        )
    if (
        "\\" in specifier
        or "%" in specifier
        or "?" in specifier
        or "#" in specifier
        or any(ord(character) <= 0x20 for character in specifier)
    ):
        fail(f"{kind} is not a canonical relative path in {importer}: {specifier!r}")

    resolved = importer.split("/")[:-1]
    for index, part in enumerate(specifier.split("/")):
        if part == "":
            fail(
                f"{kind} is not a canonical relative path in {importer}: "
                f"{specifier!r}"
            )
        if part == ".":
            if index != 0:
                fail(
                    f"{kind} is not a canonical relative path in {importer}: "
                    f"{specifier!r}"
                )
            continue
        if part == "..":
            if not resolved:
                fail(f"{kind} escapes web/: {specifier!r} in {importer}")
            resolved.pop()
            continue
        resolved.append(part)

    if not resolved:
        fail(f"{kind} does not name a file in {importer}: {specifier!r}")
    target = "/".join(resolved)
    if specifier != canonical_relative_reference(importer, target):
        fail(f"{kind} is not canonical in {importer}: {specifier!r}")
    return target


def check_module_graph(
    module_sources: dict[str, tuple[str, str]],
    application_modules: set[str],
    vendor_modules: set[str],
) -> None:
    allowed_modules = application_modules | vendor_modules
    pending = sorted(application_modules, reverse=True)
    remaining_vendor = sorted(vendor_modules, reverse=True)
    visited: set[str] = set()

    while pending or remaining_vendor:
        if not pending:
            while remaining_vendor and remaining_vendor[-1] in visited:
                remaining_vendor.pop()
            if not remaining_vendor:
                break
            pending.append(remaining_vendor.pop())
        importer = pending.pop()
        if importer in visited:
            continue
        visited.add(importer)
        entry = module_sources.get(importer)
        if entry is None:
            fail(f"manifested module is missing from output: {importer}")
        source, label = entry
        for reference in module_references(source, label):
            if not reference.specifier.endswith(".js"):
                fail(
                    f"module import must be an explicit relative .js path in "
                    f"{label}: {reference.specifier!r}"
                )
            target = resolve_web_reference(importer, reference.specifier, "module import")
            if target not in allowed_modules:
                fail(
                    f"imported module is not a manifest/output asset in {label}: "
                    f"{reference.specifier!r} ({target})"
                )
            if target not in module_sources:
                fail(f"imported module is missing from output: {target} (from {label})")
            if target not in visited:
                pending.append(target)


def source_module_map(
    application_assets: set[str], vendor_assets: set[str]
) -> tuple[dict[str, tuple[str, str]], set[str], set[str]]:
    sources: dict[str, tuple[str, str]] = {}
    for path in sorted(SRC.rglob("*")):
        if (
            not path.is_file()
            or path.suffix not in (".ts", ".tsx")
            or path.name.endswith(".d.ts")
        ):
            continue
        output = path.relative_to(WEB).with_suffix(".js").as_posix()
        if output in sources:
            fail(f"multiple TypeScript sources emit distribution path {output}")
        label = path.relative_to(ROOT).as_posix()
        sources[output] = (read_utf8(path, label), label)

    if set(sources) != application_assets:
        fail("application source/output map does not match dist-assets.json")

    vendor_modules = {f"lib/{path}" for path in vendor_assets if path.endswith(".js")}
    for output in sorted(vendor_modules):
        path = WEB / output
        label = path.relative_to(ROOT).as_posix()
        sources[output] = (read_utf8(path, label), label)
    return sources, application_assets, vendor_modules


def check_imports(application_assets: set[str], vendor_assets: set[str]) -> None:
    sources, application_modules, vendor_modules = source_module_map(
        application_assets, vendor_assets
    )
    check_module_graph(sources, application_modules, vendor_modules)


class AssetHTMLParser(HTMLParser):
    def __init__(self, label: str) -> None:
        super().__init__(convert_charrefs=True)
        self.label = label
        self.references: list[HTMLReference] = []

    def handle_starttag(
        self, tag: str, attributes: list[tuple[str, str | None]]
    ) -> None:
        values: dict[str, str | None] = {}
        for name, value in attributes:
            if name in values:
                fail(f"duplicate HTML attribute {name!r} in {self.label}:{self.getpos()[0]}")
            values[name] = value

        line = self.getpos()[0]
        if tag == "base":
            fail(f"HTML base elements are forbidden in {self.label}:{line}")
        if tag == "script":
            source = values.get("src")
            if not source:
                fail(f"HTML scripts must have a non-empty src in {self.label}:{line}")
            script_type = values.get("type")
            if not isinstance(script_type, str) or script_type.casefold() != "module":
                fail(f"HTML scripts must use type=module in {self.label}:{line}")
            self.references.append(HTMLReference("HTML script", source, line))
        elif tag == "link":
            href = values.get("href")
            if not href:
                fail(f"HTML links must have a non-empty href in {self.label}:{line}")
            rel = values.get("rel")
            relations = rel.casefold().split() if isinstance(rel, str) else []
            kind = "HTML stylesheet" if "stylesheet" in relations else "HTML link"
            self.references.append(HTMLReference(kind, href, line))


def html_references(source: str, label: str) -> list[HTMLReference]:
    parser = AssetHTMLParser(label)
    try:
        parser.feed(source)
        parser.close()
    except (UnicodeError, ValueError) as error:
        fail(f"could not parse {label}: {error}")
    return parser.references


def _css_error(source: str, label: str, offset: int, message: str) -> None:
    line = source.count("\n", 0, offset) + 1
    fail(f"{message} in {label}:{line}")


def _skip_css_space_and_comments(source: str, label: str, cursor: int) -> int:
    while cursor < len(source):
        if source[cursor].isspace():
            cursor += 1
        elif source.startswith("/*", cursor):
            end = source.find("*/", cursor + 2)
            if end < 0:
                _css_error(source, label, cursor, "unterminated CSS comment")
            cursor = end + 2
        else:
            break
    return cursor


def _scan_css_string(source: str, label: str, cursor: int) -> tuple[str, int]:
    start = cursor
    quote = source[cursor]
    cursor += 1
    while cursor < len(source):
        character = source[cursor]
        if character == "\\":
            if cursor + 1 >= len(source):
                _css_error(source, label, start, "unterminated CSS string")
            cursor += 2
        elif character == quote:
            return source[start + 1 : cursor], cursor + 1
        elif character in "\r\n\f":
            _css_error(source, label, start, "unterminated CSS string")
        else:
            cursor += 1
    _css_error(source, label, start, "unterminated CSS string")
    raise AssertionError("unreachable")


def css_url_references(source: str, label: str) -> list[HTMLReference]:
    references: list[HTMLReference] = []
    cursor = 0
    while cursor < len(source):
        if source.startswith("/*", cursor):
            cursor = _skip_css_space_and_comments(source, label, cursor)
            continue
        character = source[cursor]
        if character in "'\"":
            _, cursor = _scan_css_string(source, label, cursor)
            continue
        if character == "\\":
            _css_error(
                source,
                label,
                cursor,
                "CSS identifier escapes are unsupported by the asset verifier",
            )
        if character.isalpha() or character in "_-" or ord(character) >= 128:
            start = cursor
            cursor += 1
            while cursor < len(source) and (
                source[cursor].isalnum()
                or source[cursor] in "_-"
                or ord(source[cursor]) >= 128
            ):
                cursor += 1
            name = source[start:cursor]
            after_name = _skip_css_space_and_comments(source, label, cursor)
            if name.casefold() != "url" or after_name >= len(source) or source[after_name] != "(":
                continue

            value_start = after_name + 1
            value_cursor = _skip_css_space_and_comments(source, label, value_start)
            if value_cursor >= len(source):
                _css_error(source, label, start, "unterminated CSS url()")
            if source[value_cursor] in "'\"":
                value, value_cursor = _scan_css_string(
                    source, label, value_cursor
                )
                value_cursor = _skip_css_space_and_comments(
                    source, label, value_cursor
                )
            else:
                unquoted_start = value_cursor
                while (
                    value_cursor < len(source)
                    and source[value_cursor] != ")"
                    and not source[value_cursor].isspace()
                    and not source.startswith("/*", value_cursor)
                ):
                    if source[value_cursor] in "'\"(\\":
                        _css_error(
                            source,
                            label,
                            value_cursor,
                            "unsupported character in CSS url()",
                        )
                    value_cursor += 1
                value = source[unquoted_start:value_cursor]
                value_cursor = _skip_css_space_and_comments(
                    source, label, value_cursor
                )
            if value_cursor >= len(source) or source[value_cursor] != ")":
                _css_error(source, label, start, "malformed or unterminated CSS url()")
            references.append(
                HTMLReference(
                    "CSS url()", value, source.count("\n", 0, start) + 1
                )
            )
            cursor = value_cursor + 1
            continue
        cursor += 1
    return references


def check_document_references(
    text_assets: dict[str, str],
    manifest_assets: set[str],
    output_assets: set[str],
    module_assets: set[str],
) -> None:
    for importer in sorted(path for path in text_assets if path.endswith(".html")):
        for reference in html_references(text_assets[importer], importer):
            target = resolve_web_reference(importer, reference.value, reference.kind)
            if target not in manifest_assets:
                fail(
                    f"{reference.kind} is not a manifest asset in "
                    f"{importer}:{reference.line}: {reference.value!r}"
                )
            if target not in output_assets:
                fail(
                    f"{reference.kind} target is missing from output in "
                    f"{importer}:{reference.line}: {target}"
                )
            if reference.kind == "HTML script" and target not in module_assets:
                fail(
                    f"HTML module script is not an application/vendored JavaScript "
                    f"asset in {importer}:{reference.line}: {target}"
                )
            if reference.kind == "HTML stylesheet" and not target.endswith(".css"):
                fail(
                    f"HTML stylesheet does not reference a CSS asset in "
                    f"{importer}:{reference.line}: {target}"
                )

    for importer in sorted(path for path in text_assets if path.endswith(".css")):
        for reference in css_url_references(text_assets[importer], importer):
            target = resolve_web_reference(importer, reference.value, reference.kind)
            if target not in manifest_assets:
                fail(
                    f"CSS url() is not a manifest asset in "
                    f"{importer}:{reference.line}: {reference.value!r}"
                )
            if target not in output_assets:
                fail(
                    f"CSS url() target is missing from output in "
                    f"{importer}:{reference.line}: {target}"
                )


def check_source_document_references(
    application_assets: set[str],
    static_assets: set[str],
    vendor_assets: set[str],
) -> None:
    vendor_output_assets = {f"lib/{path}" for path in vendor_assets}
    manifest_assets = application_assets | static_assets | vendor_output_assets
    module_assets = application_assets | {
        path for path in vendor_output_assets if path.endswith(".js")
    }
    text_assets: dict[str, str] = {}
    for relative in sorted(
        path
        for path in static_assets
        if path.endswith(".html") or path.endswith(".css")
    ):
        source = STATIC / relative
        label = source.relative_to(ROOT).as_posix()
        text_assets[relative] = read_utf8(source, label)
    for relative in sorted(
        path
        for path in vendor_assets
        if path.endswith(".html") or path.endswith(".css")
    ):
        source = LIB / relative
        label = source.relative_to(ROOT).as_posix()
        text_assets[f"lib/{relative}"] = read_utf8(source, label)
    check_document_references(
        text_assets, manifest_assets, manifest_assets, module_assets
    )


def check_output_references(
    destination: Path,
    application_assets: set[str],
    static_assets: set[str],
    vendor_assets: set[str],
    output_assets: set[str],
) -> None:
    vendor_modules = {
        f"lib/{path}" for path in vendor_assets if path.endswith(".js")
    }
    module_assets = application_assets | vendor_modules
    module_sources: dict[str, tuple[str, str]] = {}
    for relative in sorted(module_assets):
        path = destination / relative
        label = f"distribution/{relative}"
        if relative not in output_assets or not path.is_file():
            fail(f"manifested module is missing from output: {relative}")
        module_sources[relative] = (read_utf8(path, label), label)
    check_module_graph(module_sources, application_assets, vendor_modules)

    manifest_assets = application_assets | static_assets | {
        f"lib/{path}" for path in vendor_assets
    }
    text_assets: dict[str, str] = {}
    for relative in sorted(
        path
        for path in manifest_assets
        if path.endswith(".html") or path.endswith(".css")
    ):
        path = destination / relative
        label = f"distribution/{relative}"
        if relative not in output_assets or not path.is_file():
            fail(f"manifested text asset is missing from output: {relative}")
        text_assets[relative] = read_utf8(path, label)
    check_document_references(
        text_assets, manifest_assets, output_assets, module_assets
    )


def validate(
    require_application: bool,
) -> tuple[list[Path], list[str], list[str], list[str]]:
    check_forbidden_package_files()
    walk_regular(SRC)
    check_config()
    vendor_files = check_vendor()
    sources = app_sources()
    if require_application and not sources:
        fail(
            "web SPA sources are missing; add TypeScript application files under web/src/ "
            "(web/src/generated/contracts.ts alone is not an application)"
        )
    application_assets, static_assets, vendor_assets = asset_lists(
        vendor_files, bool(sources)
    )
    application_set = set(application_assets)
    static_set = set(static_assets)
    vendor_set = set(vendor_assets)
    check_imports(application_set, vendor_set)
    if sources:
        check_source_document_references(application_set, static_set, vendor_set)
    return sources, application_assets, static_assets, vendor_assets


def copy_assets(destination: Path) -> None:
    _, _, static_assets, vendor_assets = validate(require_application=True)
    if destination.is_symlink() or not destination.is_dir():
        fail("asset destination must be an existing regular directory")
    for source_root, prefix, assets in (
        (STATIC, "", static_assets),
        (LIB, "lib", vendor_assets),
    ):
        for relative in assets:
            source = source_root / relative
            if not source.is_file() or source.is_symlink():
                fail(f"asset disappeared during build: {source.relative_to(ROOT)}")
            target = destination / prefix / relative
            if target.exists() or target.is_symlink():
                fail(f"distribution path collision: {target.relative_to(destination)}")
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            target.chmod(0o644)


def check_dist(destination: Path, *, normalize: bool = True) -> None:
    _, application_assets, static_assets, vendor_assets = validate(
        require_application=True
    )
    files, directories = walk_regular(destination)
    application_set = set(application_assets)
    static_set = set(static_assets)
    vendor_set = set(vendor_assets)
    expected = application_set | static_set | {
        f"lib/{path}" for path in vendor_set
    }
    if files != expected:
        fail(
            "unexpected distribution contents "
            f"(missing={sorted(expected - files)}, extra={sorted(files - expected)})"
        )
    check_output_references(
        destination, application_set, static_set, vendor_set, files
    )
    if not normalize:
        return
    epoch = 946684800
    for relative in sorted(files):
        path = destination / relative
        path.chmod(0o644)
        os.utime(path, (epoch, epoch), follow_symlinks=False)
    for relative in sorted(directories, key=lambda item: item.count("/"), reverse=True):
        path = destination / relative
        path.chmod(0o755)
        os.utime(path, (epoch, epoch), follow_symlinks=False)
    destination.chmod(0o755)
    os.utime(destination, (epoch, epoch), follow_symlinks=False)


def compare_dist(candidate: Path) -> None:
    check_dist(candidate)
    check_dist(WEB / "dist", normalize=False)
    candidate_files, _ = walk_regular(candidate)
    dist_files, _ = walk_regular(WEB / "dist")
    if candidate_files != dist_files:
        fail(
            "checked-in web/dist has drifted "
            f"(missing={sorted(candidate_files - dist_files)}, "
            f"extra={sorted(dist_files - candidate_files)})"
        )
    changed: list[str] = []
    bad_modes: list[str] = []
    for relative in sorted(candidate_files):
        expected = candidate / relative
        actual = WEB / "dist" / relative
        if expected.read_bytes() != actual.read_bytes():
            changed.append(relative)
        if stat.S_IMODE(actual.stat().st_mode) != 0o644:
            bad_modes.append(relative)
    if changed:
        fail(f"checked-in web/dist content has drifted: {changed}")
    if bad_modes:
        fail(f"checked-in web/dist files must have mode 0644: {bad_modes}")


def _expect_self_check_failure(
    description: str, expected_message: str, action: Callable[[], None]
) -> None:
    error: ValidationError | None = None
    try:
        action()
    except ValidationError as caught:
        error = caught
    if error is None:
        fail(f"self-check fixture unexpectedly passed: {description}")
    if expected_message not in str(error):
        fail(
            f"self-check fixture {description!r} failed with the wrong error: "
            f"{error}"
        )


def run_self_check() -> None:
    lexical_fixture = r'''
const endpoint = "https://example.invalid/assets";
const apparent = "/* import(hiddenName) */ https://invalid/import(otherName)";
// import(commentName)
/* export * from "./comment.js"; */
const expression = /https?:\/\/example[.]invalid\/import\(ignored\)/u;
const template = `https://example.invalid/import(templateName)`;
const grammar = { import: [/url/, "keyword"] };
export { fixture } from "./dep.js";
void import("./lazy.js");
'''
    references = module_references(lexical_fixture, "lexical-fixture.js")
    if [reference.specifier for reference in references] != [
        "./dep.js",
        "./lazy.js",
    ]:
        fail("self-check did not isolate imports from JavaScript lexical decoys")

    _expect_self_check_failure(
        "URL-like string before dynamic import",
        "dynamic import must use a string literal",
        lambda: module_references(
            'const endpoint = "https://example.invalid/api"; import(moduleName);',
            "url-before-dynamic.js",
        ),
    )
    _expect_self_check_failure(
        "dynamic import inside a template expression",
        "dynamic import must use a string literal",
        lambda: module_references(
            "const value = `prefix ${import(moduleName)}`;",
            "template-expression.js",
        ),
    )
    _expect_self_check_failure(
        "concatenated dynamic import",
        "dynamic import must use only a string literal",
        lambda: module_references(
            'void import("./prefix-" + name + ".js");',
            "concatenated-dynamic.js",
        ),
    )

    application_modules = {"src/bootstrap.js", "src/main.js"}
    vendor_modules = {"lib/vendor.js", "lib/nested.js"}
    graph_fixture = {
        "src/bootstrap.js": (
            'const before = "https://example.invalid"; void import("./main.js");',
            "src/bootstrap.js",
        ),
        "src/main.js": (
            'import "../lib/vendor.js";',
            "src/main.js",
        ),
        "lib/vendor.js": (
            'export * from "./nested.js";',
            "lib/vendor.js",
        ),
        "lib/nested.js": ("export const fixture = true;", "lib/nested.js"),
    }
    check_module_graph(graph_fixture, application_modules, vendor_modules)

    missing_fixture = dict(graph_fixture)
    del missing_fixture["lib/nested.js"]
    _expect_self_check_failure(
        "transitively missing vendored import",
        "imported module is missing from output",
        lambda: check_module_graph(
            missing_fixture, application_modules, vendor_modules
        ),
    )

    unmanifested_fixture = dict(graph_fixture)
    unmanifested_fixture["src/main.js"] = (
        'import "../lib/unmanifested.js";',
        "src/main.js",
    )
    unmanifested_fixture["lib/unmanifested.js"] = (
        "export {};",
        "lib/unmanifested.js",
    )
    _expect_self_check_failure(
        "unmanifested imported module",
        "not a manifest/output asset",
        lambda: check_module_graph(
            unmanifested_fixture, application_modules, vendor_modules
        ),
    )

    outside_fixture = dict(graph_fixture)
    outside_fixture["src/main.js"] = (
        'import "../../outside.js";',
        "src/main.js",
    )
    _expect_self_check_failure(
        "module import outside web",
        "escapes web/",
        lambda: check_module_graph(
            outside_fixture, application_modules, vendor_modules
        ),
    )

    manifest_assets = set(graph_fixture) | {
        "index.html",
        "styles.css",
        "lib/font.woff2",
    }
    document_fixture = {
        "index.html": (
            '<!doctype html><link rel="stylesheet" href="./styles.css">'
            '<script type="module" src="./src/bootstrap.js"></script>'
        ),
        "styles.css": (
            'body::before { content: "https://example.invalid/url(ignored.png)"; }\n'
            '/* url("./ignored-comment.png") */\n'
            '@font-face { src: url("./lib/font.woff2") format("woff2"); }'
        ),
    }
    check_document_references(
        document_fixture, manifest_assets, manifest_assets, set(graph_fixture)
    )

    bad_html = dict(document_fixture)
    bad_html["index.html"] = (
        '<script type="module" src="./src/unmanifested.js"></script>'
    )
    _expect_self_check_failure(
        "unmanifested HTML script",
        "not a manifest asset",
        lambda: check_document_references(
            bad_html, manifest_assets, manifest_assets, set(graph_fixture)
        ),
    )

    unmanifested_css = dict(document_fixture)
    unmanifested_css["styles.css"] = (
        'main { background: url("./unmanifested.png"); }'
    )
    _expect_self_check_failure(
        "unmanifested CSS URL",
        "not a manifest asset",
        lambda: check_document_references(
            unmanifested_css,
            manifest_assets,
            manifest_assets,
            set(graph_fixture),
        ),
    )

    bad_css = dict(document_fixture)
    bad_css["styles.css"] = 'main { background: url("../outside.png"); }'
    _expect_self_check_failure(
        "CSS URL outside web",
        "escapes web/",
        lambda: check_document_references(
            bad_css, manifest_assets, manifest_assets, set(graph_fixture)
        ),
    )

    _expect_self_check_failure(
        "missing CSS URL output",
        "target is missing from output",
        lambda: check_document_references(
            document_fixture,
            manifest_assets,
            manifest_assets - {"lib/font.woff2"},
            set(graph_fixture),
        ),
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    check = subparsers.add_parser("check")
    check.add_argument("--require-application", action="store_true")
    copy = subparsers.add_parser("copy-assets")
    copy.add_argument("destination", type=Path)
    dist = subparsers.add_parser("check-dist")
    dist.add_argument("destination", type=Path)
    compare = subparsers.add_parser("compare-dist")
    compare.add_argument("candidate", type=Path)
    subparsers.add_parser("self-check")
    arguments = parser.parse_args()
    if arguments.command == "check":
        validate(arguments.require_application)
    elif arguments.command == "copy-assets":
        copy_assets(arguments.destination.absolute())
    elif arguments.command == "check-dist":
        check_dist(arguments.destination.absolute())
    elif arguments.command == "compare-dist":
        compare_dist(arguments.candidate.absolute())
    else:
        run_self_check()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"verify-web-assets: {error}", file=sys.stderr)
        raise SystemExit(1)
