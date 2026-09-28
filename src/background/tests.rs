use super::*;

fn spec(command: &str, cwd: &std::path::Path) -> StartSpec {
    StartSpec {
        command: command.to_string(),
        description: "test task".to_string(),
        cwd: cwd.to_string_lossy().into_owned(),
        notify: NotifyTarget::Model,
        progress_pattern: None,
        watch_path: None,
        expected_bytes: None,
        todo_id: None,
    }
}

async fn finish(id: &str) -> TaskSnapshot {
    wait(id, Duration::from_secs(20), &CancellationToken::new())
        .await
        .expect("task exists")
}

#[tokio::test]
async fn task_runs_in_background_and_reports_output_and_exit() {
    let directory = tempfile::tempdir().unwrap();
    let started = start(spec("echo one; echo two; exit 3", directory.path())).unwrap();
    assert!(started.id.starts_with("bg"));
    let done = finish(&started.id).await;
    assert_eq!(done.state, TaskState::Exited(Some(3)));
    assert_eq!(output_tail(&started.id, 10).unwrap(), vec!["one", "two"]);
    assert_eq!(done.last_line, "two");
    let log = std::fs::read_to_string(done.log_path.unwrap()).unwrap();
    assert_eq!(log, "one\ntwo\n");
}

#[tokio::test]
async fn carriage_return_progress_is_detected_and_replaces_the_line() {
    let directory = tempfile::tempdir().unwrap();
    let started = start(spec(
        r"printf 'loading 10%%\rloading 55%%\rloading 80%%\n'; sleep 0.2",
        directory.path(),
    ))
    .unwrap();
    let done = finish(&started.id).await;
    assert_eq!(done.state, TaskState::Exited(Some(0)));
    assert_eq!(output_tail(&started.id, 10).unwrap(), vec!["loading 80%"]);
    // A clean exit without a pattern or watched file completes the bar.
    assert_eq!(done.progress, Some(1.0));
}

#[tokio::test]
async fn custom_pattern_reads_done_and_total() {
    let directory = tempfile::tempdir().unwrap();
    let mut task = spec("echo 'step 3/12'; sleep 5", directory.path());
    task.progress_pattern = Some(r"step (\d+)/(\d+)".to_string());
    let started = start(task).unwrap();
    for _ in 0..100 {
        if snapshot(&started.id).unwrap().progress.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let running = snapshot(&started.id).unwrap();
    assert!(running.state.is_running());
    assert_eq!(running.progress, Some(0.25));
    assert!(stop(&started.id));
    let stopped = finish(&started.id).await;
    assert_eq!(stopped.state, TaskState::Stopped);
    assert!(!stop(&started.id), "a finished task cannot be stopped again");
}

#[tokio::test]
async fn watched_file_growth_gives_progress_for_silent_commands() {
    let directory = tempfile::tempdir().unwrap();
    let mut task = spec(
        "head -c 1000 /dev/zero > out.bin; sleep 1.2",
        directory.path(),
    );
    task.watch_path = Some("out.bin".to_string());
    task.expected_bytes = Some(2000);
    task.notify = NotifyTarget::None;
    let started = start(task).unwrap();
    let done = finish(&started.id).await;
    assert_eq!(done.progress, Some(0.5));
    assert!(done.progress_label.unwrap().starts_with("1000 B of 2.0 KB"));
}

#[tokio::test]
async fn model_notifications_are_taken_once_and_poll_only_tasks_are_skipped() {
    let directory = tempfile::tempdir().unwrap();
    let pushed = start(spec("true", directory.path())).unwrap();
    let mut quiet = spec("true", directory.path());
    quiet.notify = NotifyTarget::None;
    let quiet = start(quiet).unwrap();
    for _ in 0..200 {
        let both_done = [&pushed.id, &quiet.id]
            .iter()
            .all(|id| !snapshot(id).unwrap().state.is_running());
        if both_done {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let taken: Vec<String> = take_model_notifications().into_iter().map(|t| t.id).collect();
    assert!(taken.contains(&pushed.id));
    assert!(!taken.contains(&quiet.id));
    assert!(!take_model_notifications().iter().any(|task| task.id == pushed.id));
    let notice = finish_notice(&[snapshot(&pushed.id).unwrap()]);
    assert!(notice.contains(&format!("Background task {} finished: done", pushed.id)));
}

#[test]
fn bars_and_labels_render() {
    assert_eq!(progress_bar(Some(0.5), 10, Duration::ZERO), "█████░░░░░");
    assert_eq!(progress_bar(None, 5, Duration::ZERO).chars().filter(|c| *c == '█').count(), 1);
    assert_eq!(format_duration(Duration::from_secs(135)), "2m 15s");
    assert_eq!(format_bytes(1536), "1.5 KB");
    assert!(NotifyTarget::parse(Some("bogus")).is_err());
    assert_eq!(NotifyTarget::parse(None).unwrap(), NotifyTarget::Model);
}

#[test]
fn background_mode_cycles_and_reads_from_config() {
    assert_eq!(BackgroundMode::default(), BackgroundMode::Notify);
    assert_eq!(BackgroundMode::Notify.next(), BackgroundMode::Poll);
    assert_eq!(BackgroundMode::Poll.next(), BackgroundMode::Off);
    assert_eq!(BackgroundMode::Off.next(), BackgroundMode::Notify);
    let parsed: BackgroundMode = serde_yaml::from_str("poll").unwrap();
    assert_eq!(parsed, BackgroundMode::Poll);
}
