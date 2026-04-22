use tokio::process::Command;
use std::fs;
use std::collections::HashMap;
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::path::{Path, PathBuf};

static SNIPPETS: Lazy<Mutex<HashMap<String, String>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn substitute_placeholders(input: &str) -> String {
    let mut result = input.to_string();
    let snippets = SNIPPETS.lock().unwrap();
    for (name, content) in snippets.iter() {
        let placeholder = format!("***{}***", name);
        result = result.replace(&placeholder, content);
    }
    result
}

pub async fn execute_shell(command: &str, cwd: &str) -> (String, String) {
    let processed_command = substitute_placeholders(command);
    
    // Check if the command starts with 'cd' to update the persistent state
    let mut final_cwd = PathBuf::from(cwd);
    if processed_command.trim().starts_with("cd ") {
        let parts: Vec<&str> = processed_command.trim().split_whitespace().collect();
        if parts.len() > 1 {
            let target = parts[1];
            let new_path = if target == ".." {
                final_cwd.parent().map(|p| p.to_path_buf()).unwrap_or(final_cwd.clone())
            } else {
                final_cwd.join(target)
            };
            if new_path.exists() && new_path.is_dir() {
                final_cwd = new_path;
            }
        }
    }

    let output = Command::new("bash")
        .arg("-c")
        .arg(&processed_command)
        .current_dir(&final_cwd)
        .output()
        .await;

    let res_str = match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let status = out.status.code().map_or("signaled".to_string(), |c| c.to_string());
            format!("EXIT_CODE: {}\nSTDOUT:\n{}\nSTDERR:\n{}", status, stdout, stderr)
        }
        Err(e) => format!("ERROR: {}", e),
    };

    (res_str, final_cwd.display().to_string())
}

pub async fn execute_read_file_lines(path: &str, start: usize, end: usize, cwd: &str) -> String {
    let full_path = Path::new(cwd).join(path);
    match fs::read_to_string(&full_path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = start.saturating_sub(1);
            let end = end.min(lines.len());
            if start >= lines.len() || start > end {
                return format!("ERROR: Invalid line range {}-{} for file with {} lines", start + 1, end, lines.len());
            }
            lines[start..end].join("\n")
        }
        Err(e) => format!("ERROR: Failed to read file {}: {}", full_path.display(), e),
    }
}

pub async fn execute_apply_patch(path: &str, patch: &str, cwd: &str) -> String {
    let processed_patch = substitute_placeholders(patch);
    let patch_file = Path::new(cwd).join(".tmp.patch");
    if let Err(e) = fs::write(&patch_file, &processed_patch) {
        return format!("ERROR: Failed to write temp patch file: {}", e);
    }

    let output = Command::new("patch")
        .arg("-u")
        .arg(path)
        .arg("-i")
        .arg(".tmp.patch")
        .current_dir(cwd)
        .output()
        .await;

    let _ = fs::remove_file(patch_file);

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            format!("STDOUT:\n{}\nSTDERR:\n{}", stdout, stderr)
        }
        Err(e) => format!("ERROR: {}", e),
    }
}

pub async fn execute_write_file(path: &str, content: &str, cwd: &str) -> String {
    let processed_content = substitute_placeholders(content);
    let full_path = Path::new(cwd).join(path);
    
    // Ensure parent directory exists
    if let Some(parent) = full_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    match fs::write(&full_path, processed_content) {
        Ok(_) => format!("Successfully wrote to {}", full_path.display()),
        Err(e) => format!("ERROR: Failed to write to {}: {}", full_path.display(), e),
    }
}

pub async fn execute_code_snippet(name: &str, content: &str) -> String {
    let mut snippets = SNIPPETS.lock().unwrap();
    snippets.insert(name.to_string(), content.to_string());
    format!("Successfully stored code snippet: {}", name)
}

pub async fn get_git_info() -> String {
    let status = Command::new("git")
        .arg("status")
        .arg("--porcelain=v2")
        .arg("--branch")
        .output()
        .await;

    match status {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout);
            if s.trim().is_empty() { return "clean".to_string(); }

            let mut branch = String::from("unknown");
            let mut untracked = 0;
            let mut modified = 0;
            let mut staged = 0;
            let mut renamed = 0;
            let mut deleted = 0;

            for line in s.lines() {
                if line.starts_with("# branch.head") {
                    branch = line.split_whitespace().nth(2).unwrap_or("detached").to_string();
                } else if line.starts_with("?") {
                    untracked += 1;
                } else if line.starts_with("1 ") || line.starts_with("2 ") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() > 1 {
                        let codes = parts[1];
                        let staged_code = codes.chars().nth(0).unwrap_or('.');
                        let unstaged_code = codes.chars().nth(1).unwrap_or('.');
                        
                        if staged_code != '.' { staged += 1; }
                        if unstaged_code == 'M' { modified += 1; }
                        if unstaged_code == 'D' { deleted += 1; }
                        if staged_code == 'R' { renamed += 1; }
                    }
                }
            }

            let mut res = format!(" {}", branch);
            if staged > 0 { res.push_str(&format!(" +{}", staged)); }
            if modified > 0 { res.push_str(&format!(" ~{}", modified)); }
            if deleted > 0 { res.push_str(&format!(" -{}", deleted)); }
            if untracked > 0 { res.push_str(&format!(" ?{}", untracked)); }
            if renamed > 0 { res.push_str(&format!(" r{}", renamed)); }
            
            if staged == 0 && modified == 0 && untracked == 0 && deleted == 0 && renamed == 0 {
                format!(" {} (clean)", branch)
            } else {
                res
            }
        }
        Err(_) => "not a git repo".to_string(),
    }
}
