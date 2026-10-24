fn main() {
    tauri_plugin::Builder::new(&[
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
