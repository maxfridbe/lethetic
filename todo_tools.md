# Tool Audit: Lethetic vs OpenCode

## Current Tools (17)

| Tool | Purpose | Issues |
|---|---|---|
| `read_file` | Full file read with line numbers | Line numbers add ~1k tokens/500-line file; no max_lines param |
| `read_file_lines` | Range read (start_line–end_line) | — |
| `read_folder` | Directory listing | — |
| `write_file` | Create/overwrite file | — |
| `replace_text` | Exact single-occurrence string replace | **🔴 Brittle** — fails on whitespace drift; multi-match error lacks context |
| `apply_patch` | Multi-line block replace via diffy+patch | — |
| `search_text` | grep -rn regex | **🔴 Scans `target/` and `.git/`** — slow on Rust projects |
| `run_shell_command` | Bash execution | Bash-only (no Windows) |
| `calculate` | Arithmetic (+-*/) | No trig/sqrt/pow; model tries these and gets errors |
| `web_fetch` | Raw HTTP fetch | Redundant with read_page |
| `web_search` | DuckDuckGo (10 results hardcoded) | No count param; can rate-limit |
| `read_page` | HTTP → Markdown (h2m) | Redundant with web_fetch |
| `ask_the_user` | Pause and ask user | — |
| `get_pdf_text` | PDF text extraction (pdf_oxide) | — |
| `process_image` | Vision analysis | Non-aspect-ratio resize |
| `process_pdf_image` | Render PDF page → vision | Non-aspect-ratio resize |
| `summarize_content` | LLM summarization | Schema allows both path+content to be absent |

---

## Missing Tools

### 🔴 High Priority

| Tool | What it does | Status |
|---|---|---|
| `glob` | ripgrep-based file-pattern search (`**/*.rs`) | ✅ Added in tool-improvements |
| `find_symbol` | Definition/reference/symbol search via rg patterns | ✅ Added in tool-improvements |
| `edit` | Fuzzy diff-based file editing (tolerates whitespace drift) | ✅ Added in tool-improvements |

### 🟡 Medium Priority

| Tool | What it does | Status |
|---|---|---|
| `fetch_url` | Merged web_fetch + read_page with `format` param (markdown/text/html) | ✅ Added |
| `websearch` (v2) | Added `num_results` param (1–20, default 10) | ✅ Updated |

### 🟢 Low Priority

| Tool | What it does | Status |
|---|---|---|
| `todowrite` | Structured todo list with status + priority, persisted to .lethetic/todos.json | ✅ Added |
| `repo_overview` | Ecosystem detection, entry points, 2-level dir tree, README preview | ✅ Added |
| `codesearch` | Exa API search for SDKs/docs | — |
| `repo_clone` | Managed git clone cache | — |
| `task` | Spawn sub-agent with persistent session | — |

---

## Unique Strengths (keep)

- `get_pdf_text` — not in opencode
- `process_image` / `process_pdf_image` — not in opencode
- `summarize_content` — not in opencode
- `calculate` — not in opencode (avoids shell spawn for math)

---

## Fixes Done

- `replace_text`: added `replace_all` param + line-number context on multi-match error
- `search_text`: excludes `target/`, `.git/`, `node_modules/`, `.lethetic/`; prefers ripgrep
- `read_file`: added `max_lines` param; shows truncation notice with total line count
- `summarize_content`: fixed schema — `prompt` now required; clear error when neither path nor content provided
