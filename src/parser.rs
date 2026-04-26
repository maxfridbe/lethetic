use crate::context::ToolCall;

pub fn find_tool_call(text: &str, is_final: bool) -> Option<Result<(ToolCall, usize), (String, usize)>> {
    let start_tokens = ["<|tool_call>", "<tool_call>"];
    
    let mut earliest_start = None;
    for &token in &start_tokens {
        if let Some(pos) = text.find(token) {
            match earliest_start {
                None => earliest_start = Some((pos, token)),
                Some((old_pos, _)) if pos < old_pos => earliest_start = Some((pos, token)),
                _ => {}
            }
        }
    }

    if let Some((pos, token)) = earliest_start {
        let after_start = &text[pos + token.len()..];
        
        let end_tokens = ["<|tool_call|>", "<tool_call|>"];
        let mut earliest_end = None;
        for &end_token in &end_tokens {
            if let Some(e_pos) = after_start.find(end_token) {
                match earliest_end {
                    None => earliest_end = Some((e_pos, end_token)),
                    Some((old_e_pos, _)) if e_pos < old_e_pos => earliest_end = Some((e_pos, end_token)),
                    _ => {}
                }
            }
        }

        if let Some((e_pos, _)) = earliest_end {
            let full_call_block = &after_start[..e_pos];
            match parse_native_block(full_call_block) {
                Ok(parsed) => return Some(Ok((parsed, pos))),
                Err(err) => return Some(Err((err, pos))),
            }
        } else if is_final {
             match parse_native_block(after_start) {
                 Ok(parsed) => return Some(Ok((parsed, pos))),
                 Err(err) => return Some(Err((err, pos))),
             }
        }
    }
    None
}

pub fn parse_native_block(block: &str) -> Result<ToolCall, String> {
    let call_pos = block.find("call:").ok_or("Missing 'call:' prefix")?;
    let call_content = &block[call_pos + 5..];

    let brace_start = call_content.find('{').ok_or("Missing '{' for arguments")?;
    let func_name = call_content[..brace_start].trim().to_string();
    let args_content = &call_content[brace_start..]; 

    // Character-level scan to find the closing brace.
    let mut brace_count = 0;
    let mut in_string = false;
    let mut marker_string_end: Option<&str> = None;
    let mut normal_string_char = ' ';
    let mut end_pos = None;

    let markers = [
        ("<|\"|>", ["<|\"|>", "\">", "|>"].as_slice()),
        ("<|\">", ["<|\">", "\">", "|>"].as_slice()),
        ("<|'|>", ["<|'|>", "'>", "|>"].as_slice()),
        ("<|'>", ["<|'>", "'>", "|>"].as_slice()),
        ("<|\"", ["<|\">", "\">", "|>"].as_slice()),
    ];

    let char_indices: Vec<(usize, char)> = args_content.char_indices().collect();
    let mut i = 0;
    while i < char_indices.len() {
        let (pos, c) = char_indices[i];
        let remaining = &args_content[pos..];

        if let Some(end_marker) = marker_string_end {
            if remaining.starts_with(end_marker) {
                marker_string_end = None;
                in_string = false;
                i += end_marker.chars().count();
                continue;
            }
            // Heuristic boundary check: if we see a comma followed by a likely key, we've probably leaked out of the string
            if is_at_key_boundary(remaining) {
                 marker_string_end = None;
                 in_string = false;
            }
        } else if in_string {
            if c == normal_string_char {
                let mut is_escaped = false;
                let mut k = i as i32 - 1;
                while k >= 0 && char_indices[k as usize].1 == '\\' {
                    is_escaped = !is_escaped;
                    k -= 1;
                }
                if !is_escaped {
                    in_string = false;
                }
            }
            // Also check for boundary leakage in normal strings (including backticks)
            if is_at_key_boundary(remaining) || (c == '`' && is_at_key_boundary(&remaining[1..])) {
                in_string = false;
            }
        } else {
            let mut found_marker = false;
            for (start, ends) in &markers {
                if remaining.starts_with(start) {
                    in_string = true;
                    marker_string_end = Some(ends[0]); 
                    i += start.chars().count();
                    found_marker = true;
                    break;
                }
            }
            if found_marker { continue; }

            if c == '"' || c == '\'' || c == '`' {
                in_string = true;
                normal_string_char = c;
            } else if c == '{' {
                brace_count += 1;
            } else if c == '}' {
                brace_count -= 1;
                if brace_count == 0 {
                    let byte_offset = char_indices.get(i + 1).map(|(p, _)| *p).unwrap_or(args_content.len());
                    end_pos = Some(byte_offset);
                    break;
                }
            }
        }
        i += 1;
    }

    if end_pos.is_none() {
        if let Some(last_brace) = args_content.rfind('}') {
            end_pos = Some(last_brace + 1);
        }
    }

    if end_pos.is_none() {
        return Err(format!("Missing closing '}}' for arguments. State: count={}, in_str={}, marker={:?}", brace_count, in_string, marker_string_end));
    }
    
    let args_raw = &args_content[..end_pos.unwrap()];
    let normalized = normalize_markers(args_raw);
    let fixed_json = fix_json_heuristically(&normalized);

    match serde_json::from_str::<serde_json::Value>(&fixed_json) {
        Ok(serde_json::Value::Object(args)) => {
            let tc_id = args.get("tool_call_id").and_then(|v| v.as_str())
                .or_else(|| args.get("id").and_then(|v| v.as_str()))
                .unwrap_or("raw_call").to_string();

            Ok(ToolCall {
                id: tc_id,
                function: crate::context::FunctionCall {
                    name: func_name,
                    arguments: serde_json::Value::Object(args),
                },
            })
        }
        Ok(_) => Err("Arguments must be a JSON object".to_string()),
        Err(e) => {
            Err(format!("JSON Error: {} (Fixed JSON: {})", e, fixed_json))
        }
    }
}

fn is_at_key_boundary(text: &str) -> bool {
    if !text.starts_with(',') { return false; }
    let after_comma = &text[1..];
    let trimmed = after_comma.trim_start();
    trimmed.starts_with("path:") || 
    trimmed.starts_with("description:") || 
    trimmed.starts_with("tool_call_id:") ||
    trimmed.starts_with("content:") ||
    trimmed.starts_with("command:")
}

fn normalize_markers(input: &str) -> String {
    let mut result = String::with_capacity(input.len() + 32);
    let char_indices: Vec<(usize, char)> = input.char_indices().collect();
    let mut i = 0;
    
    let markers = [
        ("<|\"|>", ["<|\"|>", "\">", "|>"].as_slice()),
        ("<|\">", ["<|\">", "\">", "|>"].as_slice()),
        ("<|'|>", ["<|'|>", "'>", "|>"].as_slice()),
        ("<|'>", ["<|'>", "'>", "|>"].as_slice()),
        ("<|\"", ["<|\">", "\">", "|>"].as_slice()),
    ];

    while i < char_indices.len() {
        let (pos, _c) = char_indices[i];
        let remaining = &input[pos..];
        
        let mut found_start = false;
        for (start, ends) in &markers {
            if remaining.starts_with(start) {
                result.push('"');
                i += start.chars().count();
                
                let mut matched_end: Option<&str> = None;
                while i < char_indices.len() {
                    let (c_pos, c) = char_indices[i];
                    let sub_remaining = &input[c_pos..];
                    
                    for &e in *ends {
                        if sub_remaining.starts_with(e) {
                            matched_end = Some(e);
                            break;
                        }
                    }
                    if matched_end.is_some() || is_at_key_boundary(sub_remaining) {
                        break;
                    }

                    if c == '"' { result.push_str("\\\""); }
                    else if c == '\\' { result.push_str("\\\\"); }
                    else { result.push(c); }
                    i += 1;
                }
                
                result.push('"');
                if let Some(e) = matched_end {
                    i += e.chars().count();
                }
                
                // Final artifact cleanup for this specific string
                if result.len() >= 2 && result.ends_with("`\"") {
                    result.pop(); // remove "
                    result.pop(); // remove `
                    result.push('"'); // add " back
                }
                
                found_start = true;
                break;
            }
        }
        
        if !found_start {
            if i < char_indices.len() {
                result.push(char_indices[i].1);
                i += 1;
            }
        }
    }
    result
}

fn fix_json_heuristically(input: &str) -> String {
    let mut output = String::with_capacity(input.len() + 32);
    let char_indices: Vec<(usize, char)> = input.char_indices().collect();
    let mut in_string = false;
    let mut string_quote = ' ';
    let mut i = 0;
    let mut expect_key = true;

    while i < char_indices.len() {
        let (pos, c) = char_indices[i];
        let remaining = &input[pos..];

        if in_string {
            if c == string_quote {
                let mut escape_count = 0;
                let mut k = i as i32 - 1;
                while k >= 0 && char_indices[k as usize].1 == '\\' {
                    escape_count += 1;
                    k -= 1;
                }
                if escape_count % 2 == 0 {
                    in_string = false;
                    output.push('"');
                } else {
                    output.push(c);
                }
            } else if is_at_key_boundary(remaining) || (c == '`' && is_at_key_boundary(&remaining[1..])) {
                output.push('"');
                in_string = false;
                // If it was a comma boundary, we'll process the comma in the next iteration (after loop i+=1)
                // but wait, we need to make sure we don't skip the comma if we just pushed the quote.
                // Actually, if we are at a boundary, we just closed the string.
                // We should NOT increment i here, let the loop do it? 
                // But wait, if c is the boundary character (comma or backtick), 
                // and we want it to be processed as a non-string character...
                
                if c == ',' {
                    // Stay on this character to let the non-string logic handle it
                    continue; 
                } else if c == '`' {
                    // Skip the backtick, it was an artifact
                    i += 1;
                    continue;
                }
            } else if c == '\n' { output.push_str("\\n"); }
            else if c == '\r' { output.push_str("\\r"); }
            else if (c as u32) < 32 { output.push_str(&format!("\\u{:04x}", c as u32)); }
            else if c == '"' && string_quote == '\'' { output.push_str("\\\""); }
            else { output.push(c); }
        } else {
            if c == '"' || c == '\'' || c == '`' {
                in_string = true;
                string_quote = c;
                output.push('"');
                expect_key = false;
            } else if c == '{' || c == ',' {
                output.push(c);
                expect_key = true;
            } else if expect_key && (c.is_alphabetic() || c == '_') {
                output.push('"');
                let mut key_idx = i;
                while key_idx < char_indices.len() && (char_indices[key_idx].1.is_alphanumeric() || char_indices[key_idx].1 == '_') {
                    output.push(char_indices[key_idx].1);
                    key_idx += 1;
                }
                output.push('"');
                i = key_idx;
                expect_key = false;
                continue;
            } else if !c.is_whitespace() && c != ':' {
                expect_key = false;
                output.push(c);
            } else {
                output.push(c);
            }
        }
        i += 1;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backtick_and_forgotten_marker_close() {
        let block = r#"call:write_file{content:<|">fn main() {}`, path: "test.rs", tool_call_id: "test"}"#;
        let result = parse_native_block(block).expect("Should recover via heuristic auto-close");
        assert_eq!(result.function.arguments.get("content").unwrap().as_str().unwrap(), "fn main() {}");
        assert_eq!(result.function.arguments.get("path").unwrap().as_str().unwrap(), "test.rs");
    }

    #[test]
    fn test_nested_quotes_with_markers() {
        let block = r#"call:write_file{content:<|">fn main() { println!("hello world"); }<|">, path: "test.rs", tool_call_id: "test"}"#;
        let result = parse_native_block(block).expect("Should handle nested quotes with markers");
        let content = result.function.arguments.get("content").unwrap().as_str().unwrap();
        assert_eq!(content, "fn main() { println!(\"hello world\"); }");
    }

    #[test]
    fn test_unquoted_keys() {
        let block = r#"call:test_tool{key1: "value1", key2: 123, tool_call_id: "test"}"#;
        let result = parse_native_block(block).unwrap();
        assert_eq!(result.function.arguments.get("key1").unwrap().as_str().unwrap(), "value1");
    }

    #[test]
    fn test_latest_run_real_world_failure() {
        // This is the exact string from the failed run. 
        // Note the <|"> start and the ` ,path boundary failure.
        let block = r#"call:write_file{content:<|">use raylib::prelude::*;

fn main() {
    println!("Hello World");
}
`,description: "Replace the spinning cube code with a bouncing cube implementation.",path: "src/main.rs",tool_call_id: "replace_with_bouncing_cube"}"#;

        let result = parse_native_block(block).expect("Should recover from real-world failure case");
        assert_eq!(result.id, "replace_with_bouncing_cube");
        let content = result.function.arguments.get("content").unwrap().as_str().unwrap();
        assert!(content.contains("println!(\"Hello World\")"));
        assert!(!content.contains("`,description:"));
    }
}
