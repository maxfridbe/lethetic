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

fn parse_native_block(block: &str) -> Result<crate::context::ToolCall, String> {
    let call_content = if let Some(c_pos) = block.find("call:") {
        &block[c_pos + 5..]
    } else {
        return Err("Missing 'call:' prefix".to_string());
    };

    let brace_start = call_content.find('{').ok_or("Missing '{' for arguments")?;
    let func_name = call_content[..brace_start].trim().to_string();
    let args_content = &call_content[brace_start + 1..];

    let mut last_brace = None;
    let mut in_marker = false;
    let mut current_marker = "";
    let char_indices: Vec<(usize, char)> = args_content.char_indices().collect();
    let mut i = 0;
    while i < char_indices.len() {
        let (byte_pos, _) = char_indices[i];
        let slice = &args_content[byte_pos..];
        
        if !in_marker {
            if slice.starts_with("<|\"|>") {
                in_marker = true;
                current_marker = "<|\"|>";
                let target = byte_pos + 5;
                while i < char_indices.len() && char_indices[i].0 < target { i += 1; }
                continue;
            } else if slice.starts_with("<|\">") {
                in_marker = true;
                current_marker = "<|\">";
                let target = byte_pos + 4;
                while i < char_indices.len() && char_indices[i].0 < target { i += 1; }
                continue;
            } else {
                if char_indices[i].1 == '}' {
                    last_brace = Some(byte_pos);
                }
            }
        } else {
            if slice.starts_with(current_marker) {
                in_marker = false;
                let target = byte_pos + current_marker.len();
                while i < char_indices.len() && char_indices[i].0 < target { i += 1; }
                continue;
            }
        }
        i += 1;
    }

    let end_brace = last_brace.ok_or("Missing closing '}' for arguments")?;
    let args_part = &args_content[..end_brace];
    
    let mut args = serde_json::Map::new();
    let mut current = args_part;
    
    while !current.is_empty() {
        current = current.trim_start_matches(|c| c == ',' || c == ' ' || c == '\n' || c == '{' || c == '}');
        if current.is_empty() { break; }

        if let Some(sep_pos) = current.find(':') {
            let mut key = current[..sep_pos].trim().to_string();
            key = key.trim_matches(|c| c == '"' || c == '\'').to_string();
            
            let after_sep = current[sep_pos + 1..].trim_start();
            let markers = ["<|\"|>", "<|\">", "<|'|>", "<|'>"];
            let mut found_marker = None;
            for &m in &markers {
                if after_sep.starts_with(m) {
                    found_marker = Some(m);
                    break;
                }
            }

            if let Some(marker) = found_marker {
                let m_len = marker.len();
                let mut end_quote_pos = after_sep[m_len..].find(marker);
                let mut actual_end_len = m_len;
                
                if end_quote_pos.is_none() {
                    let fallbacks = ["<|>", "<|", "|>"];
                    for &f in &fallbacks {
                        if let Some(p) = after_sep[m_len..].find(f) {
                            end_quote_pos = Some(p);
                            actual_end_len = f.len();
                            break;
                        }
                    }
                }

                if let Some(pos) = end_quote_pos {
                    let val = &after_sep[m_len..m_len + pos];
                    args.insert(key, serde_json::Value::String(val.to_string()));
                    current = &after_sep[m_len + pos + actual_end_len..];
                } else {
                    args.insert(key, serde_json::Value::String(after_sep[m_len..].to_string()));
                    break;
                }
            } else if after_sep.starts_with('"') {
                 if let Some(end_quote_pos) = after_sep[1..].find('"') {
                    let val = &after_sep[1..1 + end_quote_pos];
                    args.insert(key, serde_json::Value::String(val.to_string()));
                    current = &after_sep[1 + end_quote_pos + 1..];
                } else { break; }
            } else {
                let next_comma = after_sep.find(',').unwrap_or(after_sep.len());
                let val_str = after_sep[..next_comma].trim();
                let mut cleaned_val = val_str.to_string();
                for &m in &markers { cleaned_val = cleaned_val.replace(m, ""); }
                
                if let Ok(n) = cleaned_val.parse::<i64>() {
                    args.insert(key, serde_json::Value::Number(n.into()));
                } else {
                    args.insert(key, serde_json::Value::String(cleaned_val));
                }
                current = &after_sep[next_comma..];
            }
        } else { 
            return Err(format!("Malformed argument list around: '{}'", &current[..current.len().min(20)]));
        }
    }

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
