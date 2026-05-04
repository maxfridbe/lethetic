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
    let old_code = "line 2\nline 3";
    let new_code = "line 2 modified\nline 3";
    
    let token = CancellationToken::new();
    
    // 3. Apply the patch
    let result = apply_patch::execute(file_path, old_code, new_code, cwd, token.clone()).await;
    println!("Valid patch result: {}", result);
    
    let updated_content = fs::read_to_string(&full_path).unwrap();
    assert_eq!(updated_content, "line 1\nline 2 modified\nline 3\n");
    assert!(result.contains("Successfully patched"));
}

#[tokio::test]
async fn test_apply_patch_with_line_numbers() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    let file_path = "test_numbered.txt";
    let full_path = dir.path().join(file_path);
    
    // 1. Create a file
    fs::write(&full_path, "original\nline2\n").unwrap();
    
    // 2. Patch with read_file style line numbers
    let old_code = "     1\toriginal\n     2\tline2";
    let new_code = "     1\trecovered\n     2\tline2";
    
    let token = CancellationToken::new();
    
    // 3. Apply the patch
    let result = apply_patch::execute(file_path, old_code, new_code, cwd, token).await;
    println!("Recovery result: {}", result);
    
    let updated_content = fs::read_to_string(&full_path).unwrap();
    assert_eq!(updated_content, "recovered\nline2\n");
    assert!(result.contains("Successfully patched"));
}
