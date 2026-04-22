pub fn parse_json_tool_call(text: &str) -> Option<(String, String, serde_json::Value)> {
    let wrapper_tags = ["<tool_call>", "<|tool_call|>"];
    
    let mut last_tag_pos = None;
    for tag in wrapper_tags {
        if let Some(pos) = text.find(tag) {
            if last_tag_pos.map_or(true, |(p, _)| pos < p) {
                last_tag_pos = Some((pos, tag));
            }
        }
    }
    
    if let Some((pos, _)) = last_tag_pos {
        let after_tag = &text[pos..];
        if let Some(start) = after_tag.find('{') {
            if let Some(end) = after_tag.rfind('}') {
                let json_str = &after_tag[start..=end];
                let sanitized_json_str = json_str.replace("\\'", "'");
                if let Ok(tc_val) = serde_json::from_str::<serde_json::Value>(&sanitized_json_str) {
                    let mut func_name_opt = tc_val["name"].as_str().or_else(|| tc_val["function"]["name"].as_str()).map(|s| s.to_string());
                    
                    if func_name_opt.is_none() {
                        if tc_val["command"].is_string() {
                            func_name_opt = Some("run_shell_command".to_string());
                        } else if tc_val["patch"].is_string() {
                            func_name_opt = Some("apply_patch".to_string());
                        } else if tc_val["path"].is_string() {
                            func_name_opt = Some("read_file_lines".to_string());
                        } else if tc_val["expression"].is_string() {
                            func_name_opt = Some("calculate".to_string());
                        }
                    }
                    
                    if let Some(func_name) = func_name_opt {
                        let args = if tc_val["arguments"].is_object() { tc_val["arguments"].clone() } else { tc_val.clone() };
                        let tc_id = args["tool_call_id"].as_str()
                            .or_else(|| tc_val["tool_call_id"].as_str())
                            .or_else(|| tc_val["id"].as_str())
                            .unwrap_or("raw_call").to_string();
                        return Some((func_name, tc_id, args));
                    }
                }
            }
        }
    }
    None
}

pub fn find_tool_call(text: &str) -> Option<(crate::context::ToolCall, usize)> {
    let mut best_match: Option<((String, String, serde_json::Value), usize)> = None;

    // JSON Parser
    if let Some(pos) = ["<tool_call>", "<|tool_call|>"].iter().filter_map(|t| text.find(t)).min() {
        if let Some(parsed) = parse_json_tool_call(text) {
            best_match = Some((parsed, pos));
        }
    }

    // XML Parser
    let xml_pos = ["run_shell_command", "read_file_lines", "apply_patch", "calculate"]
        .iter().filter_map(|t| text.find(&format!("<{} ", t))).min();
    if let Some(pos) = xml_pos {
        if let Some(parsed) = parse_xml_tool_call(text) {
            if best_match.as_ref().map_or(true, |(_, p)| pos < *p) {
                best_match = Some((parsed, pos));
            }
        }
    }

    // Native Parser
    if let Some(pos) = text.find("call:") {
        if let Some(parsed) = parse_gemma_native_tool_call(text) {
            if best_match.as_ref().map_or(true, |(_, p)| pos < *p) {
                best_match = Some((parsed, pos));
            }
        }
    }

    // Fallback JSON check
    if let Some(pos) = text.find('{') {
        if let Some(end_pos) = text[pos..].find('}') {
            let json_str = &text[pos..pos+end_pos+1];
            let sanitized = json_str.replace("\\'", "'");
            if let Ok(tc_val) = serde_json::from_str::<serde_json::Value>(&sanitized) {
                let mut func_name_opt = tc_val["name"].as_str().or_else(|| tc_val["function"]["name"].as_str()).map(|s| s.to_string());
                if func_name_opt.is_none() {
                    if tc_val["command"].is_string() { func_name_opt = Some("run_shell_command".to_string()); }
                    else if tc_val["patch"].is_string() { func_name_opt = Some("apply_patch".to_string()); }
                }
                if let Some(func_name) = func_name_opt {
                    if best_match.as_ref().map_or(true, |(_, p)| pos < *p) {
                        let tc_id = args_id(&tc_val).unwrap_or("raw_call").to_string();
                        let args = if tc_val["arguments"].is_object() { tc_val["arguments"].clone() } else { tc_val.clone() };
                        best_match = Some(((func_name, tc_id, args), pos));
                    }
                }
            }
        }
    }

    best_match.map(|((name, id, args), pos)| {
        (crate::context::ToolCall {
            id,
            tool_type: Some("function".to_string()),
            function: crate::context::FunctionCall {
                name,
                arguments: args,
            },
            index: None,
        }, pos)
    })
}

fn args_id(val: &serde_json::Value) -> Option<&str> {
    val["tool_call_id"].as_str()
        .or_else(|| val["id"].as_str())
        .or_else(|| val["arguments"]["tool_call_id"].as_str())
}

pub fn parse_xml_tool_call(text: &str) -> Option<(String, String, serde_json::Value)> {
    let tool_names = ["run_shell_command", "read_file_lines", "apply_patch", "calculate"];
    for tool in tool_names {
        let start_tag = format!("<{} ", tool);
        if let Some(start_idx) = text.find(&start_tag) {
            let after_tag = &text[start_idx + start_tag.len()..];
            if let Some(end_idx) = after_tag.find("/>").or_else(|| after_tag.find(">")) {
                let attrs_str = &after_tag[..end_idx];
                
                let mut args = serde_json::Map::new();
                let mut chars = attrs_str.chars().peekable();
                
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() { chars.next(); continue; }
                    
                    let mut key = String::new();
                    while let Some(&k) = chars.peek() {
                        if k == '=' || k.is_whitespace() { break; }
                        key.push(k);
                        chars.next();
                    }
                    if key.is_empty() { break; }
                    
                    while let Some(&k) = chars.peek() {
                        if k == '=' || k.is_whitespace() { chars.next(); } else { break; }
                    }
                    
                    if let Some(&quote) = chars.peek() {
                        if quote == '"' || quote == '\'' {
                            chars.next();
                            let mut val = String::new();
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
                            args.insert(key.clone(), serde_json::Value::String(val));
                        } else {
                            let mut val = String::new();
                            while let Some(&v) = chars.peek() {
                                if v.is_whitespace() || v == '/' || v == '>' { break; }
                                val.push(v);
                                chars.next();
                            }
                            args.insert(key.clone(), serde_json::Value::String(val));
                        }
                    }
                }
                let tc_id = args.get("tool_call_id").and_then(|v| v.as_str())
                    .or_else(|| args.get("id").and_then(|v| v.as_str()))
                    .unwrap_or("raw_call").to_string();
                return Some((tool.to_string(), tc_id, serde_json::Value::Object(args)));
            }
        }
    }
    None
}

pub fn parse_gemma_native_tool_call(text: &str) -> Option<(String, String, serde_json::Value)> {
    if let Some(start_idx) = text.find("call:") {
        let after_call = &text[start_idx + 5..];
        if let Some(brace_idx) = after_call.find('{') {
            let func_name = after_call[..brace_idx].trim().to_string();
            if let Some(end_brace_idx) = after_call.find('}') {
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
                let tc_id = args.get("tool_call_id").and_then(|v| v.as_str())
                    .or_else(|| args.get("id").and_then(|v| v.as_str()))
                    .unwrap_or("raw_call").to_string();
                return Some((func_name, tc_id, serde_json::Value::Object(args)));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gemma_native_format() {
        let text = "<tool_call|><|tool_response><|tool_response>call:run_shell_command{command:<|\"|>chmod +x tellrandom.sh<|\"|>,tool_call_id:<|\"|>chmod_script_002<|\"|>}<tool_call|><|tool_response>";
        let parsed = parse_gemma_native_tool_call(text);
        assert!(parsed.is_some(), "Should successfully parse native Gemma format");

        if let Some((func_name, tc_id, args)) = parsed {
            assert_eq!(func_name, "run_shell_command");
            assert_eq!(tc_id, "chmod_script_002");
            assert_eq!(args["command"].as_str().unwrap(), "chmod +x tellrandom.sh");
        }
    }

    #[test]
    fn test_fail_safe_parsing_from_text() {
        let full_response_content = "<tool_call>\n{\n  \"tool_call_id\": \"plan_001\",\n  \"command\": \"echo 'Why did the programmer quit their job? Because they didn\\'t get enough arrays!' > joke.txt\"\n}\n</tool_call>\n";
        let (tc, _) = find_tool_call(full_response_content).expect("Failed to parse tool call");
        assert_eq!(tc.function.name, "run_shell_command");
        assert_eq!(tc.id, "plan_001");
    }

    #[test]
    fn test_escaped_single_quote_json() {
        let raw = r#"{"command": "echo 'Why did the programmer quit their job? Because they didn\'t get enough arrays!' > joke.txt"}"#;
        let parsed = serde_json::from_str::<serde_json::Value>(raw);
        assert!(parsed.is_err(), "serde_json should fail on \\'");
        
        let sanitized = raw.replace("\\'", "'");
        let parsed2 = serde_json::from_str::<serde_json::Value>(&sanitized);
        assert!(parsed2.is_ok(), "serde_json should succeed on unescaped '");
    }
}
