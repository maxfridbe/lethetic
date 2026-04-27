use lethetic::tools::write_file;
use tokio_util::sync::CancellationToken;
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn test_write_file_directory_error() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let token = CancellationToken::new();
    
    // 1. Attempt to write to a path that is actually a directory
    let sub_dir = dir.path().join("sub_dir");
    fs::create_dir(&sub_dir).unwrap();
    
    let result = write_file::execute("sub_dir", "some content", cwd, token).await;
    
    // 2. We expect a specific error message
    assert!(result.contains("ERROR") && result.contains("is a directory"), "Should return helpful error when path is a directory. Got: {}", result);
}

#[tokio::test]
async fn test_write_file_success() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let token = CancellationToken::new();
    
    let result = write_file::execute("new_file.txt", "content", cwd, token).await;
    assert!(result.contains("Successfully wrote"));
    
    let content = fs::read_to_string(dir.path().join("new_file.txt")).unwrap();
    assert_eq!(content, "content");
}
