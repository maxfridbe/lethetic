use lethetic::app::BlockType;
use lethetic::parser_new::{StreamParser, ParserState};

#[test]
fn test_basic_thought_to_text() {
    let mut parser = StreamParser::new();
    
    // Initial state is Thought
    let res1 = parser.parse_chunk("I am thinking");
    assert_eq!(res1, vec![(BlockType::Thought, "I am thinking".to_string())]);
    
    let res2 = parser.parse_chunk("<channel|>Now I speak");
    assert_eq!(res2, vec![(BlockType::Text, "Now I speak".to_string())]);
    assert_eq!(parser.state, ParserState::Text);
}

#[test]
fn test_fragmented_marker() {
    let mut parser = StreamParser::new();
    
    let res1 = parser.parse_chunk("Thinking... <chan");
    // Should emit "Thinking... " but hold "<chan" because it's a partial marker
    assert_eq!(res1, vec![(BlockType::Thought, "Thinking... ".to_string())]);
    
    let res2 = parser.parse_chunk("nel|>Done.");
    assert_eq!(res2, vec![(BlockType::Text, "Done.".to_string())]);
}

#[test]
fn test_tool_call_transition() {
    let mut parser = StreamParser::new();
    parser.state = ParserState::Text;
    
    let res1 = parser.parse_chunk("I will call a tool: <tool_call>call:ls{}<tool_call|>And back.");
    assert_eq!(res1, vec![
        (BlockType::Text, "I will call a tool: ".to_string()),
        (BlockType::Formulating, "call:ls{}".to_string()),
        (BlockType::Text, "And back.".to_string())
    ]);
}

#[test]
fn test_fragmented_tool_call() {
    let mut parser = StreamParser::new();
    parser.state = ParserState::Text;
    
    let res1 = parser.parse_chunk("Using tool <tool");
    assert_eq!(res1, vec![(BlockType::Text, "Using tool ".to_string())]);
    
    let res2 = parser.parse_chunk("_call>call:read_file{path:<|\">main.rs<|\">}<tool_call|>OK");
    assert_eq!(res2, vec![
        (BlockType::Formulating, "call:read_file{path:<|\">main.rs<|\">}".to_string()),
        (BlockType::Text, "OK".to_string())
    ]);
}
