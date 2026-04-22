fn main() {
    let accumulated = r#"
{
  "tool_call_id": "call_001",
  "command": "echo 'Why did the programmer quit their job? Because they didn't get enough arrays!' > joke.txt"
}
"#;
    let json_str = accumulated.trim();
    let sanitized_json_str = json_str.replace("\\'", "'");
    match serde_json::from_str::<serde_json::Value>(&sanitized_json_str) {
        Ok(v) => println!("Parsed successfully: {}", v),
        Err(e) => println!("Error: {}", e),
    }
}
