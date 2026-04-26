use regex::Regex;
use once_cell::sync::Lazy;

static KEY_FIXER: Lazy<Regex> = Lazy::new(|| Regex::new(r"([{,]\s*)([a-zA-Z_][a-zA-Z0-9_]*)(\s*:)").unwrap());

pub fn find_tool_call(text: &str, is_final: bool) -> Option<Result<(crate::context::ToolCall, usize), (String, usize)>> {
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

pub fn parse_native_block(block: &str) -> Result<crate::context::ToolCall, String> {
    // 1. Normalize all specialized string markers to standard quotes.
    // This handles cases where the model mixes " with <|"|> or uses markers as delimiters.
    let mut normalized = block.to_string();
    let markers = [
        ("<|\"|>", "\""),
        ("<|\">", "\""),
        ("<|'|>", "'"),
        ("<|'>", "'"),
        ("<|\"", "\""),
        ("\">", "\""),
    ];
    for (m, r) in &markers {
        normalized = normalized.replace(m, r);
    }

    let call_content = if let Some(c_pos) = normalized.find("call:") {
        &normalized[c_pos + 5..]
    } else {
        return Err("Missing 'call:' prefix".to_string());
    };

    let brace_start = call_content.find('{').ok_or("Missing '{' for arguments")?;
    let func_name = call_content[..brace_start].trim().to_string();
    let args_content = &call_content[brace_start..]; 

    // 2. Simple character-level scan on normalized string to find the closing brace.
    let mut brace_count = 0;
    let mut in_string = false;
    let mut string_char = ' ';
    let mut end_pos = None;

    let chars: Vec<char> = args_content.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            if c == string_char {
                // Check for escape (only handles single backslash)
                let mut is_escaped = false;
                let mut k = i as i32 - 1;
                while k >= 0 && chars[k as usize] == '\\' {
                    is_escaped = !is_escaped;
                    k -= 1;
                }
                if !is_escaped {
                    in_string = false;
                }
            }
        } else {
            if c == '"' || c == '\'' {
                in_string = true;
                string_char = c;
            } else if c == '{' {
                brace_count += 1;
            } else if c == '}' {
                brace_count -= 1;
                if brace_count == 0 {
                    // Find byte offset for slicing
                    let byte_offset = args_content.char_indices().nth(i + 1).map(|(pos, _)| pos).unwrap_or(args_content.len());
                    end_pos = Some(byte_offset);
                    break;
                }
            }
        }
    }

    if end_pos.is_none() {
        return Err(format!("Missing closing '}}' for arguments. State: count={}, in_str={}", brace_count, in_string));
    }
    
    let args_json_raw = &args_content[..end_pos.unwrap()];

    // 3. Fix unquoted keys if necessary
    let fixed_json = KEY_FIXER.replace_all(args_json_raw, "$1\"$2\"$3").to_string();

    match serde_json::from_str::<serde_json::Value>(&fixed_json) {
        Ok(serde_json::Value::Object(args)) => {
            let tc_id = args.get("tool_call_id").and_then(|v| v.as_str())
                .or_else(|| args.get("id").and_then(|v| v.as_str()))
                .unwrap_or("raw_call").to_string();

            Ok(crate::context::ToolCall {
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
