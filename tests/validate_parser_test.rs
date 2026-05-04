use lethetic::parser;
use std::fs;
use serde_json::Value;

fn main() {
    let ui_state_str = fs::read_to_string("./bob/.lethetic/sessions/session_20260423_213545/ui_state.json").unwrap_or_default();
    if ui_state_str.is_empty() {
        println!("No session found at the hardcoded path.");
        return;
    }
    let ui_state: Value = serde_json::from_str(&ui_state_str).unwrap();
    
    if let Value::Array(blocks) = ui_state {
        for (i, block) in blocks.iter().enumerate() {
            if block["block_type"] == "Formulating" {
                let content = block["content"].as_str().unwrap();
                println!("Block {}:", i);
                println!("Raw content: {:?}", content);
                let res = parser::find_tool_call(&format!("<|tool_call>{}<tool_call|>", content), true);
                println!("Parse result: {:?}\n", res);
            }
        }
    }
}
