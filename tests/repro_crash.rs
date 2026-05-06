use lethetic::parser::parse_native_block;

#[test]
fn test_crash_reproduction_non_ascii() {
    // A tool call with non-ASCII characters (em-dash: —, 3 bytes, 1 char)
    // The em-dash will cause the byte offset to be larger than the char offset.
    // parse_gemma4_args collects chars[i..] into a String, finds the tag, 
    // and adds the BYTE offset to the CHAR index i.
    let block = r#"call:write_file{content:<|"|>Story with an em-dash — and more text.<|"|>, description: "test", path: "test.md", tool_call_id: "123"}"#;
    let result = parse_native_block(block);
    assert!(result.is_ok());
}
