use serde_json::Value;

#[test]
fn test_error_parse() {
    let s = r#"{"error":{"code":500,"message":"Failed to parse input","type":"server_error"}}"#;
    let val: Value = serde_json::from_str(s).unwrap();
    let err_type = val.get("error").and_then(|e| e.get("type")).and_then(|t| t.as_str());
    println!("Err type: {:?}", err_type);
}
