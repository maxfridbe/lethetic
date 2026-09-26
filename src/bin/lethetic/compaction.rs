//! Session compaction: summarise a stored session with any configured model
//! and write the summary out as a new resumable session.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lethetic::app::{App, CompactionPopupState};
use lethetic::client::StreamEvent;

use lethetic::config::Config;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Start compacting `session_id` with `model_id` on `connection_id`. Progress
/// streams into the compaction popup; the run loop finishes the job when the
/// terminal `CompactionFinished` event arrives.
#[allow(clippy::too_many_arguments)]
pub(crate) fn begin(
    app: &mut App,
    config: &Config,
    client: &reqwest::Client,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    background_cancellation: &CancellationToken,
    session_id: String,
    connection_id: String,
    model_id: String,
) {
    app.show_session_manager = false;
    app.show_model_switcher = false;
    if app.compaction_popup.is_some() {
        app.stop_reason = "⚠ A compaction is already running".to_string();
        app.should_redraw = true;
        return;
    }
    app.refresh_session_list();
    let source_path = match app.session_path_for_id(&session_id) {
        Ok(path) => path,
        Err(error) => {
            app.stop_reason = format!("✗ Cannot compact: {error}");
            app.should_redraw = true;
            return;
        }
    };
    let log_text = match App::compaction_source_text(&source_path) {
        Ok(text) => text,
        Err(error) => {
            app.stop_reason = format!("✗ Cannot compact: {error}");
            app.should_redraw = true;
            return;
        }
    };
    let mut compact_config = config.clone();
    let activation = if connection_id == "__active" {
        compact_config.activate_current_model(&model_id)
    } else {
        compact_config.activate_model(&connection_id, &model_id)
    };
    let compact_config = lethetic::client::compaction_config(&compact_config);
    if let Err(error) = activation.and_then(|()| compact_config.validate()) {
        app.stop_reason = format!("✗ Cannot compact with {model_id}: {error}");
        app.should_redraw = true;
        return;
    }

    let cancel = background_cancellation.child_token();
    app.compaction_popup = Some(CompactionPopupState::new(&model_id, cancel.clone()));
    app.stop_reason = format!("Compacting session with {model_id}…");
    app.should_redraw = true;

    let client = client.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = lethetic::client::compact_llm_streaming(
            &client,
            &compact_config,
            &log_text,
            &tx,
            &cancel,
        )
        .await;
        if cancel.is_cancelled() {
            return;
        }
        let _ = tx.send(StreamEvent::CompactionFinished {
            source_session_id: session_id,
            result,
        });
    });
}

pub(crate) fn append_chunk(app: &mut App, text: &str) {
    if let Some(popup) = app.compaction_popup.as_mut() {
        popup.content.push_str(text);
        popup.scroll = usize::MAX;
        app.should_redraw = true;
    }
}

pub(crate) fn finish(app: &mut App, source_session_id: &str, result: Result<String, String>) {
    let Some(mut popup) = app.compaction_popup.take() else {
        return;
    };
    if popup.cancel.is_cancelled() {
        app.should_redraw = true;
        return;
    }
    match result.and_then(|summary| app.create_compacted_session(source_session_id, summary)) {
        Ok(new_session_id) => {
            popup.content.push_str(&format!(
                "\n\n✓ Compacted session saved: {new_session_id}\n"
            ));
            popup.new_session_id = Some(new_session_id.clone());
            app.stop_reason = format!("Compacted session {new_session_id} ready to resume");
        }
        Err(error) => {
            popup
                .content
                .push_str(&format!("\n\n✗ Compaction failed: {error}\n"));
            app.stop_reason = format!("✗ Compaction failed: {error}");
        }
    }
    popup.done = true;
    popup.scroll = usize::MAX;
    app.compaction_popup = Some(popup);
    app.refresh_session_list();
    app.should_redraw = true;
}

/// Keys while the compaction popup is open. Returns `true` when consumed.
pub(crate) fn handle_popup_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(popup) = app.compaction_popup.as_mut() else {
        return false;
    };
    match key.code {
        KeyCode::Esc | KeyCode::Enter => {
            if popup.done {
                app.compaction_popup = None;
                app.refresh_session_list();
                app.show_session_manager = true;
            }
        }
        KeyCode::Up => popup.scroll_up(1),
        KeyCode::Down => popup.scroll_down(1),
        KeyCode::PageUp => popup.scroll_up(20),
        KeyCode::PageDown => popup.scroll_down(20),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if popup.done {
                app.compaction_popup = None;
            } else {
                popup.cancel.cancel();
                app.compaction_popup = None;
                app.stop_reason = "Compaction cancelled".to_string();
            }
        }
        _ => {}
    }
    app.should_redraw = true;
    true
}

/// A left click on the copy badge of a block header pipes that block's content
/// to `wl-copy`.
pub(crate) fn try_copy_block_at_click(app: &App, col: u16, row: u16) {
    let rect = app.last_output_rect;
    if row < rect.top() + 1
        || row >= rect.bottom().saturating_sub(1)
        || col < rect.left() + 1
        || col >= rect.right().saturating_sub(1)
    {
        return;
    }
    let inner_right = rect.right().saturating_sub(1);
    if col < inner_right.saturating_sub(4) {
        return;
    }
    let inner_top = rect.top() + 1;
    let abs_line = app.last_start_line + (row - inner_top) as usize;
    let Some(index) = lethetic::ui::block_header_at_line(&app.last_block_line_counts, abs_line)
    else {
        return;
    };
    if let Some(block) = app.blocks.get(index) {
        let content = block.content.clone();
        tokio::spawn(async move {
            let _ = tokio::process::Command::new("wl-copy")
                .arg(content)
                .status()
                .await;
        });
    }
}
