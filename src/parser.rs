pub fn find_tool_call(text: &str, is_final: bool) -> Option<(crate::context::ToolCall, usize)> {
    let start_tokens = ["<|tool_call>", "<tool_call>"];
    
    for start_token in start_tokens {
        if let Some(pos) = text.find(start_token) {
            let after_token = &text[pos + start_token.len()..];
            
            let call_content_start = if let Some(c_pos) = after_token.find("call:") {
                &after_token[c_pos + 5..]
            } else {
                after_token
            };

            let brace_start = call_content_start.find('{');
            if let Some(brace_pos) = brace_start {
                let func_name = call_content_start[..brace_pos].trim().to_string();
                let args_content = &call_content_start[brace_pos + 1..];
                
                let explicit_end = args_content.find("<tool_call|>")
                    .or_else(|| args_content.find("<|tool_call|>"))
                    .or_else(|| args_content.find("<turn|>"))
                    .or_else(|| args_content.find("<|turn|>"));

                let end_pos = if let Some(p) = explicit_end {
                    p
                } else if is_final {
                    let mut last_brace = None;
                    let mut in_marker = false;
                    let mut current_marker = "";
                    
                    // Safe UTF-8 iteration
                    let char_indices: Vec<(usize, char)> = args_content.char_indices().collect();
                    let mut i = 0;
                    while i < char_indices.len() {
                        let (byte_pos, _) = char_indices[i];
                        let slice = &args_content[byte_pos..];
                        
                        if !in_marker {
                            if slice.starts_with("<|\"|>") {
                                in_marker = true;
                                current_marker = "<|\"|>";
                                // Skip next 4 chars (approx) - find index of the char that starts after marker
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
                    last_brace.unwrap_or(args_content.len())
                } else {
                    continue;
                };

                let args_part = &args_content[..end_pos];
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
                            if let Some(end_quote_pos) = after_sep[m_len..].find(marker) {
                                let val = &after_sep[m_len..m_len + end_quote_pos];
                                args.insert(key, serde_json::Value::String(val.to_string()));
                                current = &after_sep[m_len + end_quote_pos + m_len..];
                            } else {
                                if is_final {
                                    args.insert(key, serde_json::Value::String(after_sep[m_len..].to_string()));
                                }
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
                            for &m in &markers {
                                cleaned_val = cleaned_val.replace(m, "");
                            }
                            
                            if let Ok(n) = cleaned_val.parse::<i64>() {
                                args.insert(key, serde_json::Value::Number(n.into()));
                            } else {
                                args.insert(key, serde_json::Value::String(cleaned_val));
                            }
                            current = &after_sep[next_comma..];
                        }
                    } else { break; }
                }

                let tc_id = args.get("tool_call_id").and_then(|v| v.as_str())
                    .or_else(|| args.get("id").and_then(|v| v.as_str()))
                    .unwrap_or("raw_call").to_string();

                return Some((crate::context::ToolCall {
                    id: tc_id,
                    function: crate::context::FunctionCall {
                        name: func_name,
                        arguments: serde_json::Value::Object(args),
                    },
                }, pos));
            }
        }
    }
    None
}
