use lethetic::tools::apply_patch;
use tokio_util::sync::CancellationToken;
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn test_apply_patch_integration() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let file_path = "test.txt";
    let full_path = dir.path().join(file_path);
    
    // 1. Create a file to patch
    let original_content = "line 1\nline 2\nline 3\n";
    fs::write(&full_path, original_content).unwrap();
    
    // 2. Define a valid patch
    let patch = "--- test.txt\n+++ test.txt\n@@ -1,3 +1,3 @@\n line 1\n-line 2\n+line 2 modified\n line 3\n";
    
    let token = CancellationToken::new();
    
    // 3. Apply the patch
    let result = apply_patch::execute(file_path, patch, cwd, token.clone()).await;
    println!("Valid patch result: {}", result);
    
    let updated_content = fs::read_to_string(&full_path).unwrap();
    assert_eq!(updated_content, "line 1\nline 2 modified\nline 3\n");
    assert!(result.contains("STDOUT:"));
}

#[tokio::test]
async fn test_apply_patch_no_path_recovery() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let file_path = "recovered.txt";
    let full_path = dir.path().join(file_path);
    
    // 1. Create a file
    fs::write(&full_path, "original\n").unwrap();
    
    // 2. Patch WITHOUT path argument, but with correct header
    let patch = format!("--- {}\n+++ {}\n@@ -1,1 +1,1 @@\n-original\n+recovered\n", file_path, file_path);
    
    let token = CancellationToken::new();
    
    // 3. Apply the patch with empty path
    let result = apply_patch::execute("", &patch, cwd, token).await;
    println!("Recovery result: {}", result);
    
    let updated_content = fs::read_to_string(&full_path).unwrap();
    assert_eq!(updated_content, "recovered\n");
    assert!(result.contains("STDOUT:"));
}
