use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lethetic::app::{App, AppEventOutcome, BlockType, RenderBlock};
use lethetic::config::{Config, ToolProfile};
use lethetic::context::{FunctionCall, ToolCall};
use lethetic::ui::{Theme, render_block_to_lines};

fn python_call(source: &str) -> ToolCall {
    ToolCall {
        id: "display-call".to_string(),
        provider_id: Some("toolu-display".to_string()),
        function: FunctionCall {
            name: "python".to_string(),
            arguments: serde_json::json!({
                "code": source,
                "description": "format the preview",
                "tool_call_id": "display-call"
            }),
        },
    }
}

#[test]
fn python_tool_history_renders_real_highlighted_source_lines() {
    let call = python_call("def add( a,b ):\n return(a+b)");
    let content = format!(
        "call:python{}",
        serde_json::to_string(&call.function.arguments).unwrap()
    );
    let block = RenderBlock {
        duration_ms: None,
        block_type: BlockType::ToolCall,
        content,
        title: Some("format the preview".to_string()),
        success: None,
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    };

    let lines = render_block_to_lines(&block, 120, &Theme::default(), None);
    let rendered = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Formatted preview:"), "{rendered}");
    assert!(rendered.contains("def add(a, b):"), "{rendered}");
    assert!(rendered.contains("return a + b"), "{rendered}");
    assert!(!rendered.contains(r"\n"), "{rendered}");
    let source_lines = lines
        .iter()
        .filter(|line| {
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            text.contains("def add") || text.contains("return a + b")
        })
        .collect::<Vec<_>>();
    assert_eq!(source_lines.len(), 2);
    assert!(
        source_lines
            .iter()
            .all(|line| { line.spans.iter().any(|span| span.style.fg.is_some()) })
    );
}

#[test]
fn python_approval_v_toggles_exact_source_without_changing_decisions() {
    let config = Config {
        tool_profile: ToolProfile::PythonOnly,
        python_runtime: lethetic::config::PythonRuntimeConfig {
            target: Some(lethetic::config::PythonExecutionTarget::Host),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = App::new(&config);
    app.python_setup = None;
    app.show_prompt_editor = false;
    app.show_prompt_manager = false;
    app.show_history = false;
    app.show_session_manager = false;
    app.show_cleanup_prompt = false;
    app.show_hotkeys = false;
    app.show_palette = false;
    app.show_latest_files = false;
    app.show_model_switcher = false;
    app.show_lsp_manager = false;
    app.show_theme_menu = false;
    app.show_approval_prompt = true;
    app.pending_tool_call = Some(python_call("value=1\nvalue"));

    assert!(matches!(
        lethetic::app::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE)
        ),
        AppEventOutcome::Continue
    ));
    assert!(app.python_approval_show_original);
    lethetic::app::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    );
    assert_eq!(app.python_approval_scroll, 10);
    lethetic::app::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
    );
    assert!(!app.python_approval_show_original);
    assert_eq!(app.python_approval_scroll, 0);

    let outcome = lethetic::app::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
    );
    assert!(matches!(
        outcome,
        AppEventOutcome::ToolApproved(true, false)
    ));
    assert!(!app.python_approval_show_original);
    assert_eq!(app.python_approval_scroll, 0);
}
