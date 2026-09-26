//! Opt-in timing of the browser state projection over a large transcript:
//! `cargo test --test bench_web_projection -- --ignored --nocapture`.
use lethetic::app::{App, BlockType};
use lethetic::config::Config;
use lethetic::wfe::presentation::{ProjectionContext, project_app};
use std::time::Instant;

#[test]
#[ignore]
fn bench_projection() {
    let mut app = App::new(&Config::default());
    app.show_session_manager = false;
    let chunk =
        "fn main() { println!(\"hello world {}\", 42); } // some code line here\n".repeat(60);
    for i in 0..180 {
        let kind = match i % 3 {
            0 => BlockType::User,
            1 => BlockType::Thought,
            _ => BlockType::Markdown,
        };
        app.add_segment(format!("{i} {chunk}"), kind);
    }
    let bytes: usize = app.blocks.iter().map(|b| b.content.len()).sum();
    let n = 30;
    let start = Instant::now();
    let mut last = None;
    for _ in 0..n {
        last = Some(project_app(&app, ProjectionContext::default()));
    }
    let per = start.elapsed() / n;
    let json = serde_json::to_vec(&last.unwrap()).unwrap().len();
    println!(
        "blocks={} content={}KB project_app={:?} serialized={}KB",
        app.blocks.len(),
        bytes / 1024,
        per,
        json / 1024
    );
}
