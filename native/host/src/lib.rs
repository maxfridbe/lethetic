mod android_media;
mod dictation;
mod download;
mod download_buffer;
mod sites;
mod speech;
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
    sites: Mutex<sites::Sites>,
    speech: lethetic_network::speech::SpeechStreams,
    dictation: lethetic_dictation::Dictation,
    #[cfg(not(target_os = "android"))]
    awake: Mutex<Option<keepawake::KeepAwake>>,
    #[cfg(target_os = "android")]
    mobile: tauri::plugin::PluginHandle<tauri::Wry>,
}
#[derive(Default)]
struct NetworkGrants {
    epoch: u64,
    grants: HashMap<String, lethetic_network::Grant>,
    histories: std::collections::VecDeque<(String, lethetic_network::diagnostics::Diagnostics)>,
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
    app: tauri::AppHandle,
    mode: lethetic_network::NetworkMode,
    session_id: Option<String>,
    state: State<'_, Host>,
    token: String,
) -> Result<lethetic_network::Route, String> {
    authorize(&state, &token)?;
    let epoch = state
        .network
        .lock()
        .map_err(|error| error.to_string())?
        .epoch;
    let directory = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("tor");
    let diagnostics = {
        let mut grants = state.network.lock().map_err(|error| error.to_string())?;
        if let Some(id) = session_id {
            if id.is_empty()
                || id.len() > 128
                || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err("Invalid session identifier".into());
            }
            if let Some(index) = grants.histories.iter().position(|(key, _)| key == &id) {
                let entry = grants.histories.remove(index).expect("history index");
                grants.histories.push_back(entry);
            } else {
                if grants.histories.len() >= 16 {
                    grants.histories.pop_front();
                }
                grants.histories.push_back((id, Default::default()));
            }
            grants.histories.back().expect("history inserted").1.clone()
        } else {
            Default::default()
        }
    };
    let grant =
        lethetic_network::Grant::open_with_diagnostics(mode, Some(&directory), diagnostics).await?;
    grant.ready().await?;
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
fn network_diagnostics(
    state: State<'_, Host>,
    token: String,
    session_id: String,
) -> Result<Option<lethetic_network::diagnostics::Snapshot>, String> {
    authorize(&state, &token)?;
    let grants = state.network.lock().map_err(|error| error.to_string())?;
    Ok(grants
        .histories
        .iter()
        .find(|(id, _)| id == &session_id)
        .map(|(_, log)| log.snapshot()))
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
            android_media::load_credentials,
            android_media::save_credentials,
            android_media::response_audio,
            dictation::prepare_dictation,
            dictation::dictation_status,
            dictation::start_dictation,
            dictation::finish_dictation,
            dictation::cancel_dictation,
            speech::begin_speech,
            speech::next_speech,
            speech::cancel_speech,
            sites::forward_site,
            sites::stop_forwarded_sites,
            sites::next_site_request,
            sites::reply_site_request,
            sites::open_site_browser,
            download::begin_download,
            download::write_download,
            download::finish_download,
            download::cancel_download,
            choose_directory,
            sync_directory,
            revoke_directory,
            grant_network,
            network_diagnostics,
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
                sites: Mutex::default(),
                speech: Default::default(),
                dictation: dictation::engine(),
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
            sites: Mutex::default(),
            speech: Default::default(),
            dictation: Default::default(),
            awake: Mutex::new(None),
        };
        assert!(authorize(&host, "").is_err());
        assert!(authorize(&host, "different-launch").is_err());
        assert!(authorize(&host, "per-launch-secret").is_ok());
    }
}
