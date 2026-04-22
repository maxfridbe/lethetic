use tokio::process::Command;
use std::fs;

pub async fn execute_shell(command: &str) -> String {
    let output = Command::new("bash")
        .arg("-c")
        .arg(command)
        .output()
        .await;

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let status = out.status.code().map_or("signaled".to_string(), |c| c.to_string());
            format!("EXIT_CODE: {}\nSTDOUT:\n{}\nSTDERR:\n{}", status, stdout, stderr)
        }
        Err(e) => format!("ERROR: {}", e),
    }
}

pub async fn execute_read_file_lines(path: &str, start: usize, end: usize) -> String {
    match fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = start.saturating_sub(1);
            let end = end.min(lines.len());
            if start >= lines.len() || start > end {
                return format!("ERROR: Invalid line range {}-{} for file with {} lines", start + 1, end, lines.len());
            }
            lines[start..end].join("\n")
        }
        Err(e) => format!("ERROR: Failed to read file {}: {}", path, e),
    }
}

pub async fn execute_apply_patch(path: &str, patch: &str) -> String {
    let patch_file = ".tmp.patch";
    if let Err(e) = fs::write(patch_file, patch) {
        return format!("ERROR: Failed to write temp patch file: {}", e);
    }

    let output = Command::new("patch")
        .arg("-u")
        .arg(path)
        .arg("-i")
        .arg(patch_file)
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

pub async fn get_git_info() -> String {
    let status = Command::new("git")
        .arg("status")
        .arg("--short")
        .output()
        .await;

    match status {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() { "clean".to_string() } else { format!("dirty:\n{}", s) }
        }
        Err(_) => "not a git repo".to_string(),
    }
}
