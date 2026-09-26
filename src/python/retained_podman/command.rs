use super::model::{
    ABI_LABEL, ContainerInspection, ContainerPresence, ImageInspection, RUNTIME_IMAGE_LABEL,
    RUNTIME_IMAGE_LABEL_VALUE, ResolvedRuntimeImage, RetainedPodmanConfig,
};
use super::spec::{
    build_create_spec, build_remove_spec, build_stop_spec, normalize_container_id,
    normalize_image_id, require_current_runtime_abi, validate_container_id,
    validate_container_name, validate_image_reference, validate_uuid,
};
use crate::python::backend::{
    require_podman_image_with_cancellation, require_rootless_podman_with_cancellation,
    resolve_real_podman,
};
use crate::python::process_tree::{ProcessTree, force_kill_and_reap};
use crate::python::runtime_store::RuntimeLayoutProfile;
use crate::python::supervisor::RUNTIME_ABI;
use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

tokio::task_local! {
    static PODMAN_COMMAND_CANCELLATION: CancellationToken;
}

pub(crate) async fn with_podman_command_cancellation<F>(
    cancellation: CancellationToken,
    operation: F,
) -> F::Output
where
    F: Future,
{
    PODMAN_COMMAND_CANCELLATION
        .scope(cancellation, operation)
        .await
}

fn current_podman_command_cancellation() -> CancellationToken {
    PODMAN_COMMAND_CANCELLATION
        .try_with(CancellationToken::clone)
        .unwrap_or_else(|_| CancellationToken::new())
}

pub(crate) fn podman_command_cancellation_requested() -> bool {
    PODMAN_COMMAND_CANCELLATION
        .try_with(CancellationToken::is_cancelled)
        .unwrap_or(false)
}

pub(super) const PODMAN_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const PODMAN_MAX_OUTPUT_BYTES: usize = 1024 * 1024;

pub(crate) async fn resolve_retained_podman() -> Result<PathBuf, String> {
    let podman = resolve_real_podman()?;
    let cancellation = current_podman_command_cancellation();
    require_rootless_podman_with_cancellation(&podman, &cancellation).await?;
    Ok(podman)
}

pub async fn prepare_retained_podman(
    image: &str,
) -> Result<(PathBuf, ResolvedRuntimeImage), String> {
    let podman = resolve_retained_podman().await?;
    let image = resolve_local_runtime_image(&podman, image).await?;
    Ok((podman, image))
}

pub async fn podman_selinux_enabled(podman: &Path) -> Result<bool, String> {
    crate::python::backend::validate_podman_executable(podman)?;
    let output = run_podman_output(
        podman,
        &[
            OsString::from("info"),
            OsString::from("--format={{.Host.Security.SELinuxEnabled}}"),
        ],
        PODMAN_COMMAND_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        return Err(podman_failure("Podman SELinux status inspection", &output));
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| "Podman SELinux status is not UTF-8".to_string())?;
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("Podman returned an invalid SELinux enabled status".to_string()),
    }
}

pub(crate) async fn resolve_local_image_exact(
    podman: &Path,
    requested: &str,
) -> Result<ResolvedRuntimeImage, String> {
    validate_image_reference(requested)?;
    let cancellation = current_podman_command_cancellation();
    require_podman_image_with_cancellation(podman, requested, &cancellation).await?;
    let output = run_podman_output(
        podman,
        &[
            OsString::from("image"),
            OsString::from("inspect"),
            OsString::from("--format=json"),
            OsString::from("--"),
            OsString::from(requested),
        ],
        PODMAN_COMMAND_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        return Err(podman_failure("Podman image inspect", &output));
    }
    let inspections: Vec<ImageInspection> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Podman image inspect returned invalid JSON: {error}"))?;
    if inspections.len() != 1 {
        return Err("Podman image inspect did not resolve exactly one image".to_string());
    }
    Ok(ResolvedRuntimeImage {
        requested: requested.to_string(),
        image_id: normalize_image_id(&inspections[0].id)?,
    })
}

pub async fn resolve_local_runtime_image(
    podman: &Path,
    requested: &str,
) -> Result<ResolvedRuntimeImage, String> {
    validate_image_reference(requested)?;
    let cancellation = current_podman_command_cancellation();
    require_podman_image_with_cancellation(podman, requested, &cancellation).await?;
    let output = run_podman_output(
        podman,
        &[
            OsString::from("image"),
            OsString::from("inspect"),
            OsString::from("--format=json"),
            OsString::from("--"),
            OsString::from(requested),
        ],
        PODMAN_COMMAND_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        return Err(podman_failure("Podman image inspect", &output));
    }
    let inspections: Vec<ImageInspection> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Podman image inspect returned invalid JSON: {error}"))?;
    if inspections.len() != 1 {
        return Err("Podman image inspect did not resolve exactly one image".to_string());
    }
    let inspection = &inspections[0];
    let image_id = normalize_image_id(&inspection.id)?;
    if inspection
        .labels
        .get(RUNTIME_IMAGE_LABEL)
        .map(String::as_str)
        != Some(RUNTIME_IMAGE_LABEL_VALUE)
        || inspection.labels.get(ABI_LABEL).map(String::as_str) != Some(RUNTIME_ABI)
    {
        return Err(format!(
            "Podman image '{requested}' is not a trusted Lethetic runtime image with ABI {RUNTIME_ABI}"
        ));
    }
    Ok(ResolvedRuntimeImage {
        requested: requested.to_string(),
        image_id,
    })
}
pub(crate) async fn create_retained_container(
    config: &RetainedPodmanConfig,
) -> Result<String, String> {
    require_current_runtime_abi(&config.runtime_abi)?;
    if !matches!(
        config.layout_profile,
        RuntimeLayoutProfile::ShortSiblingV1 | RuntimeLayoutProfile::ShortSiblingSharedCwdV1
    ) {
        return Err("new retained containers require a current short-sibling layout".to_string());
    }
    let spec = build_create_spec(config)?;
    config.prepare_workspace_for_create()?;
    let output = run_podman_output(&spec.program, &spec.args, PODMAN_COMMAND_TIMEOUT).await?;
    if !output.status.success() {
        return Err(podman_failure("Podman retained-container create", &output));
    }
    let container_id = String::from_utf8(output.stdout)
        .map_err(|_| "Podman create returned a non-UTF-8 container ID".to_string())?
        .trim()
        .to_string();
    validate_container_id(&container_id)?;
    Ok(container_id)
}

pub(super) async fn load_container_inspection_reference(
    podman: &Path,
    reference: &str,
) -> Result<ContainerInspection, String> {
    crate::python::backend::validate_podman_executable(podman)?;
    let output = run_podman_output(
        podman,
        &[
            OsString::from("container"),
            OsString::from("inspect"),
            OsString::from("--format=json"),
            OsString::from("--"),
            OsString::from(reference),
        ],
        PODMAN_COMMAND_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        return Err(podman_failure("Podman container inspect", &output));
    }
    let mut inspections: Vec<ContainerInspection> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Podman container inspect returned invalid JSON: {error}"))?;
    if inspections.len() != 1 {
        return Err("Podman container inspect did not return exactly one container".to_string());
    }
    Ok(inspections.remove(0))
}

pub(super) async fn load_container_inspection(
    podman: &Path,
    container_id: &str,
) -> Result<ContainerInspection, String> {
    validate_container_id(container_id)?;
    load_container_inspection_reference(podman, container_id).await
}

pub(super) async fn container_reference_presence(
    podman: &Path,
    reference: &str,
) -> Result<ContainerPresence, String> {
    crate::python::backend::validate_podman_executable(podman)?;
    let output = run_podman_output(
        podman,
        &[
            OsString::from("container"),
            OsString::from("exists"),
            OsString::from("--"),
            OsString::from(reference),
        ],
        PODMAN_COMMAND_TIMEOUT,
    )
    .await?;
    if output.status.success() {
        return Ok(ContainerPresence::Present);
    }
    if output.status.code() == Some(1) {
        return Ok(ContainerPresence::Absent);
    }
    Err(podman_failure("Podman container existence check", &output))
}

pub(crate) async fn exact_container_presence(
    podman: &Path,
    container_id: &str,
) -> Result<ContainerPresence, String> {
    validate_container_id(container_id)?;
    container_reference_presence(podman, container_id).await
}

pub(crate) async fn resolve_exact_container_name(
    podman: &Path,
    runtime_id: &str,
    container_name: &str,
) -> Result<Option<String>, String> {
    validate_uuid(runtime_id, "runtime ID")?;
    validate_container_name(container_name, runtime_id)?;
    if container_reference_presence(podman, container_name).await? == ContainerPresence::Absent {
        return Ok(None);
    }
    let inspection = load_container_inspection_reference(podman, container_name).await?;
    if inspection.name != container_name {
        return Err("Podman exact-name lookup resolved a different container name".to_string());
    }
    Ok(Some(normalize_container_id(&inspection.id)?))
}
pub(crate) async fn run_stop(podman: &Path, container_id: &str) -> Result<(), String> {
    let spec = build_stop_spec(podman, container_id)?;
    let output = run_podman_output(&spec.program, &spec.args, Duration::from_secs(30)).await?;
    if !output.status.success() {
        return Err(podman_failure("Podman retained-container stop", &output));
    }
    Ok(())
}
pub(crate) async fn run_remove(podman: &Path, container_id: &str) -> Result<(), String> {
    let spec = build_remove_spec(podman, container_id)?;
    let output = run_podman_output(&spec.program, &spec.args, Duration::from_secs(30)).await?;
    if !output.status.success() {
        return Err(podman_failure("Podman retained-container remove", &output));
    }
    Ok(())
}
pub(super) async fn run_podman_output(
    program: &Path,
    args: &[OsString],
    duration: Duration,
) -> Result<std::process::Output, String> {
    let cancellation = current_podman_command_cancellation();
    run_podman_output_with_cancellation(program, args, duration, &cancellation).await
}

async fn run_podman_output_with_cancellation(
    program: &Path,
    args: &[OsString],
    duration: Duration,
    cancellation: &CancellationToken,
) -> Result<std::process::Output, String> {
    if cancellation.is_cancelled() {
        return Err("Podman command was cancelled before start".to_string());
    }
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut process_tree = ProcessTree::prepare(&mut command, "Podman command")?;
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start Podman command: {error}"))?;
    if let Err(error) = process_tree.attach_and_resume(&child, "Podman command") {
        let containment = terminate_podman_child_contained(child, Some(process_tree)).await;
        return Err(append_containment_error(error, containment));
    }
    let Some(stdout) = child.stdout.take() else {
        let containment = terminate_podman_child_contained(child, Some(process_tree)).await;
        return Err(append_containment_error(
            "Podman command has no stdout",
            containment,
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        drop(stdout);
        let containment = terminate_podman_child_contained(child, Some(process_tree)).await;
        return Err(append_containment_error(
            "Podman command has no stderr",
            containment,
        ));
    };

    enum RunOutcome<T> {
        Completed(T),
        Cancelled,
    }
    let outcome = {
        let operation = async {
            let stdout = drain_bounded(stdout, PODMAN_MAX_OUTPUT_BYTES);
            let stderr = drain_bounded(stderr, PODMAN_MAX_OUTPUT_BYTES);
            let wait = child.wait();
            tokio::try_join!(stdout, stderr, async {
                wait.await
                    .map_err(|error| format!("could not wait for Podman command: {error}"))
            })
        };
        tokio::pin!(operation);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => RunOutcome::Cancelled,
            outcome = tokio::time::timeout(duration, &mut operation) => {
                RunOutcome::Completed(outcome)
            }
        }
    };

    match outcome {
        RunOutcome::Completed(Ok(Ok((
            (stdout, stdout_truncated),
            (stderr, stderr_truncated),
            status,
        )))) => {
            process_tree
                .terminate_remaining(PODMAN_CHILD_REAP_TIMEOUT, "Podman command")
                .await?;
            if stdout_truncated || stderr_truncated {
                return Err("Podman command output exceeded its safety limit".to_string());
            }
            Ok(std::process::Output {
                status,
                stdout,
                stderr,
            })
        }
        RunOutcome::Completed(Ok(Err(error))) => {
            let containment = terminate_podman_child_contained(child, Some(process_tree)).await;
            Err(append_containment_error(error, containment))
        }
        RunOutcome::Completed(Err(_)) => {
            let containment = terminate_podman_child_contained(child, Some(process_tree)).await;
            Err(append_containment_error(
                format!("Podman command timed out after {}s", duration.as_secs()),
                containment,
            ))
        }
        RunOutcome::Cancelled => {
            let containment = terminate_podman_child_contained(child, Some(process_tree)).await;
            Err(append_containment_error(
                "Podman command was cancelled",
                containment,
            ))
        }
    }
}

const PODMAN_CHILD_REAP_TIMEOUT: Duration = Duration::from_secs(1);

async fn terminate_podman_child_contained(
    child: Child,
    process_tree: Option<ProcessTree>,
) -> Result<(), String> {
    if let Some(process_tree) = process_tree {
        return force_kill_and_reap(
            child,
            process_tree,
            PODMAN_CHILD_REAP_TIMEOUT,
            "Podman command",
        )
        .await;
    }
    if terminate_podman_child(child, PODMAN_CHILD_REAP_TIMEOUT).await {
        Err("Podman command reaping continued in the background".to_string())
    } else {
        Ok(())
    }
}

fn append_containment_error(message: impl Into<String>, containment: Result<(), String>) -> String {
    let message = message.into();
    match containment {
        Ok(()) => message,
        Err(error) => format!("{message}; containment: {error}"),
    }
}

async fn terminate_podman_child(mut child: Child, reap_timeout: Duration) -> bool {
    let _ = child.start_kill();
    let mut reaper = tokio::spawn(async move {
        let _ = child.start_kill();
        let _ = child.wait().await;
    });
    if reap_timeout.is_zero() {
        return true;
    }
    tokio::time::timeout(reap_timeout, &mut reaper)
        .await
        .is_err()
}

#[cfg(test)]
pub(super) async fn terminate_podman_child_for_test(child: Child, reap_timeout: Duration) -> bool {
    terminate_podman_child(child, reap_timeout).await
}

async fn drain_bounded<R>(mut reader: R, maximum: usize) -> Result<(Vec<u8>, bool), String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| format!("could not read Podman output: {error}"))?;
        if read == 0 {
            return Ok((output, truncated));
        }
        let remaining = maximum.saturating_sub(output.len());
        let keep = remaining.min(read);
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep != read;
    }
}

pub(super) fn podman_failure(label: &str, output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let details = format!("{}{}", stdout.trim(), stderr.trim());
    if details.is_empty() {
        format!("{label} failed with status {}", output.status)
    } else {
        format!("{label} failed with status {}: {details}", output.status)
    }
}
