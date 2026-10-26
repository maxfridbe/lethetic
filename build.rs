use std::{env, error::Error, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    // The portable engine has no web-asset or JavaScript toolchain dependency.
    if env::var_os("CARGO_FEATURE_NATIVE").is_none() {
        return Ok(());
    }
    for input in [
        "web/src",
        "web/lib",
        "web/static",
        "web/tsconfig.json",
        "scripts/build_web_assets.py",
        "scripts/generate_web_asset_manifest.py",
    ] {
        println!("cargo:rerun-if-changed={input}");
    }
    println!("cargo:rerun-if-env-changed=PATH");
    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest directory")?);
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing Cargo output directory")?);
    let python = if cfg!(windows) { "python" } else { "python3" };
    let status = Command::new(python)
        .arg(root.join("scripts/build_web_assets.py"))
        .arg("--output")
        .arg(output)
        .current_dir(root)
        .status()
        .map_err(|error| {
            format!("cannot build embedded web UI: install Python 3 and TypeScript 7.0.2: {error}")
        })?;
    if !status.success() {
        return Err("embedded web UI build failed; see compiler/asset diagnostics above".into());
    }
    Ok(())
}
