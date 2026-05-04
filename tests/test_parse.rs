use serde_json::json;

fn main() {
    let accumulated = "<tool_call>\n{\n  \"tool_call_id\": \"plan_001\",\n  \"command\": \"echo 'Why did the programmer quit their job? Because they didn't get enough arrays!' > joke.txt\"\n}\n</tool_call>\n";
    if let Some(start) = accumulated.find('{') {
        if let Some(end) = accumulated.rfind('}') {
            let json_str = &accumulated[start..=end];
            println!("json_str: {}", json_str);
            if let Ok(tc_val) = serde_json::from_str::<serde_json::Value>(json_str) {
                println!("tc_val parsed!");
            } else {
                println!("failed to parse json_str");
                println!("Error: {:?}", serde_json::from_str::<serde_json::Value>(json_str).err());
            }
        }
    }
}
