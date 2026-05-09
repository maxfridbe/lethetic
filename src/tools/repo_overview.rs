use serde_json::json;
use crate::tools::{Tool, FunctionDefinition};
use super::icons;
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
    ("Cargo.toml",        "Rust",       "cargo"),
    ("package.json",      "JavaScript/TypeScript", "npm/yarn/pnpm"),
    ("go.mod",            "Go",         "go modules"),
    ("pyproject.toml",    "Python",     "pyproject"),
    ("requirements.txt",  "Python",     "pip"),
    ("setup.py",          "Python",     "setuptools"),
    ("pom.xml",           "Java",       "Maven"),
    ("build.gradle",      "Java/Kotlin","Gradle"),
    ("build.gradle.kts",  "Kotlin",     "Gradle"),
    ("*.csproj",          "C#",         "dotnet"),
    ("*.fsproj",          "F#",         "dotnet"),
    ("CMakeLists.txt",    "C/C++",      "CMake"),
    ("Makefile",          "C/C++",      "Make"),
    ("composer.json",     "PHP",        "Composer"),
    ("Gemfile",           "Ruby",       "Bundler"),
    ("mix.exs",           "Elixir",     "Mix"),
    ("pubspec.yaml",      "Dart/Flutter","pub"),
    ("Package.swift",     "Swift",      "SPM"),
    ("flake.nix",         "Nix",        "flake"),
    ("Dockerfile",        "Docker",     "—"),
    ("docker-compose.yml","Docker",     "Compose"),
    ("docker-compose.yaml","Docker",    "Compose"),
];

const ENTRY_POINTS: &[&str] = &[
    "src/main.rs", "src/lib.rs",
    "main.go", "cmd/main.go",
    "main.py", "__main__.py", "app.py",
    "src/index.ts", "src/main.ts", "index.ts",
    "src/index.js", "src/main.js", "index.js",
    "src/main.cs", "Program.cs",
    "main.c", "main.cpp",
    "src/main.java", "Main.java",
    "lib.rb", "main.rb",
    "lib.ex", "main.ex",
];

pub async fn execute(
    path: &str,
    cwd: &str,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> String {
    let search_root = if path.is_empty() || path == "." {
        Path::new(cwd).to_path_buf()
    } else {
        Path::new(cwd).join(path)
    };

    tokio::select! {
        _ = cancellation_token.cancelled() => "[Operation Cancelled by User]".to_string(),
        result = analyse(&search_root) => result,
    }
}

async fn analyse(root: &Path) -> String {
    let mut out = String::new();

    out.push_str(&format!("# Repository Overview: `{}`\n\n", root.display()));

    // ── Ecosystem detection ──────────────────────────────────────────────────
    let mut detected = Vec::new();
    let mut pkg_files_found = Vec::new();

    for (pattern, ecosystem, pkg_mgr) in ECOSYSTEM_FILES {
        if pattern.contains('*') {
            // Glob-style: check by extension
            let ext = pattern.trim_start_matches("*.");
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten() {
                    let fname = entry.file_name();
                    let fname = fname.to_string_lossy();
                    if fname.ends_with(&format!(".{}", ext)) {
                        pkg_files_found.push(format!("`{}` ({}, {})", fname, ecosystem, pkg_mgr));
                        if !detected.iter().any(|d: &String| d.contains(ecosystem)) {
                            detected.push(format!("{} ({})", ecosystem, pkg_mgr));
                        }
                    }
                }
            }
        } else {
            let candidate = root.join(pattern);
            if candidate.exists() {
                pkg_files_found.push(format!("`{}` ({}, {})", pattern, ecosystem, pkg_mgr));
                if !detected.iter().any(|d: &String| d.contains(ecosystem)) {
                    detected.push(format!("{} ({})", ecosystem, pkg_mgr));
                }
            }
        }
    }

    if detected.is_empty() {
        out.push_str("## Ecosystem\nNot detected (no known package manager files found)\n\n");
    } else {
        out.push_str("## Ecosystem\n");
        for d in &detected { out.push_str(&format!("- {}\n", d)); }
        out.push('\n');
        out.push_str("## Package Manager Files\n");
        for f in &pkg_files_found { out.push_str(&format!("- {}\n", f)); }
        out.push('\n');
    }

    // ── Entry points ─────────────────────────────────────────────────────────
    let mut found_entries = Vec::new();
    for ep in ENTRY_POINTS {
        if root.join(ep).exists() {
            found_entries.push(*ep);
        }
    }
    if !found_entries.is_empty() {
        out.push_str("## Entry Points\n");
        for ep in &found_entries { out.push_str(&format!("- `{}`\n", ep)); }
        out.push('\n');
    }

    // ── Directory tree (2 levels, ignoring build artifacts) ──────────────────
    out.push_str("## Directory Structure\n```\n");
    out.push_str(&format!("{}/\n", root.file_name().unwrap_or_default().to_string_lossy()));
    dir_tree(root, root, 1, 2, &mut out);
    out.push_str("```\n");

    // ── README snippet ────────────────────────────────────────────────────────
    for readme in &["README.md", "README.txt", "README", "readme.md"] {
        let p = root.join(readme);
        if p.exists() {
            if let Ok(content) = fs::read_to_string(&p) {
                let preview: String = content.lines().take(20).collect::<Vec<_>>().join("\n");
                out.push_str(&format!("\n## README (first 20 lines)\n```\n{}\n```\n", preview));
            }
            break;
        }
    }

    out
}

fn dir_tree(root: &Path, dir: &Path, depth: usize, max_depth: usize, out: &mut String) {
    if depth > max_depth { return; }
    let skip = ["target", ".git", "node_modules", ".lethetic", "dist", "build", "__pycache__", ".idea", ".vscode"];

    let indent = "  ".repeat(depth);
    let mut entries: Vec<_> = match fs::read_dir(dir) {
        Ok(e) => e.flatten().collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if skip.contains(&name.as_ref()) { continue; }
        let meta = match entry.metadata() { Ok(m) => m, Err(_) => continue };
        if meta.is_dir() {
            out.push_str(&format!("{}{}/\n", indent, name));
            dir_tree(root, &entry.path(), depth + 1, max_depth, out);
        } else {
            out.push_str(&format!("{}{}\n", indent, name));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

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
}
