use super::{Host, authorize};
#[cfg(not(target_os = "android"))]
use tauri::Manager;
use tauri::State;

#[tauri::command]
pub(crate) async fn prepare_dictation(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    model: String,
    download: bool,
) -> Result<(), String> {
    authorize(&state, &token)?;
    #[cfg(not(target_os = "android"))]
    {
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
    #[cfg(target_os = "android")]
    {
        let _ = (app, model, download);
        Err("Local dictation is available in desktop builds".into())
    }
}
#[tauri::command]
pub(crate) fn dictation_status(
    state: State<'_, Host>,
    token: String,
) -> Result<serde_json::Value, String> {
    authorize(&state, &token)?;
    #[cfg(not(target_os = "android"))]
    {
        serde_json::to_value(state.dictation.status()?).map_err(|e| e.to_string())
    }
    #[cfg(target_os = "android")]
    {
        Err("Local dictation is available in desktop builds".into())
    }
}
#[tauri::command]
pub(crate) async fn start_dictation(
    state: State<'_, Host>,
    token: String,
    id: String,
) -> Result<(), String> {
    authorize(&state, &token)?;
    #[cfg(not(target_os = "android"))]
    {
        state.dictation.start(id).await
    }
    #[cfg(target_os = "android")]
    {
        let _ = id;
        Err("Local dictation is available in desktop builds".into())
    }
}
#[tauri::command]
pub(crate) async fn finish_dictation(
    state: State<'_, Host>,
    token: String,
    id: String,
    language: Option<String>,
) -> Result<String, String> {
    authorize(&state, &token)?;
    #[cfg(not(target_os = "android"))]
    {
        state.dictation.finish(&id, language).await
    }
    #[cfg(target_os = "android")]
    {
        let _ = (id, language);
        Err("Local dictation is available in desktop builds".into())
    }
}
#[tauri::command]
pub(crate) fn cancel_dictation(
    state: State<'_, Host>,
    token: String,
    id: Option<String>,
) -> Result<(), String> {
    authorize(&state, &token)?;
    #[cfg(not(target_os = "android"))]
    {
        state.dictation.cancel(id.as_deref())
    }
    #[cfg(target_os = "android")]
    {
        let _ = id;
        Err("Local dictation is available in desktop builds".into())
    }
}
