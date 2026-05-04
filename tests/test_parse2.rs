fn main() {
    let raw = r#"{"command": "echo 'Why did the programmer quit their job? Because they didn\'t get enough arrays!' > joke.txt"}"#;
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(_) => println!("Parsed successfully"),
        Err(e) => println!("Error: {}", e),
    }
}
