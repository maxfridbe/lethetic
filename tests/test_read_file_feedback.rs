use lethetic::app::{App, BlockType};
use lethetic::config::Config;
use lethetic::icons;

#[test]
fn test_read_file_feedback_logic() {
    let config = Config {
        server_url: "http://brainiac-nvidia:7210/v1/responses".to_string(),
        model: "Gemma-4-26B-TurboQuant-262k".to_string(),
        context_size: 2048,
        tool_wrapper: None,
        enable_image_processing_tool: false,
    };
    let mut app = App::new(&config);
    
    let path = "test_file.rs";
    let content = "fn main() { println!(\"hello\"); }";
    
    // Simulate what happens in main.rs when read_file succeeds:
    // 1. Update latest file in context
    app.context_manager.update_latest_file(path.to_string(), content.to_string());
    
    // 2. Add the feedback segment (the part I added to main.rs)
    app.add_segment(format!("\n{} File `{}` has been placed in context.\n", icons::SUCCESS, path), BlockType::Text);
    
    // VERIFY
    
    // Check if context was updated
    assert!(app.context_manager.latest_files.contains_key(path));
    assert_eq!(app.context_manager.latest_files.get(path).unwrap().content, content);
    
    // Check if UI feedback block was added
    let feedback_block = app.blocks.iter().find(|b| b.content.contains("has been placed in context"));
    assert!(feedback_block.is_some(), "Feedback block should be present in app.blocks");
    assert!(feedback_block.unwrap().content.contains(path));
    assert_eq!(feedback_block.unwrap().block_type, BlockType::Text);
}
