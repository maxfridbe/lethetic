use tokio::process::Command;
use std::fs;
use std::path::{Path, PathBuf};
use reqwest::Client;

pub async fn execute_shell(command: &str, cwd: &str) -> (String, String) {
    // Check if the command starts with 'cd' to update the persistent state
    let mut final_cwd = PathBuf::from(cwd);
    if command.trim().starts_with("cd ") {
        let parts: Vec<&str> = command.trim().split_whitespace().collect();
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
        .arg(command)
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

pub async fn execute_read_folder(path: &str, cwd: &str) -> String {
    let full_path = Path::new(cwd).join(if path.is_empty() { "." } else { path });
    match fs::read_dir(&full_path) {
        Ok(entries) => {
            let mut items = Vec::new();
            for entry in entries {
                if let Ok(entry) = entry {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    let file_type = entry.file_type().map(|t| if t.is_dir() { "DIR" } else { "FILE" }).unwrap_or("UNKNOWN");
                    items.push(format!("[{}] {}", file_type, file_name));
                }
            }
            items.sort();
            items.join("\n")
        }
        Err(e) => format!("ERROR: Failed to read directory {}: {}", full_path.display(), e),
    }
}

pub async fn execute_search_text(pattern: &str, path: &str, cwd: &str) -> String {
    let search_path = if path.is_empty() { "." } else { path };

    let output = Command::new("grep")
        .arg("-rn")
        .arg("--color=never")
        .arg("-I")
        .arg(pattern)
        .arg(search_path)
        .current_dir(cwd)
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let status = out.status.code().map_or("signaled".to_string(), |c| c.to_string());
            if stdout.is_empty() && stderr.is_empty() && status == "1" {
                return "No matches found.".to_string();
            }
            format!("EXIT_CODE: {}\nSTDOUT:\n{}\nSTDERR:\n{}", status, stdout, stderr)
        }
        Err(e) => format!("ERROR: {}", e),
    }
}

pub async fn execute_apply_patch(path: &str, patch: &str, cwd: &str) -> String {
    let patch_file = Path::new(cwd).join(".tmp.patch");
    if let Err(e) = fs::write(&patch_file, patch) {
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
    let full_path = Path::new(cwd).join(path);
    
    // Ensure parent directory exists
    if let Some(parent) = full_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    match fs::write(&full_path, content) {
        Ok(_) => format!("Successfully wrote to {}", full_path.display()),
        Err(e) => format!("ERROR: Failed to write to {}: {}", full_path.display(), e),
    }
}

pub async fn execute_replace_text(path: &str, old_string: &str, new_string: &str, cwd: &str) -> String {
    let full_path = Path::new(cwd).join(path);
    match fs::read_to_string(&full_path) {
        Ok(content) => {
            let matches: Vec<_> = content.matches(old_string).collect();
            if matches.is_empty() {
                return format!("ERROR: old_string not found in {}", path);
            }
            if matches.len() > 1 {
                return format!("ERROR: old_string matches {} occurrences in {}. It must be unique.", matches.len(), path);
            }
            let new_content = content.replace(old_string, new_string);
            match fs::write(&full_path, new_content) {
                Ok(_) => format!("Successfully replaced text in {}", path),
                Err(e) => format!("ERROR: Failed to write to {}: {}", path, e),
            }
        }
        Err(e) => format!("ERROR: Failed to read file {}: {}", path, e),
    }
}

pub async fn execute_web_fetch(url: &str) -> String {
    let client = Client::new();
    match client.get(url).send().await {
        Ok(res) => {
            match res.text().await {
                Ok(text) => text,
                Err(e) => format!("ERROR: Failed to read response body: {}", e),
            }
        }
        Err(e) => format!("ERROR: Failed to fetch URL {}: {}", url, e),
    }
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
