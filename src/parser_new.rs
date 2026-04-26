use crate::app::BlockType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            let mut input = self.buffer.as_str();

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
                        if i_pos > 0 && earliest_end.map_or(true, |(e_pos, _)| i_pos < e_pos) {
                            let content = input[..i_pos].to_string();
                            results.push((BlockType::Thought, content));
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
                    let end_markers = ["<tool_call|>", "<|tool_call|>"];
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
                            // If it's a tool start at position 0, it's likely the one we are ALREADY in (handled by continue logic)
                            // We only interrupt if it's LATER in the input.
                            if pos > 0 && earliest_interrupt.map_or(true, |(p, _, _)| pos < p) {
                                earliest_interrupt = Some((pos, m, ParserState::ToolCall));
                            }
                        }
                    }

                    if let Some((i_pos, i_marker, i_state)) = earliest_interrupt {
                        if i_pos > 0 && earliest_end.map_or(true, |(e_pos, _)| i_pos < e_pos) {
                            let content = input[..i_pos].to_string();
                            results.push((BlockType::Formulating, content));
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
