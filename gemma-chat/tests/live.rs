/// Live integration tests against the brainiac-nvidia TurboQuant server.
/// Run with: cargo test --test live -- --nocapture
use futures_util::StreamExt;
use gemma_chat::*;

const BASE_URL: &str = "http://brainiac-nvidia:7210/v1";
const MODEL: &str = "Gemma-4-26B-TurboQuant-262k";

#[tokio::test]
async fn test_live_simple_reply() {
    let client = reqwest::Client::new();
    let msgs = vec![Message::user("Reply with exactly: PONG")];
    let mut stream = stream_chat(&client, BASE_URL, MODEL, &msgs, &[], 200)
        .await
        .expect("stream_chat failed");

    let mut text = String::new();
    let mut got_done = false;
    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::TextDelta(s) => text.push_str(&s),
            StreamEvent::Done { .. } => { got_done = true; break; }
            StreamEvent::Error(e) => panic!("Error: {e}"),
            _ => {}
        }
    }

    println!("Response: {text:?}");
    assert!(got_done, "Never received Done");
    assert!(text.contains("PONG"), "Expected PONG in: {text}");
}

#[tokio::test]
async fn test_live_reasoning_present() {
    let client = reqwest::Client::new();
    let msgs = vec![Message::user("What is 17 * 43?")];
    let mut stream = stream_chat(&client, BASE_URL, MODEL, &msgs, &[], 600)
        .await
        .expect("stream_chat failed");

    let mut reasoning = String::new();
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::ReasoningDelta(s) => reasoning.push_str(&s),
            StreamEvent::TextDelta(s) => text.push_str(&s),
            StreamEvent::Done { .. } => break,
            StreamEvent::Error(e) => panic!("Error: {e}"),
            _ => {}
        }
    }

    println!("Reasoning: {reasoning:?}");
    println!("Answer: {text:?}");
    assert!(!reasoning.is_empty(), "Expected reasoning content");
    // Answer may appear in reasoning or text depending on token budget
    assert!(
        text.contains("731") || reasoning.contains("731"),
        "Expected 731 in reasoning or text. reasoning={reasoning:?} text={text:?}"
    );
}

#[tokio::test]
async fn test_live_tool_call() {
    use serde_json::json;
    let client = reqwest::Client::new();

    let tools = vec![ToolDefinition::new(
        "calculate",
        "Perform a math calculation",
        json!({
            "type": "object",
            "properties": {
                "expression": { "type": "string", "description": "Math expression to evaluate" }
            },
            "required": ["expression"]
        }),
    )];

    let msgs = vec![
        Message::system("You are a helpful assistant. Use the calculate tool when asked to compute math."),
        Message::user("What is 12 * 15? Use the calculate tool."),
    ];

    let mut stream = stream_chat(&client, BASE_URL, MODEL, &msgs, &tools, 400)
        .await
        .expect("stream_chat failed");

    let mut tool_name = String::new();
    let mut tool_args = serde_json::Value::Null;
    let mut got_done = false;

    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::ToolCallStart { name, .. } => {
                println!("Tool call started: {name}");
                tool_name = name;
            }
            StreamEvent::ToolCallComplete { name, arguments, .. } => {
                println!("Tool call complete: {name} args={arguments}");
                tool_name = name;
                tool_args = arguments;
            }
            StreamEvent::Done { completion_tokens, prompt_tokens } => {
                println!("Done: completion={completion_tokens:?} prompt={prompt_tokens:?}");
                got_done = true;
                break;
            }
            StreamEvent::Error(e) => panic!("Error: {e}"),
            StreamEvent::TextDelta(s) => print!("{s}"),
            StreamEvent::ReasoningDelta(_) => {}
            _ => {}
        }
    }

    assert!(got_done, "Never received Done");
    if !tool_name.is_empty() {
        assert_eq!(tool_name, "calculate");
        println!("Tool args: {tool_args}");
    }
}
