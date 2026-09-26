use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Typed authority for presenting and recovering a tool's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutputProvenance {
    /// Ordinary host-side output may be retained in secure workspace storage.
    OrdinaryHost,
    /// Python output is recoverable only through the worker-local cell artifact.
    PythonCell(crate::python::PythonOutputMetadata),
}

impl Default for ToolOutputProvenance {
    fn default() -> Self {
        Self::OrdinaryHost
    }
}

/// Tool outputs larger than this (in bytes) are shortened for display and context.
pub(crate) const LARGE_OUTPUT_THRESHOLD: usize = 20_000;
const OUTPUT_HEAD_BYTES: usize = 8_000;
const OUTPUT_TAIL_BYTES: usize = 8_000;
const PYTHON_ERROR_HEAD_BYTES: usize = 4_000;
const PYTHON_ERROR_TAIL_BYTES: usize = 12_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeOutputHandling {
    pub context: String,
    pub ui: String,
    pub storage_failed: bool,
}

impl LargeOutputHandling {
    fn into_parts(self) -> (String, String) {
        (self.context, self.ui)
    }

    pub fn effective_is_error(&self, execution_is_error: bool) -> bool {
        execution_is_error || self.storage_failed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedToolExecution {
    pub context: String,
    pub ui: String,
    pub cwd: String,
    pub is_error: bool,
}

/// Present one execution exactly once, preserving its output authority.
pub fn present_tool_execution(id: &str, execution: super::ToolExecution) -> PresentedToolExecution {
    let workspace = process_workspace();
    present_tool_execution_in(workspace, id, execution)
}

pub(crate) fn present_tool_execution_in(
    workspace_root: &Path,
    id: &str,
    execution: super::ToolExecution,
) -> PresentedToolExecution {
    let handled = match &execution.provenance {
        ToolOutputProvenance::OrdinaryHost => {
            handle_large_output_classified_in(workspace_root, id, execution.output)
        }
        ToolOutputProvenance::PythonCell(metadata) => {
            handle_python_output(execution.output, metadata, execution.is_error)
        }
    };
    PresentedToolExecution {
        is_error: handled.effective_is_error(execution.is_error),
        context: handled.context,
        ui: handled.ui,
        cwd: execution.cwd,
    }
}

/// Backwards-compatible ordinary-host presentation helper.
pub fn handle_large_output(id: &str, result: String) -> (String, String) {
    handle_large_output_classified(id, result).into_parts()
}

/// Backwards-compatible ordinary-host presentation helper.
pub fn handle_large_output_classified(id: &str, result: String) -> LargeOutputHandling {
    handle_large_output_classified_in(process_workspace(), id, result)
}

fn process_workspace() -> &'static Path {
    static PROCESS_WORKSPACE: OnceLock<PathBuf> = OnceLock::new();
    PROCESS_WORKSPACE
        .get_or_init(|| {
            let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            root.canonicalize().unwrap_or(root)
        })
        .as_path()
}

pub(crate) fn handle_large_output_classified_in(
    workspace_root: &Path,
    id: &str,
    result: String,
) -> LargeOutputHandling {
    if result.len() <= LARGE_OUTPUT_THRESHOLD {
        return unchanged(result);
    }

    let total_bytes = result.len();
    let excerpt = bounded_head_tail(&result);
    let file_name = large_output_file_name(id, result.as_bytes());
    match crate::platform::atomic_write_nofollow(
        workspace_root,
        &[".lethetic", "tool_responses"],
        &file_name,
        result.as_bytes(),
        0o600,
    ) {
        Ok(file_path) => {
            let display_path = file_path.display();
            LargeOutputHandling {
                context: format!(
                    "{excerpt}\n\n... [OUTPUT TRUNCATED ({total_bytes} bytes). Full output saved to `{display_path}`. Use `summarize_content` on that file or `read_file_lines` for a bounded section.] ..."
                ),
                ui: format!(
                    "{excerpt}\n\n... [Output truncated ({total_bytes} bytes). Full output saved to {display_path}.] ..."
                ),
                storage_failed: false,
            }
        }
        Err(error) => {
            let message = format!(
                "Full output was not saved because secure host storage rejected the path: {error}"
            );
            LargeOutputHandling {
                context: format!(
                    "{excerpt}\n\n... [OUTPUT TRUNCATED ({total_bytes} bytes). {message}] ..."
                ),
                ui: format!("{excerpt}\n\n... [Output truncated. {message}] ..."),
                storage_failed: true,
            }
        }
    }
}

fn handle_python_output(
    result: String,
    metadata: &crate::python::PythonOutputMetadata,
    is_error: bool,
) -> LargeOutputHandling {
    let presenter_shortened = result.len() > LARGE_OUTPUT_THRESHOLD;
    let worker_shortened = metadata.has_recoverable_output();
    let capture_loss = metadata.has_capture_loss();
    if !presenter_shortened && !worker_shortened && !capture_loss {
        return unchanged(result);
    }

    let excerpt = if presenter_shortened {
        let (head_bytes, tail_bytes) = if is_error {
            (PYTHON_ERROR_HEAD_BYTES, PYTHON_ERROR_TAIL_BYTES)
        } else {
            (OUTPUT_HEAD_BYTES, OUTPUT_TAIL_BYTES)
        };
        let head = bounded_prefix(&result, head_bytes);
        let tail = bounded_suffix(&result, tail_bytes);
        let omitted = result
            .len()
            .saturating_sub(head.len().saturating_add(tail.len()));
        format!(
            "{head}\n\n... [Python output excerpt; {omitted} rendered bytes omitted] ...\n\n{tail}",
        )
    } else {
        result
    };
    let hint = python_recovery_hint(metadata, capture_loss);
    let presented = format!("{excerpt}\n\n{hint}");
    LargeOutputHandling {
        context: presented.clone(),
        ui: presented,
        storage_failed: false,
    }
}

fn python_recovery_hint(
    metadata: &crate::python::PythonOutputMetadata,
    capture_loss: bool,
) -> String {
    let artifact_id = serde_json::to_string(&metadata.artifact_id)
        .unwrap_or_else(|_| "\"invalid-artifact-id\"".to_string());
    if !metadata.retained {
        return format!(
            "[Python output was shortened, but worker-local artifact {artifact_id} for cell {} is no longer retained.]",
            metadata.cell
        );
    }

    let sections = metadata
        .sections
        .iter()
        .filter(|section| section.captured_bytes != 0)
        .map(|section| section.section.as_str())
        .collect::<Vec<_>>();
    let section_list = if sections.is_empty() {
        "stdout, stderr, repr, traceback".to_string()
    } else {
        sections.join(", ")
    };
    let example_section = sections.first().copied().unwrap_or("stdout");
    let loss_notice = if capture_loss {
        " Some original bytes exceeded the worker capture limits and are not recoverable."
    } else {
        ""
    };
    format!(
        "[Python output shortened. Worker-local artifact {artifact_id} for cell {} (sections: {section_list}).\nRecover it only inside a later Python cell:\nimport lethetic_output\nlethetic_output.info({artifact_id})\nlethetic_output.read({artifact_id}, {:?}, 0, {})\nOffsets and limits are bytes; artifacts are bounded and may expire as newer cells run.{loss_notice}]",
        metadata.cell,
        example_section,
        crate::python::PYTHON_OUTPUT_READ_MAX_BYTES,
    )
}

fn unchanged(result: String) -> LargeOutputHandling {
    LargeOutputHandling {
        context: result.clone(),
        ui: result,
        storage_failed: false,
    }
}

fn bounded_head_tail(value: &str) -> String {
    let head = bounded_prefix(value, OUTPUT_HEAD_BYTES);
    let tail = bounded_suffix(value, OUTPUT_TAIL_BYTES);
    let omitted = value
        .len()
        .saturating_sub(head.len().saturating_add(tail.len()));
    format!("{head}\n\n... [{omitted} bytes omitted; showing bounded head and tail] ...\n\n{tail}",)
}

fn bounded_prefix(value: &str, maximum_bytes: usize) -> &str {
    if value.len() <= maximum_bytes {
        return value;
    }
    let mut end = maximum_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn bounded_suffix(value: &str, maximum_bytes: usize) -> &str {
    if value.len() <= maximum_bytes {
        return value;
    }
    let mut start = value.len() - maximum_bytes;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

pub(crate) fn large_output_file_name(id: &str, content: &[u8]) -> String {
    // Provider IDs are untrusted and may contain separators or `..`. Fixed
    // hashes keep them out of path parsing, while the content digest makes a
    // reused call ID immutable with respect to earlier transcript references.
    let bytes = if id.is_empty() {
        b"unknown".as_slice()
    } else {
        id.as_bytes()
    };
    let mut first = 0xcbf29ce484222325_u64;
    let mut second = 0x84222325cbf29ce4_u64;
    for (index, byte) in bytes.iter().copied().enumerate() {
        first ^= u64::from(byte);
        first = first.wrapping_mul(0x100000001b3);
        second ^= u64::from(byte) ^ (index as u64).rotate_left(17);
        second = second.wrapping_mul(0x100000001b3);
    }
    let content_hash = Sha256::digest(content);
    format!("response-{first:016x}{second:016x}-{content_hash:x}.txt")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python_metadata(
        cell: u64,
        captured: u64,
        original: u64,
    ) -> crate::python::PythonOutputMetadata {
        crate::python::PythonOutputMetadata {
            cell,
            artifact_id: "11111111-2222-4333-8444-555555555555".to_string(),
            retained: true,
            sections: [
                crate::python::PythonOutputSection::Stdout,
                crate::python::PythonOutputSection::Stderr,
                crate::python::PythonOutputSection::Repr,
                crate::python::PythonOutputSection::Traceback,
            ]
            .into_iter()
            .map(|section| crate::python::PythonOutputSectionMetadata {
                section,
                captured_bytes: if section == crate::python::PythonOutputSection::Stdout {
                    captured
                } else {
                    0
                },
                original_bytes: if section == crate::python::PythonOutputSection::Stdout {
                    original
                } else {
                    0
                },
                excerpt_bytes: if section == crate::python::PythonOutputSection::Stdout {
                    captured.min(64 * 1024)
                } else {
                    0
                },
                truncated: original > captured,
            })
            .collect(),
        }
    }

    #[test]
    fn large_python_output_uses_only_worker_local_recovery() {
        let workspace = tempfile::tempdir().unwrap();
        let output = format!(
            "BEGIN_SENTINEL\n{}\nSTDERR_TAIL_SENTINEL\n{}\nTRACEBACK_TAIL_SENTINEL",
            "界".repeat(LARGE_OUTPUT_THRESHOLD),
            "z".repeat(9_000),
        );
        let execution = super::super::ToolExecution {
            output,
            cwd: "/worker/cwd".to_string(),
            is_error: true,
            provenance: ToolOutputProvenance::PythonCell(python_metadata(17, 80_000, 90_000)),
        };

        let presented = present_tool_execution_in(workspace.path(), "python-call", execution);

        assert!(presented.context.contains("BEGIN_SENTINEL"));
        assert!(presented.context.contains("STDERR_TAIL_SENTINEL"));
        assert!(presented.context.contains("TRACEBACK_TAIL_SENTINEL"));
        assert!(
            presented
                .context
                .contains("lethetic_output.info(\"11111111-2222-4333-8444-555555555555\")")
        );
        assert!(presented.context.contains("not recoverable"));
        assert!(presented.context.contains(
            "lethetic_output.read(\"11111111-2222-4333-8444-555555555555\", \"stdout\", 0, 65536)"
        ));
        let workspace_text = workspace.path().to_string_lossy();
        for forbidden in [
            ".lethetic",
            "read_file",
            "read_file_lines",
            "summarize_content",
            workspace_text.as_ref(),
        ] {
            assert!(!presented.context.contains(forbidden), "{forbidden:?}");
        }
        assert!(presented.is_error);
        assert!(presented.context.len() < LARGE_OUTPUT_THRESHOLD);
        assert!(presented.ui.len() < LARGE_OUTPUT_THRESHOLD);
        assert!(!workspace.path().join(".lethetic").exists());
    }

    #[test]
    fn large_host_output_keeps_head_tail_and_secure_artifact() {
        let workspace = tempfile::tempdir().unwrap();
        let output = format!("HEAD_SENTINEL{}TAIL_SENTINEL", "x".repeat(30_000));
        let execution = super::super::ToolExecution {
            output: output.clone(),
            cwd: workspace.path().to_string_lossy().into_owned(),
            is_error: false,
            provenance: ToolOutputProvenance::OrdinaryHost,
        };

        let presented = present_tool_execution_in(workspace.path(), "host-call", execution);

        assert!(presented.context.contains("HEAD_SENTINEL"));
        assert!(presented.context.contains("TAIL_SENTINEL"));
        assert!(presented.context.contains("OUTPUT TRUNCATED"));
        assert!(!presented.is_error);
        let stored = workspace
            .path()
            .join(".lethetic/tool_responses")
            .join(large_output_file_name("host-call", output.as_bytes()));
        assert_eq!(std::fs::read_to_string(stored).unwrap(), output);
    }

    #[test]
    fn reused_call_id_cannot_replace_an_earlier_host_artifact() {
        let workspace = tempfile::tempdir().unwrap();
        let first = format!("FIRST{}FIRST_TAIL", "a".repeat(30_000));
        let second = format!("SECOND{}SECOND_TAIL", "b".repeat(30_000));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workspace_path = workspace.path();

        let (first_presented, second_presented) = std::thread::scope(|scope| {
            let first_barrier = barrier.clone();
            let first_output = first.clone();
            let first_run = scope.spawn(move || {
                first_barrier.wait();
                handle_large_output_classified_in(workspace_path, "reused", first_output)
            });
            let second_barrier = barrier.clone();
            let second_output = second.clone();
            let second_run = scope.spawn(move || {
                second_barrier.wait();
                handle_large_output_classified_in(workspace_path, "reused", second_output)
            });
            barrier.wait();
            (first_run.join().unwrap(), second_run.join().unwrap())
        });

        let first_name = large_output_file_name("reused", first.as_bytes());
        let second_name = large_output_file_name("reused", second.as_bytes());
        assert_ne!(first_name, second_name);
        assert!(first_presented.context.contains(&first_name));
        assert!(second_presented.context.contains(&second_name));
        let responses = workspace.path().join(".lethetic/tool_responses");
        assert_eq!(
            std::fs::read_to_string(responses.join(first_name)).unwrap(),
            first
        );
        assert_eq!(
            std::fs::read_to_string(responses.join(second_name)).unwrap(),
            second
        );
    }
}
