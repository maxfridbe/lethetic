use std::io::{BufRead, BufReader};
use lethetic::parser::StreamParser;
use lethetic::app::BlockType;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".lethetic/tokens.jsonl".to_string());

    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Cannot open '{}': {}", path, e);
            std::process::exit(1);
        }
    };

    println!("Replaying: {}\n{}", path, "─".repeat(60));

    let mut parser = StreamParser::new();
    let mut total_chunks = 0usize;
    let mut total_blocks = 0usize;
    let mut block_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut last_state = parser.state;

    for (line_no, line) in BufReader::new(file).lines().enumerate() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };

        let entry: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[line {}] JSON parse error: {}", line_no + 1, e);
                continue;
            }
        };

        let kind = entry["kind"].as_str().unwrap_or("unknown");
        let t_ms = entry["t"].as_u64().unwrap_or(0);
        let chunk = entry["c"].as_str().unwrap_or("");

        // Tool events bypass the text parser — log them separately
        if kind == "tool" {
            let name = entry["name"].as_str().unwrap_or("?");
            let id   = entry["id"].as_str().unwrap_or("?");
            println!("[t={}ms kind=tool] → call: {} (id={})", t_ms, name, id);
            continue;
        }

        total_chunks += 1;
        let state_before = parser.state;
        let results = parser.parse_chunk(chunk);
        let state_after = parser.state;

        let state_changed = state_before != state_after;
        let transition = if state_changed {
            format!("{:?} → {:?}", state_before, state_after)
        } else {
            format!("{:?}", state_after)
        };

        let chunk_preview = if chunk.len() > 60 {
            format!("{}…", &chunk[..60].replace('\n', "↵"))
        } else {
            chunk.replace('\n', "↵")
        };

        println!(
            "[t={}ms kind={}] {} → {} block(s)  {:?}",
            t_ms, kind, transition, results.len(), chunk_preview
        );

        for (bt, content) in &results {
            let label = block_type_label(bt);
            let preview = if content.len() > 80 {
                format!("{}…", &content[..80].replace('\n', "↵"))
            } else {
                content.replace('\n', "↵")
            };
            println!("  {}: {:?}", label, preview);
            *block_counts.entry(label.to_string()).or_insert(0) += 1;
            total_blocks += 1;
        }

        last_state = state_after;
    }

    println!("\n{}", "─".repeat(60));
    println!("Summary");
    println!("  Chunks replayed : {}", total_chunks);
    println!("  Blocks emitted  : {}", total_blocks);
    println!("  Final state     : {:?}", last_state);
    for (label, count) in &block_counts {
        println!("    {:<14}: {}", label, count);
    }
}

fn block_type_label(bt: &BlockType) -> &'static str {
    match bt {
        BlockType::Thought    => "Thought",
        BlockType::Text       => "Text",
        BlockType::Formulating => "Formulating",
        BlockType::ToolCall   => "ToolCall",
        BlockType::ToolResult => "ToolResult",
        BlockType::User       => "User",
        BlockType::Markdown   => "Markdown",
        BlockType::Divider    => "Divider",
    }
}
