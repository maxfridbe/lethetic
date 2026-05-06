use crate::context::ToolCall;
use serde_json::{Value, Map};
use crate::app::BlockType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ParserState {
    Text,
    Thought,
    ToolCall,
}

pub struct StreamParser {
    pub state: ParserState,
    buffer: String,
}

impl StreamParser {
    pub fn new() -> Self {
        Self {
            state: ParserState::Thought, // Gemma 4 usually starts in thought
            buffer: String::new(),
        }
    }

    pub fn reset(&mut self) {
        self.state = ParserState::Thought;
        self.buffer.clear();
    }

    pub fn parse_chunk(&mut self, chunk: &str) -> Vec<(BlockType, String)> {
        self.buffer.push_str(chunk);
        let mut results = Vec::new();
        
        loop {
            if self.buffer.is_empty() { break; }
            let input = self.buffer.as_str();

            match self.state {
                ParserState::Thought => {
                    let end_markers = ["<channel|>", "</thought>", "</think>"];
                    let thought_starts = ["<|channel>thought", "<thought>", "<think>"];
                    let tool_starts = ["<|tool_call>", "<tool_call>"];

                    let mut earliest_end = None;
                    for &m in &end_markers {
                        if let Some(pos) = input.find(m) {
                            if earliest_end.map_or(true, |(p, _)| pos < p) {
                                earliest_end = Some((pos, m));
                            }
                        }
                    }

                    // Heuristic: If we see a new start marker before an end marker, the previous one was likely aborted
                    let mut earliest_interrupt = None;
                    for &m in &thought_starts {
                        if let Some(pos) = input.find(m) {
                            if earliest_interrupt.map_or(true, |(p, _, _)| pos < p) {
                                earliest_interrupt = Some((pos, m, ParserState::Thought));
                            }
                        }
                    }
                    for &m in &tool_starts {
                        if let Some(pos) = input.find(m) {
                            if earliest_interrupt.map_or(true, |(p, _, _)| pos < p) {
                                earliest_interrupt = Some((pos, m, ParserState::ToolCall));
                            }
                        }
                    }

                    if let Some((i_pos, i_marker, i_state)) = earliest_interrupt {
                        if earliest_end.map_or(true, |(e_pos, _)| i_pos < e_pos) {
                            let content = input[..i_pos].to_string();
                            if !content.is_empty() {
                                results.push((BlockType::Thought, content));
                            }
                            self.state = i_state;
                            self.buffer = input[i_pos + i_marker.len()..].to_string();
                            continue;
                        }
                    }

                    if let Some((pos, marker)) = earliest_end {
                        let content = input[..pos].to_string();
                        if !content.is_empty() {
                            results.push((BlockType::Thought, content));
                        }
                        self.state = ParserState::Text;
                        self.buffer = input[pos + marker.len()..].to_string();
                        continue;
                    } else {
                        // Check for partial end marker at the end of buffer
                        if let Some(partial_start_idx) = self.find_partial_marker_start(input, &end_markers) {
                            let content = input[..partial_start_idx].to_string();
                            if !content.is_empty() {
                                results.push((BlockType::Thought, content));
                            }
                            self.buffer = input[partial_start_idx..].to_string();
                            break; 
                        }
                        
                        let to_emit = self.buffer.clone();
                        if !to_emit.is_empty() {
                            results.push((BlockType::Thought, to_emit));
                        }
                        self.buffer.clear();
                        break;
                    }
                }
                ParserState::Text => {
                    let thought_starts = ["<|channel>thought", "<thought>", "<think>"];
                    let tool_starts = ["<|tool_call>", "<tool_call>"];
                    
                    let mut earliest_start = None;
                    for &m in &thought_starts {
                        if let Some(pos) = input.find(m) {
                            if earliest_start.map_or(true, |(p, _, _)| pos < p) {
                                earliest_start = Some((pos, m, ParserState::Thought));
                            }
                        }
                    }
                    for &m in &tool_starts {
                        if let Some(pos) = input.find(m) {
                            if earliest_start.map_or(true, |(p, _, _)| pos < p) {
                                earliest_start = Some((pos, m, ParserState::ToolCall));
                            }
                        }
                    }

                    if let Some((pos, marker, next_state)) = earliest_start {
                        let content = input[..pos].to_string();
                        if !content.is_empty() {
                            results.push((BlockType::Text, content));
                        }
                        self.state = next_state;
                        self.buffer = input[pos + marker.len()..].to_string();
                        continue;
                    } else {
                        let all_starts: Vec<&str> = thought_starts.iter().chain(tool_starts.iter()).copied().collect();
                        if let Some(partial_start_idx) = self.find_partial_marker_start(input, &all_starts) {
                            let content = input[..partial_start_idx].to_string();
                            if !content.is_empty() {
                                results.push((BlockType::Text, content));
                            }
                            self.buffer = input[partial_start_idx..].to_string();
                            break; 
                        }
                        
                        let to_emit = self.buffer.clone();
                        if !to_emit.is_empty() {
                            results.push((BlockType::Text, to_emit));
                        }
                        self.buffer.clear();
                        break;
                    }
                }
                ParserState::ToolCall => {
                    let end_markers = ["<tool_call|>", "<|tool_call|>", "</tool_call>", "<|tool_call|>"];
                    let thought_starts = ["<|channel>thought", "<thought>", "<think>"];
                    let tool_starts = ["<|tool_call>", "<tool_call>"];

                    let mut earliest_end = None;
                    for &m in &end_markers {
                        if let Some(pos) = input.find(m) {
                            if earliest_end.map_or(true, |(p, _)| pos < p) {
                                earliest_end = Some((pos, m));
                            }
                        }
                    }

                    // Heuristic: If we see a new start marker before an end marker, the previous one was likely aborted
                    let mut earliest_interrupt = None;
                    for &m in &thought_starts {
                        if let Some(pos) = input.find(m) {
                            if earliest_interrupt.map_or(true, |(p, _, _)| pos < p) {
                                earliest_interrupt = Some((pos, m, ParserState::Thought));
                            }
                        }
                    }
                    for &m in &tool_starts {
                        if let Some(pos) = input.find(m) {
                            // Only interrupt if it's LATER in the input.
                            if pos > 0 && earliest_interrupt.map_or(true, |(p, _, _)| pos < p) {
                                earliest_interrupt = Some((pos, m, ParserState::ToolCall));
                            }
                        }
                    }

                    if let Some((i_pos, i_marker, i_state)) = earliest_interrupt {
                        if earliest_end.map_or(true, |(e_pos, _)| i_pos < e_pos) {
                            let content = input[..i_pos].to_string();
                            if !content.is_empty() {
                                results.push((BlockType::Formulating, content));
                            }
                            self.state = i_state;
                            self.buffer = input[i_pos + i_marker.len()..].to_string();
                            continue;
                        }
                    }

                    if let Some((pos, marker)) = earliest_end {
                        let content = input[..pos].to_string();
                        if !content.is_empty() {
                            results.push((BlockType::Formulating, content));
                        }
                        self.state = ParserState::Text;
                        self.buffer = input[pos + marker.len()..].to_string();
                        continue;
                    } else {
                        if let Some(partial_start_idx) = self.find_partial_marker_start(input, &end_markers) {
                            let content = input[..partial_start_idx].to_string();
                            if !content.is_empty() {
                                results.push((BlockType::Formulating, content));
                            }
                            self.buffer = input[partial_start_idx..].to_string();
                            break; 
                        }
                        
                        let to_emit = self.buffer.clone();
                        if !to_emit.is_empty() {
                            results.push((BlockType::Formulating, to_emit));
                        }
                        self.buffer.clear();
                        break;
                    }
                }
            }
        }
        
        results
    }

    fn find_partial_marker_start(&self, input: &str, markers: &[&str]) -> Option<usize> {
        let mut best_start = None;
        for &m in markers {
            for i in 1..m.len() {
                if input.ends_with(&m[..i]) {
                    let start_pos = input.len() - i;
                    if best_start.map_or(true, |p| start_pos < p) {
                        best_start = Some(start_pos);
                    }
                }
            }
        }
        best_start
    }
}



const PARAM_START: &str = "<|\"|>";
const PARAM_END: &str = "<|\"|>";

fn parse_gemma4_value(value_str: &str) -> Value {
    let mut s = value_str.trim();
    if s.is_empty() {
        return Value::String(String::new());
    }
    
    let mut changed = true;
    while changed {
        changed = false;
        let prev = s;
        
        // 1. Strip standard quotes
        if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
            if s.len() >= 2 {
                s = &s[1..s.len()-1];
                s = s.trim();
            }
        }
        
        // 2. Strip all known marker variants (LONGEST FIRST)
        let markers = [
            ("<|\"|>", "<|\"|>"),
            ("<|tool_parameter|>", "<|tool_parameter|>"),
            ("<|tool_parameter>", "<tool_parameter|>"),
            ("<|tool_parameter|>", "<tool_parameter|>"),
            ("<|\\\\\">", "<|\\\\\">"),
            ("<|\\\">", "<|\\\">"),
            ("<|\">", "<|\">"),
            ("<|'>", "<|'>")
        ];
        
        for (st, et) in markers {
            if s.starts_with(st) && s.ends_with(et) {
                if s.len() >= st.len() + et.len() {
                    s = &s[st.len()..s.len() - et.len()];
                    s = s.trim();
                    break;
                }
            }
        }
        
        if s != prev {
            changed = true;
        }
    }

    if s == "true" { return Value::Bool(true); }
    if s == "false" { return Value::Bool(false); }
    let lower = s.to_lowercase();
    if lower == "null" || lower == "none" || lower == "nil" { return Value::Null; }
    
    if s.contains('.') {
        if let Ok(f) = s.parse::<f64>() {
            if let Some(num) = serde_json::Number::from_f64(f) {
                return Value::Number(num);
            }
        }
    } else if let Ok(i) = s.parse::<i64>() {
        return Value::Number(i.into());
    }
    
    Value::String(s.to_string())
}

fn parse_gemma4_args(args_str: &str, partial: bool) -> Map<String, Value> {
    let mut result = Map::new();
    if args_str.trim().is_empty() { return result; }
    let chars: Vec<char> = args_str.chars().collect();
    let mut i = 0;
    let n = chars.len();

    while i < n {
        while i < n && (chars[i] == ' ' || chars[i] == ',' || chars[i] == '\n' || chars[i] == '\t') { i += 1; }
        if i >= n { break; }

        let key_start = i;
        while i < n && chars[i] != ':' { i += 1; }
        if i >= n { break; }
        
        let key_str: String = chars[key_start..i].iter().collect();
        let mut key = key_str.trim();
        if (key.starts_with('"') && key.ends_with('"')) || (key.starts_with('\'') && key.ends_with('\'')) {
            if key.len() >= 2 { key = &key[1..key.len()-1]; }
        }
        let key = key.to_string();
        i += 1; // skip ':'

        while i < n && (chars[i] == ' ' || chars[i] == '\n' || chars[i] == '\t') { i += 1; }
        if i >= n {
            if !partial { result.insert(key, Value::String(String::new())); }
            break;
        }

        let next_val;
        if chars[i] == '{' {
            let mut depth = 1;
            let obj_start = i + 1;
            i += 1;
            while i < n && depth > 0 {
                let rem: String = chars[i..].iter().collect();
                let mut et = None;
                // PRIORITY: LONG TAGS FIRST
                if rem.starts_with("<|\"|>") { et = Some("<|\"|>"); i += "<|\"|>".chars().count(); }
                else if rem.starts_with("<|\\\\\">") { et = Some("<|\\\\\">"); i += "<|\\\\\">".chars().count(); }
                else if rem.starts_with("<|\\\">") { et = Some("<|\\\">"); i += "<|\\\">".chars().count(); }
                else if rem.starts_with("<|\">") { et = Some("<|\">"); i += "<|\">".chars().count(); }
                else if rem.starts_with("<|'>") { et = Some("<|'>"); i += "<|'>".chars().count(); }

                if let Some(tag) = et {
                    let rem2: String = chars[i..].iter().collect();
                    if let Some(pos) = rem2.find(tag) { i += pos + tag.len(); }
                    else { i = n; }
                    continue;
                }
                if chars[i] == '{' { depth += 1; }
                else if chars[i] == '}' { depth -= 1; }
                i += 1;
            }
            if depth > 0 {
                let sub: String = chars[obj_start..i].iter().collect();
                next_val = Value::Object(parse_gemma4_args(&sub, true));
            } else {
                let sub: String = chars[obj_start..i - 1].iter().collect();
                next_val = Value::Object(parse_gemma4_args(&sub, partial));
            }
        } else if chars[i] == '[' {
            let mut depth = 1;
            let arr_start = i + 1;
            i += 1;
            while i < n && depth > 0 {
                let rem: String = chars[i..].iter().collect();
                let mut et = None;
                if rem.starts_with("<|\"|>") { et = Some("<|\"|>"); i += "<|\"|>".chars().count(); }
                else if rem.starts_with("<|\\\\\">") { et = Some("<|\\\\\">"); i += "<|\\\\\">".chars().count(); }
                else if rem.starts_with("<|\\\">") { et = Some("<|\\\">"); i += "<|\\\">".chars().count(); }
                else if rem.starts_with("<|\">") { et = Some("<|\">"); i += "<|\">".chars().count(); }
                else if rem.starts_with("<|'>") { et = Some("<|'>"); i += "<|'>".chars().count(); }
                if let Some(tag) = et {
                    let rem2: String = chars[i..].iter().collect();
                    if let Some(pos) = rem2.find(tag) { i += pos + tag.len(); }
                    else { i = n; }
                    continue;
                }
                if chars[i] == '[' { depth += 1; }
                else if chars[i] == ']' { depth -= 1; }
                i += 1;
            }
            if depth > 0 {
                let sub: String = chars[arr_start..i].iter().collect();
                next_val = Value::Array(parse_gemma4_array(&sub, true));
            } else {
                let sub: String = chars[arr_start..i - 1].iter().collect();
                next_val = Value::Array(parse_gemma4_array(&sub, partial));
            }
        } else {
            let val_start = i;
            let mut in_quote = None;
            while i < n {
                let rem: String = chars[i..].iter().collect();
                let mut et = None;
                if rem.starts_with("<|\"|>") { et = Some("<|\"|>"); i += "<|\"|>".chars().count(); }
                else if rem.starts_with("<|\\\\\">") { et = Some("<|\\\\\">"); i += "<|\\\\\">".chars().count(); }
                else if rem.starts_with("<|\\\">") { et = Some("<|\\\">"); i += "<|\\\">".chars().count(); }
                else if rem.starts_with("<|\">") { et = Some("<|\">"); i += "<|\">".chars().count(); }
                else if rem.starts_with("<|'>") { et = Some("<|'>"); i += "<|'>".chars().count(); }
                if let Some(tag) = et {
                    let rem2: String = chars[i..].iter().collect();
                    if let Some(pos) = rem2.find(tag) { i += pos + tag.len(); }
                    else { i = n; }
                    continue;
                }
                let c = chars[i];
                if let Some(q) = in_quote {
                    if c == q { in_quote = None; }
                } else {
                    if c == '"' || c == '\'' { in_quote = Some(c); }
                    else if c == ',' || c == '}' || c == ']' { break; }
                }
                i += 1;
            }
            let val_str: String = chars[val_start..i].iter().collect();
            next_val = parse_gemma4_value(&val_str);
        }
        result.insert(key, next_val);
    }
    result
}

fn parse_gemma4_array(arr_str: &str, partial: bool) -> Vec<Value> {
    let mut items = Vec::new();
    let chars: Vec<char> = arr_str.chars().collect();
    let mut i = 0;
    let n = chars.len();

    while i < n {
        while i < n && (chars[i] == ' ' || chars[i] == ',' || chars[i] == '\n' || chars[i] == '\t') {
            i += 1;
        }
        if i >= n { break; }

        let remaining: String = chars[i..].iter().collect();
        if remaining.starts_with(PARAM_START) {
            i += PARAM_START.chars().count();
            let val_start = i;
            let remaining_after_start: String = chars[i..].iter().collect();
            if let Some(end_pos_rel) = remaining_after_start.find(PARAM_END) {
                let end_pos = i + remaining_after_start[..end_pos_rel].chars().count();
                let val: String = chars[val_start..end_pos].iter().collect();
                items.push(Value::String(val));
                i = end_pos + PARAM_END.chars().count();
            } else {
                let val: String = chars[i..].iter().collect();
                items.push(Value::String(val));
                break;
            }
        } else if chars[i] == '{' {
            let mut depth = 1;
            let obj_start = i + 1;
            i += 1;
            while i < n && depth > 0 {
                let rem: String = chars[i..].iter().collect();
                if rem.starts_with(PARAM_START) {
                    i += PARAM_START.chars().count();
                    let rem_after: String = chars[i..].iter().collect();
                    if let Some(nd_rel) = rem_after.find(PARAM_END) {
                        i += rem_after[..nd_rel].chars().count() + PARAM_END.chars().count();
                    } else {
                        i = n;
                    }
                    continue;
                }
                if chars[i] == '{' { depth += 1; }
                else if chars[i] == '}' { depth -= 1; }
                i += 1;
            }
            if depth > 0 {
                let sub: String = chars[obj_start..i].iter().collect();
                items.push(Value::Object(parse_gemma4_args(&sub, true)));
            } else {
                let sub: String = chars[obj_start..i - 1].iter().collect();
                items.push(Value::Object(parse_gemma4_args(&sub, partial)));
            }
        } else if chars[i] == '[' {
            let mut depth = 1;
            let sub_start = i + 1;
            i += 1;
            while i < n && depth > 0 {
                let rem: String = chars[i..].iter().collect();
                if rem.starts_with(PARAM_START) {
                    i += PARAM_START.chars().count();
                    let rem_after: String = chars[i..].iter().collect();
                    if let Some(nd_rel) = rem_after.find(PARAM_END) {
                        i += rem_after[..nd_rel].chars().count() + PARAM_END.chars().count();
                    } else {
                        i = n;
                    }
                    continue;
                }
                if chars[i] == '[' { depth += 1; }
                else if chars[i] == ']' { depth -= 1; }
                i += 1;
            }
            if depth > 0 {
                let sub: String = chars[sub_start..i].iter().collect();
                items.push(Value::Array(parse_gemma4_array(&sub, true)));
            } else {
                let sub: String = chars[sub_start..i - 1].iter().collect();
                items.push(Value::Array(parse_gemma4_array(&sub, partial)));
            }
        } else {
            let val_start = i;
            let mut in_quote = None;
            while i < n {
                let rem: String = chars[i..].iter().collect();
                if rem.starts_with(PARAM_START) {
                    i += PARAM_START.chars().count();
                    let rem2: String = chars[i..].iter().collect();
                    if let Some(ep) = rem2.find(PARAM_END) {
                        i += ep + PARAM_END.chars().count();
                    } else {
                        i = n;
                    }
                    continue;
                }

                let c = chars[i];
                if let Some(q) = in_quote {
                    if c == q {
                        in_quote = None;
                    }
                } else if c == '"' || c == '\'' {
                    in_quote = Some(c);
                } else if c == ',' || c == ']' {
                    break;
                }
                i += 1;
            }
            if partial && i >= n && in_quote.is_none() { break; }
            let val: String = chars[val_start..i].iter().collect();
            items.push(parse_gemma4_value(&val));
        }
    }
    items
}

pub fn parse_native_block(block: &str) -> Result<ToolCall, String> {
    let block = block.trim();
    
    if let Ok(val) = serde_json::from_str::<Value>(block) {
        if let Value::Object(mut map) = val {
            let name = map.get("name").and_then(|v| v.as_str())
                .or_else(|| map.get("function").and_then(|v| v.as_str()))
                .unwrap_or("unknown").to_string();
            
            let args = map.remove("arguments")
                .or_else(|| map.remove("args"))
                .unwrap_or(Value::Object(Map::new()));
            
            let args_map = match args {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            
            return Ok(ToolCall {
                id: args_map.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                function: crate::context::FunctionCall {
                    name,
                    arguments: Value::Object(args_map),
                },
            });
        }
    }

    if let Some(call_pos) = block.find("call:") {
        let call_content = &block[call_pos + 5..];
        if let Some(brace_start) = call_content.find('{') {
            let func_name = call_content[..brace_start].trim().to_string();
            let mut args_str = &call_content[brace_start + 1..];
            
            if let Some(last_brace) = args_str.rfind('}') {
                args_str = &args_str[..last_brace];
            }
            
            let map = parse_gemma4_args(args_str, false);
            return Ok(ToolCall {
                id: map.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                function: crate::context::FunctionCall {
                    name: func_name,
                    arguments: Value::Object(map),
                },
            });
        }
    }

    Err("Could not parse gemma4 tool call".to_string())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repro_llm_malformed_markers() {
        let block = r#"call:write_file{content:<|"><!DOCTYPE html></html><|">,description:<|">Create index.html<|>,path:<|">index.html<|>,tool_call_id:<|">create_index_html<|>}"#;
        let result = parse_native_block(block).expect("Should parse");
        assert_eq!(result.function.name, "write_file");
        assert_eq!(result.function.arguments.get("content").unwrap().as_str().unwrap(), "<!DOCTYPE html></html>");
    }

    #[test]
    fn test_standard_json_format() {
        let block = r#"{"name": "read_file", "arguments": {"path": "src/main.rs", "description": "Read main.rs", "tool_call_id": "read_main_rs"}}"#;
        let result = parse_native_block(block).expect("Should parse standard JSON");
        assert_eq!(result.function.name, "read_file");
        assert_eq!(result.function.arguments.get("path").unwrap().as_str().unwrap(), "src/main.rs");
        assert_eq!(result.id, "read_main_rs");
    }

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
        assert_eq!(result.function.arguments.get("content").unwrap().as_str().unwrap(), "");
        assert_eq!(result.function.arguments.get("<|\">using System;<|\">,path").unwrap().as_str().unwrap(), "test.cs");
    }

    #[test]
    fn test_backtick_and_forgotten_marker_close() {
        let block = r#"call:run_shell_command{command:`ls -la`,description: "List files", tool_call_id: "list"}"#;
        let result = parse_native_block(block).expect("Should handle backticks");
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

        let result = parse_native_block(block).expect("Parse failed");
        assert_eq!(result.id, "unknown");
        let content = result.function.arguments.get("content").unwrap().as_str().unwrap();
        assert!(content.contains("replace_with_bouncing_cube"));
    }

    #[test]
    fn test_unclosed_marker_with_following_keys() {
        let block = r#"call:write_file{content: <|">using System; ,path: "test.cs", tool_call_id: "test"}"#; 
        let result = parse_native_block(block).expect("Should recover from unclosed marker");
        assert!(result.function.arguments.get("content").unwrap().as_str().unwrap().contains("using System;"));
        assert_eq!(result.id, "unknown");
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
        assert_eq!(result.id, "unknown");
        let content = result.function.arguments.get("content").unwrap().as_str().unwrap();
        assert!(content.contains("<Project Sdk=\"Microsoft.NET.Sdk\">"));
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
    fn test_new_end_token() {
        let block = r#"<|tool_call>call:ls{}<|tool_call|>"#;
        let (result, _) = find_tool_call(block, true).expect("Should find tool call").expect("Should parse tool call");
        assert_eq!(result.function.name, "ls");
    }

    #[test]
    fn test_mixed_delimiters() {
        let block = r#"call:mixed{s1: <|"|>double<|"|>, s2: <|'|>single<|'|>, s3: "regular", tool_call_id: "mixed"}"#;
        let result = parse_native_block(block).expect("Should handle mixed delimiters");
        assert_eq!(result.function.arguments.get("s1").unwrap().as_str().unwrap(), "double");
        assert_eq!(result.function.arguments.get("s3").unwrap().as_str().unwrap(), "regular");
    }

    #[test]
    fn test_escaped_symmetric_markers() {
        let block = r#"call:run_shell_command{command: "<|\\">ls -R<|\\">"}"#;
        let result = parse_native_block(block).expect("Should parse escaped symmetric tags");
        assert_eq!(result.function.arguments.get("command").unwrap().as_str().unwrap(), "ls -R");
    }

    #[test]
    fn test_quoted_parameter_tags() {
        let block = r#"call:read_folder{path: "<|"|>.<|"|>", description: "<|"|>List files.<|"|>"}"#;
        let result = parse_native_block(block).expect("Should parse quoted tags");
        assert_eq!(result.function.arguments.get("path").unwrap().as_str().unwrap(), ".");
        assert_eq!(result.function.arguments.get("description").unwrap().as_str().unwrap(), "List files.");
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
        let end_tokens = ["<tool_call|>", "</tool_call>", "<|tool_call|>"];
        
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
