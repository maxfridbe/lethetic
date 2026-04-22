#[path = "../context.rs"]
mod context;
#[path = "../parser.rs"]
mod parser;
#[path = "../tools.rs"]
mod tools;
#[path = "../system_prompt.rs"]
mod system_prompt;
#[path = "../config.rs"]
mod config;
#[path = "../client.rs"]
mod client;
#[path = "../tool_executor.rs"]
mod tool_executor;
#[path = "../app.rs"]
mod app;
#[path = "../ui.rs"]
mod ui;
#[path = "../markdown.rs"]
mod markdown;
#[path = "../icons.rs"]
mod icons;

use crate::context::{ContextManager};
use crate::parser::find_tool_call;
use crate::system_prompt::EXPERT_ENGINEER;

fn main() {
    let mut context_manager = ContextManager::new(8000, Some(EXPERT_ENGINEER.to_string()));

    // 1. User asks for jokes
    let user_input = "make me a 10 line jokes.txt";
    println!("--- STEP 1: USER INPUT ---");
    println!("{}", user_input);
    context_manager.add_message("user", user_input);

    // 2. Simulate LLM Response with a manual tool call
    let assistant_response = "<planning>\n1. Create jokes.txt\n</planning>\n\n<tool_call>\n{\n  \"name\": \"run_shell_command\",\n  \"arguments\": {\n    \"command\": \"echo 'joke 1' > jokes.txt\",\n    \"tool_call_id\": \"call_123\"\n  }\n}\n</tool_call>";
    println!("\n--- STEP 2: ASSISTANT RESPONSE (MANUAL TOOL CALL) ---");
    println!("{}", assistant_response);

    // Parse it as the app would
    let (tc, pos) = find_tool_call(assistant_response).expect("Failed to parse simulated tool call");
    
    // Add to context (using our new logic: store tool_calls even for manual calls, and TRUNCATE content)
    let mut assistant_content = assistant_response.to_string();
    assistant_content.truncate(pos);
    context_manager.add_assistant_tool_call(&assistant_content, Some(vec![tc.clone()]));

    // 3. Simulate Tool Result
    let tool_result = "EXIT_CODE: 0\nSTDOUT:\n\nSTDERR:\n";
    println!("\n--- STEP 3: TOOL RESULT ---");
    println!("{}", tool_result);
    context_manager.add_tool_result(tc.id.clone(), tool_result);

    // 4. Show final context for next turn
    println!("\n--- STEP 4: FINAL CONTEXT FOR NEXT TURN ---");
    let messages = context_manager.get_messages();
    let context_json = serde_json::to_string_pretty(&messages).unwrap();
    println!("{}", context_json);

    // Save to a file for easy inspection
    std::fs::write("test_simulation_context.json", context_json).expect("Failed to write simulation result");
    println!("\nSimulation complete. Result saved to test_simulation_context.json");
}
