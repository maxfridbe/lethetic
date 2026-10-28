use super::{Host, authorize};
use lethetic_network::speech::{SpeechEvent, SpeechRequest};
use tauri::State;

#[tauri::command]
pub(crate) async fn begin_speech(
    state: State<'_, Host>,
    token: String,
    id: String,
    request: SpeechRequest,
) -> Result<(), String> {
    authorize(&state, &token)?;
    state.speech.start(id, request)
}
#[tauri::command]
pub(crate) async fn next_speech(
    state: State<'_, Host>,
    token: String,
    id: String,
) -> Result<SpeechEvent, String> {
    authorize(&state, &token)?;
    state.speech.next(&id).await
}
#[tauri::command]
pub(crate) fn cancel_speech(
    state: State<'_, Host>,
    token: String,
    id: Option<String>,
) -> Result<(), String> {
    authorize(&state, &token)?;
    state.speech.cancel(id.as_deref())
}
