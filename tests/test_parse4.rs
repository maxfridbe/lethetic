use lethetic::parser::parse_native_block;

#[test]
fn test_weird_markers() {
    let block = r#"call:write_file{content:<|"><!DOCTYPE html></html><|"><|"|>,description:<|"|>Rewrite index.html with new Power Allocation UI.<|"|>,path:<|"|>index.html<|"|>,tool_call_id:<|"|>write_index_html_v2<|"|>}"#;
    let result = parse_native_block(block);
    println!("{:?}", result);
}
