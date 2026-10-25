mod download;
mod download_buffer;
use lethetic_directory::{Grant, Workspace};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{Manager, State};
#[cfg(not(target_os = "android"))]
use tauri_plugin_dialog::DialogExt;

struct Host {
    token: String,
    downloads: Mutex<download_buffer::Downloads>,
    directory: Mutex<Option<Grant>>,
    network: Mutex<NetworkGrants>,
    #[cfg(not(target_os = "android"))]
    awake: Mutex<Option<keepawake::KeepAwake>>,
    #[cfg(target_os = "android")]
    mobile: tauri::plugin::PluginHandle<tauri::Wry>,
}
#[derive(Default)]
struct NetworkGrants {
    epoch: u64,
    grants: HashMap<String, lethetic_network::Grant>,
}
fn authorize(host: &Host, token: &str) -> Result<(), String> {
    if token == host.token {
        Ok(())
    } else {
        Err("Native access is restricted to the main application".into())
    }
}
#[tauri::command]
async fn grant_network(
    state: State<'_, Host>,
    token: String,
) -> Result<lethetic_network::Route, String> {
    authorize(&state, &token)?;
    let epoch = state
        .network
        .lock()
        .map_err(|error| error.to_string())?
        .epoch;
    let grant = lethetic_network::Grant::open().await?;
    let mut grants = state.network.lock().map_err(|error| error.to_string())?;
    if grants.epoch != epoch {
        return Err("Network grants were reset during initialization".into());
    }
    if grants.grants.len() >= 8 {
        return Err("Too many active network runtimes".into());
    }
    let route = grant.route.clone();
    grants.grants.insert(route.token.clone(), grant);
    Ok(route)
}
#[tauri::command]
fn revoke_network(
    state: State<'_, Host>,
    token: String,
    grant: Option<String>,
) -> Result<(), String> {
    authorize(&state, &token)?;
    let mut grants = state.network.lock().map_err(|error| error.to_string())?;
    if let Some(grant) = grant {
        grants.grants.remove(&grant);
    } else {
        grants.epoch = grants.epoch.wrapping_add(1);
        grants.grants.clear();
    }
    Ok(())
}
#[tauri::command]
async fn choose_directory(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
) -> Result<Value, String> {
    authorize(&state, &token)?;
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<Host>();
        #[cfg(not(target_os = "android"))]
        {
            let selected = app.dialog().file().set_title("Grant Lethetic access to a workspace folder").blocking_pick_folder();
            let Some(selected) = selected else { return Ok(Value::Null); };
            let path = selected.into_path().map_err(|error| error.to_string())?;
            let (grant, workspace) = Grant::open(&path).map_err(|error| error.to_string())?;
            *host.directory.lock().map_err(|error| error.to_string())? = Some(grant);
            Ok(json!({ "name": path.file_name().unwrap_or_default().to_string_lossy(), "workspace": workspace }))
        }
        #[cfg(target_os = "android")]
        host.mobile.run_mobile_plugin("chooseDirectory", json!({})).map_err(|error| error.to_string())
    }).await.map_err(|error| error.to_string())?
}
#[tauri::command]
async fn sync_directory(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    workspace: Workspace,
) -> Result<(), String> {
    authorize(&state, &token)?;
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<Host>();
        #[cfg(not(target_os = "android"))]
        {
            let mut directory = host.directory.lock().map_err(|error| error.to_string())?;
            directory
                .as_mut()
                .ok_or("No directory granted")?
                .sync(&workspace)
                .map_err(|error| error.to_string())
        }
        #[cfg(target_os = "android")]
        host.mobile
            .run_mobile_plugin::<Value>("syncDirectory", json!({ "workspace": workspace }))
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn revoke_directory(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
) -> Result<(), String> {
    authorize(&state, &token)?;
    *state.directory.lock().map_err(|error| error.to_string())? = None;
    #[cfg(target_os = "android")]
    {
        tauri::async_runtime::spawn_blocking(move || {
            app.state::<Host>()
                .mobile
                .run_mobile_plugin::<Value>("revokeDirectory", json!({}))
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    }
    #[cfg(not(target_os = "android"))]
    let _ = app;
    Ok(())
}
#[tauri::command]
async fn set_working(
    app: tauri::AppHandle,
    state: State<'_, Host>,
    token: String,
    working: bool,
    active: bool,
) -> Result<(), String> {
    authorize(&state, &token)?;
    tauri::async_runtime::spawn_blocking(move || {
        let host = app.state::<Host>();
        #[cfg(not(target_os = "android"))]
        {
            let _ = active;
            let mut awake = host.awake.lock().map_err(|error| error.to_string())?;
            if working && awake.is_none() {
                *awake = Some(
                    keepawake::Builder::default()
                        .sleep(true)
                        .idle(true)
                        .reason("Waiting for a model response")
                        .app_name("Lethetic")
                        .app_reverse_domain("io.github.maxfridbe.lethetic")
                        .create()
                        .map_err(|error| error.to_string())?,
                );
            } else if !working {
                *awake = None;
            }
            Ok(())
        }
        #[cfg(target_os = "android")]
        host.mobile
            .run_mobile_plugin::<Value>(
                "setWorking",
                json!({ "working": working, "active": active }),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
pub fn init(token: String) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let builder = tauri::plugin::Builder::new("lethetic-host")
        .invoke_handler(tauri::generate_handler![
            download::begin_download,
            download::write_download,
            download::finish_download,
            download::cancel_download,
            choose_directory,
            sync_directory,
            revoke_directory,
            grant_network,
            revoke_network,
            set_working
        ])
        .setup(move |app, _api| {
            #[cfg(target_os = "android")]
            let mobile =
                _api.register_android_plugin("io.github.maxfridbe.lethetic.host", "HostPlugin")?;
            app.manage(Host {
                token,
                downloads: Mutex::default(),
                directory: Mutex::new(None),
                network: Mutex::new(NetworkGrants::default()),
                #[cfg(not(target_os = "android"))]
                awake: Mutex::new(None),
                #[cfg(target_os = "android")]
                mobile,
            });
            Ok(())
        });
    builder.build()
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;
    #[test]
    fn iframe_without_top_level_token_cannot_use_host() {
        let host = Host {
            token: "per-launch-secret".into(),
            downloads: Mutex::default(),
            directory: Mutex::new(None),
            network: Mutex::new(NetworkGrants::default()),
            awake: Mutex::new(None),
        };
        assert!(authorize(&host, "").is_err());
        assert!(authorize(&host, "different-launch").is_err());
        assert!(authorize(&host, "per-launch-secret").is_ok());
    }
}
