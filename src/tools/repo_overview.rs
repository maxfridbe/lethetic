use super::icons;
use crate::tools::{FunctionDefinition, Tool, ToolExecution};
use serde_json::json;
use std::fs;
use std::path::Path;

pub fn get_definition() -> Tool {
    Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "repo_overview".to_string(),
            description: "Get a structural overview of a repository or directory: detected ecosystem/language, package manager files, entry points, and top-level directory tree. Use this before diving into a new codebase.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Root directory to analyse (defaults to '.')"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the action"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "Unique identifier for this call"
                    }
                },
                "required": ["description", "tool_call_id"]
            }),
        },
    }
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    if let Some(desc) = arguments["description"].as_str() {
        return format!("{} {}", icons::PATH, desc);
    }
    let path = arguments["path"].as_str().unwrap_or(".");
    format!("{} Repo overview: `{}`", icons::PATH, path)
}

// Known package manager / ecosystem indicator files
const ECOSYSTEM_FILES: &[(&str, &str, &str)] = &[
    ("Cargo.toml", "Rust", "cargo"),
    ("package.json", "JavaScript/TypeScript", "npm/yarn/pnpm"),
    ("go.mod", "Go", "go modules"),
    ("pyproject.toml", "Python", "pyproject"),
    ("requirements.txt", "Python", "pip"),
    ("setup.py", "Python", "setuptools"),
    ("pom.xml", "Java", "Maven"),
    ("build.gradle", "Java/Kotlin", "Gradle"),
    ("build.gradle.kts", "Kotlin", "Gradle"),
    ("*.csproj", "C#", "dotnet"),
    ("*.fsproj", "F#", "dotnet"),
    ("CMakeLists.txt", "C/C++", "CMake"),
    ("Makefile", "C/C++", "Make"),
    ("composer.json", "PHP", "Composer"),
    ("Gemfile", "Ruby", "Bundler"),
    ("mix.exs", "Elixir", "Mix"),
    ("pubspec.yaml", "Dart/Flutter", "pub"),
    ("Package.swift", "Swift", "SPM"),
    ("flake.nix", "Nix", "flake"),
    ("Dockerfile", "Docker", "—"),
    ("docker-compose.yml", "Docker", "Compose"),
    ("docker-compose.yaml", "Docker", "Compose"),
];

const ENTRY_POINTS: &[&str] = &[
    "src/main.rs",
    "src/lib.rs",
    "main.go",
    "cmd/main.go",
    "main.py",
    "__main__.py",
    "app.py",
    "src/index.ts",
    "src/main.ts",
    "index.ts",
    "src/index.js",
    "src/main.js",
    "index.js",
    "src/main.cs",
    "Program.cs",
    "main.c",
    "main.cpp",
    "src/main.java",
    "Main.java",
    "lib.rb",
    "main.rb",
    "lib.ex",
    "main.ex",
];

pub async fn execute(
    path: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> String {
    execute_classified(path, cwd, cancellation_token)
        .await
        .output
}

pub(super) async fn execute_classified(
    path: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> ToolExecution {
    let search_root = if path.is_empty() || path == "." {
        Path::new(cwd).to_path_buf()
    } else {
        Path::new(cwd).join(path)
    };

    if cancellation_token.is_cancelled() {
        return ToolExecution::error("[Operation Cancelled by User]", cwd);
    }
    match analyse(&search_root, &cancellation_token) {
        Ok(output) => ToolExecution::success(output, cwd),
        Err(error) => ToolExecution::error(error, cwd),
    }
}

fn analyse(
    root: &Path,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    check_cancelled(cancellation_token)?;
    let metadata = fs::metadata(root).map_err(|error| {
        format!(
            "ERROR: Failed to inspect repository root {}: {error}",
            root.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!(
            "ERROR: Repository root is not a directory: {}",
            root.display()
        ));
    }

    let mut out = String::new();

    out.push_str(&format!("# Repository Overview: `{}`\n\n", root.display()));

    // ── Ecosystem detection ──────────────────────────────────────────────────
    let mut detected = Vec::new();
    let mut pkg_files_found = Vec::new();

    for (pattern, ecosystem, pkg_mgr) in ECOSYSTEM_FILES {
        check_cancelled(cancellation_token)?;
        if pattern.contains('*') {
            // Glob-style: check by extension.
            let ext = pattern.trim_start_matches("*.");
            let entries = fs::read_dir(root).map_err(|error| {
                format!(
                    "ERROR: Failed to read repository root {}: {error}",
                    root.display()
                )
            })?;
            for entry in entries {
                check_cancelled(cancellation_token)?;
                let entry = entry.map_err(|error| {
                    format!(
                        "ERROR: Failed to read an entry under {}: {error}",
                        root.display()
                    )
                })?;
                let fname = entry.file_name();
                let fname = fname.to_string_lossy();
                if fname.ends_with(&format!(".{ext}")) {
                    pkg_files_found.push(format!("`{fname}` ({ecosystem}, {pkg_mgr})"));
                    if !detected.iter().any(|d: &String| d.contains(ecosystem)) {
                        detected.push(format!("{ecosystem} ({pkg_mgr})"));
                    }
                }
            }
        } else {
            let candidate = root.join(pattern);
            if candidate.try_exists().map_err(|error| {
                format!(
                    "ERROR: Failed to inspect package marker {}: {error}",
                    candidate.display()
                )
            })? {
                pkg_files_found.push(format!("`{pattern}` ({ecosystem}, {pkg_mgr})"));
                if !detected.iter().any(|d: &String| d.contains(ecosystem)) {
                    detected.push(format!("{ecosystem} ({pkg_mgr})"));
                }
            }
        }
    }

    if detected.is_empty() {
        out.push_str("## Ecosystem\nNot detected (no known package manager files found)\n\n");
    } else {
        out.push_str("## Ecosystem\n");
        for d in &detected {
            out.push_str(&format!("- {}\n", d));
        }
        out.push('\n');
        out.push_str("## Package Manager Files\n");
        for f in &pkg_files_found {
            out.push_str(&format!("- {}\n", f));
        }
        out.push('\n');
    }

    // ── Entry points ─────────────────────────────────────────────────────────
    let mut found_entries = Vec::new();
    for ep in ENTRY_POINTS {
        check_cancelled(cancellation_token)?;
        let candidate = root.join(ep);
        if candidate.try_exists().map_err(|error| {
            format!(
                "ERROR: Failed to inspect entry point {}: {error}",
                candidate.display()
            )
        })? {
            found_entries.push(*ep);
        }
    }
    if !found_entries.is_empty() {
        out.push_str("## Entry Points\n");
        for ep in &found_entries {
            out.push_str(&format!("- `{}`\n", ep));
        }
        out.push('\n');
    }

    // ── Directory tree (2 levels, ignoring build artifacts) ──────────────────
    out.push_str("## Directory Structure\n```\n");
    out.push_str(&format!(
        "{}/\n",
        root.file_name().unwrap_or_default().to_string_lossy()
    ));
    dir_tree(root, 1, 2, &mut out, cancellation_token)?;
    out.push_str("```\n");

    // ── README snippet ────────────────────────────────────────────────────────
    for readme in &["README.md", "README.txt", "README", "readme.md"] {
        check_cancelled(cancellation_token)?;
        let path = root.join(readme);
        if path.try_exists().map_err(|error| {
            format!(
                "ERROR: Failed to inspect README candidate {}: {error}",
                path.display()
            )
        })? {
            let content = fs::read_to_string(&path).map_err(|error| {
                format!("ERROR: Failed to read README {}: {error}", path.display())
            })?;
            let preview: String = content.lines().take(20).collect::<Vec<_>>().join("\n");
            out.push_str(&format!(
                "\n## README (first 20 lines)\n```\n{}\n```\n",
                preview
            ));
            break;
        }
    }

    Ok(out)
}

fn check_cancelled(cancellation_token: &tokio_util::sync::CancellationToken) -> Result<(), String> {
    if cancellation_token.is_cancelled() {
        Err("[Operation Cancelled by User]".to_string())
    } else {
        Ok(())
    }
}

fn dir_tree(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    out: &mut String,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    check_cancelled(cancellation_token)?;
    if depth > max_depth {
        return Ok(());
    }
    let skip = [
        "target",
        ".git",
        "node_modules",
        ".lethetic",
        "dist",
        "build",
        "__pycache__",
        ".idea",
        ".vscode",
    ];

    let indent = "  ".repeat(depth);
    let entries = fs::read_dir(dir)
        .map_err(|error| format!("ERROR: Failed to read directory {}: {error}", dir.display()))?;
    let mut entries = entries.collect::<Result<Vec<_>, _>>().map_err(|error| {
        format!(
            "ERROR: Failed to read an entry under {}: {error}",
            dir.display()
        )
    })?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        check_cancelled(cancellation_token)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if skip.contains(&name.as_ref()) {
            continue;
        }
        let metadata = entry.metadata().map_err(|error| {
            format!(
                "ERROR: Failed to inspect repository entry {}: {error}",
                entry.path().display()
            )
        })?;
        if metadata.is_dir() {
            out.push_str(&format!("{}{}/\n", indent, name));
            dir_tree(&entry.path(), depth + 1, max_depth, out, cancellation_token)?;
        } else {
            out.push_str(&format!("{}{}\n", indent, name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_repo_overview_rust() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"test\"").unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main(){}").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute(".", dir.path().to_str().unwrap(), token).await;

        assert!(result.contains("Rust"), "{}", result);
        assert!(result.contains("Cargo.toml"), "{}", result);
        assert!(result.contains("src/main.rs"), "{}", result);
    }

    #[tokio::test]
    async fn test_repo_overview_unknown() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("hello.txt"), "hi").unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let result = execute(".", dir.path().to_str().unwrap(), token).await;

        assert!(result.contains("Not detected"), "{}", result);
    }

    #[tokio::test]
    async fn missing_root_is_a_typed_error() {
        let dir = tempdir().unwrap();
        let result = execute_classified(
            "missing-repository",
            dir.path().to_str().unwrap(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert!(result.is_error);
        assert!(result.output.contains("Failed to inspect repository root"));
    }

    #[tokio::test]
    async fn pre_cancelled_overview_does_not_read_the_root() {
        let dir = tempdir().unwrap();
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();

        let result = execute_classified(".", dir.path().to_str().unwrap(), token).await;

        assert!(result.is_error);
        assert_eq!(result.output, "[Operation Cancelled by User]");
    }
}
