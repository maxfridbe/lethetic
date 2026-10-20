use crate::accounting::PricingConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    #[default]
    OpenAiChatCompletions,
    ClaudeCodeProxy,
}

impl ConnectionKind {
    pub fn uses_native_tools(self) -> bool {
        matches!(self, Self::ClaudeCodeProxy)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolProfile {
    #[default]
    General,
    PythonOnly,
}

#[derive(Serialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PythonWorkspaceExposure {
    #[default]
    Policy,
    SharedLaunchCwd,
}

#[derive(Serialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PythonInvocationPolicy {
    pub workspace_exposure: PythonWorkspaceExposure,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PythonExecutionTarget {
    Host,
    Sandbox,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackend {
    Bubblewrap,
    Podman,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAccess {
    None,
    Nonlocal,
    Full,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageAccess {
    #[default]
    Disabled,
    Session,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PathGrant {
    pub path: PathBuf,
    pub access: AccessMode,
}

fn default_python_executable() -> String {
    "python3".to_string()
}

fn default_podman_image() -> String {
    "docker.io/library/python:3.13-slim".to_string()
}

pub const DEFAULT_RETAINED_PODMAN_IMAGE: &str = "localhost/lethetic-python-runtime:dev";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SandboxConfig {
    #[serde(default)]
    pub backend: Option<SandboxBackend>,
    #[serde(default)]
    pub network: Option<NetworkAccess>,
    #[serde(default)]
    pub workspace_access: Option<AccessMode>,
    #[serde(default)]
    pub grants: Vec<PathGrant>,
    #[serde(default)]
    pub package_access: PackageAccess,
    #[serde(default = "default_podman_image")]
    pub podman_image: String,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            backend: None,
            network: None,
            workspace_access: None,
            grants: Vec::new(),
            package_access: PackageAccess::Disabled,
            podman_image: default_podman_image(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PythonRuntimeConfig {
    #[serde(default)]
    pub target: Option<PythonExecutionTarget>,
    #[serde(default = "default_python_executable")]
    pub python_executable: String,
    #[serde(default)]
    pub sandbox: SandboxConfig,
}

impl Default for PythonRuntimeConfig {
    fn default() -> Self {
        Self {
            target: None,
            python_executable: default_python_executable(),
            sandbox: SandboxConfig::default(),
        }
    }
}

impl PythonRuntimeConfig {
    pub fn sandbox_is_complete(&self) -> bool {
        let Some(backend) = self.sandbox.backend else {
            return false;
        };
        let (Some(network), Some(workspace_access)) =
            (self.sandbox.network, self.sandbox.workspace_access)
        else {
            return false;
        };
        if network == NetworkAccess::Nonlocal
            && (backend != SandboxBackend::Podman
                || workspace_access != AccessMode::ReadWrite
                || self.sandbox.package_access != PackageAccess::Session
                || !self.sandbox.grants.is_empty())
        {
            return false;
        }
        if self.sandbox.package_access == PackageAccess::Session
            && (backend != SandboxBackend::Podman || network != NetworkAccess::Nonlocal)
        {
            return false;
        }
        match backend {
            SandboxBackend::Bubblewrap => !self.python_executable.trim().is_empty(),
            SandboxBackend::Podman => !self.sandbox.podman_image.trim().is_empty(),
        }
    }

    pub fn validation_error(&self) -> Option<String> {
        match self.target {
            None => Some("Python execution target is not configured".to_string()),
            Some(PythonExecutionTarget::Host) => self
                .python_executable
                .trim()
                .is_empty()
                .then(|| "Python executable cannot be empty for Host execution".to_string()),
            Some(PythonExecutionTarget::Sandbox) => {
                if self.sandbox.backend.is_none() {
                    return Some("Python sandbox backend is not configured".to_string());
                }
                if self.sandbox.network.is_none() {
                    return Some("Python sandbox network access is not configured".to_string());
                }
                if self.sandbox.workspace_access.is_none() {
                    return Some("Python sandbox workspace access is not configured".to_string());
                }
                let backend = self.sandbox.backend.expect("checked above");
                let network = self.sandbox.network.expect("checked above");
                let workspace_access = self.sandbox.workspace_access.expect("checked above");
                if network == NetworkAccess::Nonlocal {
                    if backend != SandboxBackend::Podman {
                        return Some(
                            "Nonlocal network access is supported only by the Podman backend"
                                .to_string(),
                        );
                    }
                    if workspace_access != AccessMode::ReadWrite {
                        return Some(
                            "Nonlocal package mode requires read/write workspace access"
                                .to_string(),
                        );
                    }
                    if self.sandbox.package_access != PackageAccess::Session {
                        return Some(
                            "Nonlocal network access requires session package access".to_string(),
                        );
                    }
                    if !self.sandbox.grants.is_empty() {
                        return Some(
                            "Nonlocal package mode does not allow additional path grants"
                                .to_string(),
                        );
                    }
                }
                if self.sandbox.package_access == PackageAccess::Session
                    && (backend != SandboxBackend::Podman || network != NetworkAccess::Nonlocal)
                {
                    return Some(
                        "Session package access requires Podman with nonlocal networking"
                            .to_string(),
                    );
                }
                match self.sandbox.backend {
                    Some(SandboxBackend::Bubblewrap)
                        if self.python_executable.trim().is_empty() =>
                    {
                        Some(
                            "Python executable cannot be empty for Bubblewrap execution"
                                .to_string(),
                        )
                    }
                    Some(SandboxBackend::Podman) if self.sandbox.podman_image.trim().is_empty() => {
                        Some("Podman image cannot be empty".to_string())
                    }
                    _ => None,
                }
            }
        }
    }
}

pub fn is_exact_retained_nonlocal_python_policy(
    profile: ToolProfile,
    runtime: &PythonRuntimeConfig,
) -> bool {
    profile == ToolProfile::PythonOnly
        && runtime.target == Some(PythonExecutionTarget::Sandbox)
        && runtime.sandbox.backend == Some(SandboxBackend::Podman)
        && runtime.sandbox.network == Some(NetworkAccess::Nonlocal)
        && runtime.sandbox.workspace_access == Some(AccessMode::ReadWrite)
        && runtime.sandbox.package_access == PackageAccess::Session
        && runtime.sandbox.grants.is_empty()
        && runtime.validation_error().is_none()
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ModelContextLimits {
    pub applies_to_models: Vec<String>,
    pub total_context_tokens: usize,
    pub maximum_input_tokens: usize,
    pub maximum_output_tokens: usize,
    pub request_output_tokens: usize,
    pub lethetic_input_budget_tokens: usize,
}

impl ModelContextLimits {
    pub fn applies_to(&self, model: &str) -> bool {
        self.applies_to_models
            .iter()
            .any(|candidate| candidate == model)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.applies_to_models.is_empty() {
            return Err("context_limits.applies_to_models cannot be empty".to_string());
        }
        let mut models = HashSet::new();
        for model in &self.applies_to_models {
            if model.trim().is_empty() {
                return Err(
                    "context_limits.applies_to_models cannot contain an empty model".to_string(),
                );
            }
            if !models.insert(model) {
                return Err(format!(
                    "context_limits.applies_to_models contains duplicate model '{model}'"
                ));
            }
        }
        for (field, value) in [
            ("total_context_tokens", self.total_context_tokens),
            ("maximum_input_tokens", self.maximum_input_tokens),
            ("maximum_output_tokens", self.maximum_output_tokens),
            ("request_output_tokens", self.request_output_tokens),
            (
                "lethetic_input_budget_tokens",
                self.lethetic_input_budget_tokens,
            ),
        ] {
            if value == 0 {
                return Err(format!("context_limits.{field} must be positive"));
            }
        }
        if self.maximum_input_tokens > self.total_context_tokens {
            return Err(
                "context_limits.maximum_input_tokens cannot exceed total_context_tokens"
                    .to_string(),
            );
        }
        if self.maximum_output_tokens > self.total_context_tokens {
            return Err(
                "context_limits.maximum_output_tokens cannot exceed total_context_tokens"
                    .to_string(),
            );
        }
        if self.lethetic_input_budget_tokens > self.maximum_input_tokens {
            return Err(
                "context_limits.lethetic_input_budget_tokens cannot exceed maximum_input_tokens"
                    .to_string(),
            );
        }
        if self.request_output_tokens > self.maximum_output_tokens {
            return Err(
                "context_limits.request_output_tokens cannot exceed maximum_output_tokens"
                    .to_string(),
            );
        }
        if self
            .lethetic_input_budget_tokens
            .checked_add(self.request_output_tokens)
            .is_none_or(|combined| combined > self.total_context_tokens)
        {
            return Err(
                "context_limits Lethetic input budget plus request output cannot exceed total context"
                    .to_string(),
            );
        }
        if u32::try_from(self.maximum_output_tokens).is_err() {
            return Err(
                "context_limits.maximum_output_tokens exceeds the request API range".to_string(),
            );
        }
        if u32::try_from(self.request_output_tokens).is_err() {
            return Err(
                "context_limits.request_output_tokens exceeds the request API range".to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ModelServer {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub kind: ConnectionKind,
    /// May be supplied by `config.local.yml` (e.g. private endpoints kept out of git).
    /// A server with an empty url is shown but unreachable.
    #[serde(default)]
    pub url: String,
    pub model: String,
    /// Parser dialect: "gemma4" | "qwen3" | "default"
    /// Controls initial parser state and which token markers to expect.
    #[serde(default = "default_parser")]
    pub parser: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub context_size: Option<usize>,
    #[serde(default)]
    pub context_limits: Option<ModelContextLimits>,
    #[serde(default)]
    pub input_cost_per_1m: Option<f64>,
    #[serde(default)]
    pub output_cost_per_1m: Option<f64>,
    #[serde(default)]
    pub pricing: Option<PricingConfig>,
    #[serde(default)]
    pub thinking: Option<bool>,
    #[serde(default)]
    pub extra_body: Option<serde_json::Value>,
    #[serde(default)]
    pub context_mode: Option<crate::context::ContextMode>,
    #[serde(default)]
    pub theme: Option<String>,
    /// When false, the model switcher lists only this entry's configured
    /// `model` instead of probing `/v1/models` (OpenRouter returns hundreds).
    #[serde(default = "default_discover_models")]
    pub discover_models: bool,
    /// Overrides the global `provider_retries` for this connection.
    #[serde(default)]
    pub provider_retries: Option<u32>,
    /// Per-model list prices recorded by the model picker's catalog scan
    /// (see `saved_models.rs`); used when `pricing` does not cover a model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_pricing: Vec<PricingConfig>,
    /// Optional allowlist of model IDs to show for this connection in the
    /// model switcher. Discovery results outside the list are dropped; when
    /// discovery is off or returns nothing, the listed IDs are shown as
    /// configured.
    #[serde(default)]
    pub models: Vec<String>,
}

fn default_discover_models() -> bool {
    true
}

/// Sandboxed Python-only profiles shared by the `--python-only` CLI modes and
/// the command-palette presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonPreset {
    /// Transient rootless Podman, no network, no package installs.
    Isolated,
    /// Retained Podman with the public HTTP(S) broker and `lethetic-pkg`.
    Nonlocal,
    /// Transient rootless Podman with full network reachability.
    Permissive,
}

impl PythonPreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Isolated => "isolated",
            Self::Nonlocal => "nonlocal",
            Self::Permissive => "permissive",
        }
    }
}

impl Config {
    /// Rewrite the tool profile and Python runtime for `preset`: Python-only,
    /// rootless Podman, launch cwd mounted read/write, no extra grants.
    /// Workspace exposure is left alone; only the CLI literal modes change it.
    pub fn apply_python_preset(&mut self, preset: PythonPreset) {
        self.tool_profile = ToolProfile::PythonOnly;
        self.python_runtime.target = Some(PythonExecutionTarget::Sandbox);
        self.python_runtime.sandbox.backend = Some(SandboxBackend::Podman);
        self.python_runtime.sandbox.workspace_access = Some(AccessMode::ReadWrite);
        self.python_runtime.sandbox.grants.clear();
        match preset {
            PythonPreset::Isolated => {
                self.python_runtime.sandbox.network = Some(NetworkAccess::None);
                self.python_runtime.sandbox.package_access = PackageAccess::Disabled;
            }
            PythonPreset::Nonlocal => {
                self.python_runtime.sandbox.network = Some(NetworkAccess::Nonlocal);
                self.python_runtime.sandbox.package_access = PackageAccess::Session;
                if self.python_runtime.sandbox.podman_image.trim().is_empty()
                    || self.python_runtime.sandbox.podman_image
                        == "docker.io/library/python:3.13-slim"
                {
                    self.python_runtime.sandbox.podman_image =
                        DEFAULT_RETAINED_PODMAN_IMAGE.to_string();
                }
            }
            PythonPreset::Permissive => {
                self.python_runtime.sandbox.network = Some(NetworkAccess::Full);
                self.python_runtime.sandbox.package_access = PackageAccess::Disabled;
            }
        }
    }
}

/// The model last selected in a directory, stored in `.lethetic/last_model.json`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LastModel {
    pub connection_id: String,
    pub model: String,
}

impl LastModel {
    fn path(workspace: &Path) -> PathBuf {
        workspace.join(".lethetic").join("last_model.json")
    }

    pub fn load(workspace: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::path(workspace)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, workspace: &Path) -> Result<(), String> {
        let path = Self::path(workspace);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        let json = serde_json::to_string(self).map_err(|error| error.to_string())?;
        std::fs::write(&path, json)
            .map_err(|error| format!("could not write {}: {error}", path.display()))
    }
}

impl Config {
    /// Re-activate the model last selected in `workspace`, if its connection
    /// still exists and the result validates. Returns the restored selection.
    pub fn restore_last_model(&mut self, workspace: &Path) -> Option<LastModel> {
        let last = LastModel::load(workspace)?;
        let mut candidate = self.clone();
        candidate
            .activate_model(&last.connection_id, &last.model)
            .ok()?;
        candidate.validate().ok()?;
        *self = candidate;
        Some(last)
    }
}

impl ModelServer {
    pub fn connection_id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.name)
    }
}

fn default_parser() -> String {
    "gemma4".to_string()
}

/// `context_size` values treated as "unset" when merging server-specific settings.
pub const CONTEXT_SIZE_UNSET: usize = 0;
pub const CONTEXT_SIZE_LEGACY_DEFAULT: usize = 262_144;
pub const REQUEST_OUTPUT_TOKENS_LEGACY_DEFAULT: u32 = 24_576;

#[derive(Deserialize, Debug, Clone, Default)]
pub struct Config {
    pub server_url: String,
    pub model: String,
    pub context_size: usize,
    pub tool_wrapper: Option<String>,
    #[serde(default)]
    pub tool_profile: ToolProfile,
    #[serde(default)]
    pub python_runtime: PythonRuntimeConfig,
    /// Invocation-only behavior. This is never loaded from or written to YAML/policy sidecars.
    #[serde(skip)]
    pub python_invocation: PythonInvocationPolicy,
    #[serde(default)]
    pub active_server: Option<String>,
    #[serde(default)]
    pub connection_kind: ConnectionKind,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub estimate_cost: Option<bool>,
    #[serde(default)]
    pub input_cost_per_1m: Option<f64>,
    #[serde(default)]
    pub output_cost_per_1m: Option<f64>,
    #[serde(default)]
    pub pricing: Option<PricingConfig>,
    #[serde(default)]
    pub enable_image_processing_tool: bool,
    /// `notify` (default), `poll`, or `off`; see [`crate::background::BackgroundMode`].
    #[serde(default)]
    pub background_tasks: crate::background::BackgroundMode,
    /// `single` (default) or `sequential`; see [`crate::tool_call_mode`].
    #[serde(default)]
    pub tool_calls: crate::tool_call_mode::ToolCallMode,
    /// Automatic retries of a failed model request; see [`crate::provider_retry`].
    #[serde(default)]
    pub provider_retries: Option<u32>,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub model_servers: Vec<ModelServer>,
    #[serde(default)]
    pub thinking: Option<bool>,
    #[serde(default)]
    pub extra_body: Option<serde_json::Value>,
    #[serde(default)]
    pub context_mode: Option<crate::context::ContextMode>,
}

impl Config {
    /// Load a config file, overlaying a sibling `config.local.yml` if present.
    /// The local file is for machine-specific secrets (API keys) kept out of git:
    /// mappings merge recursively, `model_servers` entries merge by stable id
    /// (or by name for legacy entries), and scalars in the local file win.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let base_text = std::fs::read_to_string(path)
            .map_err(|e| format!("Could not read {}: {}", path.display(), e))?;
        let mut value: serde_yaml::Value = serde_yaml::from_str(&base_text)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

        let local_path = path.with_file_name("config.local.yml");
        if let Ok(local_text) = std::fs::read_to_string(&local_path) {
            let overlay: serde_yaml::Value = serde_yaml::from_str(&local_text)
                .map_err(|e| format!("Failed to parse {}: {}", local_path.display(), e))?;
            merge_yaml(&mut value, overlay)
                .map_err(|error| format!("Failed to merge {}: {error}", local_path.display()))?;
        }

        let mut config: Self =
            serde_yaml::from_value(value).map_err(|e| format!("Invalid config: {}", e))?;
        if !is_trusted_global_config_path(path) {
            config.restrict_project_python_runtime()?;
        }
        config.validate()?;
        Ok(config)
    }

    fn restrict_project_python_runtime(&mut self) -> Result<(), String> {
        // A cwd config is repository-controlled. It may describe a sandbox,
        // but it cannot select a host executable or silently opt the session
        // into unrestricted Host execution. Custom executables belong in the
        // trusted per-user global config; an explicit one-time UI choice is
        // also applied after this loading boundary.
        self.python_runtime.python_executable = default_python_executable();
        if self.tool_profile == ToolProfile::PythonOnly
            && self.python_runtime.target == Some(PythonExecutionTarget::Host)
        {
            return Err(
                "Untrusted project config cannot select Python Host execution; use a trusted global policy or an explicit one-time choice"
                    .to_string(),
            );
        }
        if self.python_runtime.sandbox.network == Some(NetworkAccess::Nonlocal)
            || self.python_runtime.sandbox.package_access == PackageAccess::Session
        {
            return Err(
                "Untrusted project config cannot enable the retained Nonlocal package runtime; use trusted global policy, the setup dialog, or the explicit CLI mode"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        for server in &self.model_servers {
            if server.name.trim().is_empty() {
                return Err("Invalid config: model server name cannot be empty".to_string());
            }
            if server.id.as_deref().is_some_and(|id| id.trim().is_empty()) {
                return Err(format!(
                    "Invalid config: model server '{}' has an empty id",
                    server.name
                ));
            }
            let id = server.connection_id();
            if !ids.insert(id) {
                return Err(format!("Invalid config: duplicate model server id '{id}'"));
            }
            gemma_chat::validate_extra_body(server.extra_body.as_ref())
                .map_err(|error| format!("Invalid extra_body for model server '{id}': {error}"))?;
            if let Some(context_limits) = &server.context_limits {
                context_limits.validate().map_err(|error| {
                    format!("Invalid context limits for model server '{id}': {error}")
                })?;
            }
            validate_legacy_rate(server.input_cost_per_1m, "input_cost_per_1m", id)?;
            validate_legacy_rate(server.output_cost_per_1m, "output_cost_per_1m", id)?;
            if let Some(pricing) = &server.pricing {
                pricing
                    .validate()
                    .map_err(|error| format!("Invalid pricing for model server '{id}': {error}"))?;
            }
        }
        gemma_chat::validate_extra_body(self.extra_body.as_ref())
            .map_err(|error| format!("Invalid extra_body for active config: {error}"))?;
        validate_legacy_rate(self.input_cost_per_1m, "input_cost_per_1m", "active config")?;
        validate_legacy_rate(
            self.output_cost_per_1m,
            "output_cost_per_1m",
            "active config",
        )?;
        if let Some(pricing) = &self.pricing {
            pricing
                .validate()
                .map_err(|error| format!("Invalid active pricing: {error}"))?;
        }

        if let Some(active) = self.active_server.as_deref()
            && !self
                .model_servers
                .iter()
                .any(|server| server.connection_id() == active)
        {
            return Err(format!(
                "Invalid config: active_server '{active}' does not match a model server"
            ));
        }
        if self.active_server.is_none() {
            self.legacy_active_model_server()?;
        }

        Ok(())
    }

    pub fn python_sandbox_is_complete(&self) -> bool {
        self.python_runtime.sandbox_is_complete()
    }

    pub fn python_runtime_validation_error(&self) -> Option<String> {
        self.python_runtime.validation_error()
    }

    pub fn python_mode_validation_error(&self) -> Option<String> {
        (self.tool_profile == ToolProfile::PythonOnly)
            .then(|| self.python_runtime_validation_error())
            .flatten()
    }

    pub fn python_policy_fingerprint(&self) -> String {
        let bytes = if self.python_invocation == PythonInvocationPolicy::default() {
            serde_json::to_vec(&(self.tool_profile, &self.python_runtime))
        } else {
            serde_json::to_vec(&(
                self.tool_profile,
                &self.python_runtime,
                self.python_invocation,
            ))
        }
        .unwrap_or_default();
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{hash:016x}")
    }

    pub fn python_security_fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = if self.python_invocation == PythonInvocationPolicy::default() {
            serde_json::to_vec(&(self.tool_profile, &self.python_runtime))
        } else {
            serde_json::to_vec(&(
                self.tool_profile,
                &self.python_runtime,
                self.python_invocation,
            ))
        }
        .unwrap_or_default();
        format!("{:x}", Sha256::digest(bytes))
    }

    fn legacy_active_model_server(&self) -> Result<Option<&ModelServer>, String> {
        let exact = self
            .model_servers
            .iter()
            .filter(|server| server.url == self.server_url && server.model == self.model)
            .collect::<Vec<_>>();
        match exact.as_slice() {
            [] => {}
            [server] => return Ok(Some(*server)),
            servers => {
                let ids = servers
                    .iter()
                    .map(|server| server.connection_id())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!(
                    "Invalid config: legacy server_url/model selection is ambiguous ({ids}); set active_server explicitly"
                ));
            }
        }

        let by_url = self
            .model_servers
            .iter()
            .filter(|server| server.url == self.server_url)
            .collect::<Vec<_>>();
        match by_url.as_slice() {
            [] => Ok(None),
            [server] => Ok(Some(*server)),
            servers => {
                let ids = servers
                    .iter()
                    .map(|server| server.connection_id())
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(format!(
                    "Invalid config: legacy server_url selection is ambiguous ({ids}); set active_server explicitly"
                ))
            }
        }
    }

    pub fn active_model_server(&self) -> Option<&ModelServer> {
        if let Some(id) = self.active_server.as_deref() {
            return self
                .model_servers
                .iter()
                .find(|server| server.connection_id() == id);
        }
        self.legacy_active_model_server().ok().flatten()
    }

    pub fn active_connection_id(&self) -> Option<&str> {
        self.active_model_server().map(ModelServer::connection_id)
    }

    pub fn active_connection_kind(&self) -> ConnectionKind {
        self.active_model_server()
            .map(|server| server.kind)
            .unwrap_or(self.connection_kind)
    }

    pub fn active_parser(&self) -> &str {
        self.active_model_server()
            .map(|server| server.parser.as_str())
            .unwrap_or("gemma4")
    }

    pub fn active_context_limits(&self) -> Option<&ModelContextLimits> {
        self.active_model_server()
            .and_then(|server| server.context_limits.as_ref())
            .filter(|limits| limits.applies_to(&self.model))
    }

    pub fn input_token_budget(&self) -> usize {
        self.active_context_limits()
            .map(|limits| limits.lethetic_input_budget_tokens)
            .unwrap_or(self.context_size)
    }

    pub fn request_output_tokens(&self) -> u32 {
        self.active_context_limits()
            .and_then(|limits| u32::try_from(limits.request_output_tokens).ok())
            .unwrap_or(REQUEST_OUTPUT_TOKENS_LEGACY_DEFAULT)
    }

    pub fn maximum_output_tokens(&self) -> u32 {
        self.active_context_limits()
            .and_then(|limits| u32::try_from(limits.maximum_output_tokens).ok())
            .unwrap_or(u32::MAX)
    }

    fn apply_model_server(
        &mut self,
        server: ModelServer,
        model_id: String,
        api_key: Option<String>,
    ) {
        let context_size = server
            .context_limits
            .as_ref()
            .filter(|limits| limits.applies_to(&model_id))
            .map(|limits| limits.lethetic_input_budget_tokens)
            .or(server.context_size)
            .unwrap_or(CONTEXT_SIZE_LEGACY_DEFAULT);
        let connection_id = server.connection_id().to_string();
        let pricing = server
            .pricing
            .filter(|pricing| pricing.applies_to(&model_id))
            .or_else(|| {
                (server.model_pricing.iter())
                    .find(|pricing| pricing.applies_to(&model_id))
                    .cloned()
            });

        self.active_server = Some(connection_id);
        self.connection_kind = server.kind;
        self.server_url = server.url;
        self.model = model_id;
        self.api_key = api_key;
        self.context_size = context_size;
        self.input_cost_per_1m = server.input_cost_per_1m;
        self.output_cost_per_1m = server.output_cost_per_1m;
        self.pricing = pricing;
        self.thinking = server.thinking;
        self.extra_body = server.extra_body;
        self.context_mode = server.context_mode;
        self.theme = server.theme;
    }

    /// Resolves startup connection settings as one unit.
    ///
    /// Legacy configurations select a connection with the top-level URL while retaining a
    /// possibly overlaid model and credential. An explicit `active_server`, and later UI model
    /// activations, intentionally replace those values with the selected server defaults.
    pub fn merge_matching_server_settings(&mut self) {
        let legacy_selection = self.active_server.is_none();
        let Some(server) = self.active_model_server().cloned() else {
            return;
        };
        let (model_id, api_key) = if legacy_selection {
            (
                self.model.clone(),
                self.api_key.clone().or_else(|| server.api_key.clone()),
            )
        } else {
            (server.model.clone(), server.api_key.clone())
        };
        if legacy_selection && self.api_key.is_some() {
            let connection_id = server.connection_id();
            if let Some(resolved) = self
                .model_servers
                .iter_mut()
                .find(|candidate| candidate.connection_id() == connection_id)
            {
                // Associate a legacy root credential with the connection it resolved to. This
                // restores that credential when switching away and back without ever carrying it
                // into another connection.
                resolved.api_key = api_key.clone();
            }
        }
        self.apply_model_server(server, model_id, api_key);
    }

    pub fn activate_model(&mut self, connection_id: &str, model_id: &str) -> Result<(), String> {
        let server = self
            .model_servers
            .iter()
            .find(|server| server.connection_id() == connection_id)
            .cloned()
            .ok_or_else(|| format!("Unknown model server '{connection_id}'"))?;
        let preserve_resolved_key = self.active_connection_id() == Some(connection_id);
        let api_key = if preserve_resolved_key {
            self.api_key.clone()
        } else {
            server.api_key.clone()
        };

        self.apply_model_server(server, model_id.to_string(), api_key);
        Ok(())
    }

    pub fn activate_current_model(&mut self, model_id: &str) -> Result<(), String> {
        let Some(server) = self.active_model_server().cloned() else {
            self.model = model_id.to_string();
            return Ok(());
        };
        let api_key = self.api_key.clone();
        self.apply_model_server(server, model_id.to_string(), api_key);
        Ok(())
    }
}

fn validate_legacy_rate(value: Option<f64>, field: &str, owner: &str) -> Result<(), String> {
    if let Some(value) = value
        && (!value.is_finite() || value < 0.0)
    {
        return Err(format!(
            "Invalid {field} for {owner}: rate must be finite and nonnegative"
        ));
    }
    Ok(())
}

fn is_trusted_global_config_path(path: &Path) -> bool {
    let supplied = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(current) => current.join(path),
            Err(_) => return false,
        }
    };
    let global = crate::platform::lethetic_config_dir().join("config.yml");
    let global = if global.is_absolute() {
        global
    } else {
        match std::env::current_dir() {
            Ok(current) => current.join(global),
            Err(_) => return false,
        }
    };

    // Compare the location as supplied rather than canonicalizing it: a
    // repository symlink to the global file is still a project-scoped source,
    // and its sibling config.local.yml remains untrusted.
    supplied == global
}

/// Overlay `overlay` onto `base`: mappings merge recursively, the top-level
/// `model_servers` sequence merges by stable id (or legacy name), and every
/// other sequence replaces its base value. Scalars in the overlay win.
fn merge_yaml(base: &mut serde_yaml::Value, overlay: serde_yaml::Value) -> Result<(), String> {
    merge_yaml_value(base, overlay, false)
}

fn merge_yaml_value(
    base: &mut serde_yaml::Value,
    overlay: serde_yaml::Value,
    is_model_servers: bool,
) -> Result<(), String> {
    use serde_yaml::Value;
    match (base, overlay) {
        (Value::Mapping(base), Value::Mapping(overlay)) => {
            for (key, value) in overlay {
                if let Some(base_value) = base.get_mut(&key) {
                    let model_servers = key.as_str() == Some("model_servers");
                    merge_yaml_value(base_value, value, model_servers)?;
                } else {
                    base.insert(key, value);
                }
            }
        }
        (Value::Sequence(base), Value::Sequence(overlay)) if is_model_servers => {
            for item in overlay {
                if let Some(index) = model_server_overlay_match(base, &item)? {
                    merge_yaml_value(&mut base[index], item, false)?;
                } else {
                    base.push(item);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
    Ok(())
}

fn model_server_overlay_match(
    base: &[serde_yaml::Value],
    overlay: &serde_yaml::Value,
) -> Result<Option<usize>, String> {
    use serde_yaml::Value;
    let item_id = overlay.get("id").and_then(Value::as_str);
    let item_name = overlay.get("name").and_then(Value::as_str);

    if let Some(id) = item_id {
        let exact = base
            .iter()
            .enumerate()
            .filter(|(_, existing)| existing.get("id").and_then(Value::as_str) == Some(id))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        match exact.as_slice() {
            [index] => return Ok(Some(*index)),
            [_, _, ..] => {
                return Err(format!(
                    "model_servers overlay id '{id}' matches more than one base entry"
                ));
            }
            [] => {}
        }

        let legacy = base
            .iter()
            .enumerate()
            .filter(|(_, existing)| {
                existing.get("id").and_then(Value::as_str).is_none()
                    && item_name.is_some()
                    && existing.get("name").and_then(Value::as_str) == item_name
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        return match legacy.as_slice() {
            [] => Ok(None),
            [index] => Ok(Some(*index)),
            _ => Err(format!(
                "model_servers overlay id '{id}' has an ambiguous legacy name fallback; give the base entries stable ids"
            )),
        };
    }

    let Some(name) = item_name else {
        return Ok(None);
    };
    let named = base
        .iter()
        .enumerate()
        .filter(|(_, existing)| existing.get("name").and_then(Value::as_str) == Some(name))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match named.as_slice() {
        [] => Ok(None),
        [index] => Ok(Some(*index)),
        _ => Err(format!(
            "legacy model_servers overlay name '{name}' is ambiguous; add an explicit id"
        )),
    }
}

#[cfg(test)]
mod tests;
