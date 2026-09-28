//! Background shell tasks: commands the model starts and keeps working past.
//!
//! A process-wide registry owns every task. Each task streams its output into
//! a bounded tail and a log file under `.lethetic/background/`, detects
//! progress (a percentage in the output, a model-supplied pattern, or the
//! growth of a watched file), and records when it last showed activity so a
//! stalled task is visible. Finished tasks are pushed to their notify target:
//! the model's next request, the user interface, or nobody (poll only).

use regex::Regex;
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Output lines kept in memory per task; the log file has everything.
const TAIL_LINES: usize = 400;
/// Longest single line kept in memory.
const LINE_CHARS: usize = 2_000;
/// Log files stop growing past this size.
const LOG_BYTES: u64 = 64 * 1024 * 1024;
/// Finished tasks beyond this count are forgotten, oldest first.
const MAX_TASKS: usize = 50;
/// Grace period between SIGTERM and SIGKILL when stopping a task.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// How often a watched file's size is sampled.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// Whether the model may start background tasks, and whether their finishes
/// wake it. Set from `background_tasks` in the config and cycled from the
/// command palette.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundMode {
    /// Tasks run, and a finish starts a model turn when the agent is idle.
    #[default]
    Notify,
    /// Tasks run, but the model must poll; finishes only reach the user.
    Poll,
    /// The `background_task` tool is not offered.
    Off,
}

impl BackgroundMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Notify => "notify",
            Self::Poll => "poll only",
            Self::Off => "off",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Notify => Self::Poll,
            Self::Poll => Self::Off,
            Self::Off => Self::Notify,
        }
    }
}

static MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn mode() -> BackgroundMode {
    match MODE.load(Ordering::Relaxed) {
        1 => BackgroundMode::Poll,
        2 => BackgroundMode::Off,
        _ => BackgroundMode::Notify,
    }
}

pub fn set_mode(mode: BackgroundMode) {
    let value = match mode {
        BackgroundMode::Notify => 0,
        BackgroundMode::Poll => 1,
        BackgroundMode::Off => 2,
    };
    MODE.store(value, Ordering::Relaxed);
    let mut registry = registry();
    registry.revision = registry.revision.wrapping_add(1);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyTarget {
    /// Tell the model when the task finishes (the default).
    Model,
    /// Tell only the person watching the interface.
    User,
    /// Nobody is told; the model polls with `status` or `wait`.
    None,
}

impl NotifyTarget {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim) {
            None | Some("") | Some("model") => Ok(Self::Model),
            Some("user") => Ok(Self::User),
            Some("none") | Some("poll") => Ok(Self::None),
            Some(other) => Err(format!(
                "notify must be \"model\", \"user\" or \"none\", not {other:?}"
            )),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::User => "user",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Running,
    /// Exited on its own; `None` when killed by a signal.
    Exited(Option<i32>),
    /// Stopped on request.
    Stopped,
    /// Could not be started or its output could not be read.
    Failed(String),
}

impl TaskState {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    pub fn label(&self) -> String {
        match self {
            Self::Running => "running".to_string(),
            Self::Exited(Some(0)) => "done".to_string(),
            Self::Exited(Some(code)) => format!("failed (exit {code})"),
            Self::Exited(None) => "killed by signal".to_string(),
            Self::Stopped => "stopped".to_string(),
            Self::Failed(reason) => format!("failed: {reason}"),
        }
    }
}

/// A point-in-time copy of one task, safe to render or serialize.
#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    pub id: String,
    pub description: String,
    pub command: String,
    pub notify: NotifyTarget,
    pub state: TaskState,
    pub elapsed: Duration,
    /// Time since the last output or watched-file growth.
    pub idle: Duration,
    /// Fraction complete in `0.0..=1.0`, when known.
    pub progress: Option<f64>,
    /// Human text next to the bar, e.g. `45%` or `12.3 MB of 40 MB, 2.1 MB/s`.
    pub progress_label: Option<String>,
    pub last_line: String,
    pub output_lines: u64,
    pub log_path: Option<PathBuf>,
    pub todo_id: Option<String>,
    /// How long ago the task finished; `None` while running.
    pub finished_ago: Option<Duration>,
}

pub struct StartSpec {
    pub command: String,
    pub description: String,
    pub cwd: String,
    pub notify: NotifyTarget,
    /// Regex whose captures give progress: one group is a percentage, two
    /// groups are `done` and `total`.
    pub progress_pattern: Option<String>,
    /// A file whose growth shows progress (for example a download target).
    pub watch_path: Option<String>,
    /// Final size of `watch_path`, which turns its growth into a percentage.
    pub expected_bytes: Option<u64>,
    pub todo_id: Option<String>,
}

struct Task {
    id: String,
    description: String,
    command: String,
    notify: NotifyTarget,
    state: TaskState,
    started: Instant,
    finished: Option<Instant>,
    last_activity: Instant,
    progress: Option<f64>,
    progress_label: Option<String>,
    tail: VecDeque<String>,
    partial: String,
    output_lines: u64,
    log_path: Option<PathBuf>,
    pattern: Option<Regex>,
    watch_path: Option<PathBuf>,
    expected_bytes: Option<u64>,
    stop: CancellationToken,
    pid: Option<u32>,
    todo_id: Option<String>,
    /// The finish has been delivered to its notify target.
    delivered: bool,
    /// The user interface has shown the finish.
    announced: bool,
}

impl Task {
    fn snapshot(&self) -> TaskSnapshot {
        let now = Instant::now();
        let end = self.finished.unwrap_or(now);
        TaskSnapshot {
            id: self.id.clone(),
            description: self.description.clone(),
            command: self.command.clone(),
            notify: self.notify,
            state: self.state.clone(),
            elapsed: end.saturating_duration_since(self.started),
            idle: end.saturating_duration_since(self.last_activity),
            progress: self.progress,
            progress_label: self.progress_label.clone(),
            last_line: if self.partial.trim().is_empty() {
                self.tail.back().cloned().unwrap_or_default()
            } else {
                self.partial.clone()
            },
            output_lines: self.output_lines,
            log_path: self.log_path.clone(),
            todo_id: self.todo_id.clone(),
            finished_ago: self.finished.map(|finished| now.saturating_duration_since(finished)),
        }
    }
}

#[derive(Default)]
struct Registry {
    tasks: Vec<Task>,
    next_id: u32,
    revision: u64,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(Mutex::default);
static FINISHED: LazyLock<Notify> = LazyLock::new(Notify::new);
static PERCENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,3}(?:\.\d+)?)\s?%").expect("valid percent pattern"));

fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn with_task<T>(id: &str, apply: impl FnOnce(&mut Task) -> T) -> Option<T> {
    let mut registry = registry();
    let value = registry
        .tasks
        .iter_mut()
        .find(|task| task.id == id)
        .map(apply);
    if value.is_some() {
        registry.revision = registry.revision.wrapping_add(1);
    }
    value
}

/// Increases whenever any task changes; interfaces redraw when it moves.
pub fn revision() -> u64 {
    registry().revision
}

pub fn list() -> Vec<TaskSnapshot> {
    registry().tasks.iter().map(Task::snapshot).collect()
}

pub fn snapshot(id: &str) -> Option<TaskSnapshot> {
    registry()
        .tasks
        .iter()
        .find(|task| task.id == id)
        .map(Task::snapshot)
}

/// Running tasks plus those that finished within `window`, for status lines.
pub fn recent(window: Duration) -> Vec<TaskSnapshot> {
    list()
        .into_iter()
        .filter(|task| task.finished_ago.is_none_or(|ago| ago <= window))
        .collect()
}

pub fn running_count() -> usize {
    registry()
        .tasks
        .iter()
        .filter(|task| task.state.is_running())
        .count()
}

/// The last `lines` lines of a task's output, including an unfinished line.
pub fn output_tail(id: &str, lines: usize) -> Option<Vec<String>> {
    let registry = registry();
    let task = registry.tasks.iter().find(|task| task.id == id)?;
    let mut out: Vec<String> = task.tail.iter().cloned().collect();
    if !task.partial.trim().is_empty() {
        out.push(task.partial.clone());
    }
    let skip = out.len().saturating_sub(lines);
    Some(out.split_off(skip))
}

/// Starts `spec.command` in the background. Must run inside a Tokio runtime.
pub fn start(mut spec: StartSpec) -> Result<TaskSnapshot, String> {
    match mode() {
        BackgroundMode::Off => {
            return Err("background tasks are turned off (command palette: Background Tasks)".to_string());
        }
        BackgroundMode::Poll if spec.notify == NotifyTarget::Model => spec.notify = NotifyTarget::User,
        _ => {}
    }
    let pattern = match spec.progress_pattern.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(pattern) => {
            let regex = Regex::new(pattern).map_err(|error| format!("invalid progress_pattern: {error}"))?;
            if regex.captures_len() < 2 {
                return Err("progress_pattern needs a capture group: (percent) or (done)…(total)".to_string());
            }
            Some(regex)
        }
    };
    let watch_path = spec
        .watch_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let path = Path::new(path);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                Path::new(&spec.cwd).join(path)
            }
        });
    let mut child = crate::platform::spawn_background_shell(&spec.command, &spec.cwd)
        .map_err(|error| format!("could not start the command: {error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stop = CancellationToken::new();
    let now = Instant::now();
    let (id, snapshot) = {
        let mut registry = registry();
        registry.next_id += 1;
        let id = format!("bg{}", registry.next_id);
        let log_path = open_log(&spec.cwd, &id).map(|_| log_file(&spec.cwd, &id));
        let task = Task {
            id: id.clone(),
            description: spec.description,
            command: spec.command,
            notify: spec.notify,
            state: TaskState::Running,
            started: now,
            finished: None,
            last_activity: now,
            progress: None,
            progress_label: None,
            tail: VecDeque::new(),
            partial: String::new(),
            output_lines: 0,
            log_path,
            pattern,
            watch_path,
            expected_bytes: spec.expected_bytes.filter(|bytes| *bytes > 0),
            stop: stop.clone(),
            pid: child.id(),
            todo_id: spec.todo_id,
            delivered: false,
            announced: false,
        };
        let snapshot = task.snapshot();
        registry.tasks.push(task);
        forget_old(&mut registry);
        registry.revision = registry.revision.wrapping_add(1);
        (id, snapshot)
    };
    let log_path = snapshot.log_path.clone();
    let readers = [
        stdout.map(|out| tokio::spawn(pump(id.clone(), out, log_path.clone()))),
        stderr.map(|err| tokio::spawn(pump(id.clone(), err, log_path.clone()))),
    ];
    if snapshot_has_watch(&id) {
        tokio::spawn(watch(id.clone(), stop.clone()));
    }
    tokio::spawn(async move {
        let status = tokio::select! {
            status = child.wait() => status,
            () = stop.cancelled() => {
                if let Some(pid) = child.id() {
                    crate::platform::terminate_process_group(pid, false);
                }
                match tokio::time::timeout(STOP_GRACE, child.wait()).await {
                    Ok(status) => status,
                    Err(_) => {
                        if let Some(pid) = child.id() {
                            crate::platform::terminate_process_group(pid, true);
                        }
                        let _ = child.kill().await;
                        child.wait().await
                    }
                }
            }
        };
        for reader in readers.into_iter().flatten() {
            let _ = tokio::time::timeout(Duration::from_secs(2), reader).await;
        }
        let stopped = stop.is_cancelled();
        stop.cancel();
        with_task(&id, |task| {
            if !task.partial.is_empty() {
                flush_partial(task);
            }
            task.finished = Some(Instant::now());
            task.pid = None;
            task.state = match status {
                _ if stopped => TaskState::Stopped,
                Ok(status) => TaskState::Exited(status.code()),
                Err(error) => TaskState::Failed(error.to_string()),
            };
            if matches!(task.state, TaskState::Exited(Some(0))) && task.pattern.is_none() && task.watch_path.is_none() {
                task.progress = Some(1.0);
            }
            if task.notify == NotifyTarget::None {
                task.delivered = true;
            }
        });
        FINISHED.notify_waiters();
    });
    Ok(snapshot)
}

fn snapshot_has_watch(id: &str) -> bool {
    registry()
        .tasks
        .iter()
        .any(|task| task.id == id && task.watch_path.is_some())
}

fn forget_old(registry: &mut Registry) {
    while registry.tasks.len() > MAX_TASKS {
        let Some(index) = registry
            .tasks
            .iter()
            .position(|task| !task.state.is_running() && task.delivered)
        else {
            break;
        };
        registry.tasks.remove(index);
    }
}

fn log_file(cwd: &str, id: &str) -> PathBuf {
    Path::new(cwd)
        .join(".lethetic")
        .join("background")
        .join(format!("{id}-{}.log", std::process::id()))
}

fn open_log(cwd: &str, id: &str) -> Option<std::fs::File> {
    let path = log_file(cwd, id);
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// Reads one output stream, splitting on `\n` and treating `\r` as "replace
/// the current line", which is how progress bars redraw themselves.
async fn pump<R: tokio::io::AsyncRead + Unpin>(id: String, mut reader: R, log_path: Option<PathBuf>) {
    let mut log = log_path.and_then(|path| {
        std::fs::OpenOptions::new().append(true).open(path).ok()
    });
    let mut logged = 0_u64;
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if let Some(file) = log.as_mut()
            && logged < LOG_BYTES
        {
            let _ = file.write_all(&buffer[..read]);
            logged += read as u64;
        }
        pending.extend_from_slice(&buffer[..read]);
        // Keep an incomplete UTF-8 sequence for the next read.
        let valid = match std::str::from_utf8(&pending) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => pending.len(),
        };
        let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
        pending.drain(..valid);
        with_task(&id, |task| absorb(task, &text));
    }
    if !pending.is_empty() {
        let text = String::from_utf8_lossy(&pending).into_owned();
        with_task(&id, |task| absorb(task, &text));
    }
}

fn absorb(task: &mut Task, text: &str) {
    task.last_activity = Instant::now();
    for character in text.chars() {
        match character {
            '\n' => flush_partial(task),
            '\r' => {
                if !task.partial.trim().is_empty() {
                    detect_progress(task, &task.partial.clone());
                }
                task.partial.clear();
            }
            _ if task.partial.chars().count() < LINE_CHARS => task.partial.push(character),
            _ => {}
        }
    }
    if !task.partial.trim().is_empty() {
        detect_progress(task, &task.partial.clone());
    }
}

fn flush_partial(task: &mut Task) {
    let line = std::mem::take(&mut task.partial);
    if line.is_empty() && task.tail.back().is_some_and(String::is_empty) {
        return;
    }
    detect_progress(task, &line);
    task.tail.push_back(line);
    task.output_lines += 1;
    while task.tail.len() > TAIL_LINES {
        task.tail.pop_front();
    }
}

fn detect_progress(task: &mut Task, line: &str) {
    if task.watch_path.is_some() && task.expected_bytes.is_some() {
        return;
    }
    if let Some(pattern) = &task.pattern {
        let Some(captures) = pattern.captures(line) else {
            return;
        };
        let number = |index: usize| {
            captures
                .get(index)
                .and_then(|value| value.as_str().replace(',', "").parse::<f64>().ok())
        };
        if let (Some(done), Some(total)) = (number(1), number(2))
            && total > 0.0
        {
            task.progress = Some((done / total).clamp(0.0, 1.0));
            task.progress_label = Some(format!("{done} of {total}"));
        } else if let Some(percent) = number(1) {
            task.progress = Some((percent / 100.0).clamp(0.0, 1.0));
            task.progress_label = Some(format!("{percent:.0}%"));
        }
        return;
    }
    if let Some(percent) = PERCENT
        .captures_iter(line)
        .filter_map(|captures| captures[1].parse::<f64>().ok())
        .filter(|percent| *percent <= 100.0)
        .last()
    {
        task.progress = Some(percent / 100.0);
        task.progress_label = Some(format!("{percent:.0}%"));
    }
}

/// Samples a watched file's size so silent downloads still show progress.
async fn watch(id: String, stop: CancellationToken) {
    let Some((path, expected)) = with_task(&id, |task| (task.watch_path.clone(), task.expected_bytes))
        .and_then(|(path, expected)| path.map(|path| (path, expected)))
    else {
        return;
    };
    let mut previous: Option<(u64, Instant)> = None;
    let mut rate = 0.0_f64;
    loop {
        let size = tokio::fs::metadata(&path).await.map(|meta| meta.len()).ok();
        let now = Instant::now();
        let running = with_task(&id, |task| {
            if let Some(size) = size {
                if let Some((before, at)) = previous {
                    let seconds = now.duration_since(at).as_secs_f64();
                    if size > before {
                        task.last_activity = now;
                    }
                    if seconds > 0.0 {
                        let sample = size.saturating_sub(before) as f64 / seconds;
                        rate = if rate == 0.0 { sample } else { rate * 0.7 + sample * 0.3 };
                    }
                }
                let mut label = format_bytes(size);
                if let Some(expected) = expected {
                    task.progress = Some((size as f64 / expected as f64).clamp(0.0, 1.0));
                    label = format!("{label} of {}", format_bytes(expected));
                }
                if rate >= 1.0 && task.state.is_running() {
                    label = format!("{label}, {}/s", format_bytes(rate as u64));
                }
                task.progress_label = Some(label);
            }
            task.state.is_running()
        })
        .unwrap_or(false);
        if let Some(size) = size {
            previous = Some((size, now));
        }
        if !running {
            return;
        }
        tokio::select! {
            () = stop.cancelled() => {}
            () = tokio::time::sleep(WATCH_INTERVAL) => {}
        }
    }
}

/// Asks a running task to stop. Returns false when it is unknown or finished.
pub fn stop(id: &str) -> bool {
    let token = registry()
        .tasks
        .iter()
        .find(|task| task.id == id && task.state.is_running())
        .map(|task| task.stop.clone());
    token.map(|token| token.cancel()).is_some()
}

/// Stops every running task; used when Lethetic exits.
pub fn stop_all() {
    for task in registry().tasks.iter().filter(|task| task.state.is_running()) {
        task.stop.cancel();
        if let Some(pid) = task.pid {
            crate::platform::terminate_process_group(pid, false);
        }
    }
}

/// Waits until the task finishes, `timeout` passes, or `cancel` fires, then
/// returns its snapshot. A waited-for finish counts as delivered.
pub async fn wait(id: &str, timeout: Duration, cancel: &CancellationToken) -> Option<TaskSnapshot> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let finished = FINISHED.notified();
        let snapshot = snapshot(id)?;
        if !snapshot.state.is_running() {
            mark_delivered(id);
            return Some(snapshot);
        }
        tokio::select! {
            () = finished => {}
            () = cancel.cancelled() => return Some(snapshot),
            () = tokio::time::sleep_until(deadline) => return Some(snapshot),
        }
    }
}

/// Records that the model has seen this task's finish.
pub fn mark_delivered(id: &str) {
    with_task(id, |task| task.delivered = true);
}

/// Finished tasks the model has not been told about yet. Taking them marks
/// them delivered.
pub fn take_model_notifications() -> Vec<TaskSnapshot> {
    if mode() != BackgroundMode::Notify {
        return Vec::new();
    }
    let mut registry = registry();
    let mut out = Vec::new();
    for task in registry.tasks.iter_mut() {
        if !task.state.is_running() && !task.delivered && task.notify == NotifyTarget::Model {
            task.delivered = true;
            out.push(task.snapshot());
        }
    }
    if !out.is_empty() {
        registry.revision = registry.revision.wrapping_add(1);
    }
    out
}

/// True when a finished task is waiting to be pushed to the model.
pub fn has_model_notifications() -> bool {
    registry()
        .tasks
        .iter()
        .any(|task| !task.state.is_running() && !task.delivered && task.notify == NotifyTarget::Model)
}

/// Finished `notify=user` tasks the interface has not announced yet. Taking
/// them marks them announced and delivered.
pub fn take_user_announcements() -> Vec<TaskSnapshot> {
    let mut registry = registry();
    let mut out = Vec::new();
    for task in registry.tasks.iter_mut() {
        if !task.state.is_running() && !task.announced && task.notify == NotifyTarget::User {
            task.announced = true;
            task.delivered = true;
            out.push(task.snapshot());
        }
    }
    out
}

/// One line per task for the model's context, or `None` with no tasks.
pub fn context_summary() -> Option<String> {
    let tasks = list();
    let unreviewed: Vec<String> = {
        let registry = registry();
        registry
            .tasks
            .iter()
            .filter(|task| {
                !task.state.is_running() && !task.delivered && task.notify == NotifyTarget::Model
            })
            .map(|task| task.id.clone())
            .collect()
    };
    let relevant: Vec<&TaskSnapshot> = tasks
        .iter()
        .filter(|task| task.state.is_running() || unreviewed.contains(&task.id))
        .collect();
    if relevant.is_empty() {
        return None;
    }
    let mut out = String::from("<background_tasks>\n");
    for task in relevant {
        out.push_str(&format!("- {}\n", status_line(task)));
    }
    out.push_str("Use background_task status, wait or output for details.\n</background_tasks>");
    Some(out)
}

/// Every finish notice starts with this, so it stays out of input history.
pub const NOTICE_PREFIX: &str = "[Background task ";

/// The notice the model receives when a task it started has finished.
pub fn finish_notice(tasks: &[TaskSnapshot]) -> String {
    let mut out = String::new();
    for task in tasks {
        out.push_str(&format!(
            "{NOTICE_PREFIX}{} finished: {} after {}. {}]\n",
            task.id,
            task.state.label(),
            format_duration(task.elapsed),
            task.description
        ));
        if !task.last_line.trim().is_empty() {
            out.push_str(&format!("Last output: {}\n", task.last_line.trim()));
        }
        if let Some(path) = &task.log_path {
            out.push_str(&format!("Full log: {}\n", path.display()));
        }
    }
    out
}

/// `bg1 running 2m 10s 45% (12 MB of 27 MB) — Download model · last output 3s ago`.
pub fn status_line(task: &TaskSnapshot) -> String {
    let mut line = format!(
        "{} {} {}",
        task.id,
        task.state.label(),
        format_duration(task.elapsed)
    );
    if let Some(label) = &task.progress_label {
        line.push_str(&format!(" {label}"));
    }
    line.push_str(&format!(" — {}", task.description));
    if task.state.is_running() {
        line.push_str(&format!(" · last activity {} ago", format_duration(task.idle)));
    }
    line
}

/// A text progress bar such as `[██████░░░░]`, or a bouncing marker when the
/// fraction is unknown.
pub fn progress_bar(progress: Option<f64>, width: usize, elapsed: Duration) -> String {
    let width = width.max(3);
    match progress {
        Some(fraction) => {
            let filled = ((fraction.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
            format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
        }
        None => {
            let span = (width - 1) * 2;
            let step = (elapsed.as_millis() / 150) as usize % span.max(1);
            let position = if step < width { step } else { span - step };
            (0..width)
                .map(|index| if index == position { '█' } else { '░' })
                .collect()
        }
    }
}

pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests;
