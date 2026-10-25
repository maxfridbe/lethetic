fn main() {
    tauri_plugin::Builder::new(&[
        "begin_download",
        "write_download",
        "finish_download",
        "cancel_download",
        "choose_directory",
        "sync_directory",
        "revoke_directory",
        "grant_network",
        "revoke_network",
        "set_working",
    ])
    .android_path("android")
    .build();
}
