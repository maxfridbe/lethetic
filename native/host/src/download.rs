use crate::{Host, authorize};
use tauri::{Manager, State};
#[cfg(not(target_os = "android"))]
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub(crate) async fn begin_download(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    name: String,
    mime: String,
    size: u64,
) -> Result<String, String> {
    authorize(&state, &token)?;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = app
            .path()
            .app_cache_dir()
            .map_err(|e| e.to_string())?
            .join("downloads");
        app.state::<Host>()
            .downloads
            .lock()
            .map_err(|e| e.to_string())?
            .begin(&cache, name, mime, size)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn write_download(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    id: String,
    offset: u64,
    data: String,
) -> Result<(), String> {
    authorize(&state, &token)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<Host>()
            .downloads
            .lock()
            .map_err(|e| e.to_string())?
            .append(&id, offset, &data)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn cancel_download(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    id: Option<String>,
) -> Result<(), String> {
    authorize(&state, &token)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<Host>()
            .downloads
            .lock()
            .map_err(|e| e.to_string())?
            .cancel(id.as_deref());
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn finish_download(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    id: String,
) -> Result<bool, String> {
    authorize(&state, &token)?;
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<Host>();
        let download = host.downloads.lock().map_err(|e| e.to_string())?.finish(&id).map_err(|e| e.to_string())?;
        #[cfg(not(target_os = "android"))]
        {
            let _ = &download.mime;
            let selected = app.dialog().file().set_title("Save download").set_file_name(&download.name).blocking_save_file();
            let Some(selected) = selected else { return Ok(false); };
            let path = selected.into_path().map_err(|e| e.to_string())?;
            std::fs::copy(download.file.path(), path).map_err(|e| format!("Unable to save download: {e}"))?;
            Ok(true)
        }
        #[cfg(target_os = "android")]
        {
            let result: serde_json::Value = host.mobile.run_mobile_plugin("saveDownload", serde_json::json!({"path":download.file.path(),"name":download.name,"mime":download.mime})).map_err(|e| e.to_string())?;
            result.get("saved").and_then(serde_json::Value::as_bool).ok_or_else(|| "Invalid save result".into())
        }
    }).await.map_err(|e| e.to_string())?
}
