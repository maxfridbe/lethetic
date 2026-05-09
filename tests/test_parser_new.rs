use lethetic::app::BlockType;
use lethetic::parser::{StreamParser, ParserState};

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

#[test]
fn test_think_tags() {
    let mut parser = StreamParser::new();
    parser.state = ParserState::Text;
    
    let res = parser.parse_chunk("Here is my answer: <think>I need to be careful</think> Done.");
    assert_eq!(res, vec![
        (BlockType::Text, "Here is my answer: ".to_string()),
        (BlockType::Thought, "I need to be careful".to_string()),
        (BlockType::Text, " Done.".to_string())
    ]);
}

#[test]
fn test_tool_call_at_start() {
    let mut parser = StreamParser::new();
    // In many models, the response might start directly with a tool call
    let res = parser.parse_chunk("<tool_call>call:ls{}<tool_call|>");
    assert_eq!(res, vec![
        (BlockType::Formulating, "call:ls{}".to_string())
    ]);
}

#[test]
fn test_unclosed_tool_call_at_end() {
    let mut parser = StreamParser::new();
    parser.state = ParserState::Text;
    let res = parser.parse_chunk("I will run: <tool_call>call:ls{}");
    assert_eq!(res, vec![
        (BlockType::Text, "I will run: ".to_string()),
        (BlockType::Formulating, "call:ls{}".to_string())
    ]);
    assert_eq!(parser.state, ParserState::ToolCall);
}

// <|channel>text should exit Thought state the same way <channel|> does.
#[test]
fn test_channel_text_exits_thought() {
    let mut parser = StreamParser::new();
    let results = parser.parse_chunk("some thinking<|channel>text\nresponse text");
    assert!(
        results.contains(&(BlockType::Thought, "some thinking".to_string())),
        "expected Thought block with 'some thinking', got: {:?}", results
    );
    assert!(
        results.contains(&(BlockType::Text, "\nresponse text".to_string()))
        || results.contains(&(BlockType::Text, "response text".to_string()))
        || results.iter().any(|(bt, s)| *bt == BlockType::Text && s.contains("response text")),
        "expected Text block with 'response text', got: {:?}", results
    );
    assert_eq!(parser.state, ParserState::Text);
}

// <|channel>text arriving while already in Text state should be consumed silently —
// no literal '<|channel>text' should appear in the output.
#[test]
fn test_channel_text_noop_in_text_state() {
    let mut parser = StreamParser::new();
    // Advance parser to Text state first
    parser.parse_chunk("<channel|>");
    assert_eq!(parser.state, ParserState::Text);

    let results = parser.parse_chunk("text A<|channel>text\ntext B");
    let emitted: String = results.iter().map(|(_, s)| s.as_str()).collect();
    assert!(
        !emitted.contains("<|channel>text"),
        "marker should not appear in output, got: {:?}", results
    );
    assert!(
        emitted.contains("text A"),
        "expected 'text A' in output, got: {:?}", results
    );
    assert!(
        emitted.contains("text B"),
        "expected 'text B' in output, got: {:?}", results
    );
    assert!(
        results.iter().all(|(bt, _)| *bt == BlockType::Text),
        "all blocks should be Text type, got: {:?}", results
    );
}
