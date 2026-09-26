use crate::config::{
    AccessMode, Config, NetworkAccess, PathGrant, PythonExecutionTarget, PythonRuntimeConfig,
    SandboxBackend, ToolProfile,
};
use crate::python::backend::{BackendCapability, PythonBackendChoice};
use crate::python_policy::{
    PYTHON_POLICY_VERSION, PolicyRevision, PythonPolicyScope, PythonPolicySnapshot, policy_path,
    policy_revision,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyPersistence {
    OneTime,
    Project,
    Global,
}

impl PolicyPersistence {
    pub fn label(self) -> &'static str {
        match self {
            Self::OneTime => "One-time (this chat)",
            Self::Project => "Project setting",
            Self::Global => "Global setting",
        }
    }

    pub fn scope(self) -> Option<PythonPolicyScope> {
        match self {
            Self::OneTime => None,
            Self::Project => Some(PythonPolicyScope::Project),
            Self::Global => Some(PythonPolicyScope::Global),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonSetupStage {
    Profile,
    Target,
    Backend,
    PodmanImage,
    Network,
    WorkspaceAccess,
    Grants,
    Confirm,
    PullConfirm,
    Probing,
    Pulling,
    Applying,
}

impl PythonSetupStage {
    pub fn title(self) -> &'static str {
        match self {
            Self::Profile => "Agent profile",
            Self::Target => "Python execution target",
            Self::Backend => "Sandbox backend",
            Self::PodmanImage => "Podman image",
            Self::Network => "Network access",
            Self::WorkspaceAccess => "Workspace access",
            Self::Grants => "Additional path grants",
            Self::Confirm => "Review and apply",
            Self::PullConfirm => "Confirm Podman pull",
            Self::Probing => "Probing backends",
            Self::Pulling => "Pulling Podman image",
            Self::Applying => "Applying policy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPickerState {
    pub directory: PathBuf,
    pub entries: Vec<PathEntry>,
    pub selected: usize,
    pub show_hidden: bool,
    pub access: AccessMode,
    pub typed_path: String,
    pub error: Option<String>,
}

impl PathPickerState {
    pub fn new(directory: PathBuf) -> Self {
        let mut state = Self {
            typed_path: directory.to_string_lossy().into_owned(),
            directory,
            entries: Vec::new(),
            selected: 0,
            show_hidden: false,
            access: AccessMode::ReadOnly,
            error: None,
        };
        state.refresh();
        state
    }

    pub fn refresh(&mut self) {
        let result = (|| -> Result<Vec<PathEntry>, String> {
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(&self.directory)
                .map_err(|error| format!("Could not browse {}: {error}", self.directory.display()))?
                .flatten()
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !self.show_hidden && name.starts_with('.') {
                    continue;
                }
                let file_type = entry.file_type().map_err(|error| {
                    format!("Could not inspect {}: {error}", entry.path().display())
                })?;
                entries.push(PathEntry {
                    path: entry.path(),
                    name,
                    is_directory: file_type.is_dir(),
                });
            }
            entries.sort_by(|left, right| {
                right
                    .is_directory
                    .cmp(&left.is_directory)
                    .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            });
            Ok(entries)
        })();
        match result {
            Ok(entries) => {
                self.entries = entries;
                self.selected = self.selected.min(self.entries.len().saturating_sub(1));
                self.error = None;
            }
            Err(error) => {
                self.entries.clear();
                self.selected = 0;
                self.error = Some(error);
            }
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
    }

    pub fn enter_selected(&mut self) {
        let Some(entry) = self.entries.get(self.selected).cloned() else {
            return;
        };
        if entry.is_directory {
            self.directory = entry.path;
            self.typed_path = self.directory.to_string_lossy().into_owned();
            self.selected = 0;
            self.refresh();
        }
    }

    pub fn parent(&mut self) {
        if let Some(parent) = self.directory.parent() {
            self.directory = parent.to_path_buf();
            self.typed_path = self.directory.to_string_lossy().into_owned();
            self.selected = 0;
            self.refresh();
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.refresh();
    }

    pub fn toggle_access(&mut self) {
        self.access = match self.access {
            AccessMode::ReadOnly => AccessMode::ReadWrite,
            AccessMode::ReadWrite => AccessMode::ReadOnly,
        };
    }

    pub fn selected_path(&self) -> PathBuf {
        self.entries
            .get(self.selected)
            .map(|entry| entry.path.clone())
            .unwrap_or_else(|| self.directory.clone())
    }

    pub fn resolve_typed_path(&self) -> Result<PathBuf, String> {
        let raw = self.typed_path.trim();
        if raw.is_empty() {
            return Err("Path cannot be empty".to_string());
        }
        let path = PathBuf::from(raw);
        let absolute = if path.is_absolute() {
            path
        } else {
            self.directory.join(path)
        };
        absolute
            .canonicalize()
            .map_err(|error| format!("Could not resolve {}: {error}", absolute.display()))
    }

    pub fn replace_pasted_path(&mut self, text: &str) {
        self.typed_path = text.trim().to_string();
    }
}

/// A palette preset that pre-fills the Agent Mode dialog and jumps to its
/// confirmation screen once the backend probe finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonSetupPreset {
    General,
    Python(crate::config::PythonPreset),
}

#[derive(Debug, Clone)]
pub struct PythonSetupDialog {
    pub stage: PythonSetupStage,
    pub profile: ToolProfile,
    pub runtime: PythonRuntimeConfig,
    pub persistence: PolicyPersistence,
    pub capabilities: Vec<BackendCapability>,
    pub workspace_root: PathBuf,
    pub path_picker: PathPickerState,
    pub selected_grant: usize,
    pub error: Option<String>,
    pub previous_stage: Option<PythonSetupStage>,
    pub global_revision: PolicyRevision,
    pub project_revision: PolicyRevision,
}

impl PythonSetupDialog {
    pub fn new(config: &Config, workspace_root: PathBuf) -> Self {
        let global_revision =
            policy_revision(&policy_path(PythonPolicyScope::Global, &workspace_root))
                .unwrap_or(PolicyRevision::Missing);
        let project_revision =
            policy_revision(&policy_path(PythonPolicyScope::Project, &workspace_root))
                .unwrap_or(PolicyRevision::Missing);
        Self {
            stage: PythonSetupStage::Probing,
            profile: config.tool_profile,
            runtime: config.python_runtime.clone(),
            persistence: PolicyPersistence::OneTime,
            capabilities: Vec::new(),
            path_picker: PathPickerState::new(workspace_root.clone()),
            workspace_root,
            selected_grant: 0,
            error: None,
            previous_stage: Some(PythonSetupStage::Profile),
            global_revision,
            project_revision,
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.stage,
            PythonSetupStage::Probing | PythonSetupStage::Pulling | PythonSetupStage::Applying
        )
    }

    /// Pre-fill the dialog from a preset as a one-time policy and land on the
    /// confirmation screen after probing; Esc still walks back to edit.
    pub fn apply_preset(&mut self, preset: PythonSetupPreset, base: &Config) {
        match preset {
            PythonSetupPreset::General => {
                self.profile = ToolProfile::General;
            }
            PythonSetupPreset::Python(python) => {
                let mut draft = base.clone();
                draft.apply_python_preset(python);
                self.profile = draft.tool_profile;
                self.runtime = draft.python_runtime;
            }
        }
        self.persistence = PolicyPersistence::OneTime;
        self.previous_stage = Some(PythonSetupStage::Confirm);
    }

    pub fn snapshot(&self) -> PythonPolicySnapshot {
        PythonPolicySnapshot {
            version: PYTHON_POLICY_VERSION,
            tool_profile: self.profile,
            python_runtime: self.runtime.clone(),
        }
    }

    pub fn expected_revision(&self) -> Option<PolicyRevision> {
        match self.persistence {
            PolicyPersistence::OneTime => None,
            PolicyPersistence::Project => Some(self.project_revision.clone()),
            PolicyPersistence::Global => Some(self.global_revision.clone()),
        }
    }

    pub fn selected_backend(&self) -> Option<PythonBackendChoice> {
        match self.runtime.target {
            Some(PythonExecutionTarget::Host) => Some(PythonBackendChoice::Host),
            Some(PythonExecutionTarget::Sandbox) => match self.runtime.sandbox.backend {
                Some(SandboxBackend::Bubblewrap) => Some(PythonBackendChoice::Bubblewrap),
                Some(SandboxBackend::Podman) => Some(PythonBackendChoice::Podman),
                None => None,
            },
            None => None,
        }
    }

    pub fn selected_capability(&self) -> Option<&BackendCapability> {
        let choice = self.selected_backend()?;
        self.capabilities
            .iter()
            .find(|capability| capability.choice == choice)
    }

    pub fn set_capabilities(&mut self, capabilities: Vec<BackendCapability>) {
        self.capabilities = capabilities;
        self.stage = self
            .previous_stage
            .take()
            .unwrap_or(PythonSetupStage::Profile);
        self.error = None;
    }

    pub fn validate(&self) -> Result<(), String> {
        let snapshot = self.snapshot();
        snapshot.validate()?;
        if self.profile == ToolProfile::PythonOnly && self.selected_capability().is_none() {
            return Err("Selected Python backend has not been probed".to_string());
        }
        Ok(())
    }

    pub fn replace_pasted_image(&mut self, text: &str) {
        self.runtime.sandbox.podman_image = text.trim().to_string();
    }

    pub fn persistence_path(&self) -> Option<PathBuf> {
        self.persistence
            .scope()
            .map(|scope| policy_path(scope, &self.workspace_root))
    }

    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        match self.profile {
            ToolProfile::General => {
                lines.push("Tools: existing General tool set (Python is not exposed)".to_string())
            }
            ToolProfile::PythonOnly => {
                lines.push(
                    "Tools: python (only model tool); task tracking: import lethetic_todo"
                        .to_string(),
                );
                match self.runtime.target {
                    Some(PythonExecutionTarget::Host) => {
                        lines.push(format!(
                            "Runtime: Host ({}) — unrestricted host files, network, environment, and subprocesses",
                            self.runtime.python_executable
                        ));
                    }
                    Some(PythonExecutionTarget::Sandbox) => {
                        lines.push(format!(
                            "Runtime: {:?}; network: {:?}; workspace: {:?}",
                            self.runtime.sandbox.backend,
                            self.runtime.sandbox.network,
                            self.runtime.sandbox.workspace_access
                        ));
                        if self.runtime.sandbox.backend == Some(SandboxBackend::Podman) {
                            lines.push(format!(
                                "Podman image: {} (--pull=never)",
                                self.runtime.sandbox.podman_image
                            ));
                        }
                        if self.runtime.sandbox.network == Some(NetworkAccess::Full) {
                            lines.push(
                                "Network Full reaches host localhost, LAN, and Internet."
                                    .to_string(),
                            );
                        } else if self.runtime.sandbox.network == Some(NetworkAccess::Nonlocal) {
                            lines.push(
                                "Public packages only; host, localhost, LAN, route-visible VPN/local routes, metadata, and direct container networking remain blocked."
                                    .to_string(),
                            );
                            if crate::config::is_exact_retained_nonlocal_python_policy(
                                self.profile,
                                &self.runtime,
                            ) {
                                lines.push(
                                    "The named Podman container is stopped while detached; its package layer is retained for this chat for 14 days after last use."
                                        .to_string(),
                                );
                                lines.push(
                                    "lethetic-pkg installs signed repository packages through fixed apt-get arguments; package maintainer scripts run as namespaced container-root and can mutate this session layer."
                                        .to_string(),
                                );
                            } else {
                                lines.push(
                                    "Retained packages and lethetic-pkg remain unavailable until the complete validated Podman/read-write/session-package/no-extra-grants policy is selected."
                                        .to_string(),
                                );
                            }
                            lines.push(
                                "This mode never falls back to Full, Host, or another backend and never pulls an image automatically."
                                    .to_string(),
                            );
                        }
                        lines.push(format!("Workspace: {}", self.workspace_root.display()));
                        for grant in &self.runtime.sandbox.grants {
                            lines.push(format!(
                                "Grant: {:?} {}",
                                grant.access,
                                grant.path.display()
                            ));
                        }
                    }
                    None => lines.push("Runtime: unresolved".to_string()),
                }
                lines.push("Python globals reset on session/profile/backend/policy change or cancellation.".to_string());
                lines.push("lethetic_todo remains host-backed and can update .lethetic/todos.json even when the Python workspace is read-only or masked.".to_string());
                lines.push("The LLM connection runs outside the Python sandbox.".to_string());
                lines.push("Sandboxing is not a VM and does not guarantee CPU, memory, kernel, or runtime isolation.".to_string());
            }
        }
        lines.push(match self.persistence_path() {
            Some(path) => format!("Save: {} ({})", self.persistence.label(), path.display()),
            None => format!("Save: {} (no file write)", self.persistence.label()),
        });
        lines
    }

    pub fn security_warnings(&self) -> Vec<String> {
        if self.profile != ToolProfile::PythonOnly
            || self.runtime.target != Some(PythonExecutionTarget::Sandbox)
        {
            return Vec::new();
        }

        let mut warnings = Vec::new();
        let workspace_access = self
            .runtime
            .sandbox
            .workspace_access
            .unwrap_or(AccessMode::ReadOnly);
        append_path_warnings(
            &mut warnings,
            "Workspace mount",
            &self.workspace_root,
            workspace_access,
        );
        for grant in &self.runtime.sandbox.grants {
            append_path_warnings(&mut warnings, "Additional grant", &grant.path, grant.access);
        }
        warnings.sort();
        warnings.dedup();
        warnings
    }

    pub fn add_grant(&mut self, path: PathBuf) -> Result<(), String> {
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Path grants cannot be symlinks: {}",
                path.display()
            ));
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("Could not resolve {}: {error}", path.display()))?;
        if let Some(existing) = self
            .runtime
            .sandbox
            .grants
            .iter_mut()
            .find(|grant| grant.path == canonical)
        {
            existing.access = self.path_picker.access;
        } else {
            self.runtime.sandbox.grants.push(PathGrant {
                path: canonical,
                access: self.path_picker.access,
            });
            self.runtime
                .sandbox
                .grants
                .sort_by(|left, right| left.path.cmp(&right.path));
        }
        self.selected_grant = self
            .selected_grant
            .min(self.runtime.sandbox.grants.len().saturating_sub(1));
        Ok(())
    }

    pub fn remove_selected_grant(&mut self) {
        if self.selected_grant < self.runtime.sandbox.grants.len() {
            self.runtime.sandbox.grants.remove(self.selected_grant);
            self.selected_grant = self
                .selected_grant
                .min(self.runtime.sandbox.grants.len().saturating_sub(1));
        }
    }

    pub fn toggle_selected_grant_access(&mut self) {
        if let Some(grant) = self.runtime.sandbox.grants.get_mut(self.selected_grant) {
            grant.access = match grant.access {
                AccessMode::ReadOnly => AccessMode::ReadWrite,
                AccessMode::ReadWrite => AccessMode::ReadOnly,
            };
        }
    }

    pub fn cycle_persistence(&mut self, delta: isize) {
        let values = [
            PolicyPersistence::OneTime,
            PolicyPersistence::Project,
            PolicyPersistence::Global,
        ];
        let current = values
            .iter()
            .position(|value| *value == self.persistence)
            .unwrap_or(0) as isize;
        self.persistence = values[(current + delta).rem_euclid(values.len() as isize) as usize];
    }

    pub fn next(&mut self) {
        self.error = None;
        self.stage = match self.stage {
            PythonSetupStage::Profile => {
                if self.profile == ToolProfile::General {
                    PythonSetupStage::Confirm
                } else {
                    PythonSetupStage::Target
                }
            }
            PythonSetupStage::Target => match self.runtime.target {
                Some(PythonExecutionTarget::Host) => PythonSetupStage::Confirm,
                Some(PythonExecutionTarget::Sandbox) => PythonSetupStage::Backend,
                None => PythonSetupStage::Target,
            },
            PythonSetupStage::Backend => {
                if self.runtime.sandbox.backend == Some(SandboxBackend::Podman) {
                    PythonSetupStage::PodmanImage
                } else {
                    PythonSetupStage::Network
                }
            }
            PythonSetupStage::PodmanImage => PythonSetupStage::Network,
            PythonSetupStage::Network => PythonSetupStage::WorkspaceAccess,
            PythonSetupStage::WorkspaceAccess => PythonSetupStage::Grants,
            PythonSetupStage::Grants => PythonSetupStage::Confirm,
            other => other,
        };
    }

    pub fn back(&mut self) {
        self.error = None;
        self.stage = match self.stage {
            PythonSetupStage::Target => PythonSetupStage::Profile,
            PythonSetupStage::Backend => PythonSetupStage::Target,
            PythonSetupStage::PodmanImage => PythonSetupStage::Backend,
            PythonSetupStage::Network => {
                if self.runtime.sandbox.backend == Some(SandboxBackend::Podman) {
                    PythonSetupStage::PodmanImage
                } else {
                    PythonSetupStage::Backend
                }
            }
            PythonSetupStage::WorkspaceAccess => PythonSetupStage::Network,
            PythonSetupStage::Grants => PythonSetupStage::WorkspaceAccess,
            PythonSetupStage::Confirm => match (self.profile, self.runtime.target) {
                (ToolProfile::General, _) => PythonSetupStage::Profile,
                (_, Some(PythonExecutionTarget::Host)) => PythonSetupStage::Target,
                _ => PythonSetupStage::Grants,
            },
            PythonSetupStage::PullConfirm => PythonSetupStage::PodmanImage,
            other => other,
        };
    }
}

pub fn visible_window(length: usize, selected: usize, capacity: usize) -> std::ops::Range<usize> {
    if length == 0 || capacity == 0 {
        return 0..0;
    }
    let selected = selected.min(length - 1);
    let window_length = capacity.min(length);
    let start = selected
        .saturating_sub(window_length / 2)
        .min(length - window_length);
    start..start + window_length
}

fn append_path_warnings(warnings: &mut Vec<String>, label: &str, path: &Path, access: AccessMode) {
    let mode = match access {
        AccessMode::ReadOnly => "read-only",
        AccessMode::ReadWrite => "read/write",
    };
    let is_broad = workspace_is_broad(path);
    if is_broad && path == Path::new("/") {
        warnings.push(format!(
            "WARNING: {label} exposes the entire host filesystem ({mode}); read/write permits modification wherever host permissions allow."
        ));
    } else if is_broad && dirs::home_dir().as_deref() == Some(path) {
        warnings.push(format!(
            "WARNING: {label} exposes the full home directory ({mode}), including credentials, SSH keys, browser data, and application secrets."
        ));
    }

    if path == Path::new("/run")
        || path == Path::new("/var/run")
        || path.starts_with("/run/")
        || path.starts_with("/var/run/")
    {
        warnings.push(format!(
            "WARNING: {label} {} ({mode}) may expose host runtime and IPC sockets; socket access can control container or desktop services.",
            path.display()
        ));
    }

    if access == AccessMode::ReadWrite && path.is_dir() {
        warnings.push(format!(
            "WARNING: {label} {} is a read/write directory mount and permits recursive host modification throughout that tree.",
            path.display()
        ));
    }
}

pub fn workspace_is_broad(path: &Path) -> bool {
    path == Path::new("/")
        || dirs::home_dir().as_deref() == Some(path)
        || path == Path::new("/run")
        || path == Path::new("/var/run")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python_config() -> Config {
        let mut config = Config {
            tool_profile: ToolProfile::PythonOnly,
            python_runtime: PythonRuntimeConfig {
                target: Some(PythonExecutionTarget::Host),
                ..Default::default()
            },
            ..Default::default()
        };
        config.python_runtime.python_executable = "python3".to_string();
        config
    }

    #[test]
    fn host_flow_skips_sandbox_steps() {
        let dir = tempfile::tempdir().unwrap();
        let mut dialog = PythonSetupDialog::new(&python_config(), dir.path().to_path_buf());
        dialog.stage = PythonSetupStage::Profile;
        dialog.next();
        assert_eq!(dialog.stage, PythonSetupStage::Target);
        dialog.next();
        assert_eq!(dialog.stage, PythonSetupStage::Confirm);
        dialog.back();
        assert_eq!(dialog.stage, PythonSetupStage::Target);
    }

    #[test]
    fn path_picker_hides_dotfiles_and_adds_access_mode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("visible.txt"), "x").unwrap();
        std::fs::write(dir.path().join(".hidden.txt"), "x").unwrap();
        let mut dialog = PythonSetupDialog::new(&python_config(), dir.path().to_path_buf());
        assert!(
            dialog
                .path_picker
                .entries
                .iter()
                .any(|entry| entry.name == "visible.txt")
        );
        assert!(
            !dialog
                .path_picker
                .entries
                .iter()
                .any(|entry| entry.name == ".hidden.txt")
        );
        dialog.path_picker.toggle_hidden();
        assert!(
            dialog
                .path_picker
                .entries
                .iter()
                .any(|entry| entry.name == ".hidden.txt")
        );
        dialog.path_picker.access = AccessMode::ReadWrite;
        dialog.add_grant(dir.path().join("visible.txt")).unwrap();
        assert_eq!(
            dialog.runtime.sandbox.grants[0].access,
            AccessMode::ReadWrite
        );
    }

    #[test]
    fn one_time_summary_writes_no_path() {
        let dir = tempfile::tempdir().unwrap();
        let dialog = PythonSetupDialog::new(&python_config(), dir.path().to_path_buf());
        assert_eq!(dialog.persistence, PolicyPersistence::OneTime);
        assert!(dialog.persistence_path().is_none());
        assert!(
            dialog
                .summary_lines()
                .iter()
                .any(|line| line.contains("no file write"))
        );
    }

    #[test]
    fn visible_windows_keep_the_selection_on_screen() {
        assert_eq!(visible_window(20, 0, 8), 0..8);
        assert!(visible_window(20, 11, 8).contains(&11));
        assert_eq!(visible_window(20, 19, 8), 12..20);
        assert_eq!(visible_window(3, 2, 8), 0..3);
    }

    #[test]
    fn bracketed_paste_replaces_prefilled_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut dialog = PythonSetupDialog::new(&python_config(), dir.path().to_path_buf());
        dialog.replace_pasted_image("  registry.example/python:test\n");
        dialog.path_picker.replace_pasted_path(" /tmp/example \n");
        assert_eq!(
            dialog.runtime.sandbox.podman_image,
            "registry.example/python:test"
        );
        assert_eq!(dialog.path_picker.typed_path, "/tmp/example");
    }

    #[test]
    fn stale_unavailable_capability_defers_to_final_probe() {
        let dir = tempfile::tempdir().unwrap();
        let mut dialog = PythonSetupDialog::new(&python_config(), dir.path().to_path_buf());
        dialog.profile = ToolProfile::PythonOnly;
        dialog.runtime.target = Some(PythonExecutionTarget::Sandbox);
        dialog.runtime.sandbox.backend = Some(SandboxBackend::Podman);
        dialog.runtime.sandbox.network = Some(NetworkAccess::None);
        dialog.runtime.sandbox.workspace_access = Some(AccessMode::ReadOnly);
        dialog.runtime.sandbox.podman_image = "local/edited:image".to_string();
        assert!(dialog.validate().unwrap_err().contains("not been probed"));
        dialog.capabilities = vec![BackendCapability {
            choice: PythonBackendChoice::Podman,
            available: false,
            reason: "the old image was unavailable".to_string(),
        }];
        assert!(dialog.validate().is_ok());
    }

    #[test]
    fn broad_and_writable_grants_emit_security_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let mut dialog = PythonSetupDialog::new(&python_config(), dir.path().to_path_buf());
        dialog.runtime.target = Some(PythonExecutionTarget::Sandbox);
        dialog.runtime.sandbox.workspace_access = Some(AccessMode::ReadOnly);
        dialog.runtime.sandbox.grants = vec![
            PathGrant {
                path: PathBuf::from("/"),
                access: AccessMode::ReadOnly,
            },
            PathGrant {
                path: PathBuf::from("/run/user/example/service.sock"),
                access: AccessMode::ReadOnly,
            },
            PathGrant {
                path: dir.path().to_path_buf(),
                access: AccessMode::ReadWrite,
            },
        ];
        let warnings = dialog.security_warnings().join("\n");
        assert!(warnings.contains("entire host filesystem"));
        assert!(warnings.contains("runtime and IPC sockets"));
        assert!(warnings.contains("recursive host modification"));
    }

    #[test]
    fn selected_grant_access_can_be_edited() {
        let dir = tempfile::tempdir().unwrap();
        let mut dialog = PythonSetupDialog::new(&python_config(), dir.path().to_path_buf());
        dialog.runtime.sandbox.grants.push(PathGrant {
            path: dir.path().to_path_buf(),
            access: AccessMode::ReadOnly,
        });
        dialog.toggle_selected_grant_access();
        assert_eq!(
            dialog.runtime.sandbox.grants[0].access,
            AccessMode::ReadWrite
        );
        dialog.toggle_selected_grant_access();
        assert_eq!(
            dialog.runtime.sandbox.grants[0].access,
            AccessMode::ReadOnly
        );
    }
}
