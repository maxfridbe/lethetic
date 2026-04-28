use crate::context::ToolCall;
use serde_json::{Value, Map};

fn skip_ws(input: &str, mut pos: usize) -> usize {
    while pos < input.len() && input[pos..].starts_with(|c: char| c.is_whitespace()) {
        pos += input[pos..].chars().next().unwrap().len_utf8();
    }
    pos
}

fn parse_gemma4_value(input: &str, mut pos: usize) -> Result<(Value, usize), String> {
    pos = skip_ws(input, pos);
    
    // Sometimes the model outputs a comma right after the colon, e.g. `content:,<|">...`
    if input[pos..].starts_with(',') {
        pos += 1;
        pos = skip_ws(input, pos);
    }
    
    let markers = ["<|\"|>", "<|\">", "<|'|>", "<|'>", "<|\""];
    for marker in markers {
        if input[pos..].starts_with(marker) {
            let start = pos + marker.len();
            // We'll look for standard closing markers. To avoid matching `">` inside C# or bash code,
            // we restrict the end markers to ones that are definitely markers.
            let end_markers = ["<|\"|>", "<|\">", "<|'>", "<|'|>", "`>"];
            let mut end_offset_opt = None;
            let mut matched_end = "";
            for end_marker in end_markers {
                if let Some(offset) = input[start..].find(end_marker) {
                    if end_offset_opt.map_or(true, |existing| offset < existing) {
                        end_offset_opt = Some(offset);
                        matched_end = end_marker;
                    }
                }
            }
            if let Some(end_offset) = end_offset_opt {
                let end = start + end_offset;
                let s = &input[start..end];
                return Ok((Value::String(s.to_string()), end + matched_end.len()));
            } else {
                // If missing end marker, just take the rest of the string, but stop at the next key if there is one.
                // For simplicity, we'll try to find a key boundary, or just take the rest.
                let mut end = start;
                while end < input.len() {
                    if input[end..].starts_with(',') {
                        let after_comma = end + 1;
                        let mut key_end = skip_ws(input, after_comma);
                        // Skip optional quotes
                        if key_end < input.len() && (input[key_end..].starts_with('"') || input[key_end..].starts_with('\'') || input[key_end..].starts_with('`')) {
                            key_end += 1;
                        }
                        let key_start = key_end;
                        while key_end < input.len() && (input[key_end..].chars().next().unwrap().is_alphanumeric() || input[key_end..].starts_with('_')) {
                            key_end += input[key_end..].chars().next().unwrap().len_utf8();
                        }
                        if key_start != key_end {
                            let key = &input[key_start..key_end];
                            let known_keys = [
                                "path", "description", "tool_call_id", "content", "command", "id", 
                                "expression", "url", "patch", "old_string", "new_string", "start_line", 
                                "end_line", "pattern", "dir_path", "include_pattern", "exclude_pattern",
                                "case_sensitive", "context", "before", "after", "fixed_strings",
                                "total_max_matches", "names_only", "no_ignore", "max_matches_per_file",
                                "respect_git_ignore", "respect_gemini_ignore", "ignore", "allow_multiple",
                                "is_background", "wait_for_previous", "pid", "delay_ms", "lines",
                                "query", "questions", "reason", "agent_name", "prompt", "name", "fact", "scope",
                                "args", "type", "properties", "required", "location", "units"
                            ];
                            let mut after_key = key_end;
                            if after_key < input.len() && (input[after_key..].starts_with('"') || input[after_key..].starts_with('\'') || input[after_key..].starts_with('`')) {
                                after_key += 1;
                            }
                            after_key = skip_ws(input, after_key);
                            if after_key < input.len() && input[after_key..].starts_with(':') && known_keys.contains(&key) {
                                break;
                            }
                        }
                    } else if input[end..].starts_with('}') || input[end..].starts_with(']') {
                        // Check if it's actually the end of the JSON object/array
                        let next = skip_ws(input, end + 1);
                        if next == input.len() || input[next..].starts_with('}') || input[next..].starts_with(']') || input[next..].starts_with('<') {
                            break;
                        }
                    }
                    end += input[end..].chars().next().unwrap().len_utf8();
                }
                let mut s = input[start..end].to_string();
                if s.ends_with('`') { s.pop(); }
                return Ok((Value::String(s), end));
            }
        }
    }
    
    if input[pos..].starts_with('"') {
        let start = pos + 1;
        let mut end = start;
        let mut escaped = false;
        while end < input.len() {
            if escaped {
                escaped = false;
            } else if input[end..].starts_with('\\') {
                escaped = true;
            } else if input[end..].starts_with('"') {
                break;
            }
            end += input[end..].chars().next().unwrap().len_utf8();
        }
        if end < input.len() {
            if let Ok(v) = serde_json::from_str::<Value>(&input[pos..=end]) {
                return Ok((v, end + 1));
            } else {
                return Ok((Value::String(input[start..end].to_string()), end + 1));
            }
        } else {
             return Ok((Value::String(input[start..].to_string()), input.len()));
        }
    }

    if input[pos..].starts_with('{') {
        return parse_gemma4_dict(input, pos);
    } else if input[pos..].starts_with('[') {
        return parse_gemma4_array(input, pos);
    } else if input[pos..].starts_with("true") {
        return Ok((Value::Bool(true), pos + 4));
    } else if input[pos..].starts_with("false") {
        return Ok((Value::Bool(false), pos + 5));
    } else if input[pos..].starts_with("null") {
        return Ok((Value::Null, pos + 4));
    } else {
        let mut end = pos;
        while end < input.len() && (input[end..].starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E')) {
            end += 1;
        }
        if end > pos {
            if let Ok(num) = serde_json::from_str::<serde_json::Number>(&input[pos..end]) {
                return Ok((Value::Number(num), end));
            }
        }

        let mut end = pos;
        while end < input.len() {
            if input[end..].starts_with(',') {
                let after_comma = end + 1;
                let mut key_end = skip_ws(input, after_comma);
                if key_end < input.len() && (input[key_end..].starts_with('"') || input[key_end..].starts_with('\'') || input[key_end..].starts_with('`')) {
                    key_end += 1;
                }
                let key_start = key_end;
                while key_end < input.len() && (input[key_end..].chars().next().unwrap().is_alphanumeric() || input[key_end..].starts_with('_')) {
                    key_end += input[key_end..].chars().next().unwrap().len_utf8();
                }
                if key_start != key_end {
                    let key = &input[key_start..key_end];
                    let known_keys = [
                        "path", "description", "tool_call_id", "content", "command", "id", 
                        "expression", "url", "patch", "old_string", "new_string", "start_line", 
                        "end_line", "pattern", "dir_path", "include_pattern", "exclude_pattern",
                        "case_sensitive", "context", "before", "after", "fixed_strings",
                        "total_max_matches", "names_only", "no_ignore", "max_matches_per_file",
                        "respect_git_ignore", "respect_gemini_ignore", "ignore", "allow_multiple",
                        "is_background", "wait_for_previous", "pid", "delay_ms", "lines",
                        "query", "questions", "reason", "agent_name", "prompt", "name", "fact", "scope",
                        "args", "type", "properties", "required", "location", "units"
                    ];
                    let mut after_key = key_end;
                    if after_key < input.len() && (input[after_key..].starts_with('"') || input[after_key..].starts_with('\'') || input[after_key..].starts_with('`')) {
                        after_key += 1;
                    }
                    after_key = skip_ws(input, after_key);
                    if after_key < input.len() && input[after_key..].starts_with(':') && known_keys.contains(&key) {
                        break;
                    }
                }
            } else if input[end..].starts_with('}') || input[end..].starts_with(']') {
                let next = skip_ws(input, end + 1);
                if next == input.len() || input[next..].starts_with('}') || input[next..].starts_with(']') || input[next..].starts_with('<') {
                    break;
                }
            }
            end += input[end..].chars().next().unwrap().len_utf8();
        }
        
        let mut s = input[pos..end].trim_end().to_string();
        s = if s.starts_with(',') { s[1..].trim_start().to_string() } else { s };
        return Ok((Value::String(s), end));
    }
}

fn parse_gemma4_dict(input: &str, mut pos: usize) -> Result<(Value, usize), String> {
    pos = skip_ws(input, pos);
    if !input[pos..].starts_with('{') {
        return Err("Expected {".into());
    }
    pos += 1;
    let mut map = Map::new();
    
    loop {
        pos = skip_ws(input, pos);
        if input[pos..].starts_with('}') {
            pos += 1;
            break;
        }

        if input[pos..].starts_with(',') {
            pos += 1;
            pos = skip_ws(input, pos);
        }

        if input[pos..].starts_with('}') {
            pos += 1;
            break;
        }

        let mut key_end = pos;
        while key_end < input.len() && !input[key_end..].starts_with(':') && !input[key_end..].starts_with('}') {
            key_end += input[key_end..].chars().next().unwrap().len_utf8();
        }
        if key_end == input.len() || input[key_end..].starts_with('}') {
            break;
        }
        
        let key_str = input[pos..key_end].trim();
        let key = if key_str.starts_with('"') && key_str.ends_with('"') && key_str.len() >= 2 {
            key_str[1..key_str.len()-1].to_string()
        } else if key_str.starts_with('\'') && key_str.ends_with('\'') && key_str.len() >= 2 {
            key_str[1..key_str.len()-1].to_string()
        } else {
            key_str.to_string()
        };

        pos = key_end + 1; // skip ':'
        
        match parse_gemma4_value(input, pos) {
            Ok((val, next_pos)) => {
                map.insert(key, val);
                pos = next_pos;
            }
            Err(e) => {
                return Err(format!("Error parsing value for {}: {}", key, e));
            }
        }
    }
    Ok((Value::Object(map), pos))
}

fn parse_gemma4_array(input: &str, mut pos: usize) -> Result<(Value, usize), String> {
    pos = skip_ws(input, pos);
    if !input[pos..].starts_with('[') {
        return Err("Expected [".into());
    }
    pos += 1;
    let mut arr = Vec::new();
    
    loop {
        pos = skip_ws(input, pos);
        if input[pos..].starts_with(']') {
            pos += 1;
            break;
        }

        if input[pos..].starts_with(',') {
            pos += 1;
            pos = skip_ws(input, pos);
        }

        if input[pos..].starts_with(']') {
            pos += 1;
            break;
        }

        match parse_gemma4_value(input, pos) {
            Ok((val, next_pos)) => {
                arr.push(val);
                pos = next_pos;
            }
            Err(e) => {
                return Err(format!("Error parsing array value: {}", e));
            }
        }
    }
    Ok((Value::Array(arr), pos))
}

pub fn parse_native_block(block: &str) -> Result<ToolCall, String> {
    let block = block.trim();
    
    // Support legacy format for backward compatibility or testing
    if let Some(call_pos) = block.find("call:") {
        let call_content = &block[call_pos + 5..];
        if let Some(brace_start) = call_content.find('{') {
            let func_name = call_content[..brace_start].trim().to_string();
            let args_content = &call_content[brace_start..]; 
            
            if let Ok((Value::Object(map), _)) = parse_gemma4_dict(args_content, 0) {
                return Ok(ToolCall {
                    id: map.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                    function: crate::context::FunctionCall {
                        name: func_name,
                        arguments: Value::Object(map),
                    },
                });
            }
        }
    }

    // New format: JSON object with "name" and "args"
    let start_pos = block.find('{').unwrap_or(0);
    let (val, _) = parse_gemma4_dict(&block[start_pos..], 0)?;
    
    if let Value::Object(mut map) = val {
        let name = map.get("name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let args = map.remove("args").unwrap_or(Value::Object(Map::new()));
        
        let args_map = match args {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        
        Ok(ToolCall {
            id: args_map.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
            function: crate::context::FunctionCall {
                name,
                arguments: Value::Object(args_map),
            },
        })
    } else {
        Err("Arguments must be a JSON object".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_internal_quote_escaping() {
        let block = r#"call:run_shell_command{command:<|">ls -R . | grep "description"<|">, description:<|">searching<|">, tool_call_id:<|">search<|">}"#;
        let result = parse_native_block(block).expect("Should NOT close string early");
        assert_eq!(result.function.arguments.get("command").unwrap().as_str().unwrap(), "ls -R . | grep \"description\"");
    }

    #[test]
    fn test_extra_comma_before_marker() {
        let block = r#"call:write_file{content:,<|">using System;<|">,path: "test.cs", tool_call_id: "test"}"#;
        let result = parse_native_block(block).expect("Should recover from extra comma before marker");
        assert_eq!(result.function.arguments.get("content").unwrap().as_str().unwrap(), "using System;");
    }

    #[test]
    fn test_backtick_and_forgotten_marker_close() {
        let block = r#"call:run_shell_command{command:`ls -la`,description: "List files", tool_call_id: "list"}"#;
        let result = parse_native_block(block).expect("Should handle backticks");
        // Backtick string fallback captures the backticks in our new parser implementation.
        assert_eq!(result.function.arguments.get("command").unwrap().as_str().unwrap(), "`ls -la`");
    }

    #[test]
    fn test_no_premature_closing() {
        let block = r#"call:run_shell_command{command:<|">ls -R . | grep description<|">, description:<|">searching<|">, tool_call_id:<|">search<|">}"#;
        let result = parse_native_block(block).expect("Should NOT close string early");
        assert_eq!(result.function.arguments.get("command").unwrap().as_str().unwrap(), "ls -R . | grep description");
    }

    #[test]
    fn test_nested_quotes_with_markers() {
        let block = r#"call:write_file{content:<|">fn main() { println!("test"); }<|">, path: "test.rs", tool_call_id: "test"}"#;
        let result = parse_native_block(block).expect("Should parse nested quotes inside markers");
        assert_eq!(result.function.arguments.get("content").unwrap().as_str().unwrap(), "fn main() { println!(\"test\"); }");
    }

    #[test]
    fn test_latest_run_real_world_failure() {
        let block = r#"call:write_file{content:<|">use raylib::prelude::*;

fn main() {
    println!("Hello World");
}
`,description: "Replace the spinning cube code with a bouncing cube implementation.",path: "src/main.rs",tool_call_id: "replace_with_bouncing_cube"}"#;

        let result = match parse_native_block(block) {
            Ok(r) => r,
            Err(e) => panic!("Parse failed: {}", e),
        };
        println!("Parsed map: {:?}", result.function.arguments);
        assert_eq!(result.id, "replace_with_bouncing_cube");
        let content = result.function.arguments.get("content").unwrap().as_str().unwrap();
        assert!(content.contains("println!(\"Hello World\")"));
    }

    #[test]
    fn test_unclosed_marker_with_following_keys() {
        let block = r#"call:write_file{content: <|">using System; ,path: "test.cs", tool_call_id: "test"}"#; 
        let result = parse_native_block(block).expect("Should recover from unclosed marker");
        assert_eq!(result.function.arguments.get("path").unwrap().as_str().unwrap(), "test.cs");
        assert!(result.function.arguments.get("content").unwrap().as_str().unwrap().contains("using System;"));
    }

    #[test]
    fn test_unquoted_multiline_value() {
        let block = r#"call:write_file{content:using System;
using System.Collections.Generic;
,path: "Program.cs", tool_call_id: "write_program"}"#;
        let result = parse_native_block(block).expect("Should recover from unquoted multiline value");
        assert_eq!(result.function.arguments.get("path").unwrap().as_str().unwrap(), "Program.cs");
        assert!(result.function.arguments.get("content").unwrap().as_str().unwrap().contains("using System;"));
    }

    #[test]
    fn test_pipe_delimiter_failure() {
        let block = r#"call:write_file{content:|<Project Sdk="Microsoft.NET.Sdk"> ... | ,path: "test.csproj", tool_call_id: "test"}"#;
        let result = parse_native_block(block).expect("Should recover from pipe delimiter");
        assert_eq!(result.function.arguments.get("path").unwrap().as_str().unwrap(), "test.csproj");
        assert!(result.function.arguments.get("content").unwrap().as_str().unwrap().contains("<Project"));
    }

    #[test]
    fn test_unquoted_keys() {
        let block = r#"call:write_file{content: "hello", path: "test.txt", tool_call_id: "test"}"#;
        let result = parse_native_block(block).expect("Should parse unquoted keys");
        assert_eq!(result.function.arguments.get("path").unwrap().as_str().unwrap(), "test.txt");
    }

    #[test]
    fn test_session_20260427_090929_csproj_failure() {
        let block = r#"call:write_file{content:<|"><Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net6.0</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
  </PropertyGroup>

  <ItemGroup>
    <PackageReference Include="Microsoft.Diagnostics.Runtime" Version="2.2.1" />
  </ItemGroup>
</Project>
,description:"Writing the project file for the analyzer",path:"DumpAnalyzer.csproj",tool_call_id:"write_csproj"}"#;

        let result = parse_native_block(block).expect("Should recover from csproj failure with unclosed marker");
        assert_eq!(result.id, "write_csproj");
        assert_eq!(result.function.arguments.get("path").unwrap().as_str().unwrap(), "DumpAnalyzer.csproj");
        let content = result.function.arguments.get("content").unwrap().as_str().unwrap();
        assert!(content.contains("<Project Sdk=\"Microsoft.NET.Sdk\">"));
        assert!(content.contains("</Project>"));
    }

    #[test]
    fn test_csharp_live_failure() {
        let block = r#"call:write_file{content:<|"|>using System;
using System.Text;

namespace AsciiPong {
    class Program {
        static void Main() {
            Console.WriteLine("Pong");
        }
    }
}
<|"|>,description:<|"|>Write a reference implementation of an ASCII Pong game in C#.<|"|>,path:<|"|>pong.cs<|"|>,tool_call_id:<|"|>write_pong_cs<|"|>}"#;
        let result = parse_native_block(block).expect("Should recover from C# live failure");
        assert_eq!(result.function.arguments.get("path").unwrap().as_str().unwrap(), "pong.cs");
        assert!(result.function.arguments.get("content").unwrap().as_str().unwrap().contains("using System;"));
    }

    #[test]
    fn test_get_current_temperature() {
        let block = r#"<|tool_call>call:get_current_temperature{location:<|"|>London<|"|>}<tool_call|><|tool_response>"#;
        let (result, _) = find_tool_call(block, true).expect("Should find tool call").expect("Should parse tool call");
        assert_eq!(result.function.name, "get_current_temperature");
        assert_eq!(result.function.arguments.get("location").unwrap().as_str().unwrap(), "London");
    }

    #[test]
    fn test_json_object_format() {
        let block = r#"<|tool_call>{"name": "write_file", "args": {"path": "test.txt", "content": "hello", "description": "test", "tool_call_id": "123"}}<tool_call|>"#;
        let (result, _) = find_tool_call(block, true).expect("Should find tool call").expect("Should parse tool call");
        assert_eq!(result.function.name, "write_file");
        assert_eq!(result.id, "123");
        assert_eq!(result.function.arguments.get("path").unwrap().as_str().unwrap(), "test.txt");
    }

    #[test]
    fn test_nested_structures() {
        let block = r#"call:test_tool{items: [1, 2, {"a": "b"}], options: {debug: true, count: 42}, tool_call_id: "nested"}"#;
        let result = parse_native_block(block).expect("Should parse nested structures");
        assert_eq!(result.function.name, "test_tool");
        let items = result.function.arguments.get("items").unwrap().as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[2].get("a").unwrap().as_str().unwrap(), "b");
        let options = result.function.arguments.get("options").unwrap().as_object().unwrap();
        assert_eq!(options.get("debug").unwrap().as_bool().unwrap(), true);
        assert_eq!(options.get("count").unwrap().as_i64().unwrap(), 42);
    }

    #[test]
    fn test_mixed_delimiters() {
        let block = r#"call:mixed{s1: <|"|>double<|"|>, s2: <|'|>single<|'|>, s3: "regular", tool_call_id: "mixed"}"#;
        let result = parse_native_block(block).expect("Should handle mixed delimiters");
        assert_eq!(result.function.arguments.get("s1").unwrap().as_str().unwrap(), "double");
        assert_eq!(result.function.arguments.get("s2").unwrap().as_str().unwrap(), "single");
        assert_eq!(result.function.arguments.get("s3").unwrap().as_str().unwrap(), "regular");
    }

    #[test]
    fn test_partial_find_tool_call() {
        let block = r#"Thinking... <|tool_call>call:ls{}"#;
        let result = find_tool_call(block, false);
        assert!(result.is_none(), "Should return None if call is not closed and is_final is false");
        
        let result_final = find_tool_call(block, true);
        assert!(result_final.is_some(), "Should return result if is_final is true even if unclosed");
        let (tc, _) = result_final.unwrap().expect("Should parse");
        assert_eq!(tc.function.name, "ls");
    }
}
pub fn find_tool_call(text: &str, is_final: bool) -> Option<Result<(ToolCall, usize), (String, usize)>> {
    let start_tokens = ["<|tool_call>", "<tool_call>"];
    
    let mut earliest_start = None;
    let mut chosen_token = "";

    for &token in &start_tokens {
        if let Some(pos) = text.find(token) {
            if earliest_start.map_or(true, |p| pos < p) {
                earliest_start = Some(pos);
                chosen_token = token;
            }
        }
    }

    if let Some(start_idx) = earliest_start {
        let after_start = &text[start_idx + chosen_token.len()..];
        let end_tokens = ["<tool_call|>", "</tool_call>"];
        
        let mut end_idx_rel = None;
        let mut chosen_end_token = "";
        for &token in &end_tokens {
            if let Some(pos) = after_start.find(token) {
                if end_idx_rel.map_or(true, |p| pos < p) {
                    end_idx_rel = Some(pos);
                    chosen_end_token = token;
                }
            }
        }
        
        if let Some(rel) = end_idx_rel {
            let full_call_block = &after_start[..rel];
            let total_len = start_idx + chosen_token.len() + rel + chosen_end_token.len();
            match parse_native_block(full_call_block) {
                Ok(tc) => Some(Ok((tc, total_len))),
                Err(e) => Some(Err((e, total_len))),
            }
        } else if is_final {
             match parse_native_block(after_start) {
                 Ok(tc) => Some(Ok((tc, text.len()))),
                 Err(e) => Some(Err((e, text.len()))),
             }
        } else {
             None
        }
    } else {
        None
    }
}
