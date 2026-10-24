fn main() {
    tauri_plugin::Builder::new(&[
        "choose_directory",
        "sync_directory",
        "revoke_directory",
        "set_working",
    ])
    .android_path("android")
    .build();
}
