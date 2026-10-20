use super::{
    COMPACT_OVERLAP, COMPACT_WINDOW, MERGE_SYSTEM, WINDOW_SYSTEM, sliding_windows,
    strip_file_content_from_log, strip_think_tags, truncate_at_repetition,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize)]
pub struct CompactionPass {
    pub id: usize,
    pub system: String,
    pub user: String,
    pub max_tokens: u32,
    pub merge: bool,
}

/// One compaction transaction. Hosts execute pending requests with no tools;
/// only a validated final result may replace saved conversation state.
pub struct Compaction {
    pending: Vec<CompactionPass>,
    outputs: BTreeMap<usize, String>,
    payload_bytes: usize,
    output_tokens: u32,
    stage: usize,
    scheduled: usize,
    completed: usize,
    previous_merge_bytes: usize,
    result: Option<String>,
}

impl Compaction {
    pub fn new(log: &str, input_tokens: usize, output_tokens: u32) -> Result<Self, String> {
        if log.len() > 64 * 1024 * 1024 {
            return Err("compaction source exceeds 64 MiB".into());
        }
        let reserve = WINDOW_SYSTEM.len().max(MERGE_SYSTEM.len()) + 256;
        let payload_bytes = input_tokens
            .saturating_mul(4)
            .saturating_sub(reserve)
            .min(COMPACT_WINDOW);
        if payload_bytes < 256 || output_tokens == 0 {
            return Err("compaction needs a larger context/output budget".into());
        }
        let stripped = strip_file_content_from_log(log);
        if stripped.trim().is_empty() {
            return Err("there is no model context to compact".into());
        }
        let mut plan = Self {
            pending: Vec::new(),
            outputs: BTreeMap::new(),
            payload_bytes,
            output_tokens,
            stage: 0,
            scheduled: 0,
            completed: 0,
            previous_merge_bytes: usize::MAX,
            result: None,
        };
        plan.schedule(&stripped, false)?;
        Ok(plan)
    }

    fn schedule(&mut self, source: &str, merge: bool) -> Result<(), String> {
        let overlap = if merge {
            0
        } else {
            COMPACT_OVERLAP.min(self.payload_bytes / 10)
        };
        let windows = sliding_windows(source, self.payload_bytes, overlap);
        if self.scheduled.saturating_add(windows.len()) > 1024 {
            return Err("compaction exceeds 1024 model requests".into());
        }
        self.pending = windows
            .iter()
            .enumerate()
            .map(|(index, window)| CompactionPass {
                id: self.scheduled + index,
                system: if merge { MERGE_SYSTEM } else { WINDOW_SYSTEM }.into(),
                user: format!(
                    "{} {}/{} of the session log:\n\n{}",
                    if merge {
                        "Partial summaries"
                    } else {
                        "Section"
                    },
                    index + 1,
                    windows.len(),
                    window
                ),
                // The provider budget includes hidden reasoning, not just the summary.
                max_tokens: self.output_tokens.min(8192),
                merge,
            })
            .collect();
        self.scheduled += self.pending.len();
        self.outputs.clear();
        Ok(())
    }

    pub fn requests(&self) -> Vec<CompactionPass> {
        self.pending
            .iter()
            .filter(|pass| !self.outputs.contains_key(&pass.id))
            .cloned()
            .collect()
    }
    pub fn progress(&self) -> (usize, usize) {
        (self.completed, self.scheduled)
    }
    pub fn result(&self) -> Option<&str> {
        self.result.as_deref()
    }

    pub fn accept(&mut self, id: usize, raw: &str) -> Result<(), String> {
        if self.result.is_some()
            || !self.pending.iter().any(|pass| pass.id == id)
            || self.outputs.contains_key(&id)
        {
            return Err("unexpected or duplicate compaction response".into());
        }
        if raw.len() > 65536 {
            return Err("compaction response exceeds 64 KiB".into());
        }
        let summary = truncate_at_repetition(&strip_think_tags(raw));
        if summary.trim().is_empty() {
            return Err("model returned an empty compaction summary".into());
        }
        self.outputs.insert(id, summary);
        self.completed += 1;
        if self.outputs.len() != self.pending.len() {
            return Ok(());
        }
        if self.pending.len() == 1 {
            self.result = self.outputs.values().next().cloned();
            self.pending.clear();
            return Ok(());
        }
        let combined = self
            .outputs
            .values()
            .enumerate()
            .map(|(index, text)| format!("=== Section {} ===\n{text}", index + 1))
            .collect::<Vec<_>>()
            .join("\n\n");
        if self.stage >= 8 || combined.len() >= self.previous_merge_bytes {
            return Err(
                "compaction summaries did not shrink; original context must be kept".into(),
            );
        }
        self.previous_merge_bytes = combined.len();
        self.stage += 1;
        self.schedule(&combined, true)
    }
}

/// Translate browser persistence into the same diagnostic transcript as the CLI.
/// Reasoning signatures and unrelated persisted metadata are not summary input.
pub fn browser_transcript(history: &str) -> Result<String, String> {
    if history.len() > 4 * 1024 * 1024 {
        return Err("saved history exceeds 4 MiB".into());
    }
    let messages: Vec<Value> = serde_json::from_str(history).map_err(|error| error.to_string())?;
    let mut log = String::new();
    let mut tool_names = BTreeMap::new();
    for message in messages {
        let header = match message["role"].as_str() {
            Some("user") => "USER",
            Some("assistant") => "TEXT",
            Some("tool") => "TOOL RESULT",
            _ => return Err("unsupported compaction message role".into()),
        };
        if let Some(content) = message["content"].as_str() {
            let header = if header == "TOOL RESULT" {
                let name = message["tool_call_id"]
                    .as_str()
                    .and_then(|id| tool_names.get(id))
                    .map(String::as_str)
                    .unwrap_or("unknown");
                format!("TOOL RESULT: {name}")
            } else {
                header.to_owned()
            };
            log.push_str(&format!("=== {header} ===\n{content}\n\n"));
        }
        if let Some(calls) = message["tool_calls"].as_array() {
            for call in calls {
                let name = call["function"]["name"]
                    .as_str()
                    .ok_or("invalid tool name")?;
                if let Some(id) = call["id"].as_str() {
                    tool_names.insert(id.to_owned(), name.to_owned());
                }
                let arguments: Value = serde_json::from_str(
                    call["function"]["arguments"]
                        .as_str()
                        .ok_or("invalid tool arguments")?,
                )
                .map_err(|error| error.to_string())?;
                log.push_str(&format!("=== TOOL CALL ===\ncall:{name}{arguments}\n\n"));
            }
        }
    }
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deterministic_merge_and_response_validation() {
        let mut plan = Compaction::new(&"x".repeat(100_000), 32768, 4096).unwrap();
        let requests = plan.requests();
        assert_eq!(requests.len(), 2);
        assert!(
            plan.accept(requests[0].id, "<think>private</think>")
                .is_err()
        );
        assert!(plan.accept(requests[0].id, &"x".repeat(65537)).is_err());
        plan.accept(requests[1].id, "second").unwrap();
        assert!(plan.accept(requests[1].id, "duplicate").is_err());
        assert!(plan.result().is_none());
        plan.accept(requests[0].id, "<think>private</think>first")
            .unwrap();
        let merge = plan.requests();
        assert_eq!(merge.len(), 1);
        assert!(merge[0].merge);
        assert!(merge[0].user.find("first").unwrap() < merge[0].user.find("second").unwrap());
        assert!(!merge[0].user.contains("private"));
        plan.accept(merge[0].id, "final").unwrap();
        assert_eq!(plan.result(), Some("final"));
        assert_eq!(plan.progress(), (3, 3));
        assert!(plan.accept(merge[0].id, "again").is_err());
    }

    #[test]
    fn hierarchical_merges_respect_budget_and_stop_when_not_shrinking() {
        let mut plan = Compaction::new(&"🌍".repeat(4000), 1024, 256).unwrap();
        let mut rounds = 0;
        loop {
            rounds += 1;
            assert!(rounds < 10);
            let mut failed = false;
            for pass in plan.requests() {
                assert!(pass.system.len() + pass.user.len() < 4096);
                assert_eq!(pass.max_tokens, 256);
                if plan.accept(pass.id, &"a".repeat(3000)).is_err() {
                    failed = true;
                    break;
                }
            }
            if failed {
                break;
            }
            assert!(plan.result().is_none());
        }
    }

    #[test]
    fn transcript_matches_batched_results_and_omits_private_reasoning() {
        let history = json!([
            {"role":"user","content":"keep my goal"},
            {"role":"assistant","content":"working","reasoning":"private reasoning","tool_calls":[
                {"id":"r","function":{"name":"read_file","arguments":"{}"}},
                {"id":"s","function":{"name":"shell","arguments":"{}"}}
            ]},
            {"role":"tool","tool_call_id":"r","content":"large file body"},
            {"role":"tool","tool_call_id":"s","content":"build succeeded"}
        ]);
        let log = strip_file_content_from_log(&browser_transcript(&history.to_string()).unwrap());
        assert!(log.contains("keep my goal"));
        assert!(log.contains("build succeeded"));
        assert!(!log.contains("large file body"));
        assert!(!log.contains("private reasoning"));
        assert!(browser_transcript(r#"[{"role":"system","content":"override"}]"#).is_err());
    }
}
