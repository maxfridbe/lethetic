use std::time::Duration;
use tokio::process::{Child, Command};

pub(crate) const DEFAULT_SETTLE_TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(windows)]
struct OwnedWindowsHandle(usize);

#[cfg(windows)]
impl OwnedWindowsHandle {
    fn from_nullable(
        handle: windows_sys::Win32::Foundation::HANDLE,
        context: &str,
    ) -> Result<Self, String> {
        if handle.is_null() {
            Err(format!("{context}: {}", std::io::Error::last_os_error()))
        } else {
            Ok(Self(handle as usize))
        }
    }

    fn from_snapshot(
        handle: windows_sys::Win32::Foundation::HANDLE,
        context: &str,
    ) -> Result<Self, String> {
        if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            Err(format!("{context}: {}", std::io::Error::last_os_error()))
        } else {
            Self::from_nullable(handle, context)
        }
    }

    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0 as windows_sys::Win32::Foundation::HANDLE
    }
}

#[cfg(windows)]
impl Drop for OwnedWindowsHandle {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.raw());
            }
        }
    }
}

/// Tracks the ordinary process tree rooted at a launched command.
///
/// Unix descendants are covered while they remain in the inherited process
/// group. A process that deliberately creates a new session is outside this
/// mechanism. Windows descendants are assigned to a kill-on-close Job before
/// the suspended leader is resumed.
pub(crate) struct ProcessTree {
    #[cfg(unix)]
    process_group: Option<i32>,
    #[cfg(windows)]
    job: OwnedWindowsHandle,
}

impl ProcessTree {
    pub(crate) fn prepare(command: &mut Command, label: &str) -> Result<Self, String> {
        #[cfg(unix)]
        {
            let _ = label;
            command.process_group(0);
            Ok(Self {
                process_group: None,
            })
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::JobObjects::{
                CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            };
            use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

            let job = OwnedWindowsHandle::from_nullable(
                unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) },
                &format!("Could not create {label} containment job"),
            )?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = unsafe {
                SetInformationJobObject(
                    job.raw(),
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    u32::try_from(std::mem::size_of_val(&limits))
                        .expect("job limits structure size fits u32"),
                )
            };
            if configured == 0 {
                return Err(format!(
                    "Could not configure {label} containment job: {}",
                    std::io::Error::last_os_error()
                ));
            }
            command.creation_flags(CREATE_SUSPENDED);
            Ok(Self { job })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (command, label);
            Ok(Self {})
        }
    }

    pub(crate) fn attach_and_resume(&mut self, child: &Child, label: &str) -> Result<(), String> {
        #[cfg(unix)]
        {
            let id = child
                .id()
                .ok_or_else(|| format!("{label} process did not expose a process ID"))?;
            self.process_group = Some(
                i32::try_from(id)
                    .map_err(|_| format!("{label} process ID exceeded the platform range"))?,
            );
            Ok(())
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

            let process = child
                .raw_handle()
                .ok_or_else(|| format!("Suspended {label} process handle disappeared"))?
                as windows_sys::Win32::Foundation::HANDLE;
            if unsafe { AssignProcessToJobObject(self.job.raw(), process) } == 0 {
                return Err(format!(
                    "Could not assign suspended {label} process to containment job: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let process_id = child
                .id()
                .ok_or_else(|| format!("Suspended {label} process has no process ID"))?;
            resume_suspended_process(process_id, label)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (child, label);
            Ok(())
        }
    }

    #[cfg(unix)]
    pub(crate) fn signal_terminate(&self, label: &str) -> Result<(), String> {
        self.signal_unix(libc::SIGTERM, label)
    }

    pub(crate) fn signal_force(&self, label: &str) -> Result<(), String> {
        #[cfg(unix)]
        {
            self.signal_unix(libc::SIGKILL, label)
        }
        #[cfg(windows)]
        {
            if unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job.raw(), 1)
            } == 0
            {
                Err(format!(
                    "Could not terminate {label} containment job: {}",
                    std::io::Error::last_os_error()
                ))
            } else {
                Ok(())
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = label;
            Ok(())
        }
    }

    #[cfg(unix)]
    fn signal_unix(&self, signal: i32, label: &str) -> Result<(), String> {
        let id = self
            .process_group
            .ok_or_else(|| format!("{label} process group was not attached"))?;
        let result = unsafe { libc::kill(-id, signal) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(format!(
                "Could not signal {label} process group {id}: {error}"
            ))
        }
    }

    pub(crate) async fn terminate_remaining(
        &mut self,
        timeout: Duration,
        label: &str,
    ) -> Result<(), String> {
        self.signal_force(label)?;
        let wait = async {
            loop {
                if process_tree_is_empty(self, label)? {
                    return Ok::<(), String>(());
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_| format!("{label} process tree did not settle after termination"))??;
        #[cfg(unix)]
        {
            self.process_group.take();
        }
        Ok(())
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.process_group.is_some() {
            let _ = self.signal_force("launched");
        }
        // On Windows, closing the configured Job handle terminates members.
    }
}

#[cfg(windows)]
fn resume_suspended_process(process_id: u32, label: &str) -> Result<(), String> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    let snapshot = OwnedWindowsHandle::from_snapshot(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) },
        &format!("Could not inspect suspended {label} process threads"),
    )?;
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(std::mem::size_of::<THREADENTRY32>())
            .expect("thread entry structure size fits u32"),
        ..Default::default()
    };
    let mut available = unsafe { Thread32First(snapshot.raw(), &raw mut entry) } != 0;
    while available {
        if entry.th32OwnerProcessID == process_id {
            let thread = OwnedWindowsHandle::from_nullable(
                unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) },
                &format!("Could not open suspended {label} process thread"),
            )?;
            if unsafe { ResumeThread(thread.raw()) } == u32::MAX {
                return Err(format!(
                    "Could not resume contained {label} process: {}",
                    std::io::Error::last_os_error()
                ));
            }
            return Ok(());
        }
        available = unsafe { Thread32Next(snapshot.raw(), &raw mut entry) } != 0;
    }
    Err(format!(
        "Suspended {label} process primary thread was not found"
    ))
}

pub(crate) fn force_kill_and_reap_blocking(
    child: &mut Child,
    process_tree: &mut ProcessTree,
    timeout: Duration,
    label: &str,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if let Err(error) = process_tree.signal_force(label) {
        errors.push(error);
    }
    if let Err(error) = child.start_kill()
        && child.id().is_some()
    {
        errors.push(format!("Could not terminate {label} process: {error}"));
    }

    let deadline = std::time::Instant::now() + timeout;
    let mut leader_reaped = false;
    let mut tree_empty = false;
    while !leader_reaped || !tree_empty {
        if !leader_reaped {
            match child.try_wait() {
                Ok(Some(_)) => leader_reaped = true,
                Ok(None) => {}
                Err(error) => {
                    errors.push(format!("Could not reap {label} process: {error}"));
                    break;
                }
            }
        }
        if !tree_empty {
            match process_tree_is_empty(process_tree, label) {
                Ok(empty) => tree_empty = empty,
                Err(error) => {
                    errors.push(error);
                    break;
                }
            }
        }
        if leader_reaped && tree_empty {
            break;
        }
        if std::time::Instant::now() >= deadline {
            if !leader_reaped {
                errors.push(format!(
                    "{label} process did not reap within the containment deadline"
                ));
            }
            if !tree_empty {
                errors.push(format!(
                    "{label} process tree did not settle after termination"
                ));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    #[cfg(unix)]
    if tree_empty {
        process_tree.process_group.take();
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn process_tree_is_empty(process_tree: &ProcessTree, label: &str) -> Result<bool, String> {
    #[cfg(unix)]
    {
        let id = process_tree
            .process_group
            .ok_or_else(|| format!("{label} process group was not attached"))?;
        let result = unsafe { libc::kill(-id, 0) };
        if result == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(true)
        } else {
            Err(format!(
                "Could not verify {label} process group {id} settlement: {error}"
            ))
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
            QueryInformationJobObject,
        };

        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                process_tree.job.raw(),
                JobObjectBasicAccountingInformation,
                (&raw mut accounting).cast(),
                u32::try_from(std::mem::size_of_val(&accounting))
                    .expect("job accounting structure size fits u32"),
                std::ptr::null_mut(),
            )
        };
        if queried == 0 {
            Err(format!(
                "Could not verify {label} containment job: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(accounting.ActiveProcesses == 0)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (process_tree, label);
        Ok(true)
    }
}

pub(crate) async fn force_kill_and_reap(
    mut child: Child,
    mut process_tree: ProcessTree,
    timeout: Duration,
    label: &'static str,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if let Err(error) = process_tree.signal_force(label) {
        errors.push(error);
    }
    if let Err(error) = child.start_kill()
        && child.id().is_some()
    {
        errors.push(format!("Could not terminate {label} process: {error}"));
    }
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => errors.push(format!("Could not reap {label} process: {error}")),
        Err(_) => {
            errors.push(format!(
                "{label} process did not reap within the containment deadline"
            ));
            tokio::spawn(async move {
                let _ = process_tree.signal_force(label);
                let _ = child.start_kill();
                let _ = child.wait().await;
                let _ = process_tree.terminate_remaining(timeout, label).await;
            });
            return Err(errors.join("; "));
        }
    }
    if let Err(error) = process_tree.terminate_remaining(timeout, label).await {
        errors.push(error);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
