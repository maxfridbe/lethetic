use super::{Host, authorize};
use lethetic_network::sites::{SiteBridge, SiteGrant, SiteRequest, SiteResponse, SiteRoute};
use std::{collections::HashMap, sync::Arc};
#[cfg(target_os = "android")]
use tauri::Manager;
use tauri::State;

#[derive(Default)]
pub(crate) struct Sites {
    pub epoch: u64,
    pub grants: HashMap<String, SiteGrant>,
}
fn bridge(host: &Host, id: &str) -> Result<Arc<SiteBridge>, String> {
    host.sites
        .lock()
        .map_err(|e| e.to_string())?
        .grants
        .get(id)
        .map(|g| g.bridge.clone())
        .ok_or_else(|| "Forwarding stopped".into())
}
#[tauri::command]
pub async fn forward_site(
    state: State<'_, Host>,
    token: String,
    port: u16,
    network: bool,
) -> Result<SiteRoute, String> {
    authorize(&state, &token)?;
    let epoch = state.sites.lock().map_err(|e| e.to_string())?.epoch;
    let grant = SiteGrant::open(port, network).await?;
    let mut sites = state.sites.lock().map_err(|e| e.to_string())?;
    if sites.epoch != epoch {
        return Err("Site forwarding was reset".into());
    }
    if sites.grants.len() >= 8 {
        return Err("At most eight sites may be forwarded".into());
    }
    let route = grant.route.clone();
    sites.grants.insert(route.id.clone(), grant);
    Ok(route)
}
#[tauri::command]
pub fn stop_forwarded_sites(
    state: State<'_, Host>,
    token: String,
    id: Option<String>,
) -> Result<(), String> {
    authorize(&state, &token)?;
    let mut sites = state.sites.lock().map_err(|e| e.to_string())?;
    if let Some(id) = id {
        sites.grants.remove(&id);
    } else {
        sites.epoch = sites.epoch.wrapping_add(1);
        sites.grants.clear();
    }
    Ok(())
}
#[tauri::command]
pub async fn next_site_request(
    state: State<'_, Host>,
    token: String,
    id: String,
) -> Result<SiteRequest, String> {
    authorize(&state, &token)?;
    bridge(&state, &id)?.next().await
}
#[tauri::command]
pub fn reply_site_request(
    state: State<'_, Host>,
    token: String,
    id: String,
    request_id: String,
    response: SiteResponse,
) -> Result<(), String> {
    authorize(&state, &token)?;
    bridge(&state, &id)?.reply(&request_id, response)
}
#[tauri::command]
pub async fn open_site_browser(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    id: String,
) -> Result<(), String> {
    authorize(&state, &token)?;
    let url = state
        .sites
        .lock()
        .map_err(|e| e.to_string())?
        .grants
        .get(&id)
        .ok_or("Forwarding stopped")?
        .route
        .url
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "android")]
        {
            app.state::<Host>()
                .mobile
                .run_mobile_plugin::<serde_json::Value>(
                    "openSiteBrowser",
                    serde_json::json!({"url":url}),
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = app;
            #[cfg(target_os = "windows")]
            let mut command = {
                let mut command = std::process::Command::new("rundll32.exe");
                command.arg("url.dll,FileProtocolHandler");
                command
            };
            #[cfg(target_os = "macos")]
            let mut command = std::process::Command::new("open");
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            let mut command = std::process::Command::new("xdg-open");
            command
                .arg(url)
                .status()
                .map_err(|e| e.to_string())
                .and_then(|status| {
                    if status.success() {
                        Ok(())
                    } else {
                        Err("The browser could not be opened".into())
                    }
                })
        }
    })
    .await
    .map_err(|e| e.to_string())?
}
