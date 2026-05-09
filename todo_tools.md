# Tool Audit: Lethetic vs OpenCode

_Last verified: 2026-05-09_

---

## Current Tools (19 active + 2 legacy/compat)

| Tool | Purpose | Status |
|---|---|---|
| `read_file` | Full file read with line numbers | ✅ `max_lines` param added; truncation notice shows total line count |
| `read_file_lines` | Range read (start_line–end_line) | ✅ OK |
| `read_folder` | Directory listing | ✅ OK |
| `write_file` | Create/overwrite file | ✅ OK |
| `replace_text` | Exact string replace | ✅ `replace_all` param; multi-match error shows line numbers |
| `apply_patch` | Multi-line block replace via diffy+patch | ✅ OK |
| `edit` | Fuzzy diff-based edit (tolerates whitespace drift) | ✅ Added |
| `search_text` | grep/rg regex search | ✅ Excludes `target/`, `.git/`, `node_modules/`, `.lethetic/`; prefers rg |
| `glob` | ripgrep file-pattern search (`**/*.rs`) | ✅ Added; sorted by mtime, 200-result limit |
| `find_symbol` | Definition/reference/symbol search via rg | ✅ Added |
| `fetch_url` | HTTP fetch with format param (markdown/text/html) | ✅ Added; replaces web_fetch + read_page |
| `web_search` | DuckDuckGo search | ✅ `num_results` param (1–20, default 10) |
| `calculate` | Arithmetic | ✅ `meval` — `sin/cos/tan/sqrt/pow/ln/log/abs/floor/ceil/round`, `pi`/`e`, `^` |
| `lsp` | Language Server Protocol: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol | ✅ Added; auto-fallback to find_symbol when server not installed |
| `task` | Spawn sub-agent (all tools except task+ask_the_user, 5-min timeout) | ✅ Added |
| `ask_the_user` | Pause and ask user a question | ✅ OK |
| `get_pdf_text` | PDF text extraction (pdf_oxide) | ✅ OK |
| `process_image` | Vision analysis | ✅ `img.resize()` correctly preserves aspect ratio (original audit note was wrong) |
| `process_pdf_image` | Render PDF page → vision | ✅ Same — aspect ratio preserved |
| `summarize_content` | LLM summarization | ✅ `prompt` now required; clear error when neither path nor content provided |
| `todowrite` | Structured todo list → `.lethetic/todos.json` | ✅ Added |
| `repo_overview` | Ecosystem detection, dir tree, README preview | ✅ Added |
| `web_fetch` _(legacy)_ | Raw HTTP fetch | ⚠ Removed from tool list; backwards-compat dispatch only |
| `read_page` _(legacy)_ | HTTP → Markdown | ⚠ Removed from tool list; backwards-compat dispatch only |

---

## Engine Fixes (not tool-level, but affect tool reliability)

| Fix | Status |
|---|---|
| Tool output truncation threshold: `10k` → `20k` chars | ✅ Done |
| Duplicate tool-call detection: 3× same (tool+args) → user warning + guidance | ✅ Done |
| Stop-reason status message (⚠/→/✗ in status bar) | ✅ Done |
| `max_tokens` bumped from 16384 → 24576 | ✅ Done |
| Tool call ID mismatch fix (server UUID vs model human-readable ID) | ✅ Done |

---

## Remaining Work

### 🟢 Low Priority / Optional

| Tool | What it does | Notes |
|---|---|---|
| `codesearch` | Exa API search for SDKs/docs | Requires API key |
| `repo_clone` | Managed git clone cache | Niche use case |

---

## Unique Strengths (keep — not in opencode)

- `get_pdf_text` — PDF text extraction
- `process_image` / `process_pdf_image` — vision analysis
- `summarize_content` — LLM summarization pass
- `calculate` — avoids shell spawn for math (when fixed)
- `todowrite` — structured task tracking
- `repo_overview` — ecosystem detection
