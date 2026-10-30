use super::{Host, authorize};
use tauri::{Manager, State};

pub(crate) fn engine() -> lethetic_dictation::Dictation {
    #[cfg(target_os = "android")]
    {
        static ENGINE: std::sync::OnceLock<lethetic_dictation::Dictation> =
            std::sync::OnceLock::new();
        ENGINE
            .get_or_init(|| {
                let engine = lethetic_dictation::Dictation::default();
                engine.set_foreground(false);
                engine
            })
            .clone()
    }
    #[cfg(not(target_os = "android"))]
    {
        lethetic_dictation::Dictation::default()
    }
}

// Called directly by the Activity, so backgrounding cancels even with a paused WebView.
// JNI's environment/object pointers are opaque and never dereferenced here.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_maxfridbe_lethetic_host_HostPlugin_setDictationForegroundNative(
    _env: *mut std::ffi::c_void,
    _object: *mut std::ffi::c_void,
    active: u8,
) {
    engine().set_foreground(active != 0);
}

#[cfg(target_os = "android")]
fn microphone_permission(state: &Host, request: bool) -> Result<(), String> {
    let response = state
        .mobile
        .run_mobile_plugin::<serde_json::Value>(
            if request {
                "requestMicrophone"
            } else {
                "checkMicrophone"
            },
            serde_json::json!({}),
        )
        .map_err(|e| e.to_string())?;
    if response.get("granted").and_then(serde_json::Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err("Microphone permission is off. Open Speech to text and choose Download / load model to grant access, or enable Microphone in Android app settings.".into())
    }
}

#[tauri::command]
pub(crate) async fn prepare_dictation(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    model: String,
    download: bool,
) -> Result<(), String> {
    authorize(&state, &token)?;
    // Cached model warmup must never prompt. Permission is requested only by explicit setup.
    #[cfg(target_os = "android")]
    if download {
        microphone_permission(&state, true)?;
    }
    state
        .dictation
        .prepare(
            app.path()
                .app_local_data_dir()
                .map_err(|e| e.to_string())?
                .join("whisper"),
            model,
            download,
        )
        .await
}
#[tauri::command]
pub(crate) fn dictation_status(
    state: State<'_, Host>,
    token: String,
) -> Result<serde_json::Value, String> {
    authorize(&state, &token)?;
    serde_json::to_value(state.dictation.status()?).map_err(|e| e.to_string())
}
#[tauri::command]
pub(crate) async fn start_dictation(
    state: State<'_, Host>,
    token: String,
    id: String,
) -> Result<(), String> {
    authorize(&state, &token)?;
    #[cfg(target_os = "android")]
    microphone_permission(&state, false)?;
    state.dictation.start(id).await
}
#[tauri::command]
pub(crate) async fn finish_dictation(
    state: State<'_, Host>,
    token: String,
    id: String,
    language: Option<String>,
) -> Result<String, String> {
    authorize(&state, &token)?;
    state.dictation.finish(&id, language).await
}
#[tauri::command]
pub(crate) fn cancel_dictation(
    state: State<'_, Host>,
    token: String,
    id: Option<String>,
) -> Result<(), String> {
    authorize(&state, &token)?;
    state.dictation.cancel(id.as_deref())
}
