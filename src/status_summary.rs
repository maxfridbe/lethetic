use crate::accounting::{AccountingTotals, EstimatedCost, Usage};
use crate::app::App;
use crate::config::{
    AccessMode, ConnectionKind, NetworkAccess, PythonExecutionTarget, SandboxBackend, ToolProfile,
};
use crate::python_policy::PythonPolicySource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextUsageSource {
    ServerUsage,
    ServerPrompt,
    LocalEstimate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoarseGitState {
    Clean,
    Dirty,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonStatusSummary {
    pub profile: ToolProfile,
    pub target: Option<PythonExecutionTarget>,
    pub backend: Option<SandboxBackend>,
    pub network: Option<NetworkAccess>,
    pub workspace_access: Option<AccessMode>,
    pub grant_count: usize,
    pub policy_source: PythonPolicySource,
}

impl PythonStatusSummary {
    pub fn display(&self) -> String {
        match self.profile {
            ToolProfile::General => format!("General/{}", self.policy_source.label()),
            ToolProfile::PythonOnly => match self.target {
                Some(PythonExecutionTarget::Host) => {
                    format!("Python Host/{}", self.policy_source.label())
                }
                Some(PythonExecutionTarget::Sandbox) => {
                    let backend = match self.backend {
                        Some(SandboxBackend::Bubblewrap) => "bwrap",
                        Some(SandboxBackend::Podman) => "podman",
                        None => "unresolved",
                    };
                    let network = match self.network {
                        Some(NetworkAccess::None) => "net:none",
                        Some(NetworkAccess::Nonlocal) => "net:public-only",
                        Some(NetworkAccess::Full) => "net:full",
                        None => "net:?",
                    };
                    let workspace = match self.workspace_access {
                        Some(AccessMode::ReadOnly) => "ws:ro",
                        Some(AccessMode::ReadWrite) => "ws:rw",
                        None => "ws:?",
                    };
                    format!(
                        "Python {backend} {network} {workspace} +{} /{}",
                        self.grant_count,
                        self.policy_source.label()
                    )
                }
                None => format!("Python unresolved/{}", self.policy_source.label()),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct StatusSummary {
    pub stop_reason: String,
    pub model_label: String,
    pub provider_label: String,
    pub provider_kind: ConnectionKind,
    pub python: PythonStatusSummary,
    pub tokens_per_second: Option<f64>,
    pub prompt_tokens_per_second: Option<f64>,
    pub context_tokens: u64,
    pub context_limit_tokens: u64,
    pub context_source: ContextUsageSource,
    pub request_usage: Option<Usage>,
    pub latest_turn_cost: Option<EstimatedCost>,
    pub session_cost: Option<EstimatedCost>,
    pub memory_mebibytes: u64,
    pub file_count: usize,
    pub visible_block_count: usize,
    pub git_state: CoarseGitState,
}

impl StatusSummary {
    pub fn from_app(app: &App) -> Self {
        let provider_kind = app.config.active_connection_kind();
        let provider_label = app
            .config
            .active_model_server()
            .map(|server| server.name.trim())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| match provider_kind {
                ConnectionKind::OpenAiChatCompletions => "OpenAI-compatible".to_string(),
                ConnectionKind::ClaudeCodeProxy => "Claude Code proxy".to_string(),
            });
        let sandbox_active = app.config.tool_profile == ToolProfile::PythonOnly
            && app.config.python_runtime.target == Some(PythonExecutionTarget::Sandbox);
        let python = PythonStatusSummary {
            profile: app.config.tool_profile,
            target: (app.config.tool_profile == ToolProfile::PythonOnly)
                .then_some(app.config.python_runtime.target)
                .flatten(),
            backend: sandbox_active
                .then_some(app.config.python_runtime.sandbox.backend)
                .flatten(),
            network: sandbox_active
                .then_some(app.config.python_runtime.sandbox.network)
                .flatten(),
            workspace_access: sandbox_active
                .then_some(app.config.python_runtime.sandbox.workspace_access)
                .flatten(),
            grant_count: if sandbox_active {
                app.config.python_runtime.sandbox.grants.len()
            } else {
                0
            },
            policy_source: app.python_policy.effective_source(),
        };
        let (context_tokens, context_source) = if let Some(usage) = app.server_usage {
            (usage.total_input(), ContextUsageSource::ServerUsage)
        } else if let Some(prompt_tokens) = app.server_prompt_tokens {
            (u64::from(prompt_tokens), ContextUsageSource::ServerPrompt)
        } else {
            (
                app.context_manager.get_token_count() as u64,
                ContextUsageSource::LocalEstimate,
            )
        };
        let request_usage = app.server_usage.or_else(|| {
            Usage::from_legacy_counts(
                app.server_prompt_tokens.map(u64::from),
                app.server_completion_tokens.map(u64::from),
            )
        });
        let estimate_cost = app.config.estimate_cost.unwrap_or(true);

        Self {
            stop_reason: app.stop_reason.clone(),
            model_label: app.model_name.clone(),
            provider_label,
            provider_kind,
            python,
            tokens_per_second: finite_rate(app.tokens_per_s),
            prompt_tokens_per_second: finite_rate(app.pp_tokens_per_s),
            context_tokens,
            context_limit_tokens: app.max_tokens as u64,
            context_source,
            request_usage,
            latest_turn_cost: if estimate_cost {
                accounting_cost(&app.accounting.latest_logical_turn)
            } else {
                None
            },
            session_cost: if estimate_cost {
                accounting_cost(&app.accounting.session)
            } else {
                None
            },
            memory_mebibytes: app.memory_usage,
            file_count: app
                .context_manager
                .active_files
                .len()
                .saturating_add(app.context_manager.latest_files.len()),
            visible_block_count: app.blocks.len(),
            git_state: coarse_git_state(&app.git_status),
        }
    }
}

pub fn accounting_cost(totals: &AccountingTotals) -> Option<EstimatedCost> {
    let mut cost = totals.estimated_cost.clone()?;
    if totals.unpriced_request_count > 0 || totals.incomplete_usage_request_count > 0 {
        cost.incomplete = true;
    }
    Some(cost)
}

/// "cost" for charges the provider reported, "EST API-eq" for estimates
/// from a price table.
pub fn cost_label(cost: &EstimatedCost) -> &'static str {
    if cost.provenance_kind == "provider_reported" {
        "cost"
    } else {
        "EST API-eq"
    }
}

pub fn format_estimated_cost(cost: &EstimatedCost) -> String {
    let amount = if cost.currency == "USD" {
        format!("${:.6}", cost.amount())
    } else {
        format!("{:.6} {}", cost.amount(), cost.currency)
    };
    let incomplete = if cost.incomplete { "*" } else { "" };
    let stale = if cost.is_stale_on(chrono::Local::now().date_naive()) {
        "†"
    } else {
        ""
    };
    format!("{amount}{incomplete}{stale}")
}

pub fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    if tokens < 1_000_000 {
        return format!("{:.1}k", tokens as f64 / 1_000.0);
    }
    format!("{:.1}m", tokens as f64 / 1_000_000.0)
}

fn finite_rate(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn coarse_git_state(status: &str) -> CoarseGitState {
    let status = status.trim();
    if status.eq_ignore_ascii_case("clean")
        || status
            .to_ascii_lowercase()
            .strip_suffix(" (clean)")
            .is_some()
    {
        CoarseGitState::Clean
    } else if status.is_empty()
        || status.eq_ignore_ascii_case("n/a")
        || status.eq_ignore_ascii_case("not a git repo")
        || status.eq_ignore_ascii_case("unknown")
        || status.eq_ignore_ascii_case("unavailable")
    {
        CoarseGitState::Unknown
    } else {
        CoarseGitState::Dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn summary_prefers_server_usage_and_marks_incomplete_accounting() {
        let mut app = App::new(&Config::default());
        app.server_usage = Some(Usage {
            uncached_input_tokens: 3,
            cache_read_input_tokens: 7,
            output_tokens: 2,
            total_input_tokens: Some(10),
            breakdown_complete: true,
            ..Usage::default()
        });
        app.server_prompt_tokens = Some(999);
        app.accounting.latest_logical_turn.estimated_cost = Some(EstimatedCost {
            currency: "USD".to_string(),
            nanos: 1_000,
            incomplete: false,
            mixed_pricing: false,
            long_context_applied: false,
            pricing_effective_as_of: "2026-01-01".to_string(),
            pricing_valid_through: None,
            provenance_kind: "test".to_string(),
        });
        app.accounting.latest_logical_turn.unpriced_request_count = 1;

        let summary = StatusSummary::from_app(&app);
        assert_eq!(summary.context_tokens, 10);
        assert_eq!(summary.context_source, ContextUsageSource::ServerUsage);
        assert_eq!(summary.request_usage, app.server_usage);
        assert!(summary.latest_turn_cost.unwrap().incomplete);
    }

    #[test]
    fn summary_falls_back_from_server_prompt_to_local_estimate() {
        let mut app = App::new(&Config::default());
        app.server_prompt_tokens = Some(42);
        let summary = StatusSummary::from_app(&app);
        assert_eq!(summary.context_tokens, 42);
        assert_eq!(summary.context_source, ContextUsageSource::ServerPrompt);

        app.server_prompt_tokens = None;
        let summary = StatusSummary::from_app(&app);
        assert_eq!(
            summary.context_tokens,
            app.context_manager.get_token_count() as u64
        );
        assert_eq!(summary.context_source, ContextUsageSource::LocalEstimate);
    }

    #[test]
    fn summary_honors_disabled_cost_estimation() {
        let config = Config {
            estimate_cost: Some(false),
            ..Config::default()
        };
        let mut app = App::new(&config);
        let cost = EstimatedCost {
            currency: "USD".to_string(),
            nanos: 1_000,
            incomplete: false,
            mixed_pricing: false,
            long_context_applied: false,
            pricing_effective_as_of: "2026-01-01".to_string(),
            pricing_valid_through: None,
            provenance_kind: "test".to_string(),
        };
        app.accounting.latest_logical_turn.estimated_cost = Some(cost.clone());
        app.accounting.session.estimated_cost = Some(cost);

        let summary = StatusSummary::from_app(&app);
        assert_eq!(summary.latest_turn_cost, None);
        assert_eq!(summary.session_cost, None);
    }

    #[test]
    fn coarse_git_state_understands_only_producer_sentinels_and_clean_suffix() {
        assert_eq!(coarse_git_state("clean"), CoarseGitState::Clean);
        assert_eq!(
            coarse_git_state(" feature/error-reporting (clean)"),
            CoarseGitState::Clean
        );
        assert_eq!(coarse_git_state("not a git repo"), CoarseGitState::Unknown);
        assert_eq!(coarse_git_state("N/A"), CoarseGitState::Unknown);
        assert_eq!(
            coarse_git_state(" feature/error-reporting ~1"),
            CoarseGitState::Dirty
        );
        assert_eq!(
            coarse_git_state(" feature/unknown-state ?1"),
            CoarseGitState::Dirty
        );
    }
}

/// Human-readable size: 512B, 14K, 200M, 1.4G.
pub fn format_bytes(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let value = bytes as f64;
    if value < K {
        format!("{bytes}B")
    } else if value < K * K {
        format!("{:.0}K", value / K)
    } else if value < K * K * K {
        format!("{:.0}M", value / (K * K))
    } else {
        format!("{:.1}G", value / (K * K * K))
    }
}

#[cfg(test)]
mod format_bytes_tests {
    #[test]
    fn sizes_are_short() {
        assert_eq!(super::format_bytes(512), "512B");
        assert_eq!(super::format_bytes(200 * 1024 * 1024), "200M");
        assert_eq!(super::format_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5G");
    }
}

/// "850ms", "4.2s", "2m 05s", "1h 03m".
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else if ms < 3_600_000 {
        format!("{}m {:02}s", ms / 60_000, (ms / 1_000) % 60)
    } else {
        format!("{}h {:02}m", ms / 3_600_000, (ms / 60_000) % 60)
    }
}

/// Time a session spent with the model working, tools running, and Lethetic
/// waiting for the user (including approval prompts and questions).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionTimes {
    #[serde(default)]
    pub engine_ms: u64,
    #[serde(default)]
    pub tool_ms: u64,
    #[serde(default)]
    pub idle_ms: u64,
}

/// Compact duration for the status line: `45s`, `33m`, `3h 05m`.
pub fn format_compact_duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

impl crate::app::App {
    /// Adds the time since the last call to the bucket the app is in now.
    /// The run loop calls this every pass.
    pub fn accrue_session_time(&mut self) {
        let now = std::time::Instant::now();
        let Some(last) = self.session_times_tick.replace(now) else {
            return;
        };
        let elapsed = u64::try_from(now.duration_since(last).as_millis()).unwrap_or(u64::MAX);
        let bucket = if self.is_executing_tool {
            &mut self.session_times.tool_ms
        } else if self.is_processing || self.active_request_id.is_some() {
            &mut self.session_times.engine_ms
        } else {
            &mut self.session_times.idle_ms
        };
        *bucket = bucket.saturating_add(elapsed);
    }
}

/// The line shown under a timed block, if it has a duration.
pub fn block_duration_label(block: &crate::app::RenderBlock) -> Option<String> {
    use crate::app::BlockType;
    let duration = format_duration_ms(block.duration_ms?);
    match block.block_type {
        BlockType::Thought => Some(format!("engine thought for {duration}")),
        BlockType::ToolResult => Some(format!("tool call took {duration}")),
        BlockType::ToolError => Some(format!("tool call failed after {duration}")),
        BlockType::Text => Some(format!("model turn took {duration}")),
        BlockType::ToolCall => Some(format!("model turn took {duration} to make this call")),
        _ => None,
    }
}

#[cfg(test)]
mod duration_tests {
    #[test]
    fn durations_read_naturally() {
        assert_eq!(super::format_duration_ms(850), "850ms");
        assert_eq!(super::format_duration_ms(4_200), "4.2s");
        assert_eq!(super::format_duration_ms(125_000), "2m 05s");
        assert_eq!(super::format_duration_ms(3_780_000), "1h 03m");
    }
}
