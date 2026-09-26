use super::*;
use serde::{Deserialize, Serialize};

const SESSION_STATE_SCHEMA_VERSION: u32 = 5;

pub(super) fn is_forbidden_session_name_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

pub fn normalize_session_display_name(value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 256 || value.chars().count() > 80 {
        return Err("session name must be at most 80 characters and 256 UTF-8 bytes".to_string());
    }
    if value.chars().any(is_forbidden_session_name_character) {
        return Err(
            "session name contains a control or bidirectional formatting character".to_string(),
        );
    }
    Ok(Some(value.to_string()))
}

fn validate_stored_session_display_name(value: &Option<String>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    if normalize_session_display_name(value)?.as_deref() != Some(value.as_str()) {
        return Err("stored session name is not normalized".to_string());
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionWorkspaceBinding {
    pub canonical_path: std::path::PathBuf,
    pub device: u64,
    pub inode: u64,
    pub binding_hash: String,
}

impl SessionWorkspaceBinding {
    pub(crate) fn validate_stored(&self) -> Result<(), String> {
        if !self.canonical_path.is_absolute() || self.canonical_path.to_str().is_none() {
            return Err("managed Python workspace path must be absolute UTF-8".to_string());
        }
        if self.binding_hash.len() != 64
            || !self
                .binding_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(
                "managed Python workspace binding hash must be 64 lowercase hex characters"
                    .to_string(),
            );
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn from_runtime_identity(
        identity: &crate::python::runtime_store::WorkspaceIdentity,
    ) -> Self {
        Self {
            canonical_path: identity.canonical_path.clone(),
            device: identity.device,
            inode: identity.inode,
            binding_hash: identity.binding_hash.clone(),
        }
    }

    #[cfg(target_os = "linux")]
    pub fn to_runtime_identity(
        &self,
    ) -> Result<crate::python::runtime_store::WorkspaceIdentity, String> {
        self.validate_stored()?;
        Ok(crate::python::runtime_store::WorkspaceIdentity {
            canonical_path: self.canonical_path.clone(),
            device: self.device,
            inode: self.inode,
            binding_hash: self.binding_hash.clone(),
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionDirectoryBinding {
    pub canonical_path: std::path::PathBuf,
    pub device: u64,
    pub inode: u64,
    pub binding_hash: String,
}

impl SessionDirectoryBinding {
    pub(crate) fn validate_stored(&self, session_id: &str) -> Result<(), String> {
        validate_session_uuid(session_id, "session ID")?;
        if !self.canonical_path.is_absolute() || self.canonical_path.to_str().is_none() {
            return Err("session directory binding path must be absolute UTF-8".to_string());
        }
        if self.binding_hash.len() != 64
            || !self
                .binding_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(
                "session directory binding hash must be 64 lowercase hex characters".to_string(),
            );
        }
        let expected = session_directory_binding_hash(
            &self.canonical_path,
            self.device,
            self.inode,
            session_id,
        );
        if self.binding_hash != expected {
            return Err("session directory binding hash does not match its identity".to_string());
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn capture_parts(
        canonical_path: &std::path::Path,
        device: u64,
        inode: u64,
        session_id: &str,
    ) -> Result<Self, String> {
        validate_session_uuid(session_id, "session ID")?;
        if !canonical_path.is_absolute() || canonical_path.to_str().is_none() {
            return Err("session directory binding path must be absolute UTF-8".to_string());
        }
        let binding = Self {
            canonical_path: canonical_path.to_path_buf(),
            device,
            inode,
            binding_hash: session_directory_binding_hash(canonical_path, device, inode, session_id),
        };
        binding.validate_stored(session_id)?;
        Ok(binding)
    }

    #[cfg(target_os = "linux")]
    pub fn verify_current(&self, session_id: &str) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;

        self.validate_stored(session_id)?;
        let link_metadata = std::fs::symlink_metadata(&self.canonical_path)
            .map_err(|error| format!("could not inspect session directory binding: {error}"))?;
        if link_metadata.file_type().is_symlink() || !link_metadata.is_dir() {
            return Err("session directory binding no longer names a real directory".to_string());
        }
        let canonical = self.canonical_path.canonicalize().map_err(|error| {
            format!("could not canonicalize session directory binding: {error}")
        })?;
        if canonical != self.canonical_path {
            return Err("session directory binding path changed".to_string());
        }
        let metadata = canonical
            .metadata()
            .map_err(|error| format!("could not inspect bound session directory: {error}"))?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
        {
            return Err("session directory identity changed".to_string());
        }
        Ok(())
    }
}

fn session_directory_binding_hash(
    canonical_path: &std::path::Path,
    device: u64,
    inode: u64,
    session_id: &str,
) -> String {
    use sha2::{Digest, Sha256};

    let mut hash = Sha256::new();
    hash.update(b"lethetic-session-directory-v1\0");
    hash.update(session_id.as_bytes());
    hash.update(b"\0");
    hash.update(canonical_path.as_os_str().as_encoded_bytes());
    hash.update(b"\0");
    hash.update(device.to_le_bytes());
    hash.update(inode.to_le_bytes());
    format!("{:x}", hash.finalize())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionState {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub session_directory_binding: Option<SessionDirectoryBinding>,
    #[serde(default)]
    pub python_runtime_id: Option<String>,
    #[serde(default)]
    pub managed_python_workspace: Option<SessionWorkspaceBinding>,
    #[serde(default)]
    pub shared_python_workspace: Option<SessionWorkspaceBinding>,
    #[serde(default)]
    pub messages: Vec<crate::context::Message>,
    #[serde(default)]
    pub blocks: Vec<RenderBlock>,
    #[serde(default)]
    pub history: Vec<String>,
    #[serde(default)]
    pub theme_name: String,
    #[serde(default)]
    pub accounting: crate::accounting::SessionAccounting,
    /// Connection and model last in use, so resuming reconnects to the same model.
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub model_name: String,
    /// Raw (unresolved) system prompt template in use for this session.
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub hide_thinking: bool,
    /// Agent Mode (tool profile and Python isolation) in use for this session.
    #[serde(default)]
    pub python_policy: Option<crate::python_policy::PythonPolicySnapshot>,
    #[serde(default)]
    pub loop_mode: Option<crate::loop_detector::LoopDetectionMode>,
    /// Remote-control target last active in this session (informational;
    /// resuming never starts a listener on its own).
    #[serde(default)]
    pub remote_control: Option<String>,
    #[serde(skip)]
    pub needs_migration_save: bool,
}

/// Settings restored when a session is resumed. The run loop applies them
/// because model switching and policy installation live in the binary.
#[derive(Debug, Clone, Default)]
pub struct SessionSettings {
    pub system_prompt: String,
    pub connection_id: Option<String>,
    pub model_name: String,
    pub python_policy: Option<crate::python_policy::PythonPolicySnapshot>,
    pub loop_mode: Option<crate::loop_detector::LoopDetectionMode>,
}

/// One-line description of an Agent Mode snapshot for session lists.
pub fn describe_python_policy(snapshot: &crate::python_policy::PythonPolicySnapshot) -> String {
    use crate::config::{NetworkAccess, PythonExecutionTarget, SandboxBackend, ToolProfile};
    if snapshot.tool_profile == ToolProfile::General {
        return "general tools".to_string();
    }
    let runtime = &snapshot.python_runtime;
    match runtime.target {
        Some(PythonExecutionTarget::Host) => "python-only (host)".to_string(),
        _ => {
            let backend = match runtime.sandbox.backend {
                Some(SandboxBackend::Podman) => "podman",
                Some(SandboxBackend::Bubblewrap) => "bubblewrap",
                None => "sandbox",
            };
            let network = match runtime.sandbox.network {
                Some(NetworkAccess::None) | None => "isolated",
                Some(NetworkAccess::Nonlocal) => "nonlocal",
                Some(NetworkAccess::Full) => "full network",
            };
            format!("python-only ({backend}, {network})")
        }
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            schema_version: SESSION_STATE_SCHEMA_VERSION,
            session_id: None,
            display_name: None,
            session_directory_binding: None,
            python_runtime_id: None,
            managed_python_workspace: None,
            shared_python_workspace: None,
            messages: Vec::new(),
            blocks: Vec::new(),
            history: Vec::new(),
            theme_name: String::new(),
            accounting: crate::accounting::SessionAccounting::default(),
            connection_id: None,
            model_name: String::new(),
            system_prompt: String::new(),
            hide_thinking: false,
            python_policy: None,
            loop_mode: None,
            remote_control: None,
            needs_migration_save: false,
        }
    }
}

impl SessionState {
    /// Loads and validates a session. The unified state is authoritative when
    /// present; a malformed or unsupported unified file never falls back to
    /// legacy files.
    pub fn load_checked(session_dir: &str) -> Result<Self, String> {
        if let Some(content) = read_session_file(session_dir, "session_state.json")? {
            let content = String::from_utf8(content)
                .map_err(|_| "session state is not valid UTF-8".to_string())?;
            let mut state = serde_json::from_str::<SessionState>(&content)
                .map_err(|error| format!("could not parse session state: {error}"))?;
            state.finish_load(session_dir)?;
            return Ok(state);
        }

        let legacy_blocks = read_legacy_session_file(session_dir, "ui_state.json")?;
        let legacy_messages = read_legacy_session_file(session_dir, "context.json")?;
        if legacy_blocks.is_none() && legacy_messages.is_none() {
            return Err("session directory has no durable or legacy state".to_string());
        }
        let blocks = legacy_blocks
            .map(|content| {
                serde_json::from_str(&content)
                    .map_err(|error| format!("could not parse legacy UI state: {error}"))
            })
            .transpose()?
            .unwrap_or_default();
        let messages = legacy_messages
            .map(|content| {
                serde_json::from_str(&content)
                    .map_err(|error| format!("could not parse legacy context: {error}"))
            })
            .transpose()?
            .unwrap_or_default();
        let mut state = SessionState {
            blocks,
            messages,
            ..Default::default()
        };
        migrate_legacy_error_blocks(&mut state.blocks);
        crate::context::migrate_legacy_tool_result_errors(&mut state.messages);
        let repairs =
            crate::context::repair_interrupted_tool_calls_with_details(&mut state.messages);
        reconcile_interrupted_tool_error_blocks(&mut state.blocks, &repairs);
        state.session_id = Some(legacy_session_id(session_dir)?);
        state.needs_migration_save = true;
        Ok(state)
    }

    /// Compatibility wrapper used by non-interactive callers that historically
    /// treated unreadable sessions as empty. Security-sensitive resume paths use
    /// `load_checked` and surface the error instead.
    pub fn load(session_dir: &str) -> SessionState {
        Self::load_checked(session_dir).unwrap_or_default()
    }

    pub fn save_to_directory_checked(&self, session_dir: &str) -> Result<(), String> {
        if self.schema_version != SESSION_STATE_SCHEMA_VERSION {
            return Err("session state must be migrated before it can be saved".to_string());
        }
        let session_id = self
            .session_id
            .as_deref()
            .ok_or_else(|| "session state is missing its session ID".to_string())?;
        validate_session_uuid(session_id, "session ID")?;
        validate_stored_session_display_name(&self.display_name)?;
        let binding = self
            .session_directory_binding
            .as_ref()
            .ok_or_else(|| "session state is missing its directory identity binding".to_string())?;
        binding.validate_stored(session_id)?;
        let canonical = std::path::Path::new(session_dir)
            .canonicalize()
            .map_err(|error| format!("could not canonicalize session save target: {error}"))?;
        if canonical != binding.canonical_path {
            return Err("session save target does not match its directory binding".to_string());
        }
        #[cfg(target_os = "linux")]
        binding.verify_current(session_id)?;
        if let Some(runtime_id) = &self.python_runtime_id {
            validate_session_uuid(runtime_id, "Python runtime ID")?;
            if self.managed_python_workspace.is_none() {
                return Err(
                    "Python runtime ID is missing its managed workspace binding".to_string()
                );
            }
        }
        if let Some(workspace) = &self.managed_python_workspace {
            workspace.validate_stored()?;
        }
        if let Some(workspace) = &self.shared_python_workspace {
            workspace.validate_stored()?;
            if self.managed_python_workspace.as_ref() == Some(workspace) {
                return Err(
                    "shared Python workspace must differ from the managed workspace".to_string(),
                );
            }
        }
        let json = serde_json::to_vec(self)
            .map_err(|error| format!("could not serialize session state: {error}"))?;
        write_session_file(session_dir, "session_state.json", &json)
    }

    fn finish_load(&mut self, session_dir: &str) -> Result<(), String> {
        let legacy_identity_schema = matches!(self.schema_version, 0..=2);
        let legacy_error_schema = self.schema_version <= 4;
        match self.schema_version {
            0..=4 => {
                self.schema_version = SESSION_STATE_SCHEMA_VERSION;
                self.needs_migration_save = true;
            }
            SESSION_STATE_SCHEMA_VERSION => {}
            version => {
                return Err(format!("unsupported session state schema {version}"));
            }
        }
        if self.session_id.is_none() {
            if !legacy_identity_schema {
                return Err("session state is missing its session ID".to_string());
            }
            self.session_id = Some(legacy_session_id(session_dir)?);
            self.needs_migration_save = true;
        }
        let session_id = self
            .session_id
            .as_deref()
            .expect("session ID was initialized above");
        validate_session_uuid(session_id, "session ID")?;
        validate_stored_session_display_name(&self.display_name)?;
        match &self.session_directory_binding {
            Some(binding) => binding.validate_stored(session_id)?,
            None if !legacy_identity_schema => {
                return Err("session state is missing its directory identity binding".to_string());
            }
            None => {}
        }
        if let Some(runtime_id) = &self.python_runtime_id {
            validate_session_uuid(runtime_id, "Python runtime ID")?;
            if self.managed_python_workspace.is_none() {
                return Err(
                    "Python runtime ID is missing its managed workspace binding".to_string()
                );
            }
        }
        if let Some(workspace) = &self.managed_python_workspace {
            workspace.validate_stored()?;
        }
        if let Some(workspace) = &self.shared_python_workspace {
            workspace.validate_stored()?;
            if self.managed_python_workspace.as_ref() == Some(workspace) {
                return Err(
                    "shared Python workspace must differ from the managed workspace".to_string(),
                );
            }
        }
        if legacy_error_schema
            && crate::context::migrate_legacy_tool_result_errors(&mut self.messages) > 0
        {
            self.needs_migration_save = true;
        }
        let repairs =
            crate::context::repair_interrupted_tool_calls_with_details(&mut self.messages);
        if !repairs.is_empty() {
            self.needs_migration_save = true;
        }
        if legacy_error_schema && migrate_legacy_error_blocks(&mut self.blocks) {
            self.needs_migration_save = true;
        }
        if reconcile_interrupted_tool_error_blocks(&mut self.blocks, &repairs) {
            self.needs_migration_save = true;
        }
        self.accounting
            .rebuild_totals()
            .map_err(|error| format!("could not rebuild session accounting: {error}"))?;
        let mut seen_turns = std::collections::HashSet::new();
        for block in &mut self.blocks {
            let Some(logical_turn_id) = block.logical_turn_id.as_deref() else {
                continue;
            };
            if block.block_type != BlockType::User {
                return Err(
                    "session logical-turn identity is attached to a non-user block".to_string(),
                );
            }
            if logical_turn_id.trim().is_empty() || !seen_turns.insert(logical_turn_id.to_string())
            {
                return Err(
                    "session contains an invalid or duplicate logical-turn block".to_string(),
                );
            }
            let totals = self.accounting.totals_for_logical_turn(logical_turn_id);
            if apply_accounting_totals_to_user_block(block, &totals) {
                self.needs_migration_save = true;
            }
        }
        Ok(())
    }
}

fn read_session_file(session_dir: &str, file_name: &str) -> Result<Option<Vec<u8>>, String> {
    let path = std::path::Path::new(session_dir);
    let result = if path.is_absolute() {
        crate::platform::read_file_nofollow(path, &[], file_name)
    } else {
        let components = relative_session_components(path)?;
        let component_refs = components.iter().map(String::as_str).collect::<Vec<_>>();
        crate::platform::read_file_nofollow(std::path::Path::new("."), &component_refs, file_name)
    };
    result.map_err(|error| format!("could not read session file {file_name}: {error}"))
}

pub(crate) fn write_session_file(
    session_dir: &str,
    file_name: &str,
    content: &[u8],
) -> Result<(), String> {
    let path = std::path::Path::new(session_dir);
    let result = if path.is_absolute() {
        crate::platform::atomic_write_nofollow(path, &[], file_name, content, 0o600)
    } else {
        let components = relative_session_components(path)?;
        let component_refs = components.iter().map(String::as_str).collect::<Vec<_>>();
        crate::platform::atomic_write_nofollow(
            std::path::Path::new("."),
            &component_refs,
            file_name,
            content,
            0o600,
        )
    };
    result
        .map(|_| ())
        .map_err(|error| format!("could not save session file {file_name}: {error}"))
}

pub(crate) fn append_session_file(
    session_dir: &str,
    file_name: &str,
    content: &[u8],
) -> Result<(), String> {
    let path = std::path::Path::new(session_dir);
    let result = if path.is_absolute() {
        crate::platform::append_file_nofollow(path, &[], file_name, content, 0o600)
    } else {
        let components = relative_session_components(path)?;
        let component_refs = components.iter().map(String::as_str).collect::<Vec<_>>();
        crate::platform::append_file_nofollow(
            std::path::Path::new("."),
            &component_refs,
            file_name,
            content,
            0o600,
        )
    };
    result
        .map(|_| ())
        .map_err(|error| format!("could not append session file {file_name}: {error}"))
}

fn relative_session_components(path: &std::path::Path) -> Result<Vec<String>, String> {
    let mut components = Vec::new();
    for component in path.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(
                "session directory must not contain root, parent, or current-directory components"
                    .to_string(),
            );
        };
        let component = component
            .to_str()
            .ok_or_else(|| "session directory must be UTF-8".to_string())?;
        components.push(component.to_string());
    }
    if components.is_empty() {
        return Err("session directory must not be empty".to_string());
    }
    Ok(components)
}

fn read_legacy_session_file(session_dir: &str, file_name: &str) -> Result<Option<String>, String> {
    read_session_file(session_dir, file_name)?
        .map(|content| {
            String::from_utf8(content)
                .map_err(|_| format!("legacy session file {file_name} is not valid UTF-8"))
        })
        .transpose()
}

fn legacy_session_id(session_dir: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};

    let path = std::path::Path::new(session_dir);
    let link = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect legacy session directory: {error}"))?;
    if link.file_type().is_symlink() || !link.is_dir() {
        return Err("legacy session path must be a real directory".to_string());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("could not canonicalize legacy session directory: {error}"))?;
    let metadata = std::fs::metadata(&canonical)
        .map_err(|error| format!("could not inspect canonical legacy session: {error}"))?;
    let mut hash = Sha256::new();
    hash.update(b"lethetic-legacy-session-id-v1\0");
    hash.update(canonical.as_os_str().as_encoded_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(b"\0");
        hash.update(metadata.dev().to_le_bytes());
        hash.update(metadata.ino().to_le_bytes());
    }
    #[cfg(not(unix))]
    let _ = metadata;
    let digest = hash.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes).hyphenated().to_string())
}

pub(super) fn validate_session_uuid(value: &str, label: &str) -> Result<(), String> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| format!("{label} is not a canonical lowercase UUID"))?;
    if parsed.hyphenated().to_string() != value {
        return Err(format!("{label} is not a canonical lowercase UUID"));
    }
    Ok(())
}

impl App {
    pub fn save_session(&mut self) {
        if let Err(error) = self.save_session_checked() {
            self.stop_reason = format!("Session save failed: {error}");
            self.needs_save = true;
            self.should_redraw = true;
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn save_session_checked(&mut self) -> Result<(), String> {
        if let Some(dir) = &self.current_session_dir {
            return Err(format!(
                "persistent session save is disabled on this platform: {dir}"
            ));
        }
        self.needs_save = false;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn save_session_checked(&mut self) -> Result<(), String> {
        let Some(dir) = self.current_session_dir.clone() else {
            self.needs_save = false;
            return Ok(());
        };
        let binding = self
            .session_directory_binding
            .as_ref()
            .ok_or_else(|| "active session has no directory identity binding".to_string())?;
        binding.validate_stored(&self.session_id)?;
        #[cfg(target_os = "linux")]
        self.session_lease
            .as_ref()
            .ok_or_else(|| "active session has no advisory lease".to_string())?
            .verify(std::path::Path::new(&dir), &self.session_id, binding)?;

        let state = SessionState {
            schema_version: SESSION_STATE_SCHEMA_VERSION,
            session_id: Some(self.session_id.clone()),
            display_name: self.display_name.clone(),
            session_directory_binding: Some(binding.clone()),
            python_runtime_id: self.python_runtime_id.clone(),
            managed_python_workspace: self.managed_python_workspace.clone(),
            shared_python_workspace: self.shared_python_workspace.clone(),
            messages: {
                let mut messages = self.context_manager.get_messages().to_vec();
                if let Some(partial) = &self.partial_assistant_checkpoint
                    && messages.last() != Some(partial)
                {
                    messages.push(partial.clone());
                }
                messages
            },
            blocks: self.blocks.clone(),
            history: self.history.clone(),
            theme_name: self.theme.name.clone(),
            accounting: self.accounting.clone(),
            connection_id: self.config.active_connection_id().map(str::to_string),
            model_name: self.model_name.clone(),
            system_prompt: self.system_prompt.clone(),
            hide_thinking: self.hide_thinking,
            python_policy: Some(crate::python_policy::PythonPolicySnapshot::from_config(
                &self.config,
            )),
            loop_mode: Some(self.loop_detector.config.mode),
            remote_control: self.remote_control_target.clone(),
            needs_migration_save: false,
        };
        let needs_creation_commit = !self.session_creation_committed;
        state.save_to_directory_checked(&dir)?;
        if needs_creation_commit {
            self.session_store
                .as_ref()
                .ok_or_else(|| "secure session storage is unavailable".to_string())?
                .commit_locked_session_creation(
                    self.session_lease
                        .as_deref()
                        .ok_or_else(|| "active session has no advisory lease".to_string())?,
                )?;
            self.session_creation_committed = true;
        }
        self.needs_save = false;
        Ok(())
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn agent_mode_descriptions_are_short_and_specific() {
        let mut config = crate::config::Config::default();
        let general = crate::python_policy::PythonPolicySnapshot::from_config(&config);
        assert_eq!(describe_python_policy(&general), "general tools");
        config.apply_python_preset(crate::config::PythonPreset::Nonlocal);
        let nonlocal = crate::python_policy::PythonPolicySnapshot::from_config(&config);
        assert_eq!(
            describe_python_policy(&nonlocal),
            "python-only (podman, nonlocal)"
        );

        let state = SessionState {
            model_name: "gpt-5.6-sol".into(),
            python_policy: Some(nonlocal),
            remote_control: Some("https://brainiac:11223".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.remote_control.as_deref(),
            Some("https://brainiac:11223")
        );
        assert_eq!(back.python_policy, state.python_policy);
    }
}
