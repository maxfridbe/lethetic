//! Authenticated bridge to Android-owned credentials and background media.
use super::{Host, authorize};
use serde_json::{Value, json};
#[cfg(target_os = "android")]
use tauri::Manager;
use tauri::State;

async fn call(app: tauri::AppHandle, method: &'static str, args: Value) -> Result<Value, String> {
    #[cfg(target_os = "android")]
    return tauri::async_runtime::spawn_blocking(move || {
        app.state::<Host>()
            .mobile
            .run_mobile_plugin(method, args)
            .map_err(|_| "Android operation failed; retry or reopen settings".into())
    })
    .await
    .map_err(|_| "Android operation interrupted")?;
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, method, args);
        Err("Android media is unavailable on this platform".into())
    }
}
#[tauri::command]
pub(crate) async fn load_credentials(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
) -> Result<Value, String> {
    authorize(&state, &token)?;
    call(app, "loadCredentials", json!({})).await
}
#[tauri::command]
pub(crate) async fn save_credentials(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    key: String,
    value: Option<String>,
) -> Result<Value, String> {
    authorize(&state, &token)?;
    call(app, "saveCredentials", json!({"key":key,"value":value})).await
}
#[tauri::command]
pub(crate) async fn response_audio(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    operation: String,
    payload: Value,
) -> Result<Value, String> {
    authorize(&state, &token)?;
    let method = match operation.as_str() {
        "start" => "startResponseAudio",
        "status" => "responseAudioStatus",
        "stop" => "stopResponseAudio",
        "save" => "saveResponseAudio",
        _ => return Err("Unknown audio operation".into()),
    };
    call(app, method, payload).await
}
