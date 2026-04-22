fn main() {
    let text = r#"<run_shell_command command="echo 'hello' > jokes.txt" tool_call_id="call_001" />"#;
    let tool_names = ["run_shell_command", "read_file_lines", "apply_patch", "calculate"];
    
    for tool in tool_names {
        let tag = format!("<{} ", tool);
        if let Some(start_idx) = text.find(&tag) {
            let after_tag = &text[start_idx + tag.len()..];
            if let Some(end_idx) = after_tag.find("/>") {
                let attrs_str = &after_tag[..end_idx];
                println!("Found attrs for {}: {}", tool, attrs_str);
                
                // Extremely simple attribute parser
                let mut args = serde_json::Map::new();
                let mut chars = attrs_str.chars().peekable();
                
                while let Some(c) = chars.peek() {
                    if c.is_whitespace() {
                        chars.next();
                        continue;
                    }
                    
                    let mut key = String::new();
                    while let Some(&k) = chars.peek() {
                        if k == '=' || k.is_whitespace() { break; }
                        key.push(k);
                        chars.next();
                    }
                    
                    // skip whitespace and '='
                    while let Some(&k) = chars.peek() {
                        if k == '=' || k.is_whitespace() { chars.next(); }
                        else { break; }
                    }
                    
                    // Read value
                    if let Some(&quote) = chars.peek() {
                        if quote == '"' || quote == '\'' {
                            chars.next(); // consume quote
                            let mut val = String::new();
                            while let Some(&v) = chars.peek() {
                                if v == quote { 
                                    chars.next(); // consume closing quote
                                    break; 
                                }
                                val.push(v);
                                chars.next();
                            }
                            args.insert(key, serde_json::Value::String(val));
                        }
                    }
                }
                println!("Args JSON: {}", serde_json::Value::Object(args));
            }
        }
    }
}
