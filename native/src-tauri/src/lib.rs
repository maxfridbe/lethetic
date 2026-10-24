use tauri::{WebviewUrl, WebviewWindowBuilder};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let token = uuid::Uuid::new_v4().to_string();
    let builder = tauri::Builder::default();
    #[cfg(not(target_os = "android"))]
    let builder = builder.plugin(tauri_plugin_dialog::init());
    builder
        .plugin(tauri_plugin_lethetic_host::init(token.clone()))
        .setup(move |app| {
            // Tauri's iframe origin distinction is incomplete on Linux/Android.
            // A per-launch secret is injected only into the top-level bundled document.
            // Sandboxed previews and workers receive neither this secret nor a filesystem API.
            let script = format!("if (window === window.top && ['tauri://localhost', 'http://tauri.localhost', 'https://tauri.localhost'].includes(location.protocol + '//' + location.host) && ['/', '/index.html'].includes(location.pathname)) Object.defineProperty(window, '__LETHETIC_HOST_TOKEN__', {{ value: {} }});", serde_json::to_string(&token)?);
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Lethetic").inner_size(1200.0, 800.0)
                .initialization_script(&script)
                .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
                .on_navigation(|url| {
                    matches!(url.as_str(), "about:blank" | "about:srcdoc") || (matches!((url.scheme(), url.host_str()), ("tauri", Some("localhost")) | ("http" | "https", Some("tauri.localhost")))
                        && url.port().is_none()
                        && matches!(url.path(), "/" | "/index.html"))
                })
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Unable to start Lethetic");
}
