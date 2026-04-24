use regex::Regex;

fn main() {
    // Current regex + optional channel name
    let re = Regex::new(r"<\|?/?(?:channel|thought|tool_call|tool_response|turn|bos|eos|think|\||\x22|')[^>]*>?(?:thought|text|model|system)?").unwrap();
    let texts = vec![
        "<|channel>thought",
        "<channel|>",
        "<|turn>model",
        "<|turn>user",
        "<|thought>",
        "<think>",
        "<|tool_call>",
    ];
    
    for t in texts {
        println!("Text: {}, Match: {:?}", t, re.find(t).map(|m| m.as_str()));
    }
}