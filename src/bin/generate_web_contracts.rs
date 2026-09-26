use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const GENERATED_RELATIVE_PATH: &str = "web/src/generated/contracts.ts";

fn main() {
    if let Err(error) = run(std::env::args().skip(1)) {
        eprintln!("generate_web_contracts: {error}");
        std::process::exit(1);
    }
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<(), String> {
    let mut check = false;
    let mut root = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--check" if !check => check = true,
            "--check" => return Err("--check may be supplied only once".to_string()),
            "--root" if root.is_none() => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--root requires a path".to_string())?;
                root = Some(PathBuf::from(value));
            }
            "--root" => return Err("--root may be supplied only once".to_string()),
            "-h" | "--help" => {
                println!(
                    "Usage: generate_web_contracts [--check] [--root <PATH>]\n\n\
                     Generate {GENERATED_RELATIVE_PATH} from the Rust WFE contracts.\n\
                     --check        fail if the checked-in file differs\n\
                     --root <PATH>  repository root (defaults to the current directory)"
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument `{argument}`")),
        }
    }

    let root = root.unwrap_or(
        std::env::current_dir()
            .map_err(|error| format!("could not determine the current directory: {error}"))?,
    );
    if !root.join("Cargo.toml").is_file() || !root.join("src/wfe/contracts.rs").is_file() {
        return Err(format!(
            "{} is not a Lethetic repository root; use --root <PATH>",
            root.display()
        ));
    }

    let source = lethetic::wfe::generated_typescript_source()?;
    let output = root.join(GENERATED_RELATIVE_PATH);
    if check {
        let existing = fs::read_to_string(&output)
            .map_err(|error| format!("could not read {}: {error}", output.display()))?;
        if existing.replace("\r\n", "\n") != source {
            return Err(format!(
                "{} is stale; run `cargo run --bin generate_web_contracts`",
                output.display()
            ));
        }
        return Ok(());
    }

    write_generated_file(&output, source.as_bytes())
}

fn write_generated_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Ok(existing) = fs::read(path)
        && existing == contents
    {
        return Ok(());
    }
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(format!("refusing to replace symlink {}", path.display()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {}: {error}", parent.display()))?;

    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("could not create {}: {error}", temporary.display()))?;
        file.write_all(contents)
            .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("could not sync {}: {error}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, path).map_err(|error| {
            format!(
                "could not atomically replace {} with {}: {error}",
                path.display(),
                temporary.display()
            )
        })?;
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("could not sync {}: {error}", parent.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("contracts.ts");
    path.with_file_name(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ))
}
