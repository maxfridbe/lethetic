# Archived experiments

These files are preserved for reference and are no longer Cargo build or test
targets. Paths below mirror their original locations in the repository. Contents
were moved unchanged on 2026-10-19.

## Review findings

| Original path | Reason for archiving |
| --- | --- |
| `src/bin/parse_xml.rs` | Standalone parser experiment that prints a single hardcoded XML tool call; does not use the application parser. |
| `src/bin/dump_prompt.rs` | One-off prompt dump with a hardcoded C# edit request and `test_prompt.txt` output. |
| `tests/test_gemma_parse.rs` | Standalone copy of an experimental Gemma parser with one printed sample. |
| `tests/test_lines.rs` | Scratch program printing three string lines. |
| `tests/test_md.rs` | Scratch program printing Markdown parser events. |
| `tests/test_parse.rs` | Scratch program printing JSON extraction results. |
| `tests/test_parse2.rs` | Scratch program inspecting JSON escape errors. |
| `tests/test_parse3.rs` | Scratch program trying JSON escape replacement. |
| `tests/test_regex.rs` | Scratch program printing model-marker regex matches. |
| `tests/test_winnow.rs` | Placeholder that only prints a greeting. |
| `tests/test_xml.rs` | Empty `main()` placeholder. |
| `tests/validate_parser_test.rs` | Manual parser probe tied to a hardcoded local session directory. |

The ten files formerly under `tests/` have no test functions: Cargo's test harness
discovered them but ran zero tests and did not execute their `main()` functions.
No active source, documentation, build script, or workflow referenced these twelve
files. The two binaries were only implicitly discovered through `src/bin/`.

Actual parser and Markdown regression tests remain active, including
`tests/test_parse4.rs`, `tests/test_parser_new.rs`, and
`tests/test_markdown_rendering.rs`. The session compaction, playback, evaluation,
and contract-generation utilities also remain active. Generated web assets,
platform-specific modules, browser workers, and compatibility migrations are
still referenced and were retained.

## Restoring an experiment

Copy a file back to its original path to restore Cargo discovery. Review it
against current APIs first. Convert scratch programs into assertion-based tests
before returning them to `tests/`; copying a `main()` there does not create a test.
Archived sources are excluded from the active source-line policy and are not
maintained or validated by Rust CI.
