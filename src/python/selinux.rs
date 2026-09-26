use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};

const PROCESS_TYPE: &str = "container_t";
const FILE_TYPE: &str = "container_file_t";
const BROKER_TYPE: &str = "container_runtime_t";
const MAX_CONTEXT_BYTES: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelinuxLabels {
    pub process_label: String,
    pub mount_label: String,
}

impl SelinuxLabels {
    pub fn new(process_label: String, mount_label: String) -> Result<Self, String> {
        let labels = Self {
            process_label,
            mount_label,
        };
        labels.validate()?;
        Ok(labels)
    }

    pub fn validate(&self) -> Result<(), String> {
        let process = ParsedContext::parse(&self.process_label, "Podman process label")?;
        let mount = ParsedContext::parse(&self.mount_label, "Podman mount label")?;
        if process.role != "system_r" || process.context_type != PROCESS_TYPE {
            return Err("Podman process label must use system_r:container_t".to_string());
        }
        if mount.role != "object_r" || mount.context_type != FILE_TYPE {
            return Err("Podman mount label must use object_r:container_file_t".to_string());
        }
        if process.user != mount.user || process.range != mount.range {
            return Err("Podman process and mount labels must use the same SELinux user and private MCS range"
                .to_string());
        }
        validate_private_mcs(process.range)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct SelinuxSocketSecurity {
    api: Arc<SelinuxApi>,
    labels: SelinuxLabels,
}

impl SelinuxSocketSecurity {
    pub fn prepare(labels: SelinuxLabels) -> Result<Self, String> {
        labels.validate()?;
        let api = SelinuxApi::load()?;
        match api.is_enabled()? {
            1 => {}
            0 => {
                return Err(
                    "Podman supplied SELinux labels but SELinux is disabled on this host"
                        .to_string(),
                );
            }
            _ => return Err("libselinux returned an invalid enabled state".to_string()),
        }
        let broker_context = api.current_context()?;
        let parsed = ParsedContext::parse(&broker_context, "broker SELinux context")?;
        if parsed.context_type != BROKER_TYPE {
            return Err(format!(
                "labeled egress broker must run as {BROKER_TYPE}, not {}",
                parsed.context_type
            ));
        }
        Ok(Self { api, labels })
    }

    pub(crate) fn bind_listener(
        &self,
        path: &Path,
    ) -> Result<(UnixListener, BoundSocketIdentity), String> {
        let mount = CString::new(self.labels.mount_label.as_bytes())
            .map_err(|_| "Podman mount label contains NUL".to_string())?;
        let process = CString::new(self.labels.process_label.as_bytes())
            .map_err(|_| "Podman process label contains NUL".to_string())?;
        let mut contexts = CreateContextGuard::arm(self.api.as_ref(), &mount, &process)?;
        let listener_result = UnixListener::bind(path)
            .map_err(|error| format!("could not bind labeled broker socket: {error}"));
        let identity_result = match &listener_result {
            Ok(_) => BoundSocketIdentity::capture(path).map(Some),
            Err(_) => Ok(None),
        };
        let reset_result = contexts.reset();

        let identity = match identity_result {
            Ok(identity) => identity,
            Err(identity_error) => {
                drop(listener_result);
                return Err(match reset_result {
                    Ok(()) => identity_error,
                    Err(reset_error) => format!(
                        "{identity_error}; could not reset SELinux socket create contexts: {reset_error}"
                    ),
                });
            }
        };
        let listener = match (listener_result, reset_result) {
            (Ok(listener), Ok(())) => listener,
            (Err(bind_error), Ok(())) => return Err(bind_error),
            (Ok(_), Err(reset_error)) => {
                drop(identity);
                return Err(format!(
                    "could not reset SELinux socket create contexts: {reset_error}"
                ));
            }
            (Err(bind_error), Err(reset_error)) => {
                return Err(format!(
                    "{bind_error}; could not reset SELinux socket create contexts: {reset_error}"
                ));
            }
        };
        self.verify_path_label(path)?;
        let identity = identity
            .ok_or_else(|| "labeled broker bind did not capture a socket identity".to_string())?;
        Ok((listener, identity))
    }

    pub fn verify_process_label(&self, pid: i32) -> Result<(), String> {
        let observed = read_process_context(pid)?;
        if observed != self.labels.process_label {
            return Err(
                "registered adapter SELinux context does not match Podman ProcessLabel".to_string(),
            );
        }
        Ok(())
    }

    pub fn verify_peer_label(&self, stream: &UnixStream) -> Result<(), String> {
        let observed = self.api.peer_context(stream.as_raw_fd())?;
        if observed != self.labels.process_label {
            return Err(
                "broker peer SELinux context does not match Podman ProcessLabel".to_string(),
            );
        }
        Ok(())
    }

    pub fn verify_path_label(&self, path: &Path) -> Result<(), String> {
        let observed = self.api.path_context(path)?;
        if observed != self.labels.mount_label {
            return Err(
                "broker socket pathname SELinux context does not match Podman MountLabel"
                    .to_string(),
            );
        }
        Ok(())
    }
}

pub(crate) fn validate_mount_label_only(mount_label: &str) -> Result<(), String> {
    let mount = ParsedContext::parse(mount_label, "Podman mount label")?;
    if mount.role != "object_r" || mount.context_type != FILE_TYPE {
        return Err("Podman mount label must use object_r:container_file_t".to_string());
    }
    validate_private_mcs(mount.range)
}

/// Unlabeled operation is permitted only when libselinux positively reports
/// that SELinux is disabled. Missing libselinux, an indeterminate state, or an
/// enabled host fails closed; same-context peers are not a substitute for the
/// exact private Podman labels.
pub fn verify_selinux_disabled() -> Result<(), String> {
    let api = SelinuxApi::load()
        .map_err(|error| format!("could not verify that SELinux is disabled: {error}"))?;
    match api.is_enabled()? {
        0 => Ok(()),
        1 => Err(
            "SELinux-enabled broker peers require exact Podman ProcessLabel and MountLabel"
                .to_string(),
        ),
        _ => Err("libselinux returned an invalid enabled state".to_string()),
    }
}

pub fn verify_unlabeled_peer_context(_pid: i32) -> Result<(), String> {
    verify_selinux_disabled()
}

struct ParsedContext<'a> {
    user: &'a str,
    role: &'a str,
    context_type: &'a str,
    range: &'a str,
}

impl<'a> ParsedContext<'a> {
    fn parse(value: &'a str, label: &str) -> Result<Self, String> {
        if value.is_empty()
            || value.len() > MAX_CONTEXT_BYTES
            || value
                .bytes()
                .any(|byte| byte == 0 || byte.is_ascii_control() || byte.is_ascii_whitespace())
        {
            return Err(format!("{label} is empty, oversized, or malformed"));
        }
        let mut fields = value.splitn(4, ':');
        let user = fields.next().unwrap_or_default();
        let role = fields.next().unwrap_or_default();
        let context_type = fields.next().unwrap_or_default();
        let range = fields.next().unwrap_or_default();
        if user.is_empty() || role.is_empty() || context_type.is_empty() || range.is_empty() {
            return Err(format!("{label} is not a complete SELinux context"));
        }
        Ok(Self {
            user,
            role,
            context_type,
            range,
        })
    }
}

fn validate_private_mcs(range: &str) -> Result<(), String> {
    let categories = range.strip_prefix("s0:").ok_or_else(|| {
        "Podman SELinux labels must include a private s0 MCS category pair".to_string()
    })?;
    let parsed = categories
        .split(',')
        .map(|category| {
            category
                .strip_prefix('c')
                .ok_or_else(|| "Podman MCS categories are malformed".to_string())?
                .parse::<u16>()
                .map_err(|_| "Podman MCS categories are malformed".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if parsed.len() != 2 || parsed[0] == parsed[1] || parsed.iter().any(|category| *category > 1023)
    {
        return Err(
            "Podman SELinux labels must include two distinct private MCS categories".to_string(),
        );
    }
    Ok(())
}

fn read_process_context(pid: i32) -> Result<String, String> {
    if pid <= 0 {
        return Err("SELinux peer PID is invalid".to_string());
    }
    let path = PathBuf::from(format!("/proc/{pid}/attr/current"));
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if bytes.is_empty() || bytes.len() > MAX_CONTEXT_BYTES {
        return Err("peer SELinux context has an invalid size".to_string());
    }
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| "peer SELinux context is not valid UTF-8".to_string())?
        .trim_end_matches(['\n', '\0']);
    ParsedContext::parse(value, "peer SELinux context")?;
    Ok(value.to_string())
}

type IsSelinuxEnabled = unsafe extern "C" fn() -> c_int;
type GetConRaw = unsafe extern "C" fn(*mut *mut c_char) -> c_int;
type GetPeerConRaw = unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int;
type LGetFileConRaw = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int;
type SetCreateConRaw = unsafe extern "C" fn(*const c_char) -> c_int;
type FreeCon = unsafe extern "C" fn(*mut c_char);

struct SelinuxApi {
    handle: *mut c_void,
    is_selinux_enabled: IsSelinuxEnabled,
    getcon_raw: GetConRaw,
    getpeercon_raw: GetPeerConRaw,
    lgetfilecon_raw: LGetFileConRaw,
    setfscreatecon_raw: SetCreateConRaw,
    setsockcreatecon_raw: SetCreateConRaw,
    freecon: FreeCon,
}

// libselinux's process-global code and these immutable function pointers are
// thread-safe. Create contexts themselves are scoped to the calling task and
// are never held across an await point.
unsafe impl Send for SelinuxApi {}
unsafe impl Sync for SelinuxApi {}

impl SelinuxApi {
    fn load() -> Result<Arc<Self>, String> {
        let library = c"libselinux.so.1";
        // SAFETY: `library` is a static NUL-terminated string and flags are valid.
        let handle = unsafe { libc::dlopen(library.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return Err(format!("could not load libselinux.so.1: {}", dl_error()));
        }

        let loaded = (|| {
            // SAFETY: each required symbol has the corresponding stable
            // libselinux ABI signature.
            unsafe {
                Ok(Self {
                    handle,
                    is_selinux_enabled: std::mem::transmute::<*mut c_void, IsSelinuxEnabled>(
                        required_symbol(handle, c"is_selinux_enabled")?,
                    ),
                    getcon_raw: std::mem::transmute::<*mut c_void, GetConRaw>(required_symbol(
                        handle,
                        c"getcon_raw",
                    )?),
                    getpeercon_raw: std::mem::transmute::<*mut c_void, GetPeerConRaw>(
                        required_symbol(handle, c"getpeercon_raw")?,
                    ),
                    lgetfilecon_raw: std::mem::transmute::<*mut c_void, LGetFileConRaw>(
                        required_symbol(handle, c"lgetfilecon_raw")?,
                    ),
                    setfscreatecon_raw: std::mem::transmute::<*mut c_void, SetCreateConRaw>(
                        required_symbol(handle, c"setfscreatecon_raw")?,
                    ),
                    setsockcreatecon_raw: std::mem::transmute::<*mut c_void, SetCreateConRaw>(
                        required_symbol(handle, c"setsockcreatecon_raw")?,
                    ),
                    freecon: std::mem::transmute::<*mut c_void, FreeCon>(required_symbol(
                        handle, c"freecon",
                    )?),
                })
            }
        })();
        match loaded {
            Ok(api) => Ok(Arc::new(api)),
            Err(error) => {
                // SAFETY: `handle` came from a successful dlopen and is not used again.
                unsafe { libc::dlclose(handle) };
                Err(error)
            }
        }
    }

    fn is_enabled(&self) -> Result<c_int, String> {
        // SAFETY: function pointer was resolved from libselinux.
        let enabled = unsafe { (self.is_selinux_enabled)() };
        if enabled < 0 {
            Err(format!(
                "could not determine whether SELinux is enabled: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(enabled)
        }
    }

    fn current_context(&self) -> Result<String, String> {
        let mut context = std::ptr::null_mut();
        // SAFETY: libselinux initializes `context` on success.
        let result = unsafe { (self.getcon_raw)(&mut context) };
        self.copy_and_free_context(result, context, "current process SELinux context")
    }

    fn peer_context(&self, fd: RawFd) -> Result<String, String> {
        let mut context = std::ptr::null_mut();
        // SAFETY: fd is a live Unix stream and libselinux initializes `context`.
        let result = unsafe { (self.getpeercon_raw)(fd, &mut context) };
        self.copy_and_free_context(result, context, "Unix peer SELinux context")
    }

    fn path_context(&self, path: &Path) -> Result<String, String> {
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "broker socket path contains NUL".to_string())?;
        let mut context = std::ptr::null_mut();
        // SAFETY: path is NUL-terminated and libselinux initializes `context`.
        let result = unsafe { (self.lgetfilecon_raw)(path.as_ptr(), &mut context) };
        self.copy_and_free_context(result, context, "broker socket pathname SELinux context")
    }

    fn copy_and_free_context(
        &self,
        result: c_int,
        context: *mut c_char,
        label: &str,
    ) -> Result<String, String> {
        if result < 0 || context.is_null() {
            return Err(format!(
                "could not read {label}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: successful libselinux context calls return an allocated,
        // NUL-terminated string released with freecon.
        let copied = unsafe { CStr::from_ptr(context) }
            .to_str()
            .map(str::to_string)
            .map_err(|_| format!("{label} is not valid UTF-8"));
        // SAFETY: `context` is owned by libselinux after a successful call.
        unsafe { (self.freecon)(context) };
        let copied = copied?;
        ParsedContext::parse(&copied, label)?;
        Ok(copied)
    }
}

impl Drop for SelinuxApi {
    fn drop(&mut self) {
        // SAFETY: this is the one successful dlopen handle and all Arc users are gone.
        unsafe { libc::dlclose(self.handle) };
    }
}

trait CreateContextOps {
    fn set_fs_create(&self, context: Option<&CStr>) -> Result<(), String>;
    fn set_socket_create(&self, context: Option<&CStr>) -> Result<(), String>;
}

impl CreateContextOps for SelinuxApi {
    fn set_fs_create(&self, context: Option<&CStr>) -> Result<(), String> {
        let pointer = context.map_or(std::ptr::null(), CStr::as_ptr);
        // SAFETY: pointer is either NULL or a live NUL-terminated context.
        let result = unsafe { (self.setfscreatecon_raw)(pointer) };
        if result == 0 {
            Ok(())
        } else {
            Err(format!(
                "setfscreatecon_raw failed: {}",
                std::io::Error::last_os_error()
            ))
        }
    }

    fn set_socket_create(&self, context: Option<&CStr>) -> Result<(), String> {
        let pointer = context.map_or(std::ptr::null(), CStr::as_ptr);
        // SAFETY: pointer is either NULL or a live NUL-terminated context.
        let result = unsafe { (self.setsockcreatecon_raw)(pointer) };
        if result == 0 {
            Ok(())
        } else {
            Err(format!(
                "setsockcreatecon_raw failed: {}",
                std::io::Error::last_os_error()
            ))
        }
    }
}

struct CreateContextGuard<'a, T: CreateContextOps + ?Sized> {
    ops: &'a T,
    fs_set: bool,
    socket_set: bool,
}

impl<'a, T: CreateContextOps + ?Sized> CreateContextGuard<'a, T> {
    fn arm(ops: &'a T, fs: &CStr, socket: &CStr) -> Result<Self, String> {
        ops.set_fs_create(Some(fs))?;
        let mut guard = Self {
            ops,
            fs_set: true,
            socket_set: false,
        };
        if let Err(error) = ops.set_socket_create(Some(socket)) {
            let reset = guard.reset();
            return Err(match reset {
                Ok(()) => error,
                Err(reset) => format!("{error}; context reset also failed: {reset}"),
            });
        }
        guard.socket_set = true;
        Ok(guard)
    }

    fn reset(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        if self.socket_set {
            match self.ops.set_socket_create(None) {
                Ok(()) => self.socket_set = false,
                Err(error) => errors.push(error),
            }
        }
        if self.fs_set {
            match self.ops.set_fs_create(None) {
                Ok(()) => self.fs_set = false,
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

impl<T: CreateContextOps + ?Sized> Drop for CreateContextGuard<'_, T> {
    fn drop(&mut self) {
        let _ = self.reset();
    }
}

pub(crate) struct BoundSocketIdentity {
    path: PathBuf,
    device: u64,
    inode: u64,
    armed: bool,
}

impl BoundSocketIdentity {
    fn capture(path: &Path) -> Result<Self, String> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| format!("could not inspect newly bound broker socket: {error}"))?;
        if !metadata.file_type().is_socket() {
            return Err("newly bound broker path is not a Unix socket".to_string());
        }
        Ok(Self {
            path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
            armed: true,
        })
    }

    pub(crate) fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for BoundSocketIdentity {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
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

unsafe fn required_symbol(handle: *mut c_void, name: &CStr) -> Result<*mut c_void, String> {
    // SAFETY: clearing and reading dlerror is valid around a dlsym call.
    unsafe { libc::dlerror() };
    // SAFETY: handle is live and name is NUL-terminated.
    let symbol = unsafe { libc::dlsym(handle, name.as_ptr()) };
    // SAFETY: dlerror returns either NULL or a NUL-terminated diagnostic.
    let error = unsafe { libc::dlerror() };
    if !error.is_null() || symbol.is_null() {
        let detail = if error.is_null() {
            "symbol resolved to NULL".to_string()
        } else {
            // SAFETY: non-NULL dlerror is a NUL-terminated diagnostic.
            unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        };
        Err(format!(
            "libselinux is missing {}: {detail}",
            name.to_string_lossy()
        ))
    } else {
        Ok(symbol)
    }
}

fn dl_error() -> String {
    // SAFETY: dlerror returns either NULL or a NUL-terminated diagnostic.
    let error = unsafe { libc::dlerror() };
    if error.is_null() {
        "unknown dynamic-loader error".to_string()
    } else {
        // SAFETY: checked non-NULL above.
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn podman_labels_require_matching_private_mcs_types() {
        SelinuxLabels::new(
            "system_u:system_r:container_t:s0:c42,c100".to_string(),
            "system_u:object_r:container_file_t:s0:c42,c100".to_string(),
        )
        .unwrap();
        assert!(
            SelinuxLabels::new(
                "system_u:system_r:container_t:s0".to_string(),
                "system_u:object_r:container_file_t:s0".to_string(),
            )
            .is_err()
        );
        assert!(
            SelinuxLabels::new(
                "system_u:system_r:container_t:s0:c42,c100".to_string(),
                "system_u:object_r:container_file_t:s0:c42,c101".to_string(),
            )
            .is_err()
        );
        assert!(
            SelinuxLabels::new(
                "system_u:system_r:spc_t:s0:c42,c100".to_string(),
                "system_u:object_r:container_file_t:s0:c42,c100".to_string(),
            )
            .is_err()
        );
    }

    struct FakeOps {
        calls: RefCell<Vec<&'static str>>,
        fail_socket_set: bool,
    }

    impl CreateContextOps for FakeOps {
        fn set_fs_create(&self, context: Option<&CStr>) -> Result<(), String> {
            self.calls.borrow_mut().push(if context.is_some() {
                "fs:set"
            } else {
                "fs:clear"
            });
            Ok(())
        }

        fn set_socket_create(&self, context: Option<&CStr>) -> Result<(), String> {
            self.calls.borrow_mut().push(if context.is_some() {
                "socket:set"
            } else {
                "socket:clear"
            });
            if context.is_some() && self.fail_socket_set {
                Err("injected socket context failure".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn create_contexts_are_set_and_reset_in_strict_order() {
        let ops = FakeOps {
            calls: RefCell::new(Vec::new()),
            fail_socket_set: false,
        };
        let fs = c"system_u:object_r:container_file_t:s0:c1,c2";
        let socket = c"system_u:system_r:container_t:s0:c1,c2";
        let mut guard = CreateContextGuard::arm(&ops, fs, socket).unwrap();
        guard.reset().unwrap();
        drop(guard);
        assert_eq!(
            ops.calls.into_inner(),
            ["fs:set", "socket:set", "socket:clear", "fs:clear"]
        );
    }

    #[test]
    fn partial_context_setup_is_cleared_without_fallback() {
        let ops = FakeOps {
            calls: RefCell::new(Vec::new()),
            fail_socket_set: true,
        };
        let error = CreateContextGuard::arm(
            &ops,
            c"system_u:object_r:container_file_t:s0:c1,c2",
            c"system_u:system_r:container_t:s0:c1,c2",
        )
        .err()
        .unwrap();
        assert!(error.contains("injected socket context failure"));
        assert_eq!(ops.calls.into_inner(), ["fs:set", "socket:set", "fs:clear"]);
    }
}
