#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use tokio::net::UnixStream;

pub(crate) fn validate_unix_socket_path_length(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        let length = path.as_os_str().as_bytes().len();
        if length >= address.sun_path.len() {
            return Err(format!(
                "broker socket path is {length} bytes but Unix requires fewer than {}: {}",
                address.sun_path.len(),
                path.display()
            ));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("egress broker sockets require Unix".to_string())
    }
}

pub(super) fn validate_private_parent(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if !path.is_absolute() {
            return Err(format!("broker path must be absolute: {}", path.display()));
        }
        let parent = path
            .parent()
            .ok_or_else(|| format!("broker path has no parent: {}", path.display()))?;
        let canonical = parent
            .canonicalize()
            .map_err(|error| format!("could not canonicalize {}: {error}", parent.display()))?;
        if canonical != parent {
            return Err(format!(
                "broker path parent must be canonical and contain no symlink: {}",
                parent.display()
            ));
        }
        let metadata = std::fs::metadata(parent)
            .map_err(|error| format!("could not inspect {}: {error}", parent.display()))?;
        let uid = rustix::process::geteuid().as_raw();
        if !metadata.is_dir() || metadata.uid() != uid || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(format!(
                "broker path parent must be a private directory owned by uid {uid}: {}",
                parent.display()
            ));
        }
        if path.exists() {
            let destination = std::fs::symlink_metadata(path)
                .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
            if destination.file_type().is_symlink() {
                return Err(format!(
                    "broker path may not be a symlink: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("egress broker paths require Unix".to_string())
    }
}

pub(super) struct SocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl SocketGuard {
    pub(super) fn new(path: PathBuf) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| format!("could not inspect broker socket: {error}"))?;
            if !metadata.file_type().is_socket() {
                return Err("bound broker path is not a Unix socket".to_string());
            }
            return Ok(Self {
                path,
                device: metadata.dev(),
                inode: metadata.ino(),
            });
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err("egress broker sockets require Unix".to_string())
        }
    }

    pub(super) fn secure_pathname(&self) -> Result<(), String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("could not chmod broker socket pathname: {error}"))?;
            let metadata = std::fs::symlink_metadata(&self.path)
                .map_err(|error| format!("could not verify broker socket pathname: {error}"))?;
            if !metadata.file_type().is_socket()
                || metadata.dev() != self.device
                || metadata.ino() != self.inode
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.nlink() != 1
                || metadata.permissions().mode() & 0o7777 != 0o600
            {
                return Err(
                    "broker socket pathname is not the exact owner-only socket that was bound"
                        .to_string(),
                );
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err("egress broker sockets require Unix".to_string())
        }
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let Ok(metadata) = std::fs::symlink_metadata(&self.path) else {
                return;
            };
            if metadata.file_type().is_socket()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) struct PeerVerifier {
    pub(super) uid: u32,
    pub(super) gid: u32,
    pid: i32,
    start_time: String,
    cgroup: String,
    executable_device: u64,
    executable_inode: u64,
    selinux: Option<crate::python::selinux::SelinuxSocketSecurity>,
    allow_label_disabled_peer: bool,
    _pidfd: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
impl PeerVerifier {
    pub(super) fn new(
        uid: u32,
        gid: u32,
        pid: u32,
        derive_credentials: bool,
        selinux: Option<crate::python::selinux::SelinuxSocketSecurity>,
        allow_label_disabled_peer: bool,
    ) -> Result<Self, String> {
        use std::os::unix::fs::MetadataExt;

        let pid =
            i32::try_from(pid).map_err(|_| "expected peer PID is out of range".to_string())?;
        let rustix_pid = rustix::process::Pid::from_raw(pid)
            .ok_or_else(|| "expected peer PID may not be zero".to_string())?;
        let pidfd =
            rustix::process::pidfd_open(rustix_pid, rustix::process::PidfdFlags::empty())
                .map_err(|error| format!("could not pin expected adapter PID {pid}: {error}"))?;
        if let Some(security) = &selinux {
            security.verify_process_label(pid)?;
        } else if !allow_label_disabled_peer {
            crate::python::selinux::verify_unlabeled_peer_context(pid)?;
        }
        let (process_uid, process_gid) = read_peer_process_credentials(pid)?;
        if !derive_credentials && (uid != process_uid || gid != process_gid) {
            return Err(
                "configured broker peer credentials do not match the pinned process".to_string(),
            );
        }
        let (start_time, cgroup, executable) = read_peer_process_identity(pid)?;
        Ok(Self {
            uid: process_uid,
            gid: process_gid,
            pid,
            start_time,
            cgroup,
            executable_device: executable.dev(),
            executable_inode: executable.ino(),
            selinux,
            allow_label_disabled_peer,
            _pidfd: pidfd,
        })
    }

    pub(super) fn verify(&self, stream: &UnixStream) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;

        let credentials = stream
            .peer_cred()
            .map_err(|error| format!("could not read broker peer credentials: {error}"))?;
        if credentials.uid() != self.uid
            || credentials.gid() != self.gid
            || credentials.pid() != Some(self.pid)
        {
            return Err("broker peer credentials do not match the registered adapter".to_string());
        }
        if let Some(security) = &self.selinux {
            security.verify_peer_label(stream)?;
            security.verify_process_label(self.pid)?;
        } else if !self.allow_label_disabled_peer {
            crate::python::selinux::verify_unlabeled_peer_context(self.pid)?;
        }
        let (uid, gid) = read_peer_process_credentials(self.pid)?;
        let (start_time, cgroup, executable) = read_peer_process_identity(self.pid)?;
        if uid != self.uid
            || gid != self.gid
            || start_time != self.start_time
            || cgroup != self.cgroup
            || executable.dev() != self.executable_device
            || executable.ino() != self.executable_inode
        {
            return Err("registered adapter process identity changed".to_string());
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn read_peer_process_credentials(pid: i32) -> Result<(u32, u32), String> {
    let status = read_small_text_file(&PathBuf::from(format!("/proc/{pid}/status")), 64 * 1024)?;
    let parse = |field: &str| -> Result<u32, String> {
        let values = status
            .lines()
            .find_map(|line| line.strip_prefix(field))
            .ok_or_else(|| format!("adapter process status has no {field} field"))?
            .split_ascii_whitespace()
            .map(|value| {
                value
                    .parse::<u32>()
                    .map_err(|_| format!("adapter process {field} field is malformed"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let Some(first) = values.first().copied() else {
            return Err(format!("adapter process {field} field is empty"));
        };
        if values.len() != 4 || values.iter().any(|value| *value != first) {
            return Err(format!(
                "adapter process {field} credentials are transitional"
            ));
        }
        Ok(first)
    };
    Ok((parse("Uid:")?, parse("Gid:")?))
}

#[cfg(target_os = "linux")]
fn read_peer_process_identity(pid: i32) -> Result<(String, String, std::fs::Metadata), String> {
    let proc = PathBuf::from(format!("/proc/{pid}"));
    let stat = read_small_text_file(&proc.join("stat"), 16 * 1024)?;
    let command_end = stat
        .rfind(')')
        .ok_or_else(|| "adapter process stat record is malformed".to_string())?;
    let start_time = stat[command_end + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| "adapter process stat record has no start time".to_string())?
        .to_string();
    let cgroup = read_small_text_file(&proc.join("cgroup"), 64 * 1024)?;
    if cgroup.is_empty() {
        return Err("adapter process cgroup record is empty".to_string());
    }
    let executable = std::fs::metadata(proc.join("exe"))
        .map_err(|error| format!("could not inspect adapter executable: {error}"))?;
    if !executable.is_file() {
        return Err("registered adapter executable is not a regular file".to_string());
    }
    Ok((start_time, cgroup, executable))
}

pub(super) fn read_small_text_file(path: &Path, maximum: usize) -> Result<String, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(format!("{} has an invalid size", path.display()));
    }
    String::from_utf8(bytes).map_err(|_| format!("{} is not valid UTF-8", path.display()))
}

#[cfg(not(target_os = "linux"))]
pub(super) struct PeerVerifier;

#[cfg(not(target_os = "linux"))]
impl PeerVerifier {
    pub(super) fn new(
        _uid: u32,
        _gid: u32,
        _pid: u32,
        _derive_credentials: bool,
        _selinux: Option<crate::python::selinux::SelinuxSocketSecurity>,
        _allow_label_disabled_peer: bool,
    ) -> Result<Self, String> {
        Err("egress broker peer verification requires Linux".to_string())
    }

    pub(super) fn verify(&self, _stream: &UnixStream) -> Result<(), String> {
        Err("egress broker peer verification requires Linux".to_string())
    }
}
