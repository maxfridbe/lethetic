//! Opt-in: rebuild a saved session's request and report its size and shape.
//! `LETHETIC_REPLAY_SESSION=<session dir> cargo test --test context_replay -- --ignored --nocapture`

use lethetic::app::SessionState;
use lethetic::context::ContextManager;
use lethetic::transport::Role;

#[test]
#[ignore]
fn replay_session_context() {
    let Ok(dir) = std::env::var("LETHETIC_REPLAY_SESSION") else {
        return;
    };
    let state = SessionState::load(&dir);
    let mut ctx = ContextManager::new(212_992, Some("system prompt".to_string()));
    ctx.set_messages(state.messages);
    let messages = ctx.prepare_api_context().into_messages();
    let chars = |role: Role| -> usize {
        messages
            .iter()
            .filter(|m| m.role == role)
            .map(|m| m.content.text().len())
            .sum()
    };
    println!(
        "messages={} tool_chars={} assistant_chars={} user_chars={} total_tool_results={}",
        messages.len(),
        chars(Role::Tool),
        chars(Role::Assistant),
        chars(Role::User),
        messages.iter().filter(|m| m.role == Role::Tool).count(),
    );
}
