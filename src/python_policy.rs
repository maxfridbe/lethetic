use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{
    AccessMode, Config, NetworkAccess, PackageAccess, PythonExecutionTarget,
    PythonInvocationPolicy, PythonRuntimeConfig, PythonWorkspaceExposure, SandboxBackend,
    ToolProfile,
};

pub const PYTHON_POLICY_VERSION: u32 = 2;
const LEGACY_PYTHON_POLICY_VERSION: u32 = 1;
const POLICY_FILE_NAME: &str = "python-mode.yml";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PythonPolicySnapshot {
    pub version: u32,
    pub tool_profile: ToolProfile,
    pub python_runtime: PythonRuntimeConfig,
}

impl PythonPolicySnapshot {
    pub fn from_config(config: &Config) -> Self {
        Self {
            version: PYTHON_POLICY_VERSION,
            tool_profile: config.tool_profile,
            python_runtime: config.python_runtime.clone(),
        }
    }

    pub fn apply_to(&self, config: &mut Config) {
        config.tool_profile = self.tool_profile;
        config.python_runtime = self.python_runtime.clone();
        config.python_invocation = PythonInvocationPolicy::default();
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != PYTHON_POLICY_VERSION {
            return Err(format!(
                "Unsupported Python policy version {} (expected {})",
                self.version, PYTHON_POLICY_VERSION
            ));
        }
        if self.tool_profile == ToolProfile::PythonOnly
            && let Some(error) = self.python_runtime.validation_error()
        {
            return Err(format!("Incomplete Python-only policy: {error}"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PythonPolicySource {
    #[default]
    Config,
    Global,
    Project,
    OneTime,
    CliLocked,
}

impl PythonPolicySource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Global => "global",
            Self::Project => "project",
            Self::OneTime => "one-time",
            Self::CliLocked => "cli-locked",
        }
    }
}

pub const CLI_PYTHON_POLICY_LOCKED_ERROR: &str = "Agent Mode is locked by a literal CLI Python flag for this process; restart without that flag to change it.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcedPythonPolicy {
    snapshot: PythonPolicySnapshot,
    source: PythonPolicySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessPythonPolicy {
    snapshot: PythonPolicySnapshot,
    invocation: PythonInvocationPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonPolicyState {
    persisted: SourcedPythonPolicy,
    chat_one_time: Option<PythonPolicySnapshot>,
    process_literal: Option<ProcessPythonPolicy>,
}

impl PythonPolicyState {
    pub fn from_config(config: &Config, source: PythonPolicySource) -> Self {
        Self::from_persisted(PythonPolicySnapshot::from_config(config), source)
    }

    pub fn from_persisted(snapshot: PythonPolicySnapshot, source: PythonPolicySource) -> Self {
        assert!(
            !matches!(
                source,
                PythonPolicySource::OneTime | PythonPolicySource::CliLocked
            ),
            "persisted Python policy requires a persisted source"
        );
        Self {
            persisted: SourcedPythonPolicy { snapshot, source },
            chat_one_time: None,
            process_literal: None,
        }
    }

    pub fn with_process_literal(mut self, config: &Config) -> Result<Self, String> {
        if self.process_literal.is_some() {
            return Err("literal CLI Python policy was already installed".to_string());
        }
        let literal_shape = config.tool_profile == ToolProfile::PythonOnly
            && config.python_runtime.target == Some(PythonExecutionTarget::Sandbox)
            && config.python_runtime.sandbox.backend == Some(SandboxBackend::Podman)
            && config.python_runtime.sandbox.workspace_access == Some(AccessMode::ReadWrite)
            && config.python_runtime.sandbox.grants.is_empty()
            && config.python_invocation.workspace_exposure
                == PythonWorkspaceExposure::SharedLaunchCwd
            && matches!(
                (
                    config.python_runtime.sandbox.network,
                    config.python_runtime.sandbox.package_access,
                ),
                (
                    Some(NetworkAccess::None | NetworkAccess::Full),
                    PackageAccess::Disabled
                ) | (Some(NetworkAccess::Nonlocal), PackageAccess::Session)
            );
        if !literal_shape {
            return Err(
                "literal CLI Python policy does not match an exact literal mode".to_string(),
            );
        }
        let snapshot = PythonPolicySnapshot::from_config(config);
        snapshot.validate()?;
        self.process_literal = Some(ProcessPythonPolicy {
            snapshot,
            invocation: config.python_invocation,
        });
        Ok(self)
    }

    pub fn is_cli_locked(&self) -> bool {
        self.process_literal.is_some()
    }

    pub fn ensure_ui_mutable(&self) -> Result<(), String> {
        if self.is_cli_locked() {
            Err(CLI_PYTHON_POLICY_LOCKED_ERROR.to_string())
        } else {
            Ok(())
        }
    }

    pub fn effective_source(&self) -> PythonPolicySource {
        if self.process_literal.is_some() {
            PythonPolicySource::CliLocked
        } else if self.chat_one_time.is_some() {
            PythonPolicySource::OneTime
        } else {
            self.persisted.source
        }
    }

    pub fn persisted_snapshot(&self) -> &PythonPolicySnapshot {
        &self.persisted.snapshot
    }

    pub fn persisted_source(&self) -> PythonPolicySource {
        self.persisted.source
    }

    pub fn set_chat_one_time(&mut self, snapshot: PythonPolicySnapshot) -> Result<(), String> {
        self.ensure_ui_mutable()?;
        snapshot.validate()?;
        self.chat_one_time = Some(snapshot);
        Ok(())
    }

    pub fn replace_persisted(
        &mut self,
        snapshot: PythonPolicySnapshot,
        source: PythonPolicySource,
    ) -> Result<(), String> {
        self.ensure_ui_mutable()?;
        if matches!(
            source,
            PythonPolicySource::OneTime | PythonPolicySource::CliLocked
        ) {
            return Err("persisted Python policy has an invalid source".to_string());
        }
        snapshot.validate()?;
        self.persisted = SourcedPythonPolicy { snapshot, source };
        self.chat_one_time = None;
        Ok(())
    }

    pub fn expire_chat_one_time(&mut self) {
        self.chat_one_time = None;
    }

    pub fn apply_effective_to(&self, config: &mut Config) {
        if let Some(process) = &self.process_literal {
            process.snapshot.apply_to(config);
            config.python_invocation = process.invocation;
        } else if let Some(snapshot) = &self.chat_one_time {
            snapshot.apply_to(config);
        } else {
            self.persisted.snapshot.apply_to(config);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonPolicyScope {
    Global,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyRevision {
    Missing,
    Present(u64),
}

#[derive(Debug, Clone)]
pub struct LoadedPythonPolicy {
    pub path: PathBuf,
    pub snapshot: PythonPolicySnapshot,
    pub revision: PolicyRevision,
}

pub fn global_policy_path() -> PathBuf {
    crate::platform::lethetic_config_dir().join(POLICY_FILE_NAME)
}

pub fn project_policy_path(workspace: &Path) -> PathBuf {
    workspace.join(".lethetic").join(POLICY_FILE_NAME)
}

pub fn policy_path(scope: PythonPolicyScope, workspace: &Path) -> PathBuf {
    match scope {
        PythonPolicyScope::Global => global_policy_path(),
        PythonPolicyScope::Project => project_policy_path(workspace),
    }
}

pub fn effective_policy_after_write(
    scope: PythonPolicyScope,
    workspace: &Path,
    written: &PythonPolicySnapshot,
) -> Result<(PythonPolicySnapshot, PythonPolicySource), String> {
    written.validate()?;
    if scope == PythonPolicyScope::Global
        && let Some(project) = load_project_policy(workspace)?
    {
        validate_project_policy(&project.snapshot)?;
        let mut effective = project.snapshot;
        effective.python_runtime.python_executable =
            written.python_runtime.python_executable.clone();
        return Ok((effective, PythonPolicySource::Project));
    }
    let source = match scope {
        PythonPolicyScope::Global => PythonPolicySource::Global,
        PythonPolicyScope::Project => PythonPolicySource::Project,
    };
    Ok((written.clone(), source))
}

pub fn load_resolved_policy(
    config: &mut Config,
    workspace: &Path,
) -> Result<PythonPolicySource, String> {
    let mut source = PythonPolicySource::Config;
    if let Some(loaded) = load_policy(&global_policy_path())? {
        apply_policy_from_source(config, &loaded.snapshot, PythonPolicySource::Global)?;
        source = PythonPolicySource::Global;
    }
    if let Some(loaded) = load_project_policy(workspace)? {
        apply_policy_from_source(config, &loaded.snapshot, PythonPolicySource::Project)?;
        source = PythonPolicySource::Project;
    }
    Ok(source)
}

pub fn load_resolved_policy_from_paths(
    config: &mut Config,
    global_path: &Path,
    project_path: &Path,
) -> Result<PythonPolicySource, String> {
    let mut source = PythonPolicySource::Config;
    if let Some(loaded) = load_policy(global_path)? {
        apply_policy_from_source(config, &loaded.snapshot, PythonPolicySource::Global)?;
        source = PythonPolicySource::Global;
    }
    if let Some(loaded) = load_policy(project_path)? {
        apply_policy_from_source(config, &loaded.snapshot, PythonPolicySource::Project)?;
        source = PythonPolicySource::Project;
    }
    Ok(source)
}

pub fn load_policy(path: &Path) -> Result<Option<LoadedPythonPolicy>, String> {
    let Some(bytes) = read_regular_file(path)? else {
        return Ok(None);
    };
    let snapshot = parse_snapshot(path, &bytes)?;
    Ok(Some(LoadedPythonPolicy {
        path: path.to_path_buf(),
        snapshot,
        revision: PolicyRevision::Present(hash_bytes(&bytes)),
    }))
}

fn apply_policy_from_source(
    config: &mut Config,
    snapshot: &PythonPolicySnapshot,
    source: PythonPolicySource,
) -> Result<(), String> {
    if source != PythonPolicySource::Project {
        snapshot.apply_to(config);
        return Ok(());
    }

    validate_project_policy(snapshot)?;
    let trusted_executable = config.python_runtime.python_executable.clone();
    snapshot.apply_to(config);
    // Project policy may choose sandbox details, but executable selection stays
    // at the trusted global/default layer. This also protects capability probes
    // that test Host even while a sandbox backend is selected.
    config.python_runtime.python_executable = trusted_executable;
    Ok(())
}

fn validate_project_policy(snapshot: &PythonPolicySnapshot) -> Result<(), String> {
    if snapshot.tool_profile == ToolProfile::PythonOnly
        && snapshot.python_runtime.target == Some(PythonExecutionTarget::Host)
    {
        return Err(
            "Untrusted project Python policy cannot select Host execution; use a trusted global policy or an explicit one-time choice"
                .to_string(),
        );
    }
    if snapshot.python_runtime.sandbox.network == Some(NetworkAccess::Nonlocal)
        || snapshot.python_runtime.sandbox.package_access == PackageAccess::Session
    {
        return Err(
            "Untrusted project Python policy cannot enable the retained Nonlocal package runtime; use trusted global policy, the setup dialog, or the explicit CLI mode"
                .to_string(),
        );
    }
    Ok(())
}

fn load_project_policy(workspace: &Path) -> Result<Option<LoadedPythonPolicy>, String> {
    let workspace = canonical_project_workspace(workspace)?;
    validate_project_policy_parent(&workspace)?;
    let path = project_policy_path(&workspace);
    let Some(bytes) =
        crate::platform::read_file_nofollow(&workspace, &[".lethetic"], POLICY_FILE_NAME)
            .map_err(|error| format!("Could not safely read {}: {error}", path.display()))?
    else {
        return Ok(None);
    };
    let snapshot = parse_snapshot(&path, &bytes)?;
    Ok(Some(LoadedPythonPolicy {
        path,
        snapshot,
        revision: PolicyRevision::Present(hash_bytes(&bytes)),
    }))
}

fn canonical_project_workspace(workspace: &Path) -> Result<PathBuf, String> {
    let canonical = workspace.canonicalize().map_err(|error| {
        format!(
            "Could not canonicalize project workspace {}: {error}",
            workspace.display()
        )
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| {
        format!(
            "Could not inspect project workspace {}: {error}",
            canonical.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!(
            "Project workspace is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn validate_project_policy_parent(workspace: &Path) -> Result<(), String> {
    let parent = workspace.join(".lethetic");
    match fs::symlink_metadata(&parent) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "Refusing symlinked project Python policy directory: {}",
            parent.display()
        )),
        Ok(metadata) if !metadata.is_dir() => Err(format!(
            "Project Python policy parent is not a directory: {}",
            parent.display()
        )),
        Ok(_) => {
            let canonical_parent = parent
                .canonicalize()
                .map_err(|error| format!("Could not canonicalize {}: {error}", parent.display()))?;
            if !canonical_parent.starts_with(workspace) {
                return Err(format!(
                    "Project Python policy parent escapes canonical workspace {}: {}",
                    workspace.display(),
                    canonical_parent.display()
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not inspect {}: {error}", parent.display())),
    }
}

pub fn policy_revision(path: &Path) -> Result<PolicyRevision, String> {
    Ok(match read_regular_file(path)? {
        Some(bytes) => PolicyRevision::Present(hash_bytes(&bytes)),
        None => PolicyRevision::Missing,
    })
}

pub fn persist_policy(
    scope: PythonPolicyScope,
    workspace: &Path,
    snapshot: &PythonPolicySnapshot,
    expected_revision: Option<&PolicyRevision>,
) -> Result<PolicyRevision, String> {
    snapshot.validate()?;
    if scope == PythonPolicyScope::Project {
        return persist_project_policy(workspace, snapshot, expected_revision);
    }

    let destination = policy_path(scope, workspace);
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "Python policy path has no parent: {}",
            destination.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Could not create Python policy directory {}: {error}",
            parent.display()
        )
    })?;

    reject_unsafe_destination(&destination)?;
    check_revision(&destination, expected_revision)?;

    let mut encoded = serde_yaml::to_string(snapshot)
        .map_err(|error| format!("Could not serialize Python policy: {error}"))?;
    if !encoded.ends_with('\n') {
        encoded.push('\n');
    }

    let (temporary_path, mut temporary) = create_temporary(parent)?;
    let write_result = (|| -> Result<(), String> {
        temporary
            .write_all(encoded.as_bytes())
            .map_err(|error| format!("Could not write {}: {error}", temporary_path.display()))?;
        temporary
            .sync_all()
            .map_err(|error| format!("Could not sync {}: {error}", temporary_path.display()))?;
        drop(temporary);

        reject_unsafe_destination(&destination)?;
        check_revision(&destination, expected_revision)?;
        fs::rename(&temporary_path, &destination).map_err(|error| {
            format!(
                "Could not atomically replace {}: {error}",
                destination.display()
            )
        })?;
        sync_directory(parent)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result?;
    policy_revision(&destination)
}

fn persist_project_policy(
    workspace: &Path,
    snapshot: &PythonPolicySnapshot,
    expected_revision: Option<&PolicyRevision>,
) -> Result<PolicyRevision, String> {
    validate_project_policy(snapshot)?;
    let workspace = canonical_project_workspace(workspace)?;
    validate_project_policy_parent(&workspace)?;
    let destination = project_policy_path(&workspace);
    check_project_revision(&workspace, expected_revision)?;

    let mut encoded = serde_yaml::to_string(snapshot)
        .map_err(|error| format!("Could not serialize Python policy: {error}"))?;
    if !encoded.ends_with('\n') {
        encoded.push('\n');
    }

    // Recheck immediately before the descriptor-relative atomic write. The
    // write helper creates its temporary in the opened `.lethetic` directory,
    // rejects symlink components/final entries, and renames via that directory
    // descriptor, so a concurrent path swap cannot redirect it outside root.
    check_project_revision(&workspace, expected_revision)?;
    crate::platform::atomic_write_nofollow(
        &workspace,
        &[".lethetic"],
        POLICY_FILE_NAME,
        encoded.as_bytes(),
        0o600,
    )
    .map_err(|error| {
        format!(
            "Could not safely persist {}: {error}",
            destination.display()
        )
    })?;
    project_policy_revision(&workspace)
}

fn project_policy_revision(workspace: &Path) -> Result<PolicyRevision, String> {
    validate_project_policy_parent(workspace)?;
    let path = project_policy_path(workspace);
    let bytes = crate::platform::read_file_nofollow(workspace, &[".lethetic"], POLICY_FILE_NAME)
        .map_err(|error| format!("Could not safely read {}: {error}", path.display()))?;
    Ok(match bytes {
        Some(bytes) => PolicyRevision::Present(hash_bytes(&bytes)),
        None => PolicyRevision::Missing,
    })
}

fn check_project_revision(
    workspace: &Path,
    expected: Option<&PolicyRevision>,
) -> Result<(), String> {
    if let Some(expected) = expected {
        let current = project_policy_revision(workspace)?;
        if &current != expected {
            return Err(format!(
                "Python policy changed while it was being edited: {}",
                project_policy_path(workspace).display()
            ));
        }
    }
    Ok(())
}

fn parse_snapshot(path: &Path, bytes: &[u8]) -> Result<PythonPolicySnapshot, String> {
    let value: serde_yaml::Value = serde_yaml::from_slice(bytes)
        .map_err(|error| format!("Failed to parse {}: {error}", path.display()))?;
    let version = value
        .get("version")
        .and_then(serde_yaml::Value::as_u64)
        .ok_or_else(|| {
            format!(
                "Python policy {} is missing a numeric version",
                path.display()
            )
        })?;
    if !matches!(
        version,
        value if value == u64::from(LEGACY_PYTHON_POLICY_VERSION)
            || value == u64::from(PYTHON_POLICY_VERSION)
    ) {
        return Err(format!(
            "Unsupported Python policy version {version} in {} (supported: {} and {})",
            path.display(),
            LEGACY_PYTHON_POLICY_VERSION,
            PYTHON_POLICY_VERSION
        ));
    }
    let mut snapshot: PythonPolicySnapshot = serde_yaml::from_value(value)
        .map_err(|error| format!("Invalid Python policy {}: {error}", path.display()))?;
    if snapshot.version == LEGACY_PYTHON_POLICY_VERSION {
        snapshot.version = PYTHON_POLICY_VERSION;
    }
    snapshot.validate()?;
    Ok(snapshot)
}

fn read_regular_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!("Could not inspect {}: {error}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "Refusing Python policy symlink: {}",
            path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "Python policy path is not a regular file: {}",
            path.display()
        ));
    }
    fs::read(path)
        .map(Some)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))
}

fn reject_unsafe_destination(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "Refusing to replace Python policy symlink: {}",
            path.display()
        )),
        Ok(metadata) if !metadata.is_file() => Err(format!(
            "Refusing to replace non-regular Python policy: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not inspect {}: {error}", path.display())),
    }
}

fn check_revision(path: &Path, expected: Option<&PolicyRevision>) -> Result<(), String> {
    if let Some(expected) = expected {
        let current = policy_revision(path)?;
        if &current != expected {
            return Err(format!(
                "Python policy changed while it was being edited: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn create_temporary(parent: &Path) -> Result<(PathBuf, File), String> {
    for _ in 0..64 {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = parent.join(format!(
            ".{POLICY_FILE_NAME}.tmp-{}-{nanos}-{counter}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!("Could not create {}: {error}", path.display()));
            }
        }
    }
    Err(format!(
        "Could not create a unique temporary Python policy in {}",
        parent.display()
    ))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Could not sync {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AccessMode, NetworkAccess, PathGrant, PythonExecutionTarget, PythonWorkspaceExposure,
        SandboxBackend,
    };

    fn host_snapshot() -> PythonPolicySnapshot {
        PythonPolicySnapshot {
            version: PYTHON_POLICY_VERSION,
            tool_profile: ToolProfile::PythonOnly,
            python_runtime: PythonRuntimeConfig {
                target: Some(PythonExecutionTarget::Host),
                ..Default::default()
            },
        }
    }

    fn sandbox_snapshot(grants: Vec<PathGrant>) -> PythonPolicySnapshot {
        let mut python_runtime = PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Sandbox),
            ..Default::default()
        };
        python_runtime.sandbox.backend = Some(SandboxBackend::Bubblewrap);
        python_runtime.sandbox.network = Some(NetworkAccess::None);
        python_runtime.sandbox.workspace_access = Some(AccessMode::ReadOnly);
        python_runtime.sandbox.grants = grants;
        PythonPolicySnapshot {
            version: PYTHON_POLICY_VERSION,
            tool_profile: ToolProfile::PythonOnly,
            python_runtime,
        }
    }

    fn nonlocal_snapshot() -> PythonPolicySnapshot {
        let mut python_runtime = PythonRuntimeConfig {
            target: Some(PythonExecutionTarget::Sandbox),
            ..Default::default()
        };
        python_runtime.sandbox.backend = Some(SandboxBackend::Podman);
        python_runtime.sandbox.network = Some(NetworkAccess::Nonlocal);
        python_runtime.sandbox.workspace_access = Some(AccessMode::ReadWrite);
        python_runtime.sandbox.package_access = PackageAccess::Session;
        PythonPolicySnapshot {
            version: PYTHON_POLICY_VERSION,
            tool_profile: ToolProfile::PythonOnly,
            python_runtime,
        }
    }

    fn write_snapshot(path: &Path, snapshot: &PythonPolicySnapshot) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, serde_yaml::to_string(snapshot).unwrap()).unwrap();
    }

    #[test]
    fn invocation_workspace_override_is_never_persisted_and_policy_apply_clears_it() {
        let mut config = Config::default();
        config.python_invocation.workspace_exposure = PythonWorkspaceExposure::SharedLaunchCwd;
        let snapshot = PythonPolicySnapshot::from_config(&config);
        let encoded = serde_yaml::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("workspace_exposure"), "{encoded}");
        assert!(!encoded.contains("shared_launch_cwd"), "{encoded}");

        snapshot.apply_to(&mut config);
        assert_eq!(config.python_invocation, PythonInvocationPolicy::default());
    }

    #[test]
    fn process_literal_is_complete_immutable_and_highest_precedence() {
        let baseline = Config::default();
        let mut literal = Config::default();
        literal.tool_profile = ToolProfile::PythonOnly;
        literal.python_runtime.target = Some(PythonExecutionTarget::Sandbox);
        literal.python_runtime.sandbox.backend = Some(SandboxBackend::Podman);
        literal.python_runtime.sandbox.network = Some(NetworkAccess::Full);
        literal.python_runtime.sandbox.workspace_access = Some(AccessMode::ReadWrite);
        literal.python_invocation.workspace_exposure = PythonWorkspaceExposure::SharedLaunchCwd;
        let mut state = PythonPolicyState::from_config(&baseline, PythonPolicySource::Config)
            .with_process_literal(&literal)
            .unwrap();

        assert!(state.is_cli_locked());
        assert_eq!(state.effective_source(), PythonPolicySource::CliLocked);
        assert_eq!(
            state.set_chat_one_time(host_snapshot()).unwrap_err(),
            CLI_PYTHON_POLICY_LOCKED_ERROR
        );
        assert_eq!(
            state
                .replace_persisted(host_snapshot(), PythonPolicySource::Global)
                .unwrap_err(),
            CLI_PYTHON_POLICY_LOCKED_ERROR
        );
        state.expire_chat_one_time();

        let mut effective = baseline;
        state.apply_effective_to(&mut effective);
        assert_eq!(effective.tool_profile, ToolProfile::PythonOnly);
        assert_eq!(
            effective.python_runtime.sandbox.network,
            Some(NetworkAccess::Full)
        );
        assert_eq!(
            effective.python_invocation.workspace_exposure,
            PythonWorkspaceExposure::SharedLaunchCwd
        );
    }

    #[test]
    fn chat_one_time_expires_back_to_the_persisted_policy_without_a_literal() {
        let baseline = Config::default();
        let one_time = sandbox_snapshot(Vec::new());
        let mut state = PythonPolicyState::from_config(&baseline, PythonPolicySource::Config);
        state.set_chat_one_time(one_time).unwrap();

        let mut effective = baseline.clone();
        state.apply_effective_to(&mut effective);
        assert_eq!(effective.tool_profile, ToolProfile::PythonOnly);
        assert_eq!(state.effective_source(), PythonPolicySource::OneTime);

        state.expire_chat_one_time();
        state.apply_effective_to(&mut effective);
        assert_eq!(effective.tool_profile, ToolProfile::General);
        assert_eq!(state.effective_source(), PythonPolicySource::Config);
    }

    #[test]
    fn test_policy_paths_are_scoped() {
        let workspace = Path::new("/tmp/example-workspace");
        assert_eq!(
            project_policy_path(workspace),
            workspace.join(".lethetic/python-mode.yml")
        );
        assert_eq!(
            global_policy_path()
                .file_name()
                .and_then(|name| name.to_str()),
            Some("python-mode.yml")
        );
    }

    #[test]
    fn test_global_write_remains_shadowed_by_project_policy() {
        let dir = tempfile::tempdir().unwrap();
        let project = sandbox_snapshot(Vec::new());
        write_snapshot(&project_policy_path(dir.path()), &project);

        let (effective, source) =
            effective_policy_after_write(PythonPolicyScope::Global, dir.path(), &host_snapshot())
                .unwrap();

        assert_eq!(effective, project);
        assert_eq!(source, PythonPolicySource::Project);
    }

    #[test]
    fn test_project_policy_replaces_global_policy_and_empty_grants() {
        let dir = tempfile::tempdir().unwrap();
        let global_path = dir.path().join("global.yml");
        let project_path = dir.path().join("project.yml");
        write_snapshot(
            &global_path,
            &sandbox_snapshot(vec![PathGrant {
                path: PathBuf::from("/global"),
                access: AccessMode::ReadWrite,
            }]),
        );
        write_snapshot(&project_path, &sandbox_snapshot(Vec::new()));
        let mut config = Config::default();

        let source =
            load_resolved_policy_from_paths(&mut config, &global_path, &project_path).unwrap();

        assert_eq!(source, PythonPolicySource::Project);
        assert_eq!(config.tool_profile, ToolProfile::PythonOnly);
        assert!(config.python_runtime.sandbox.grants.is_empty());
    }

    #[test]
    fn test_unsupported_policy_version_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.yml");
        fs::write(
            &path,
            "version: 99\ntool_profile: general\npython_runtime: {}\n",
        )
        .unwrap();
        let error = load_policy(&path).unwrap_err();
        assert!(error.contains("Unsupported Python policy version 99"));
    }

    #[test]
    fn test_v1_policy_migrates_in_memory_and_writes_v2_only_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_path = dir.path().join("legacy.yml");
        let legacy = "version: 1\ntool_profile: python_only\npython_runtime:\n  target: sandbox\n  python_executable: /trusted/python3\n  sandbox:\n    backend: bubblewrap\n    network: none\n    workspace_access: read_only\n    grants: []\n    podman_image: local/python:old\n";
        fs::write(&legacy_path, legacy).unwrap();

        let loaded = load_policy(&legacy_path).unwrap().unwrap();

        assert_eq!(loaded.snapshot.version, PYTHON_POLICY_VERSION);
        assert_eq!(loaded.snapshot.tool_profile, ToolProfile::PythonOnly);
        assert_eq!(
            loaded.snapshot.python_runtime.python_executable,
            "/trusted/python3"
        );
        assert_eq!(
            loaded.snapshot.python_runtime.sandbox.package_access,
            PackageAccess::Disabled
        );
        assert_eq!(fs::read_to_string(&legacy_path).unwrap(), legacy);

        persist_policy(
            PythonPolicyScope::Project,
            dir.path(),
            &loaded.snapshot,
            Some(&PolicyRevision::Missing),
        )
        .unwrap();
        let saved = fs::read_to_string(project_policy_path(dir.path())).unwrap();
        assert!(saved.starts_with("version: 2\n"), "{saved}");
    }

    #[test]
    fn test_nonlocal_policy_is_trusted_only() {
        let dir = tempfile::tempdir().unwrap();
        let global_path = dir.path().join("global.yml");
        let project_path = dir.path().join("project.yml");
        let snapshot = nonlocal_snapshot();
        write_snapshot(&global_path, &snapshot);
        let mut config = Config::default();

        let source = load_resolved_policy_from_paths(
            &mut config,
            &global_path,
            &dir.path().join("missing-project.yml"),
        )
        .unwrap();
        assert_eq!(source, PythonPolicySource::Global);
        assert_eq!(
            config.python_runtime.sandbox.network,
            Some(NetworkAccess::Nonlocal)
        );
        assert_eq!(
            config.python_runtime.sandbox.package_access,
            PackageAccess::Session
        );

        write_snapshot(&project_path, &snapshot);
        let error = load_resolved_policy_from_paths(
            &mut Config::default(),
            &dir.path().join("missing-global.yml"),
            &project_path,
        )
        .unwrap_err();
        assert!(
            error.contains("retained Nonlocal package runtime"),
            "{error}"
        );

        let workspace = tempfile::tempdir().unwrap();
        let error = persist_policy(
            PythonPolicyScope::Project,
            workspace.path(),
            &snapshot,
            Some(&PolicyRevision::Missing),
        )
        .unwrap_err();
        assert!(
            error.contains("retained Nonlocal package runtime"),
            "{error}"
        );

        let mut general = snapshot;
        general.tool_profile = ToolProfile::General;
        let error = persist_policy(
            PythonPolicyScope::Project,
            workspace.path(),
            &general,
            Some(&PolicyRevision::Missing),
        )
        .unwrap_err();
        assert!(
            error.contains("retained Nonlocal package runtime"),
            "{error}"
        );
    }

    #[test]
    fn test_incomplete_python_sidecar_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.yml");
        fs::write(
            &path,
            "version: 1\ntool_profile: python_only\npython_runtime: {}\n",
        )
        .unwrap();
        let error = load_policy(&path).unwrap_err();
        assert!(error.contains("Incomplete Python-only policy"));
    }

    #[test]
    fn test_atomic_project_write_and_conflict_detection() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();
        let primary = workspace.join("config.yml");
        fs::write(&primary, "api_key: untouched # keep this comment\n").unwrap();
        let expected = PolicyRevision::Missing;
        let snapshot = sandbox_snapshot(Vec::new());

        let revision = persist_policy(
            PythonPolicyScope::Project,
            workspace,
            &snapshot,
            Some(&expected),
        )
        .unwrap();
        assert!(matches!(revision, PolicyRevision::Present(_)));
        let loaded = load_policy(&project_policy_path(workspace))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.snapshot, snapshot);
        assert_eq!(
            fs::read_to_string(primary).unwrap(),
            "api_key: untouched # keep this comment\n"
        );

        fs::write(project_policy_path(workspace), "externally changed\n").unwrap();
        let error = persist_policy(
            PythonPolicyScope::Project,
            workspace,
            &snapshot,
            Some(&revision),
        )
        .unwrap_err();
        assert!(error.contains("changed while it was being edited"));
    }

    #[cfg(unix)]
    #[test]
    fn test_atomic_policy_permissions_and_symlink_rejection() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let snapshot = sandbox_snapshot(Vec::new());
        persist_policy(
            PythonPolicyScope::Project,
            &workspace,
            &snapshot,
            Some(&PolicyRevision::Missing),
        )
        .unwrap();
        let path = project_policy_path(&workspace);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        fs::remove_file(&path).unwrap();
        let target = workspace.join("target.yml");
        fs::write(&target, "do not replace").unwrap();
        symlink(&target, &path).unwrap();
        let error =
            persist_policy(PythonPolicyScope::Project, &workspace, &snapshot, None).unwrap_err();
        assert!(error.contains("symlink"));
        assert_eq!(fs::read_to_string(target).unwrap(), "do not replace");
    }

    #[test]
    fn test_project_policy_cannot_select_executable() {
        let dir = tempfile::tempdir().unwrap();
        let global_path = dir.path().join("global.yml");
        let project_path = dir.path().join("project.yml");
        let mut global = sandbox_snapshot(Vec::new());
        global.python_runtime.python_executable = "/trusted/global/python".to_string();
        let mut project = sandbox_snapshot(Vec::new());
        project.python_runtime.python_executable = "/project/payload".to_string();
        write_snapshot(&global_path, &global);
        write_snapshot(&project_path, &project);
        let mut config = Config::default();

        let source =
            load_resolved_policy_from_paths(&mut config, &global_path, &project_path).unwrap();

        assert_eq!(source, PythonPolicySource::Project);
        assert_eq!(
            config.python_runtime.python_executable,
            "/trusted/global/python"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_project_host_policy_fails_closed_before_headless_readiness() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let global_path = dir.path().join("missing-global.yml");
        let project_path = dir.path().join("project.yml");
        let marker = dir.path().join("executed");
        let payload = dir.path().join("payload");
        fs::write(
            &payload,
            format!("#!/bin/sh\n: > '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&payload).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&payload, permissions).unwrap();

        let mut project = host_snapshot();
        project.python_runtime.python_executable = payload.display().to_string();
        write_snapshot(&project_path, &project);
        let mut config = Config::default();

        let load_error =
            match load_resolved_policy_from_paths(&mut config, &global_path, &project_path) {
                Err(error) => error,
                Ok(_) => {
                    // This is the headless preflight path. On the vulnerable
                    // implementation it launches the project-controlled payload.
                    let runtime = crate::tool_runtime::ToolRuntime::headless(dir.path());
                    let _ = runtime.ensure_ready(&config, dir.path()).await;
                    "project Host policy unexpectedly loaded".to_string()
                }
            };

        assert!(load_error.contains("project"), "{load_error}");
        assert!(load_error.contains("Host"), "{load_error}");
        assert!(!marker.exists(), "project executable was launched");
        assert_eq!(config.python_runtime, PythonRuntimeConfig::default());
    }

    #[test]
    fn test_project_host_policy_cannot_be_persisted() {
        let workspace = tempfile::tempdir().unwrap();
        let error = persist_policy(
            PythonPolicyScope::Project,
            workspace.path(),
            &host_snapshot(),
            Some(&PolicyRevision::Missing),
        )
        .unwrap_err();

        assert!(error.contains("Host"), "{error}");
        assert!(!project_policy_path(workspace.path()).exists());
    }

    #[test]
    fn test_general_project_policy_allows_stale_host_draft_without_executable_control() {
        let workspace = tempfile::tempdir().unwrap();
        let mut snapshot = host_snapshot();
        snapshot.tool_profile = ToolProfile::General;
        snapshot.python_runtime.python_executable = "/project/payload".to_string();

        persist_policy(
            PythonPolicyScope::Project,
            workspace.path(),
            &snapshot,
            Some(&PolicyRevision::Missing),
        )
        .unwrap();
        let mut config = Config::default();
        let source = load_resolved_policy_from_paths(
            &mut config,
            &workspace.path().join("missing-global.yml"),
            &project_policy_path(workspace.path()),
        )
        .unwrap();

        assert_eq!(source, PythonPolicySource::Project);
        assert_eq!(config.tool_profile, ToolProfile::General);
        assert_eq!(
            config.python_runtime.target,
            Some(PythonExecutionTarget::Host)
        );
        assert_eq!(config.python_runtime.python_executable, "python3");
    }

    #[cfg(unix)]
    #[test]
    fn test_project_policy_rejects_symlinked_parent_without_outside_write() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let outside = dir.path().join("outside");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&outside).unwrap();
        let outside_policy = outside.join(POLICY_FILE_NAME);
        fs::write(&outside_policy, "outside-safe").unwrap();
        symlink(&outside, workspace.join(".lethetic")).unwrap();

        let error = persist_policy(
            PythonPolicyScope::Project,
            &workspace,
            &sandbox_snapshot(Vec::new()),
            None,
        )
        .unwrap_err();

        assert!(error.contains("symlink"), "{error}");
        assert_eq!(fs::read_to_string(outside_policy).unwrap(), "outside-safe");
    }

    #[test]
    fn test_project_policy_rejects_non_directory_parent() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join(".lethetic"), "not a directory").unwrap();

        let error = persist_policy(
            PythonPolicyScope::Project,
            workspace.path(),
            &sandbox_snapshot(Vec::new()),
            None,
        )
        .unwrap_err();

        assert!(error.contains("not a directory"), "{error}");
    }
}
