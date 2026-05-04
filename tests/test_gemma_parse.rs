fn parse_gemma_native_tool_call(text: &str) -> Option<(String, String, serde_json::Value)> {
    if let Some(start_idx) = text.rfind("call:") {
        let after_call = &text[start_idx + 5..];
        if let Some(brace_idx) = after_call.find('{') {
            let func_name = after_call[..brace_idx].trim().to_string();
            if let Some(end_brace_idx) = after_call.rfind('}') {
                let attrs_str = &after_call[brace_idx + 1..end_brace_idx];
                let mut args = serde_json::Map::new();
                
                let mut chars = attrs_str.chars().peekable();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == ',' { chars.next(); continue; }
                    let mut key = String::new();
                    while let Some(&k) = chars.peek() {
                        if k == ':' || k.is_whitespace() || k == '=' { break; }
                        key.push(k);
                        chars.next();
                    }
                    if key.is_empty() { break; }
                    while let Some(&k) = chars.peek() {
                        if k == ':' || k.is_whitespace() || k == '=' { chars.next(); } else { break; }
                    }
                    
                    let mut val = String::new();
                    let chars_count = chars.clone().count();
                    let rest = if chars_count <= attrs_str.len() { &attrs_str[attrs_str.len() - chars_count..] } else { "" };
                    
                    if rest.starts_with("<|\"|>") {
                        for _ in 0..5 { chars.next(); }
                        while let Some(&v) = chars.peek() {
                            let r_count = chars.clone().count();
                            let rest_inner = if r_count <= attrs_str.len() { &attrs_str[attrs_str.len() - r_count..] } else { "" };
                            if rest_inner.starts_with("<|\"|>") {
                                for _ in 0..5 { chars.next(); }
                                break;
                            }
                            val.push(v);
                            chars.next();
                        }
                    } else if let Some(&quote) = chars.peek() {
                        if quote == '"' || quote == '\'' {
                            chars.next();
                            let mut escaped = false;
                            while let Some(&v) = chars.peek() {
                                chars.next();
                                if escaped {
                                    val.push(v);
                                    escaped = false;
                                } else if v == '\\' {
                                    escaped = true;
                                } else if v == quote {
                                    break;
                                } else {
                                    val.push(v);
                                }
                            }
                        } else {
                            while let Some(&v) = chars.peek() {
                                if v == ',' || v.is_whitespace() { break; }
                                val.push(v);
                                chars.next();
                            }
                        }
                    } else {
                         while let Some(&v) = chars.peek() {
                                if v == ',' || v.is_whitespace() { break; }
                                val.push(v);
                                chars.next();
                         }
                    }
                    args.insert(key, serde_json::Value::String(val));
                }
                let tc_id = args.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("raw_call").to_string();
                return Some((func_name, tc_id, serde_json::Value::Object(args)));
            }
        }
    }
    None
}
fn main() {
    let text = "<tool_call|><|tool_response><|tool_response>call:run_shell_command{command:<|\"|>chmod +x tellrandom.sh<|\"|>,tool_call_id:<|\"|>chmod_script_002<|\"|>}<tool_call|><|tool_response>";
    println!("{:?}", parse_gemma_native_tool_call(text));
}
