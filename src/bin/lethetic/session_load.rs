use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use lethetic::app::{App, BlockType, SessionState};
use lethetic::client::StreamEvent;
use lethetic::config::Config;
use lethetic::icons;

use crate::app_events::prepare_python_policy_for_session_transition;

/// Starts the existing checked background loader. `false` means the request was
/// rejected synchronously and the caller should preserve its run-loop boundary.
pub(crate) fn begin(
    app: &mut App,
    session_id: String,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancellation: CancellationToken,
) -> bool {
    if cancellation.is_cancelled() {
        return false;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (session_id, tx);
        app.stop_reason = "✗ Durable session resume is disabled because secure session locking is available only on Linux".to_string();
        app.should_redraw = true;
        false
    }

    #[cfg(target_os = "linux")]
    {
        if !app.is_fully_idle() {
            app.stop_reason = "✗ Wait for active work before resuming a session".to_string();
            app.should_redraw = true;
            return false;
        }
        if app.current_session_dir.is_some() && app.session_id == session_id {
            app.show_session_manager = false;
            app.stop_reason = format!("Session {session_id} is already active");
            app.should_redraw = true;
            return false;
        }
        app.refresh_session_list();
        let filename = match app.session_path_for_id(&session_id) {
            Ok(path) => path,
            Err(error) => {
                app.stop_reason = format!("✗ Cannot resolve target session: {error}");
                app.should_redraw = true;
                return false;
            }
        };
        if let Err(error) = app.save_session_checked() {
            app.stop_reason = format!("✗ Cannot switch sessions: {error}");
            app.should_redraw = true;
            return false;
        }
        let session_path_lock = match app.acquire_session_path_lock(&filename) {
            Ok(lock) => lock,
            Err(error) => {
                app.stop_reason = format!("✗ Cannot lock target session: {error}");
                app.should_redraw = true;
                return false;
            }
        };

        app.is_loading_session = true;
        app.load_progress = 0.0;
        app.load_status = "Starting to load session...".to_string();
        app.should_redraw = true;

        let tx = tx.clone();
        let filename_for_task = filename.clone();
        let expected_session_id = session_id;
        let theme = app.theme.clone();
        let terminal_width = app.last_rendered_width;
        let include_estimated_cost = app.config.estimate_cost.unwrap_or(true);
        tokio::spawn(async move {
            if cancellation.is_cancelled() {
                let _ = tx.send(StreamEvent::SessionLoadFailed(
                    "Session loading was cancelled".to_string(),
                ));
                return;
            }
            let _ = tx.send(StreamEvent::LoadProgress(
                10.0,
                "Reading session state...".to_string(),
            ));

            let load_dir = filename_for_task.clone();
            let requested_session_id = expected_session_id;
            let loaded = tokio::task::spawn_blocking(move || {
                let mut state = SessionState::load_checked(&load_dir)?;
                if state.session_id.as_deref() != Some(requested_session_id.as_str()) {
                    return Err(
                        "selected session state contains a different session ID".to_string()
                    );
                }
                let session_id = state
                    .session_id
                    .as_deref()
                    .ok_or_else(|| "checked session state has no session ID".to_string())?;
                let lease = session_path_lock.bind_identity(session_id)?;
                match &state.session_directory_binding {
                    Some(binding) if binding != lease.binding() => {
                        return Err(
                            "session state directory binding does not match its identity registry"
                                .to_string(),
                        );
                    }
                    Some(_) => {}
                    None => {
                        state.session_directory_binding = Some(lease.binding().clone());
                        state.needs_migration_save = true;
                    }
                }
                lease.verify(
                    std::path::Path::new(&load_dir),
                    session_id,
                    state
                        .session_directory_binding
                        .as_ref()
                        .expect("binding was established above"),
                )?;
                if terminal_width > 0 {
                    for block in &mut state.blocks {
                        let rendered = lethetic::ui::render_block_to_lines_with_cost_visibility(
                            block,
                            terminal_width,
                            &theme,
                            None,
                            include_estimated_cost,
                        );
                        block.cached_line_count = Some(rendered.len());
                        block.cached_lines = Some(rendered);
                    }
                }
                Ok::<_, String>((state, lease))
            })
            .await;

            if cancellation.is_cancelled() {
                let _ = tx.send(StreamEvent::SessionLoadFailed(
                    "Session loading was cancelled".to_string(),
                ));
                return;
            }
            let (state, lease) = match loaded {
                Ok(Ok(loaded)) => loaded,
                Ok(Err(error)) => {
                    let _ = tx.send(StreamEvent::SessionLoadFailed(error));
                    return;
                }
                Err(error) => {
                    let _ = tx.send(StreamEvent::SessionLoadFailed(format!(
                        "session loader failed: {error}"
                    )));
                    return;
                }
            };
            if state.blocks.is_empty() && state.messages.is_empty() {
                let _ = tx.send(StreamEvent::DebugLog(
                    "Loaded session has no conversation content".to_string(),
                ));
            }
            let _ = tx.send(StreamEvent::LoadProgress(100.0, "Finishing...".to_string()));
            let _ = tx.send(StreamEvent::SessionLoaded {
                dir: filename_for_task,
                state,
                lease: std::sync::Arc::new(lease),
            });
        });
        true
    }
}

pub(crate) fn progress(app: &mut App, percentage: f32, status: String) {
    app.load_progress = percentage;
    app.load_status = status;
    app.should_redraw = true;
}

pub(crate) fn failed(app: &mut App, error: String) {
    app.is_loading_session = false;
    app.load_progress = 0.0;
    app.load_status.clear();
    app.stop_reason = format!("✗ Session load failed: {error}");
    app.add_segment(
        format!("\n{} SESSION LOAD ERROR: {error}\n", icons::WARNING),
        BlockType::Text,
    );
    app.should_redraw = true;
}

pub(crate) fn discard_loaded_for_shutdown(app: &mut App) {
    app.is_loading_session = false;
    app.load_progress = 0.0;
    app.load_status.clear();
    app.should_redraw = true;
}

fn finish_loaded_view_state(app: &mut App) {
    app.reset_session_view_state();
    app.is_loading_session = false;
}

pub(crate) async fn apply_loaded(
    app: &mut App,
    config: &mut Config,
    dir: String,
    state: SessionState,
    cancellation: CancellationToken,
    #[cfg(target_os = "linux")] lease: std::sync::Arc<lethetic::session_store::SessionLease>,
) {
    if cancellation.is_cancelled() {
        discard_loaded_for_shutdown(app);
        return;
    }
    let detach_result = prepare_python_policy_for_session_transition(app, config).await;
    if cancellation.is_cancelled() {
        discard_loaded_for_shutdown(app);
        return;
    }
    #[cfg(target_os = "linux")]
    let identity_result =
        detach_result.and_then(|()| app.install_loaded_session_identity(&dir, &state, lease));
    #[cfg(not(target_os = "linux"))]
    let identity_result: Result<(), String> = detach_result;

    if let Err(error) = &identity_result {
        app.is_loading_session = false;
        app.load_progress = 0.0;
        app.load_status.clear();
        app.stop_reason = format!("✗ Session identity validation failed: {error}");
        app.should_redraw = true;
        return;
    }

    let SessionState {
        session_id,
        display_name,
        session_directory_binding,
        python_runtime_id,
        managed_python_workspace,
        shared_python_workspace,
        messages,
        blocks,
        history,
        theme_name,
        accounting,
        needs_migration_save,
        ..
    } = state;
    app.current_session_dir = Some(dir);
    app.session_id = session_id.expect("checked session state always has a session ID");
    app.display_name = display_name;
    app.session_directory_binding = session_directory_binding;
    app.python_runtime_id = python_runtime_id;
    app.managed_python_workspace = managed_python_workspace;
    app.shared_python_workspace = shared_python_workspace;
    app.accounting = accounting;
    app.blocks = blocks;
    app.logical_turn_usage = app
        .blocks
        .iter()
        .rev()
        .find(|block| block.block_type == BlockType::User)
        .and_then(|block| block.usage);
    app.history = history;

    if !theme_name.is_empty()
        && theme_name != app.theme.name
        && let Some(index) = app.themes.iter().position(|theme| theme.name == theme_name)
    {
        app.theme = app.themes[index].clone();
        app.theme_state.select(Some(index));
        for block in &mut app.blocks {
            block.invalidate();
        }
    }
    app.context_manager.clear();
    app.context_manager.set_messages(messages);
    finish_loaded_view_state(app);
    app.needs_save = needs_migration_save;

    let migration_result = if needs_migration_save {
        app.save_session_checked()
    } else {
        Ok(())
    };
    #[cfg(target_os = "linux")]
    let nonlocal_result = match migration_result {
        Ok(()) if cancellation.is_cancelled() => Err("session loading was cancelled".to_string()),
        Ok(()) => {
            let restore_result = if app.managed_python_workspace.is_some() {
                app.restore_managed_python_session_with_cancellation(cancellation.child_token())
                    .await
            } else {
                Ok(())
            };
            match restore_result {
                Ok(()) => {
                    app.ensure_nonlocal_python_session_with_cancellation(cancellation.child_token())
                        .await
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    };
    #[cfg(not(target_os = "linux"))]
    let nonlocal_result = migration_result;
    if cancellation.is_cancelled() {
        app.should_redraw = true;
        return;
    }
    if let Err(error) = nonlocal_result {
        app.stop_reason = format!("✗ Session Python binding failed: {error}");
        app.needs_save = true;
    }
    app.should_redraw = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_loader_stops_before_session_resolution() {
        let mut app = App::new(&Config::default());
        let original_reason = app.stop_reason.clone();
        let (tx, _rx) = mpsc::unbounded_channel();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert!(!begin(
            &mut app,
            "not-a-session-id".to_string(),
            &tx,
            cancellation,
        ));
        assert!(!app.is_loading_session);
        assert_eq!(app.stop_reason, original_reason);
    }

    #[test]
    fn shutdown_discard_clears_loader_without_replacing_active_session() {
        let mut app = App::new(&Config::default());
        let original_session = app.session_id.clone();
        app.is_loading_session = true;
        app.load_progress = 75.0;
        app.load_status = "Loading target".to_string();
        app.stop_reason = "Termination requested; shutting down safely…".to_string();

        discard_loaded_for_shutdown(&mut app);

        assert!(!app.is_loading_session);
        assert_eq!(app.load_progress, 0.0);
        assert!(app.load_status.is_empty());
        assert_eq!(app.session_id, original_session);
        assert_eq!(
            app.stop_reason,
            "Termination requested; shutting down safely…"
        );
    }

    #[test]
    fn failed_load_clears_loading_state_once() {
        let mut app = App::new(&Config::default());
        app.is_loading_session = true;
        app.load_progress = 42.0;
        failed(&mut app, "checked failure".to_string());
        assert!(!app.is_loading_session);
        assert_eq!(app.load_progress, 0.0);
        assert!(app.stop_reason.contains("checked failure"));
    }

    #[test]
    fn loaded_view_drops_previous_request_metrics_and_scroll() {
        let mut app = App::new(&Config::default());
        app.blocks[0].cached_line_count = Some(6);
        app.server_prompt_tokens = Some(10);
        app.server_completion_tokens = Some(20);
        app.server_usage = Some(lethetic::accounting::Usage::default());
        app.scroll = 40;
        app.auto_scroll = false;
        app.total_line_count = 400;
        app.output_state.select(Some(12));
        app.is_loading_session = true;

        finish_loaded_view_state(&mut app);

        assert_eq!(app.server_prompt_tokens, None);
        assert_eq!(app.server_completion_tokens, None);
        assert_eq!(app.server_usage, None);
        assert_eq!(app.scroll, 0);
        assert!(app.auto_scroll);
        assert_eq!(app.total_line_count, 6);
        assert_eq!(app.output_state.selected(), Some(5));
        assert!(!app.is_loading_session);
    }
}
