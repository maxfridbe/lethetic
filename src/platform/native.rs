//! Desktop implementations shared by Linux, macOS, and Windows.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::process::Output;

/// Process handle types, re-exported so callers of the spawn helpers don't
/// import `tokio::process` directly (native-only; absent on wasm32).
pub use tokio::process::{Child, ChildStdin, ChildStdout};

#[cfg(not(windows))]
const SHELL: (&str, &str) = ("bash", "-c");
#[cfg(windows)]
const SHELL: (&str, &str) = ("cmd", "/C");

#[cfg(not(windows))]
const WHICH: &str = "which";
#[cfg(windows)]
const WHICH: &str = "where";

/// Root directory for lethetic's per-user configuration
/// (e.g. `~/.config/lethetic` on Linux).
pub fn lethetic_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".config"))
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .join("lethetic")
}

/// Root directory for Lethetic's per-user durable runtime state.
pub fn lethetic_state_dir() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|home| home.join(".local/state"))
                .unwrap_or_else(|| PathBuf::from("."))
        })
        .join("lethetic")
}

/// Run a program with arguments and capture its output. The child is killed
/// if the returned future is dropped (e.g. by a cancellation `select!`).
pub async fn command_output<I, S>(
    program: &str,
    args: I,
    cwd: Option<&str>,
) -> std::io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.spawn()?.wait_with_output().await
}

/// Run a command line through the platform shell and capture its output.
/// The child is killed if the returned future is dropped.
pub async fn shell_output(command: &str, cwd: Option<&str>) -> std::io::Result<Output> {
    command_output(SHELL.0, [SHELL.1, command], cwd).await
}

/// Spawn a command line through the platform shell with piped stdout/stderr
/// for line-by-line streaming. The child is killed when dropped.
pub fn spawn_streaming_shell(command: &str, cwd: &str) -> std::io::Result<tokio::process::Child> {
    tokio::process::Command::new(SHELL.0)
        .arg(SHELL.1)
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

/// Spawn a shell command that outlives the tool call that started it. Stdin
/// is closed, and on Unix the command leads its own process group so
/// [`terminate_process_group`] also stops whatever it spawned.
pub fn spawn_background_shell(command: &str, cwd: &str) -> std::io::Result<tokio::process::Child> {
    let mut shell = tokio::process::Command::new(SHELL.0);
    shell
        .arg(SHELL.1)
        .arg(command)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    shell.process_group(0);
    shell.spawn()
}

/// Signals a whole process group started by [`spawn_background_shell`]:
/// SIGTERM, or SIGKILL when `force` is set. Elsewhere this is a no-op and the
/// caller kills the direct child.
pub fn terminate_process_group(pid: u32, force: bool) {
    #[cfg(unix)]
    {
        use rustix::process::{Pid, Signal, kill_process_group};
        let signal = if force { Signal::KILL } else { Signal::TERM };
        if let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
            let _ = kill_process_group(pid, signal);
        }
    }
    #[cfg(not(unix))]
    let _ = (pid, force);
}

/// Spawn a long-lived server process (e.g. an LSP server) with piped
/// stdin/stdout for JSON-RPC style communication. The child is killed when
/// dropped.
pub fn spawn_piped_server(program: &str, args: &[&str]) -> std::io::Result<tokio::process::Child> {
    tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
}

/// True when `program` resolves on the user's PATH.
pub fn binary_on_path(program: &str) -> bool {
    std::process::Command::new(WHICH)
        .arg(program)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Atomically writes a regular file below an already-existing trusted root.
///
/// Every relative component is a single caller-supplied basename. On Unix the
/// walk, temporary-file creation, validation, and rename are descriptor-relative
/// and use `O_NOFOLLOW`, so a writable child cannot redirect a host write by
/// swapping a directory or destination for a symlink.
pub(crate) fn atomic_write_nofollow(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    contents: &[u8],
    mode: u32,
) -> std::io::Result<PathBuf> {
    validate_storage_components(directories, file_name)?;
    atomic_write_nofollow_impl(root, directories, file_name, contents, mode)
}

/// Atomically creates a regular file below a trusted root without replacing an
/// existing entry. The destination becomes visible only after its contents are
/// synced.
pub(crate) fn atomic_create_nofollow(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    contents: &[u8],
    mode: u32,
) -> std::io::Result<PathBuf> {
    validate_storage_components(directories, file_name)?;
    atomic_create_nofollow_impl(root, directories, file_name, contents, mode)
}

/// Reads a regular file below a trusted root without following any relative
/// symlink. A missing directory or file is returned as `None`.
pub(crate) fn read_file_nofollow(
    root: &Path,
    directories: &[&str],
    file_name: &str,
) -> std::io::Result<Option<Vec<u8>>> {
    validate_storage_components(directories, file_name)?;
    read_file_nofollow_impl(root, directories, file_name, None)
}

pub(crate) fn read_file_nofollow_bounded(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    max_bytes: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    validate_storage_components(directories, file_name)?;
    read_file_nofollow_impl(root, directories, file_name, Some(max_bytes))
}

pub(crate) fn remove_file_nofollow(
    root: &Path,
    directories: &[&str],
    file_name: &str,
) -> std::io::Result<bool> {
    validate_storage_components(directories, file_name)?;
    remove_file_nofollow_impl(root, directories, file_name)
}

/// Appends bytes to a trusted regular file without following any relative symlink.
pub(crate) fn append_file_nofollow(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    contents: &[u8],
    mode: u32,
) -> std::io::Result<PathBuf> {
    validate_storage_components(directories, file_name)?;
    append_file_nofollow_impl(root, directories, file_name, contents, mode)
}

/// Opens or creates a regular lock file below a trusted root without following
/// relative symlinks. Callers are responsible for applying an OS-level lock.
pub(crate) fn open_lock_file_nofollow(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    mode: u32,
) -> std::io::Result<std::fs::File> {
    validate_storage_components(directories, file_name)?;
    open_lock_file_nofollow_impl(root, directories, file_name, mode)
}

fn validate_storage_components(directories: &[&str], file_name: &str) -> std::io::Result<()> {
    for component in directories
        .iter()
        .copied()
        .chain(std::iter::once(file_name))
    {
        let mut parts = Path::new(component).components();
        if component.is_empty()
            || !matches!(parts.next(), Some(Component::Normal(_)))
            || parts.next().is_some()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unsafe storage path component {component:?}"),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn atomic_write_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    contents: &[u8],
    mode: u32,
) -> std::io::Result<PathBuf> {
    use rustix::fs::{AtFlags, Mode, OFlags};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut directory = open_root_directory(root)?;
    for component in directories {
        directory = open_or_create_child_directory(&directory, component)?;
    }

    reject_non_regular_entry(&directory, file_name)?;

    let mut temporary_name = None;
    let mut temporary_file = None;
    for _ in 0..128 {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let candidate = format!(".{file_name}.tmp-{}-{nanos}-{counter}", std::process::id());
        match rustix::fs::openat(
            &directory,
            candidate.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(mode),
        ) {
            Ok(fd) => {
                temporary_name = Some(candidate);
                temporary_file = Some(std::fs::File::from(fd));
                break;
            }
            Err(error) if error == rustix::io::Errno::EXIST => continue,
            Err(error) => return Err(errno_to_io(error)),
        }
    }

    let temporary_name = temporary_name.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a unique temporary host-storage file",
        )
    })?;
    let mut temporary_file = temporary_file.expect("temporary name and file are set together");

    let write_result = (|| -> std::io::Result<()> {
        temporary_file.write_all(contents)?;
        temporary_file.sync_all()?;
        drop(temporary_file);

        // This check gives a clear rejection for a pre-existing symlink. If an
        // attacker races it afterwards, renameat replaces the link itself and
        // never follows it.
        reject_non_regular_entry(&directory, file_name)?;
        rustix::fs::renameat(&directory, temporary_name.as_str(), &directory, file_name)
            .map_err(errno_to_io)?;
        rustix::fs::fsync(&directory).map_err(errno_to_io)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = rustix::fs::unlinkat(&directory, temporary_name.as_str(), AtFlags::empty());
    }
    write_result?;

    Ok(storage_path(root, directories, file_name))
}

#[cfg(unix)]
fn atomic_create_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    contents: &[u8],
    mode: u32,
) -> std::io::Result<PathBuf> {
    use rustix::fs::{AtFlags, Mode, OFlags};
    use std::io::Write;

    let mut directory = open_root_directory(root)?;
    for component in directories {
        directory = open_or_create_child_directory(&directory, component)?;
    }

    let mut temporary_name = None;
    let mut temporary_file = None;
    for _ in 0..128 {
        let candidate = format!(".{file_name}.tmp-{}", uuid::Uuid::new_v4().simple());
        match rustix::fs::openat(
            &directory,
            candidate.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(mode),
        ) {
            Ok(fd) => {
                temporary_name = Some(candidate);
                temporary_file = Some(std::fs::File::from(fd));
                break;
            }
            Err(error) if error == rustix::io::Errno::EXIST => continue,
            Err(error) => return Err(errno_to_io(error)),
        }
    }

    let temporary_name = temporary_name.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a unique no-clobber temporary file",
        )
    })?;
    let mut temporary_file = temporary_file.expect("temporary name and file are set together");
    let result = (|| -> std::io::Result<()> {
        temporary_file.write_all(contents)?;
        temporary_file.sync_all()?;
        drop(temporary_file);
        rustix::fs::linkat(
            &directory,
            temporary_name.as_str(),
            &directory,
            file_name,
            AtFlags::empty(),
        )
        .map_err(errno_to_io)?;
        rustix::fs::unlinkat(&directory, temporary_name.as_str(), AtFlags::empty())
            .map_err(errno_to_io)?;
        rustix::fs::fsync(&directory).map_err(errno_to_io)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = rustix::fs::unlinkat(&directory, temporary_name.as_str(), AtFlags::empty());
    }
    result?;
    Ok(storage_path(root, directories, file_name))
}

#[cfg(unix)]
fn reject_non_regular_entry<Fd: std::os::fd::AsFd>(
    directory: &Fd,
    file_name: &str,
) -> std::io::Result<()> {
    use rustix::fs::{AtFlags, FileType};

    match rustix::fs::statat(directory, file_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            let file_type = FileType::from_raw_mode(stat.st_mode);
            if file_type.is_symlink() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("refusing symlink destination {file_name:?}"),
                ))
            } else if !file_type.is_file() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("refusing non-regular destination {file_name:?}"),
                ))
            } else {
                Ok(())
            }
        }
        Err(error) if error == rustix::io::Errno::NOENT => Ok(()),
        Err(error) => Err(errno_to_io(error)),
    }
}

#[cfg(unix)]
fn read_file_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    max_bytes: Option<usize>,
) -> std::io::Result<Option<Vec<u8>>> {
    use rustix::fs::{FileType, Mode, OFlags};
    use std::io::Read;

    let mut directory = open_root_directory(root)?;
    for component in directories {
        match open_child_directory(&directory, component) {
            Ok(child) => directory = child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
    }

    let fd = match rustix::fs::openat(
        &directory,
        file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(error) if error == rustix::io::Errno::LOOP => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("refusing symlink destination {file_name:?}"),
            ));
        }
        Err(error) => return Err(errno_to_io(error)),
    };
    let stat = rustix::fs::fstat(&fd).map_err(errno_to_io)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing non-regular destination {file_name:?}"),
        ));
    }
    let mut file = std::fs::File::from(fd);
    let mut bytes = Vec::new();
    if let Some(max_bytes) = max_bytes {
        if stat.st_size < 0 || stat.st_size as u64 > max_bytes as u64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("storage file exceeds the {max_bytes}-byte limit"),
            ));
        }
        let limit = max_bytes.saturating_add(1) as u64;
        file.take(limit).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("storage file exceeds the {max_bytes}-byte limit"),
            ));
        }
    } else {
        file.read_to_end(&mut bytes)?;
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
fn remove_file_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
) -> std::io::Result<bool> {
    use rustix::fs::{AtFlags, FileType};

    let mut directory = open_root_directory(root)?;
    for component in directories {
        match open_child_directory(&directory, component) {
            Ok(child) => directory = child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    let stat = match rustix::fs::statat(&directory, file_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(false),
        Err(error) => return Err(errno_to_io(error)),
    };
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_nlink != 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing untrusted removal destination {file_name:?}"),
        ));
    }
    rustix::fs::unlinkat(&directory, file_name, AtFlags::empty()).map_err(errno_to_io)?;
    rustix::fs::fsync(&directory).map_err(errno_to_io)?;
    Ok(true)
}

#[cfg(unix)]
fn append_file_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    contents: &[u8],
    mode: u32,
) -> std::io::Result<PathBuf> {
    use rustix::fs::{FileType, Mode, OFlags};
    use std::io::Write;

    let mut directory = open_root_directory(root)?;
    for component in directories {
        directory = open_or_create_child_directory(&directory, component)?;
    }
    let fd = rustix::fs::openat(
        &directory,
        file_name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::APPEND | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(mode),
    )
    .map_err(errno_to_io)?;
    let stat = rustix::fs::fstat(&fd).map_err(errno_to_io)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_nlink != 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing untrusted append destination {file_name:?}"),
        ));
    }
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(mode)).map_err(errno_to_io)?;
    let mut file = std::fs::File::from(fd);
    file.write_all(contents)?;
    Ok(storage_path(root, directories, file_name))
}

#[cfg(unix)]
fn open_lock_file_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    mode: u32,
) -> std::io::Result<std::fs::File> {
    use rustix::fs::{FileType, Mode, OFlags};

    let mut directory = open_root_directory(root)?;
    for component in directories {
        directory = open_or_create_child_directory(&directory, component)?;
    }
    let fd = rustix::fs::openat(
        &directory,
        file_name,
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(mode),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::LOOP {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("refusing symlink lock destination {file_name:?}"),
            )
        } else {
            errno_to_io(error)
        }
    })?;
    let stat = rustix::fs::fstat(&fd).map_err(errno_to_io)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_nlink != 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing untrusted lock destination {file_name:?}"),
        ));
    }
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(mode)).map_err(errno_to_io)?;
    rustix::fs::fsync(&directory).map_err(errno_to_io)?;
    Ok(std::fs::File::from(fd))
}

#[cfg(unix)]
fn open_root_directory(root: &Path) -> std::io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags};

    rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        let source = errno_to_io(error);
        std::io::Error::new(
            source.kind(),
            format!(
                "refusing host-storage root {} unless it is a real directory: {source}",
                root.display()
            ),
        )
    })
}

#[cfg(unix)]
fn open_child_directory<Fd: std::os::fd::AsFd>(
    parent: &Fd,
    component: &str,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags};

    rustix::fs::openat(
        parent,
        component,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        let source = errno_to_io(error);
        if error == rustix::io::Errno::LOOP || error == rustix::io::Errno::NOTDIR {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "refusing symlink or non-directory host-storage component {component:?}: {source}"
                ),
            )
        } else {
            source
        }
    })
}

#[cfg(unix)]
fn open_or_create_child_directory<Fd: std::os::fd::AsFd>(
    parent: &Fd,
    component: &str,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::Mode;

    match open_child_directory(parent, component) {
        Ok(directory) => Ok(directory),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match rustix::fs::mkdirat(parent, component, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
                Ok(()) => {}
                Err(error) if error == rustix::io::Errno::EXIST => {}
                Err(error) => return Err(errno_to_io(error)),
            }
            // Persist the directory entry before callers write state below it.
            // Sync even after a raced EEXIST: the initial open observed ENOENT,
            // so this process cannot otherwise know that the winning entry is
            // durably linked in the parent.
            rustix::fs::fsync(parent).map_err(errno_to_io)?;
            open_child_directory(parent, component)
        }
        Err(error) => Err(error),
    }
}

/// Creates an absolute private directory path one component at a time without
/// following symlinks. Every newly linked component is parent-synced before the
/// walk continues, so a later durable file cannot outlive missing ancestry.
#[cfg(target_os = "linux")]
pub(crate) fn ensure_private_directory_durable(path: &Path, mode: u32) -> std::io::Result<()> {
    use rustix::fs::{FileType, Mode};

    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "private directory path must be absolute: {}",
                path.display()
            ),
        ));
    }
    let mut directory = open_root_directory(Path::new("/"))?;
    let mut saw_component = false;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(component) => {
                let component = component.to_str().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "private directory path must be UTF-8",
                    )
                })?;
                directory = open_or_create_child_directory(&directory, component)?;
                saw_component = true;
            }
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "private directory path is not normalized: {}",
                        path.display()
                    ),
                ));
            }
        }
    }
    if !saw_component {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to use the filesystem root as private storage",
        ));
    }

    let stat = rustix::fs::fstat(&directory).map_err(errno_to_io)?;
    if !FileType::from_raw_mode(stat.st_mode).is_dir()
        || stat.st_uid != rustix::process::geteuid().as_raw()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "private directory is not owner-controlled: {}",
                path.display()
            ),
        ));
    }
    rustix::fs::fchmod(&directory, Mode::from_raw_mode(mode)).map_err(errno_to_io)?;
    rustix::fs::fsync(&directory).map_err(errno_to_io)?;

    let canonical = path.canonicalize()?;
    if canonical != path {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "private directory path is not canonical: {}",
                path.display()
            ),
        ));
    }
    let current = rustix::fs::fstat(&directory).map_err(errno_to_io)?;
    if current.st_dev != stat.st_dev
        || current.st_ino != stat.st_ino
        || current.st_uid != stat.st_uid
        || current.st_mode & 0o777 != mode
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "private directory changed while securing it: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn remove_tree_nofollow_same_mount(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "deletion target has no parent directory",
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "deletion target has no basename",
        )
    })?;
    if matches!(name.as_encoded_bytes(), b"." | b"..") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing dot deletion target",
        ));
    }

    let parent = open_root_directory(parent)?;
    let parent_mount_id = mount_id_for_fd(&parent)?;
    remove_entry_at(&parent, name, parent_mount_id)
}

#[cfg(target_os = "linux")]
struct DeletionPathNode {
    parent: Option<std::sync::Arc<DeletionPathNode>>,
    name: std::ffi::OsString,
    device: u64,
    inode: u64,
}

#[cfg(target_os = "linux")]
struct DeletionNode {
    path: std::sync::Arc<DeletionPathNode>,
    expanded: bool,
}

#[cfg(target_os = "linux")]
fn remove_entry_at<Fd: std::os::fd::AsFd>(
    root_parent: &Fd,
    name: &OsStr,
    expected_mount_id: u64,
) -> std::io::Result<()> {
    use rustix::fs::{AtFlags, FileType};
    use std::os::unix::ffi::OsStringExt;
    use std::sync::Arc;

    let (_, root_stat) = inspect_deletion_entry(root_parent, name, expected_mount_id)?;
    let root_type = FileType::from_raw_mode(root_stat.st_mode);
    if !root_type.is_dir() {
        return unlink_inspected_entry(
            root_parent,
            name,
            root_stat.st_dev,
            root_stat.st_ino,
            root_type,
            expected_mount_id,
        );
    }
    if try_remove_empty_unowned_directory(root_parent, name, &root_stat, expected_mount_id)? {
        return Ok(());
    }
    normalize_deletion_directory(root_parent, name, &root_stat, expected_mount_id)?;
    let root_path = Arc::new(DeletionPathNode {
        parent: None,
        name: name.to_os_string(),
        device: root_stat.st_dev,
        inode: root_stat.st_ino,
    });
    let mut pending = vec![DeletionNode {
        path: root_path,
        expanded: false,
    }];

    while let Some(node) = pending.pop() {
        let parent = open_deletion_parent(root_parent, &node.path, expected_mount_id)?;
        let (_, stat) = inspect_deletion_entry(&parent, &node.path.name, expected_mount_id)?;
        if stat.st_dev != node.path.device
            || stat.st_ino != node.path.inode
            || !FileType::from_raw_mode(stat.st_mode).is_dir()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "directory changed while preparing deletion: {:?}",
                    node.path.name
                ),
            ));
        }

        if node.expanded {
            rustix::fs::unlinkat(&parent, &node.path.name, AtFlags::REMOVEDIR)
                .map_err(errno_to_io)?;
            continue;
        }

        normalize_deletion_directory(&parent, &node.path.name, &stat, expected_mount_id)?;
        let directory =
            open_inspected_directory(&parent, &node.path.name, &stat, expected_mount_id)?;
        let entries = rustix::fs::Dir::read_from(&directory)
            .map_err(errno_to_io)?
            .map(|entry| {
                entry
                    .map(|entry| {
                        std::ffi::OsString::from_vec(entry.file_name().to_bytes().to_vec())
                    })
                    .map_err(errno_to_io)
            })
            .filter(|entry| {
                entry.as_ref().map_or(true, |name| {
                    name.as_encoded_bytes() != b"." && name.as_encoded_bytes() != b".."
                })
            })
            .collect::<std::io::Result<Vec<_>>>()?;

        pending.push(DeletionNode {
            path: node.path.clone(),
            expanded: true,
        });
        for entry in entries {
            let (_, child_stat) = inspect_deletion_entry(&directory, &entry, expected_mount_id)?;
            let child_type = FileType::from_raw_mode(child_stat.st_mode);
            if child_type.is_dir() {
                if try_remove_empty_unowned_directory(
                    &directory,
                    &entry,
                    &child_stat,
                    expected_mount_id,
                )? {
                    continue;
                }
                normalize_deletion_directory(&directory, &entry, &child_stat, expected_mount_id)?;
                pending.push(DeletionNode {
                    path: Arc::new(DeletionPathNode {
                        parent: Some(node.path.clone()),
                        name: entry,
                        device: child_stat.st_dev,
                        inode: child_stat.st_ino,
                    }),
                    expanded: false,
                });
            } else {
                unlink_inspected_entry(
                    &directory,
                    &entry,
                    child_stat.st_dev,
                    child_stat.st_ino,
                    child_type,
                    expected_mount_id,
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_deletion_parent<Fd: std::os::fd::AsFd>(
    root_parent: &Fd,
    path: &std::sync::Arc<DeletionPathNode>,
    expected_mount_id: u64,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{FileType, Mode, OFlags};

    let mut ancestors = Vec::new();
    let mut current = path.parent.clone();
    while let Some(ancestor) = current {
        current = ancestor.parent.clone();
        ancestors.push(ancestor);
    }
    ancestors.reverse();

    let mut directory = rustix::fs::openat(
        root_parent,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno_to_io)?;
    for ancestor in ancestors {
        let (_, stat) = inspect_deletion_entry(&directory, &ancestor.name, expected_mount_id)?;
        if stat.st_dev != ancestor.device
            || stat.st_ino != ancestor.inode
            || !FileType::from_raw_mode(stat.st_mode).is_dir()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "ancestor changed while reopening deletion path",
            ));
        }
        normalize_deletion_directory(&directory, &ancestor.name, &stat, expected_mount_id)?;
        directory = open_inspected_directory(&directory, &ancestor.name, &stat, expected_mount_id)?;
    }
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn inspect_deletion_entry<Fd: std::os::fd::AsFd>(
    parent: &Fd,
    name: &OsStr,
    expected_mount_id: u64,
) -> std::io::Result<(std::os::fd::OwnedFd, rustix::fs::Stat)> {
    use rustix::fs::{Mode, OFlags};

    let handle = rustix::fs::openat(
        parent,
        name,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno_to_io)?;
    let stat = rustix::fs::fstat(&handle).map_err(errno_to_io)?;
    if mount_id_for_fd(&handle)? != expected_mount_id {
        return Err(std::io::Error::new(
            std::io::ErrorKind::CrossesDevices,
            format!("refusing to cross mount boundary while deleting {:?}", name),
        ));
    }
    Ok((handle, stat))
}

#[cfg(target_os = "linux")]
fn try_remove_empty_unowned_directory<Fd: std::os::fd::AsFd>(
    parent: &Fd,
    name: &OsStr,
    expected: &rustix::fs::Stat,
    expected_mount_id: u64,
) -> std::io::Result<bool> {
    use rustix::fs::{AtFlags, FileType};

    if expected.st_uid == rustix::process::geteuid().as_raw() {
        return Ok(false);
    }
    let (_, current) = inspect_deletion_entry(parent, name, expected_mount_id)?;
    if current.st_dev != expected.st_dev
        || current.st_ino != expected.st_ino
        || !FileType::from_raw_mode(current.st_mode).is_dir()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "unowned directory changed before exact empty-directory removal",
        ));
    }
    match rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR) {
        Ok(()) => Ok(true),
        Err(error)
            if matches!(
                error,
                rustix::io::Errno::NOTEMPTY | rustix::io::Errno::EXIST
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(errno_to_io(error)),
    }
}

#[cfg(target_os = "linux")]
fn normalize_deletion_directory<Fd: std::os::fd::AsFd>(
    parent: &Fd,
    name: &OsStr,
    stat: &rustix::fs::Stat,
    expected_mount_id: u64,
) -> std::io::Result<()> {
    if stat.st_uid != rustix::process::geteuid().as_raw() {
        return Ok(());
    }
    if stat.st_mode & 0o700 != 0o700 {
        let (handle, current) = inspect_deletion_entry(parent, name, expected_mount_id)?;
        if current.st_dev != stat.st_dev || current.st_ino != stat.st_ino {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "directory changed before permission normalization",
            ));
        }
        chmod_directory_handle(&handle, 0o700)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_inspected_directory<Fd: std::os::fd::AsFd>(
    parent: &Fd,
    name: &OsStr,
    expected: &rustix::fs::Stat,
    expected_mount_id: u64,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags};

    let directory = rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno_to_io)?;
    let opened = rustix::fs::fstat(&directory).map_err(errno_to_io)?;
    if mount_id_for_fd(&directory)? != expected_mount_id
        || opened.st_dev != expected.st_dev
        || opened.st_ino != expected.st_ino
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::CrossesDevices,
            format!("directory changed while preparing deletion: {:?}", name),
        ));
    }
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn unlink_inspected_entry<Fd: std::os::fd::AsFd>(
    parent: &Fd,
    name: &OsStr,
    expected_device: u64,
    expected_inode: u64,
    expected_type: rustix::fs::FileType,
    expected_mount_id: u64,
) -> std::io::Result<()> {
    use rustix::fs::AtFlags;

    let (_, current) = inspect_deletion_entry(parent, name, expected_mount_id)?;
    if current.st_dev != expected_device
        || current.st_ino != expected_inode
        || rustix::fs::FileType::from_raw_mode(current.st_mode) != expected_type
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("entry changed while preparing deletion: {:?}", name),
        ));
    }
    rustix::fs::unlinkat(parent, name, AtFlags::empty()).map_err(errno_to_io)
}

#[cfg(target_os = "linux")]
fn mount_id_for_fd<Fd: std::os::fd::AsFd>(fd: &Fd) -> std::io::Result<u64> {
    use rustix::fs::{AtFlags, StatxFlags};
    use std::os::fd::AsRawFd;

    if let Ok(stat) = rustix::fs::statx(
        fd,
        "",
        AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW | AtFlags::NO_AUTOMOUNT,
        StatxFlags::MNT_ID,
    ) && stat.stx_mask & StatxFlags::MNT_ID.bits() != 0
    {
        return Ok(stat.stx_mnt_id);
    }

    let fdinfo = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_fd().as_raw_fd()))?;
    let mut values = fdinfo.lines().filter_map(|line| {
        line.strip_prefix("mnt_id:")
            .and_then(|value| value.trim().parse::<u64>().ok())
    });
    let mount_id = values.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "kernel did not expose a mount identity for safe recursive deletion",
        )
    })?;
    if values.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "kernel exposed ambiguous mount identities",
        ));
    }
    Ok(mount_id)
}

#[cfg(target_os = "linux")]
fn chmod_directory_handle<Fd: std::os::fd::AsFd>(directory: &Fd, mode: u32) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;

    // The pinned libc bindings omit this asm-generic syscall on AArch64.
    #[cfg(target_arch = "aarch64")]
    let syscall_number: libc::c_long = 452;
    #[cfg(not(target_arch = "aarch64"))]
    let syscall_number = libc::SYS_fchmodat2;
    let result = unsafe {
        libc::syscall(
            syscall_number,
            directory.as_fd().as_raw_fd(),
            c"".as_ptr(),
            mode as libc::mode_t,
            libc::AT_EMPTY_PATH,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if !matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP)
    ) {
        return Err(error);
    }

    let proc_path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_fd().as_raw_fd()));
    std::fs::set_permissions(proc_path, std::fs::Permissions::from_mode(mode))?;
    let stat = rustix::fs::fstat(directory).map_err(errno_to_io)?;
    if stat.st_mode & 0o777 != mode {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "directory permission repair did not affect the opened inode",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn errno_to_io(error: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(not(unix))]
fn atomic_create_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    contents: &[u8],
    _mode: u32,
) -> std::io::Result<PathBuf> {
    use std::io::Write;

    let root_metadata = std::fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing unsafe no-clobber storage root",
        ));
    }
    let mut parent = root.to_path_buf();
    for component in directories {
        parent.push(component);
        match std::fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "refusing unsafe no-clobber storage directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&parent)?;
            }
            Err(error) => return Err(error),
        }
    }
    let destination = parent.join(file_name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)?;
    if let Err(error) = (|| -> std::io::Result<()> {
        file.write_all(contents)?;
        file.sync_all()
    })() {
        drop(file);
        let _ = std::fs::remove_file(&destination);
        return Err(error);
    }
    Ok(destination)
}

#[cfg(not(unix))]
fn atomic_write_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    contents: &[u8],
    _mode: u32,
) -> std::io::Result<PathBuf> {
    use std::io::Write;

    let root_metadata = std::fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "refusing non-directory host-storage root {}",
                root.display()
            ),
        ));
    }
    let mut parent = root.to_path_buf();
    for component in directories {
        parent.push(component);
        match std::fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "refusing unsafe host-storage component {}",
                        parent.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&parent)?;
            }
            Err(error) => return Err(error),
        }
    }
    let destination = parent.join(file_name);
    if let Ok(metadata) = std::fs::symlink_metadata(&destination)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "refusing unsafe host-storage destination {}",
                destination.display()
            ),
        ));
    }
    let mut temporary_file = None;
    for _ in 0..8 {
        let temporary = parent.join(format!(".{file_name}.tmp-{}", uuid::Uuid::new_v4()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => {
                temporary_file = Some((temporary, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let (temporary, mut file) = temporary_file.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique atomic-write temporary file",
        )
    })?;
    if let Err(error) = (|| -> std::io::Result<()> {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        replace_file_nonunix(&temporary, &destination)
    })() {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(destination)
}

#[cfg(windows)]
fn replace_file_nonunix(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(not(unix), not(windows)))]
fn replace_file_nonunix(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(not(unix))]
fn append_file_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    contents: &[u8],
    _mode: u32,
) -> std::io::Result<PathBuf> {
    use std::io::Write;

    let root_metadata = std::fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing unsafe append root",
        ));
    }
    let mut directory = root.to_path_buf();
    for component in directories {
        directory.push(component);
        match std::fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "refusing unsafe append directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&directory)?;
            }
            Err(error) => return Err(error),
        }
    }
    let destination = directory.join(file_name);
    if let Ok(metadata) = std::fs::symlink_metadata(&destination)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing unsafe append destination",
        ));
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&destination)?;
    file.write_all(contents)?;
    Ok(destination)
}

#[cfg(not(unix))]
fn open_lock_file_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    _mode: u32,
) -> std::io::Result<std::fs::File> {
    let root_metadata = std::fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing unsafe lock root",
        ));
    }
    let mut directory = root.to_path_buf();
    for component in directories {
        directory.push(component);
        match std::fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "refusing unsafe lock directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&directory)?;
            }
            Err(error) => return Err(error),
        }
    }
    let destination = directory.join(file_name);
    if let Ok(metadata) = std::fs::symlink_metadata(&destination)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing unsafe lock destination",
        ));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&destination)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing non-regular lock destination",
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn read_file_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
    max_bytes: Option<usize>,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut path = root.to_path_buf();
    for component in directories {
        path.push(component);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("refusing unsafe host-storage component {}", path.display()),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    path.push(file_name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "refusing unsafe host-storage destination {}",
                    path.display()
                ),
            ))
        }
        Ok(metadata) => {
            use std::io::Read;
            if let Some(max_bytes) = max_bytes
                && metadata.len() > max_bytes as u64
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("storage file exceeds the {max_bytes}-byte limit"),
                ));
            }
            let mut file = std::fs::File::open(path)?;
            let mut bytes = Vec::new();
            if let Some(max_bytes) = max_bytes {
                file.take(max_bytes.saturating_add(1) as u64)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > max_bytes {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("storage file exceeds the {max_bytes}-byte limit"),
                    ));
                }
            } else {
                file.read_to_end(&mut bytes)?;
            }
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(not(unix))]
fn remove_file_nofollow_impl(
    root: &Path,
    directories: &[&str],
    file_name: &str,
) -> std::io::Result<bool> {
    let mut path = root.to_path_buf();
    for component in directories {
        path.push(component);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("refusing unsafe host-storage component {}", path.display()),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    path.push(file_name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "refusing unsafe host-storage destination {}",
                    path.display()
                ),
            ))
        }
        Ok(_) => {
            std::fs::remove_file(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn storage_path(root: &Path, directories: &[&str], file_name: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(directories);
    path.push(file_name);
    path
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[cfg(target_os = "linux")]
    #[test]
    fn recursive_deletion_normalizes_permissions_and_unlinks_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let locked = target.join("locked/nested");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("data"), b"inside").unwrap();
        std::fs::set_permissions(
            target.join("locked"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        symlink(outside.path(), target.join("outside-link")).unwrap();

        remove_tree_nofollow_same_mount(&target).unwrap();

        assert!(!target.exists());
        assert!(outside.path().exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires the configured rootless Podman user namespace"]
    fn recursive_deletion_removes_empty_mapped_subuid_directory() {
        use std::os::unix::fs::MetadataExt;

        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let mapped = target.join("mapped-root-empty");
        std::fs::create_dir_all(&mapped).unwrap();
        let status = std::process::Command::new("/usr/local/bin/podman")
            .args(["unshare", "chown", "1:1", "--"])
            .arg(&mapped)
            .status()
            .unwrap();
        assert!(status.success());
        assert_ne!(
            std::fs::symlink_metadata(&mapped).unwrap().uid(),
            rustix::process::geteuid().as_raw()
        );

        remove_tree_nofollow_same_mount(&target).unwrap();
        assert!(!target.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_directory_creation_walks_and_secures_new_ancestry() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("new/state/lethetic");

        ensure_private_directory_durable(&target, 0o700).unwrap();

        for directory in [
            root.path().join("new"),
            root.path().join("new/state"),
            target,
        ] {
            let metadata = std::fs::symlink_metadata(directory).unwrap();
            assert!(metadata.is_dir());
            assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_directory_creation_rejects_symlinked_ancestry() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join("redirect")).unwrap();

        let error = ensure_private_directory_durable(&root.path().join("redirect/lethetic"), 0o700)
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!outside.path().join("lethetic").exists());
    }

    #[test]
    fn atomic_create_is_no_clobber_and_private() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let path = atomic_create_nofollow(&root, &["prompts"], "demo.md", b"first", 0o600).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let error =
            atomic_create_nofollow(&root, &["prompts"], "demo.md", b"second", 0o600).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        assert_eq!(
            read_file_nofollow_bounded(&root, &["prompts"], "demo.md", 5)
                .unwrap()
                .unwrap(),
            b"first"
        );
        assert_eq!(
            read_file_nofollow_bounded(&root, &["prompts"], "demo.md", 4)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn remove_file_is_descriptor_relative_and_rejects_untrusted_targets() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let session = root.join("session");
        std::fs::create_dir(&session).unwrap();

        std::fs::write(session.join("record.json"), b"cleanup").unwrap();
        assert!(remove_file_nofollow(&root, &["session"], "record.json").unwrap());
        assert!(!remove_file_nofollow(&root, &["session"], "record.json").unwrap());

        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), b"outside").unwrap();
        symlink(outside.path(), session.join("linked.json")).unwrap();
        assert!(remove_file_nofollow(&root, &["session"], "linked.json").is_err());
        assert_eq!(std::fs::read(outside.path()).unwrap(), b"outside");
        std::fs::remove_file(session.join("linked.json")).unwrap();

        std::fs::write(session.join("shared.json"), b"shared").unwrap();
        std::fs::hard_link(session.join("shared.json"), session.join("alias.json")).unwrap();
        assert!(remove_file_nofollow(&root, &["session"], "shared.json").is_err());
        assert!(session.join("shared.json").exists());
        assert!(session.join("alias.json").exists());
    }

    #[test]
    fn append_file_is_private_and_rejects_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        append_file_nofollow(&root, &["session"], "log", b"one\n", 0o600).unwrap();
        append_file_nofollow(&root, &["session"], "log", b"two\n", 0o600).unwrap();
        let log = root.join("session/log");
        assert_eq!(std::fs::read(&log).unwrap(), b"one\ntwo\n");
        assert_eq!(
            std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let outside = tempfile::NamedTempFile::new().unwrap();
        let linked = root.join("session/linked");
        symlink(outside.path(), &linked).unwrap();
        assert!(append_file_nofollow(&root, &["session"], "linked", b"bad", 0o600).is_err());
        assert!(std::fs::read(outside.path()).unwrap().is_empty());

        let outside_directory = tempfile::tempdir().unwrap();
        symlink(outside_directory.path(), root.join("redirect")).unwrap();
        assert!(append_file_nofollow(&root, &["redirect"], "log", b"bad", 0o600).is_err());
        assert!(!outside_directory.path().join("log").exists());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn atomic_write_replaces_an_existing_file_without_predelete() {
        let root = tempfile::tempdir().unwrap();
        atomic_write_nofollow(root.path(), &[], "state.json", b"old", 0o600).unwrap();

        atomic_write_nofollow(root.path(), &[], "state.json", b"new", 0o600).unwrap();

        assert_eq!(
            std::fs::read(root.path().join("state.json")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn failed_atomic_replacement_preserves_the_previous_file() {
        let root = tempfile::tempdir().unwrap();
        let destination =
            atomic_write_nofollow(root.path(), &[], "state.json", b"durable-old", 0o600).unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&destination)
            .unwrap();

        let error =
            atomic_write_nofollow(root.path(), &[], "state.json", b"uncommitted-new", 0o600)
                .unwrap_err();
        drop(held);

        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(std::fs::read(destination).unwrap(), b"durable-old");
        let names = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["state.json"]);
    }
}

/// Total size of regular files under `root` (symlinks are not followed).
/// `None` when the directory does not exist.
pub fn directory_size_bytes(root: &Path) -> Option<u64> {
    let metadata = std::fs::symlink_metadata(root).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file()
                && let Ok(metadata) = entry.metadata()
            {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Some(total)
}
