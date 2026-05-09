use std::fs;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use lethetic::tools::{calculate, read_folder, search_text, read_file_lines, replace_text, run_shell_command};
use tokio::sync::mpsc;

#[tokio::test]
async fn test_calculate_tool_success() {
    let result = calculate::execute("2 + 2 * 3").await;
    assert!(result.contains("8"));
}

#[tokio::test]
async fn test_calculate_tool_error() {
    let result = calculate::execute("2 + 2 *").await;
    assert!(result.contains("error") || result.contains("Error") || result.contains("failed") || result.contains("Unexpected end"));
}

#[tokio::test]
async fn test_read_folder_tool() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    
    fs::write(dir.path().join("file1.txt"), "hello").unwrap();
    fs::write(dir.path().join("file2.txt"), "world").unwrap();
    fs::create_dir(dir.path().join("subdir")).unwrap();

    let token = CancellationToken::new();
    let result = read_folder::execute(".", cwd, token).await;
    
    assert!(result.contains("file1.txt"));
    assert!(result.contains("file2.txt"));
    assert!(result.contains("subdir"));
}

#[tokio::test]
async fn test_search_text_tool() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    
    fs::write(dir.path().join("test_file.txt"), "apple\nbanana\ncherry\ndate").unwrap();

    let token = CancellationToken::new();
    let result = search_text::execute("banana", "test_file.txt", cwd, token).await;
    
    assert!(result.contains("banana"));
    assert!(!result.contains("apple"));
}

#[tokio::test]
async fn test_read_file_lines_tool() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    
    let content = "line 1\nline 2\nline 3\nline 4\nline 5\n";
    let file_path = "test_lines.txt";
    fs::write(dir.path().join(file_path), content).unwrap();

    let token = CancellationToken::new();
    let result = read_file_lines::execute(file_path, 2, 4, cwd, token).await;
    
    assert!(result.contains("line 2"));
    assert!(result.contains("line 4"));
    assert!(!result.contains("line 1"));
    assert!(!result.contains("line 5"));
}

#[tokio::test]
async fn test_read_file_lines_out_of_bounds() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    
    let content = "line 1\nline 2\n";
    let file_path = "test_short.txt";
    fs::write(dir.path().join(file_path), content).unwrap();

    let token = CancellationToken::new();
    let result = read_file_lines::execute(file_path, 1, 10, cwd, token).await;
    
    assert!(result.contains("line 1"));
    assert!(result.contains("line 2"));
}

#[tokio::test]
async fn test_replace_text_tool_success() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    
    let file_path = "test_replace.txt";
    fs::write(dir.path().join(file_path), "hello world\ngoodbye world").unwrap();

    let token = CancellationToken::new();
    let result = replace_text::execute(file_path, "goodbye world", "hello moon", false, cwd, token).await;
    
    assert!(result.contains("Successfully replaced"));
    let updated = fs::read_to_string(dir.path().join(file_path)).unwrap();
    assert_eq!(updated, "hello world\nhello moon");
}

#[tokio::test]
async fn test_replace_text_tool_multiple_occurrences() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    
    let file_path = "test_multi.txt";
    fs::write(dir.path().join(file_path), "apple\nbanana\napple\n").unwrap();

    let token = CancellationToken::new();
    let result = replace_text::execute(file_path, "apple", "orange", false, cwd, token).await;
    
    assert!(result.contains("ERROR") || result.contains("Error") || result.contains("matches"));
}

#[tokio::test]
async fn test_run_shell_command_success() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    
    let token = CancellationToken::new();
    let (tx, _rx) = mpsc::unbounded_channel();
    
    let (output, _result) = run_shell_command::execute("echo 'hello shell'", cwd, token, tx).await;
    
    assert!(output.contains("hello shell"));
    assert!(output.contains("EXIT_CODE: 0"));
}

#[tokio::test]
async fn test_run_shell_command_failure() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    
    let token = CancellationToken::new();
    let (tx, _rx) = mpsc::unbounded_channel();
    
    let (output, _result) = run_shell_command::execute("nonexistent_command_12345", cwd, token, tx).await;
    
    assert!(!output.contains("EXIT_CODE: 0"));
    assert!(output.contains("EXIT_CODE: 127") || output.contains("not found"));
}
