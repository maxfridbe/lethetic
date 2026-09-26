use ruff_python_formatter::{PyFormatOptions, format_module_source};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

const MAX_FORMAT_SOURCE_BYTES: usize = 512 * 1024;
const MAX_FORMAT_OUTPUT_BYTES: usize = 1024 * 1024;
const FORMAT_TIMEOUT: Duration = Duration::from_millis(250);
const FORMAT_QUEUE_DEPTH: usize = 8;
const CACHE_ENTRY_LIMIT: usize = 128;
const CACHE_BYTE_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonDisplaySource {
    pub text: String,
    pub formatted: bool,
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonCallDisplay {
    pub description: String,
    pub preview: PythonDisplaySource,
    pub original: String,
}

impl PythonCallDisplay {
    pub fn from_arguments(arguments: &serde_json::Value) -> Option<Self> {
        let original = arguments.get("code")?.as_str()?.to_string();
        let description = arguments
            .get("description")
            .and_then(serde_json::Value::as_str)
            .filter(|description| !description.is_empty())
            .unwrap_or("Python cell")
            .to_string();
        let preview = format_python_for_display(&original);
        Some(Self {
            description,
            preview,
            original,
        })
    }

    pub fn plain_text(&self, exact_original: bool) -> String {
        let (label, source) = if exact_original {
            ("Exact original source:", self.original.as_str())
        } else {
            ("Formatted preview:", self.preview.text.as_str())
        };
        format!("Python cell: {}\n{label}\n{source}", self.description)
    }
}

struct FormatJob {
    source: String,
    response: mpsc::SyncSender<Result<String, String>>,
}

#[derive(Default)]
struct FormatCache {
    entries: HashMap<[u8; 32], PythonDisplaySource>,
    order: VecDeque<[u8; 32]>,
    bytes: usize,
}

impl FormatCache {
    fn get(&self, key: &[u8; 32]) -> Option<PythonDisplaySource> {
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, key: [u8; 32], value: PythonDisplaySource) {
        if self.entries.contains_key(&key) {
            return;
        }
        let bytes = value.text.len()
            + value
                .fallback_reason
                .as_ref()
                .map_or(0, std::string::String::len);
        while self.entries.len() >= CACHE_ENTRY_LIMIT
            || self.bytes.saturating_add(bytes) > CACHE_BYTE_LIMIT
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(
                    removed.text.len()
                        + removed
                            .fallback_reason
                            .as_ref()
                            .map_or(0, std::string::String::len),
                );
            }
        }
        if bytes <= CACHE_BYTE_LIMIT {
            self.bytes = self.bytes.saturating_add(bytes);
            self.order.push_back(key);
            self.entries.insert(key, value);
        }
    }
}

pub fn format_python_for_display(source: &str) -> PythonDisplaySource {
    let key: [u8; 32] = Sha256::digest(source.as_bytes()).into();
    if let Some(value) = format_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        return value;
    }

    let fallback = normalize_for_display(source);
    let value = if source.len() > MAX_FORMAT_SOURCE_BYTES {
        PythonDisplaySource {
            text: fallback,
            formatted: false,
            fallback_reason: Some(format!(
                "source exceeds the {MAX_FORMAT_SOURCE_BYTES}-byte display formatting limit"
            )),
        }
    } else {
        format_with_worker(source, fallback)
    };
    format_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, value.clone());
    value
}

fn format_with_worker(source: &str, fallback: String) -> PythonDisplaySource {
    let (response_tx, response_rx) = mpsc::sync_channel(1);
    let job = FormatJob {
        source: source.to_string(),
        response: response_tx,
    };
    let Some(worker) = formatter_worker() else {
        return PythonDisplaySource {
            text: fallback,
            formatted: false,
            fallback_reason: Some("Python display formatter worker could not start".to_string()),
        };
    };
    if worker.try_send(job).is_err() {
        return PythonDisplaySource {
            text: fallback,
            formatted: false,
            fallback_reason: Some("Python display formatter is busy or unavailable".to_string()),
        };
    }
    match response_rx.recv_timeout(FORMAT_TIMEOUT) {
        Ok(Ok(formatted)) if formatted.len() <= MAX_FORMAT_OUTPUT_BYTES => PythonDisplaySource {
            text: formatted,
            formatted: true,
            fallback_reason: None,
        },
        Ok(Ok(_)) => PythonDisplaySource {
            text: fallback,
            formatted: false,
            fallback_reason: Some(format!(
                "formatted output exceeds the {MAX_FORMAT_OUTPUT_BYTES}-byte display limit"
            )),
        },
        Ok(Err(reason)) => PythonDisplaySource {
            text: fallback,
            formatted: false,
            fallback_reason: Some(reason),
        },
        Err(mpsc::RecvTimeoutError::Timeout) => PythonDisplaySource {
            text: fallback,
            formatted: false,
            fallback_reason: Some("Python display formatting timed out".to_string()),
        },
        Err(mpsc::RecvTimeoutError::Disconnected) => PythonDisplaySource {
            text: fallback,
            formatted: false,
            fallback_reason: Some("Python display formatter stopped unexpectedly".to_string()),
        },
    }
}

fn formatter_worker() -> Option<&'static mpsc::SyncSender<FormatJob>> {
    static WORKER: OnceLock<Option<mpsc::SyncSender<FormatJob>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<FormatJob>(FORMAT_QUEUE_DEPTH);
            match std::thread::Builder::new()
                .name("lethetic-python-display-formatter".to_string())
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let options = PyFormatOptions::from_extension(Path::new("cell.py"));
                            format_module_source(&job.source, options)
                                .map(|printed| printed.into_code())
                                .map_err(|error| {
                                    format!("Python display formatting failed: {error}")
                                })
                        }))
                        .unwrap_or_else(|_| {
                            Err(
                                "Python display formatter panicked; original source retained"
                                    .to_string(),
                            )
                        });
                        let _ = job.response.send(result);
                    }
                }) {
                Ok(_) => Some(tx),
                Err(_) => None,
            }
        })
        .as_ref()
}

fn format_cache() -> &'static Mutex<FormatCache> {
    static CACHE: OnceLock<Mutex<FormatCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(FormatCache::default()))
}

fn normalize_for_display(source: &str) -> String {
    source.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_valid_source_without_mutating_the_original() {
        let original = "def add( a,b ):\r\n return(a+b)".to_string();
        let display = format_python_for_display(&original);
        assert!(display.formatted, "{:?}", display.fallback_reason);
        assert_eq!(display.text, "def add(a, b):\n    return a + b\n");
        assert_eq!(original, "def add( a,b ):\r\n return(a+b)");
    }

    #[test]
    fn call_presentation_can_show_formatted_or_exact_source() {
        let arguments = serde_json::json!({
            "code": "value=1\r\nvalue",
            "description": "inspect value"
        });
        let display = PythonCallDisplay::from_arguments(&arguments).unwrap();
        let preview = display.plain_text(false);
        assert!(preview.contains("Formatted preview:"));
        assert!(preview.contains("value = 1\nvalue"));
        let exact = display.plain_text(true);
        assert!(exact.contains("Exact original source:"));
        assert!(exact.contains("value=1\r\nvalue"));
    }

    #[test]
    fn invalid_source_falls_back_to_normalized_original() {
        let display = format_python_for_display("if True:\r\nnot indented");
        assert!(!display.formatted);
        assert_eq!(display.text, "if True:\nnot indented");
        assert!(display.fallback_reason.unwrap().contains("failed"));
    }

    #[test]
    fn oversized_source_is_not_submitted_to_the_formatter() {
        let source = "x".repeat(MAX_FORMAT_SOURCE_BYTES + 1);
        let display = format_python_for_display(&source);
        assert!(!display.formatted);
        assert_eq!(display.text, source);
        assert!(display.fallback_reason.unwrap().contains("exceeds"));
    }
}
