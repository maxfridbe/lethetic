use super::{APT_GET_PATH, PYTHON_PATH, WORKER_HOME, WORKER_SOURCE};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;

pub fn validate_runtime_image_installation() -> Result<(), String> {
    validate_trusted_runtime_file(Path::new(PYTHON_PATH), true)?;
    validate_trusted_runtime_file(Path::new(APT_GET_PATH), true)?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve runtime executable: {error}"))?;
    let metadata = executable
        .metadata()
        .map_err(|error| format!("could not inspect runtime executable: {error}"))?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o022 != 0
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err("runtime executable is not a trusted root-owned executable".to_string());
    }
    let helper = Path::new("/usr/local/bin/lethetic-pkg");
    let helper_metadata = std::fs::symlink_metadata(helper)
        .map_err(|error| format!("could not inspect package helper link: {error}"))?;
    if !helper_metadata.file_type().is_symlink()
        || helper_metadata.uid() != 0
        || helper
            .canonicalize()
            .map_err(|error| format!("could not resolve package helper link: {error}"))?
            != executable
    {
        return Err(
            "package helper does not resolve to the trusted runtime executable".to_string(),
        );
    }
    Ok(())
}

pub fn exec_worker_process(uid: u32, gid: u32, workspace: &Path) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("runtime worker privilege drop requires Linux".to_string());
    }
    if uid == 0 || gid == 0 || rustix::process::geteuid().as_raw() != 0 {
        return Err("runtime worker privilege transition requires container root".to_string());
    }
    if rustix::process::getppid() != Some(rustix::process::Pid::INIT) {
        return Err("runtime worker must be launched directly by supervisor PID 1".to_string());
    }
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("could not canonicalize runtime worker workspace: {error}"))?;
    if !workspace.is_dir() {
        return Err("runtime worker workspace is not a directory".to_string());
    }
    validate_trusted_runtime_file(Path::new(PYTHON_PATH), true)?;

    let empty_groups = unsafe { libc::setgroups(0, std::ptr::null()) };
    if empty_groups != 0 {
        return Err(format!(
            "could not clear runtime worker supplementary groups: {}",
            std::io::Error::last_os_error()
        ));
    }
    for capability in 0..=63 {
        let result = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, capability, 0, 0, 0) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINVAL) {
                return Err(format!(
                    "could not drop worker bounding capability {capability}: {error}"
                ));
            }
        }
    }
    let _ = unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        )
    };
    if unsafe { libc::setresgid(gid, gid, gid) } != 0 {
        return Err(format!(
            "could not set runtime worker GID: {}",
            std::io::Error::last_os_error()
        ));
    }
    if unsafe { libc::setresuid(uid, uid, uid) } != 0 {
        return Err(format!(
            "could not set runtime worker UID: {}",
            std::io::Error::last_os_error()
        ));
    }
    clear_process_capabilities()?;
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(format!(
            "could not set no-new-privileges for runtime worker: {}",
            std::io::Error::last_os_error()
        ));
    }
    unsafe { libc::umask(0o077) };
    std::env::set_current_dir(&workspace)
        .map_err(|error| format!("could not enter runtime worker workspace: {error}"))?;
    verify_worker_security_status(uid, gid)?;

    let error = std::process::Command::new(PYTHON_PATH)
        .args(["-u", "-B", "-c", WORKER_SOURCE])
        .exec();
    Err(format!("could not exec Python runtime worker: {error}"))
}

pub(super) fn clear_process_capabilities() -> Result<(), String> {
    #[repr(C)]
    struct CapabilityHeader {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CapabilityData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    let mut header = CapabilityHeader {
        version: 0x2008_0522,
        pid: 0,
    };
    let mut data = [
        CapabilityData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
        CapabilityData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
    ];
    let result = unsafe {
        libc::syscall(
            libc::SYS_capset,
            &mut header as *mut CapabilityHeader,
            data.as_mut_ptr(),
        )
    };
    if result != 0 {
        return Err(format!(
            "could not clear runtime worker capabilities: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub(super) fn verify_worker_security_status(uid: u32, gid: u32) -> Result<(), String> {
    if rustix::process::geteuid().as_raw() != uid || rustix::process::getegid().as_raw() != gid {
        return Err("runtime worker credential verification failed".to_string());
    }
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("could not inspect runtime worker security status: {error}"))?;
    for field in ["CapInh:", "CapPrm:", "CapEff:", "CapAmb:"] {
        let value = status
            .lines()
            .find_map(|line| line.strip_prefix(field))
            .map(str::trim)
            .ok_or_else(|| format!("runtime worker status is missing {field}"))?;
        if value != "0000000000000000" {
            return Err(format!("runtime worker retained capabilities in {field}"));
        }
    }
    let no_new_privs = status
        .lines()
        .find_map(|line| line.strip_prefix("NoNewPrivs:"))
        .map(str::trim);
    if no_new_privs != Some("1") {
        return Err("runtime worker no-new-privileges verification failed".to_string());
    }
    Ok(())
}

pub(super) fn read_capability(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect broker capability file: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 256 {
        return Err("broker capability file is not a small regular file".to_string());
    }
    let capability = std::fs::read_to_string(path)
        .map_err(|error| format!("could not read broker capability file: {error}"))?;
    Ok(capability.trim().to_string())
}

pub(super) fn prepare_worker_home(uid: u32, gid: u32) -> Result<(), String> {
    let path = Path::new(WORKER_HOME);
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (!metadata.is_dir() || metadata.file_type().is_symlink())
    {
        return Err("runtime worker home is not a trusted directory".to_string());
    }
    std::fs::create_dir_all(path)
        .map_err(|error| format!("could not create runtime worker home: {error}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not secure runtime worker home: {error}"))?;
    if unsafe { libc::chown(path_to_c_string(path)?.as_ptr(), uid, gid) } != 0 {
        return Err(format!(
            "could not assign runtime worker home ownership: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub(super) fn secure_package_socket_path(path: &Path, worker_gid: u32) -> Result<(), String> {
    let c_path = path_to_c_string(path)?;
    if unsafe { libc::lchown(c_path.as_ptr(), 0, worker_gid) } != 0 {
        return Err(format!(
            "could not assign package supervisor socket group: {}",
            std::io::Error::last_os_error()
        ));
    }
    if unsafe { libc::chmod(c_path.as_ptr(), 0o660) } != 0 {
        return Err(format!(
            "could not secure package supervisor socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not verify package supervisor socket: {error}"))?;
    if !metadata.file_type().is_socket()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.gid() != worker_gid
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o777 != 0o660
    {
        return Err("package supervisor socket pathname metadata is unsafe".to_string());
    }
    Ok(())
}

pub(super) fn prepare_package_socket_parent(worker_gid: u32) -> Result<(), String> {
    let path = Path::new("/run/lethetic-pkg");
    prepare_root_directory(path, 0o750)?;
    if unsafe { libc::chown(path_to_c_string(path)?.as_ptr(), 0, worker_gid) } != 0 {
        return Err(format!(
            "could not assign package socket directory group: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub(super) fn prepare_root_directory(path: &Path, mode: u32) -> Result<(), String> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (!metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != 0)
    {
        return Err(format!(
            "{} is not a trusted root-owned directory",
            path.display()
        ));
    }
    std::fs::create_dir_all(path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|error| format!("could not secure {}: {error}", path.display()))
}

pub(super) fn validate_trusted_runtime_file(path: &Path, executable: bool) -> Result<(), String> {
    let link_metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "could not inspect trusted runtime file {}: {error}",
            path.display()
        )
    })?;
    if !link_metadata.is_file() && !link_metadata.file_type().is_symlink() {
        return Err(format!("untrusted runtime file {}", path.display()));
    }
    if link_metadata.uid() != 0
        || (!link_metadata.file_type().is_symlink()
            && link_metadata.permissions().mode() & 0o022 != 0)
    {
        return Err(format!("untrusted runtime file {}", path.display()));
    }
    if link_metadata.file_type().is_symlink() && !executable {
        return Err(format!(
            "trusted runtime data file may not be a symlink: {}",
            path.display()
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        format!(
            "could not resolve trusted runtime file {}: {error}",
            path.display()
        )
    })?;
    if executable && !(canonical.starts_with("/usr/bin") || canonical.starts_with("/usr/local/bin"))
    {
        return Err(format!(
            "runtime executable resolved outside trusted system paths: {}",
            path.display()
        ));
    }
    let metadata = canonical.metadata().map_err(|error| {
        format!(
            "could not inspect resolved runtime file {}: {error}",
            canonical.display()
        )
    })?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o022 != 0
        || (executable && metadata.permissions().mode() & 0o111 == 0)
    {
        return Err(format!("untrusted runtime file {}", path.display()));
    }
    Ok(())
}

pub(super) fn remove_stale_root_socket(path: &Path) -> Result<(), String> {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !metadata.file_type().is_socket() || metadata.uid() != 0 {
        return Err("stale package control path is not a root-owned socket".to_string());
    }
    std::fs::remove_file(path)
        .map_err(|error| format!("could not remove stale package control socket: {error}"))
}

pub(super) fn remove_socket_if_owned(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect package socket during cleanup: {error}"))?;
    if metadata.file_type().is_socket() && metadata.uid() == 0 {
        std::fs::remove_file(path)
            .map_err(|error| format!("could not remove package socket: {error}"))?;
    }
    Ok(())
}

pub(super) fn path_to_c_string(path: &Path) -> Result<std::ffi::CString, String> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("path contains NUL: {}", path.display()))
}
