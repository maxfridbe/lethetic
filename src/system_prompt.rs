use crate::tools;
use std::fs;
use std::path::PathBuf;

pub const MAX_SYSTEM_PROMPT_NAME_CHARS: usize = 80;
pub const MAX_SYSTEM_PROMPT_NAME_BYTES: usize = 256;
pub const MAX_SYSTEM_PROMPT_FILE_BYTES: usize = 256 * 1024;

pub fn normalize_system_prompt_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("system prompt name cannot be empty".to_string());
    }
    if value.chars().count() > MAX_SYSTEM_PROMPT_NAME_CHARS
        || value.len() > MAX_SYSTEM_PROMPT_NAME_BYTES
    {
        return Err("system prompt name is too long".to_string());
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'-' | b'_'))
    {
        return Err(
            "system prompt name may contain only ASCII letters, digits, spaces, '-' and '_'"
                .to_string(),
        );
    }
    Ok(value.to_string())
}

pub const DEFAULT_PROMPT_TEMPLATE: &str = r#"You are Lethetic, an expert senior software engineer and autonomous agent.

### Core Rules & Constraints
1. READ BEFORE WRITE: Never edit or write to a file unless you have read its current contents or context in this session.
2. SURGICAL CHANGES: Implement minimal, clean, and robust fixes. Do not rewrite, refactor, or delete unrelated code or comments.
3. PREDICT NEEDS: Anticipate production requirements (e.g., error handling, logging, pagination, or security boundaries) without over-engineering.
4. VERIFICATION LOOP: Provide a linter/type-check check or unit test snippet alongside your code changes to verify correctness.

### Output Format
- Root Cause: [One concise sentence explaining the bug or requirement]
- Proposed Changes: [Bulleted list of modified files and specific logic changes]
- Code Block: [Clean, production-ready code with necessary comments only]
- Self-Critique: [Brief verification that public signatures and constraints are respected]

CurrentWorkingDir:[CWD]

[TOOLS_DEFINITIONS]

[TOOL_CALL_FORMAT]
"#;

pub const TOOL_CALL_FORMAT_GEMMA4: &str = r#"# Tool call format
ALL tool call argument values that are strings MUST be wrapped in asymmetric markers:
  <|"|>your content here<|"|>
Strings inside those markers do not need escaping. Do NOT use <|'|> markers."#;

pub const TOOL_CALL_FORMAT_QWEN3: &str = r#"# Tool call format
Use standard JSON format for all tool call arguments. Do NOT wrap strings in any special markers."#;

pub const TOOL_CALL_FORMAT_NATIVE: &str = r#"# Native tool use
Use the provided API tools directly. Never print tool-call JSON or XML as text.
For this connection, call at most one tool per assistant turn."#;

const PYTHON_ONLY_GUIDANCE: &str = r#"# Python-only mode
Exactly one model tool is available: `python`. `lethetic_todo` is a host-backed module imported inside Python, not a second model tool.
Use `python` for computation, data inspection, and code execution. Interactive stdin is unavailable. For task tracking, use `import lethetic_todo`; `lethetic_todo.get()` returns `{"revision": N, "todos": [...]}`, and `lethetic_todo.set(todos, expected_revision=N)` atomically replaces the list. Read the current revision first and handle `lethetic_todo.RevisionConflict` instead of overwriting concurrent changes.
Python imports, variables, and the worker working directory persist only while the current worker lives. They reset on a new or loaded chat, cancellation, worker failure, explicit reset, execution-policy/backend change, and retained detach/resume. Host-backed `lethetic_todo` state persists independently.
Large Python results include bounded excerpts and worker-local output metadata. To inspect retained output, use `import lethetic_output` inside the same `python` tool. Set `artifact_id` to the exact quoted UUID string supplied with the result, never its numeric display-only cell number. `lethetic_output.info(artifact_id)` lists the captured sections; `lethetic_output.read(artifact_id, section, offset, limit)` returns a UTF-8 text chunk and `next_offset` (byte offsets; limit 1..65536). Start at zero and use the returned next_offset to continue; inspect the traceback/stderr tail for failures. Sections are `stdout`, `stderr`, `repr`, and `traceback`. The FIFO ring retains at most eight artifacts and 8 MiB in the current worker; capture limits may discard bytes, and eviction/reset raises KeyError. Old artifact IDs never identify output in a replacement worker. This module is not another tool and provides no host path or additional filesystem authority.
Do not request, invent, or emit calls to shell, file, web, sub-agent, `todowrite`, or other unavailable model tools."#;

const PYTHON_INVALID_GUIDANCE: &str = r#"The current Python execution policy is unresolved or invalid, so the runspace cannot execute. Do not call `python` until the host supplies a valid policy. Lethetic-managed package installation is unavailable."#;

const PYTHON_RETAINED_NONLOCAL_GUIDANCE: &str = r#"This is a retained Podman sandbox with public-only networking. Direct container networking is disabled; public HTTP/HTTPS package traffic is available only through Lethetic's constrained broker. Host, localhost, LAN, VPN/local routes, metadata, and special ranges remain blocked.
If a required distro tool is missing, invoke only `lethetic-pkg refresh` or `lethetic-pkg install NAME...` with fixed package names from a Python subprocess. Do not invoke `apt-get` directly and do not pass options, URLs, paths, repository changes, or shell syntax to `lethetic-pkg`.
The package layer persists for this chat for up to 14 days and the container is stopped between attachments. Python globals reset on detach/resume. Signed packages may run maintainer scripts as namespaced container-root and mutate this chat's retained package layer."#;

pub fn python_capability_guidance(config: &crate::config::Config) -> Option<String> {
    use crate::config::{
        AccessMode, NetworkAccess, PythonExecutionTarget, SandboxBackend, ToolProfile,
    };

    if config.tool_profile != ToolProfile::PythonOnly {
        return None;
    }
    let mut guidance = PYTHON_ONLY_GUIDANCE.to_string();
    guidance.push_str("\n\n# Python execution policy\n");
    if config.python_mode_validation_error().is_some() {
        guidance.push_str(PYTHON_INVALID_GUIDANCE);
        return Some(guidance);
    }

    match config.python_runtime.target {
        Some(PythonExecutionTarget::Host) => {
            guidance.push_str(
                "Execution target: Host. Python runs directly as the Lethetic host user with the configured executable. Lethetic adds no filesystem or network sandbox and never falls back to another target or backend. Host permissions and policy determine workspace, network, subprocess, and ordinary package-operation access. Lethetic-managed package installation is unavailable in this mode.",
            );
        }
        Some(PythonExecutionTarget::Sandbox) => {
            let backend = match config.python_runtime.sandbox.backend {
                Some(SandboxBackend::Bubblewrap) => "Bubblewrap",
                Some(SandboxBackend::Podman) => "Podman",
                None => unreachable!("validated sandbox policy has a backend"),
            };
            let network = match config.python_runtime.sandbox.network {
                Some(NetworkAccess::None) => {
                    "Network policy: None; the worker has no external network access."
                }
                Some(NetworkAccess::Full) => {
                    "Network policy: Full; ordinary networking is available inside the selected sandbox."
                }
                Some(NetworkAccess::Nonlocal) => {
                    "Network policy: Public-only through Lethetic's constrained broker."
                }
                None => unreachable!("validated sandbox policy has a network mode"),
            };
            let workspace = match config.python_runtime.sandbox.workspace_access {
                Some(AccessMode::ReadOnly) => "read-only",
                Some(AccessMode::ReadWrite) => "read/write",
                None => unreachable!("validated sandbox policy has workspace access"),
            };
            guidance.push_str(&format!(
                "Execution target: Sandbox with the {backend} backend. Lethetic never falls back to Host, another backend, or another network policy. {network} The configured workspace is mounted {workspace}; {} additional path grant(s) are mounted with their configured access. ",
                config.python_runtime.sandbox.grants.len()
            ));
            if crate::config::is_exact_retained_nonlocal_python_policy(
                config.tool_profile,
                &config.python_runtime,
            ) {
                guidance.push_str(PYTHON_RETAINED_NONLOCAL_GUIDANCE);
            } else {
                if config.python_runtime.sandbox.backend == Some(SandboxBackend::Podman) {
                    guidance.push_str(
                        "This is a transient container: it exists only for the current worker and is removed when that worker resets or terminates. ",
                    );
                } else {
                    guidance.push_str(
                        "This is a transient sandbox process that exists only for the current worker. ",
                    );
                }
                guidance.push_str(
                    "Lethetic-managed package installation is unavailable in this mode. Ordinary package operations, if attempted from Python, remain subject to the selected network, workspace, and sandbox permissions and may be unavailable.",
                );
            }
        }
        None => unreachable!("validated Python-only policy has a target"),
    }
    Some(guidance)
}

pub const DEFAULT_COMPACTION_PROMPT: &str = r#"You are a session compactor. You receive a conversation log between a user and an AI coding agent.

Produce a concise but complete summary capturing:
- The user's original goal
- All files created or modified (exact paths)
- Key decisions and the reasoning behind them
- Current project state: what is done, what is pending or blocked
- Any important errors encountered and how they were resolved
- Context a developer would need to continue the work immediately

Rules:
- Begin writing the summary immediately. Do not plan, reason out loud, or explain your approach.
- Output ONLY the summary text. No preamble ("Here is...", "Summary:", "I will..."), no postamble.
- Be terse. Omit repetition and tool-call noise.
- Preserve exact file paths, function names, library names, and command lines.
- The summary must be significantly shorter than the original conversation.
"#;

pub struct SystemPromptManager {
    prompts_dir: PathBuf,
}

fn checked_prompt_file_name(name: &str) -> Result<String, String> {
    Ok(format!("{}.md", normalize_system_prompt_name(name)?))
}

impl Default for SystemPromptManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemPromptManager {
    pub fn new() -> Self {
        let root = crate::platform::lethetic_config_dir();
        let prompts_dir = root.join("prompts");
        #[cfg(target_os = "linux")]
        let root_ready = crate::platform::ensure_private_directory_durable(&root, 0o700).is_ok();
        #[cfg(not(target_os = "linux"))]
        let root_ready = fs::create_dir_all(&root).is_ok();
        if root_ready {
            let current = crate::platform::read_file_nofollow_bounded(
                &root,
                &["prompts"],
                "software_engineer.md",
                MAX_SYSTEM_PROMPT_FILE_BYTES,
            )
            .ok()
            .flatten();
            if current.as_deref() != Some(DEFAULT_PROMPT_TEMPLATE.as_bytes()) {
                let _ = crate::platform::atomic_write_nofollow(
                    &root,
                    &["prompts"],
                    "software_engineer.md",
                    DEFAULT_PROMPT_TEMPLATE.as_bytes(),
                    0o600,
                );
            }
            // Seed the compaction prompt once; users may edit it afterwards.
            let compaction = crate::platform::read_file_nofollow_bounded(
                &root,
                &["prompts"],
                "compaction.md",
                MAX_SYSTEM_PROMPT_FILE_BYTES,
            )
            .ok()
            .flatten();
            if compaction.is_none() {
                let _ = crate::platform::atomic_write_nofollow(
                    &root,
                    &["prompts"],
                    "compaction.md",
                    DEFAULT_COMPACTION_PROMPT.as_bytes(),
                    0o600,
                );
            }
        }

        Self { prompts_dir }
    }

    pub fn list_prompts(&self) -> Vec<String> {
        let mut prompts = Vec::new();
        let safe_directory = fs::symlink_metadata(&self.prompts_dir)
            .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir());
        if safe_directory && let Ok(entries) = fs::read_dir(&self.prompts_dir) {
            for entry in entries.filter_map(Result::ok) {
                if let Some(name) = entry.file_name().to_str()
                    && name.ends_with(".md")
                {
                    prompts.push(name.trim_end_matches(".md").to_string());
                }
            }
        }
        prompts.sort();
        prompts
    }

    pub fn prompt_exists_checked(&self, name: &str) -> Result<bool, String> {
        let file_name = checked_prompt_file_name(name)?;
        crate::platform::read_file_nofollow_bounded(
            &crate::platform::lethetic_config_dir(),
            &["prompts"],
            &file_name,
            MAX_SYSTEM_PROMPT_FILE_BYTES,
        )
        .map(|content| content.is_some())
        .map_err(|error| format!("could not inspect system prompt: {error}"))
    }

    pub fn load_prompt_checked(&self, name: &str) -> Result<Option<String>, String> {
        let file_name = checked_prompt_file_name(name)?;
        let content = crate::platform::read_file_nofollow_bounded(
            &crate::platform::lethetic_config_dir(),
            &["prompts"],
            &file_name,
            MAX_SYSTEM_PROMPT_FILE_BYTES,
        )
        .map_err(|error| format!("could not read system prompt: {error}"))?;
        content
            .map(|bytes| {
                String::from_utf8(bytes).map_err(|_| "system prompt is not valid UTF-8".to_string())
            })
            .transpose()
    }

    pub fn save_prompt_checked(
        &self,
        name: &str,
        content: &str,
        overwrite: bool,
    ) -> Result<(), String> {
        if content.len() > MAX_SYSTEM_PROMPT_FILE_BYTES {
            return Err(format!(
                "system prompt exceeds the {}-byte limit",
                MAX_SYSTEM_PROMPT_FILE_BYTES
            ));
        }
        let file_name = checked_prompt_file_name(name)?;
        let root = crate::platform::lethetic_config_dir();
        let result = if overwrite {
            crate::platform::atomic_write_nofollow(
                &root,
                &["prompts"],
                &file_name,
                content.as_bytes(),
                0o600,
            )
        } else {
            crate::platform::atomic_create_nofollow(
                &root,
                &["prompts"],
                &file_name,
                content.as_bytes(),
                0o600,
            )
        };
        result
            .map(|_| ())
            .map_err(|error| format!("could not save system prompt: {error}"))
    }

    pub fn load_prompt(&self, name: &str) -> Option<String> {
        let file_name = format!("{name}.md");
        crate::platform::read_file_nofollow_bounded(
            &crate::platform::lethetic_config_dir(),
            &["prompts"],
            &file_name,
            MAX_SYSTEM_PROMPT_FILE_BYTES,
        )
        .ok()
        .flatten()
        .and_then(|content| String::from_utf8(content).ok())
    }

    pub fn save_prompt(&self, name: &str, content: &str) -> std::io::Result<()> {
        if content.len() > MAX_SYSTEM_PROMPT_FILE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "system prompt is too large",
            ));
        }
        let file_name = format!("{name}.md");
        crate::platform::atomic_write_nofollow(
            &crate::platform::lethetic_config_dir(),
            &["prompts"],
            &file_name,
            content.as_bytes(),
            0o600,
        )
        .map(|_| ())
    }

    pub fn resolve_prompt(template: &str, cwd: &str, config: &crate::config::Config) -> String {
        let tool_declarations = tools::get_all_prompt_templates(config);
        let active_parser = config.active_parser();
        let tool_call_fmt = if config.active_connection_kind().uses_native_tools() {
            TOOL_CALL_FORMAT_NATIVE
        } else {
            match active_parser {
                "qwen3" | "default" | "generic" => TOOL_CALL_FORMAT_QWEN3,
                _ => TOOL_CALL_FORMAT_GEMMA4,
            }
        };
        let resolved = template
            .replace("[TOOLS_DEFINITIONS]", &tool_declarations)
            .replace("[CWD]", cwd)
            .replace("[TOOL_CALL_FORMAT]", tool_call_fmt);
        match python_capability_guidance(config) {
            Some(guidance) => format!("{resolved}\n\n{guidance}"),
            None => resolved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_output_guidance_uses_artifact_identity_not_cell_numbers() {
        assert!(PYTHON_ONLY_GUIDANCE.contains("Exactly one model tool is available: `python`"));
        assert!(PYTHON_ONLY_GUIDANCE.contains("exact quoted UUID string"));
        assert!(PYTHON_ONLY_GUIDANCE.contains("lethetic_output.info(artifact_id)"));
        assert!(
            PYTHON_ONLY_GUIDANCE
                .contains("lethetic_output.read(artifact_id, section, offset, limit)")
        );
        assert!(!PYTHON_ONLY_GUIDANCE.contains("lethetic_output.info(cell)"));
        assert!(!PYTHON_ONLY_GUIDANCE.contains("lethetic_output.read(cell,"));
    }

    #[test]
    fn remote_prompt_names_are_single_safe_basenames() {
        assert_eq!(normalize_system_prompt_name(" Demo_1 ").unwrap(), "Demo_1");
        for invalid in [
            "",
            "../escape",
            "nested/name",
            "name.md",
            "line\nbreak",
            "é",
        ] {
            assert!(
                normalize_system_prompt_name(invalid).is_err(),
                "{invalid:?}"
            );
        }
    }
}
