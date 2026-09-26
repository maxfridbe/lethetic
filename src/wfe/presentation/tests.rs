use super::*;
use crate::config::Config;

#[test]
fn themes_have_unique_safe_ids_and_normalized_colors() {
    let themes = theme_catalog();
    let mut ids = HashSet::new();
    assert!(!themes.is_empty());
    for theme in themes {
        assert!(ids.insert(theme.theme_id));
        for color in [
            theme.colors.output_fg,
            theme.colors.input_fg,
            theme.colors.highlight_fg,
            theme.colors.system_fg,
            theme.colors.thought_fg,
            theme.colors.tool_fg,
            theme.colors.success_fg,
            theme.colors.error_fg,
            theme.colors.warning_fg,
            theme.colors.json_key_fg,
            theme.colors.json_val_fg,
            theme.colors.input_bg,
            theme.colors.thought_bg,
            theme.colors.tool_bg,
            theme.colors.terminal_bg,
        ] {
            assert_eq!(color.len(), 7);
            assert!(color.starts_with('#'));
            assert!(color[1..].bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
    }
}

#[test]
fn snapshot_projection_redacts_known_secrets_endpoints_and_paths() {
    let config = Config {
        api_key: Some("api-secret-value".to_string()),
        server_url: "https://private.example.invalid/v1".to_string(),
        ..Config::default()
    };
    let mut app = App::new(&config);
    app.cwd = "/home/private/project".to_string();
    app.current_dir = app.cwd.clone();
    let prompt_excerpt = "Do not reveal this private system instruction segment to remote views.";
    app.system_prompt = format!("system header\n{prompt_excerpt}\nsystem footer");
    app.blocks.push(RenderBlock {
        block_type: BlockType::Text,
        content: format!(
            "key={} url={} cwd={} extra=controller-secret path=/etc/private/config redirect=https://unregistered.internal/error password=hunter2 prompt={prompt_excerpt}",
            config.api_key.as_deref().unwrap(),
            config.server_url,
            app.cwd
        ),
        title: Some(config.api_key.clone().unwrap()),
        success: None,
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: Some("private-turn-id".to_string()),
        cached_lines: None,
        cached_line_count: None,
    });

    let snapshot = project_app(
        &app,
        ProjectionContext {
            additional_sensitive_values: vec!["controller-secret".to_string()],
            ..ProjectionContext::default()
        },
    );
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("api-secret-value"));
    assert!(!json.contains("private.example.invalid"));
    assert!(!json.contains("/home/private/project"));
    assert!(!json.contains("private-turn-id"));
    assert!(!json.contains("controller-secret"));
    assert!(!json.contains("/etc/private/config"));
    assert!(!json.contains("unregistered.internal"));
    assert!(!json.contains("hunter2"));
    assert!(!json.contains(prompt_excerpt));
    assert!(json.contains("[REDACTED]"));
}

#[test]
fn block_projection_is_utf8_safe_and_bounded() {
    let config = Config::default();
    let mut app = App::new(&config);
    for index in 0..205 {
        app.blocks.push(RenderBlock {
            block_type: BlockType::Text,
            content: format!("{index}:{}", "🦀".repeat(20_000)),
            title: None,
            success: None,
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        });
    }
    let snapshot = project_app(&app, ProjectionContext::default());
    assert!(snapshot.blocks.blocks.len() <= MAX_WEB_BLOCKS);
    assert!(snapshot.blocks.truncated);
    assert!(snapshot.blocks.omitted_before > 0);
    assert!(
        snapshot
            .blocks
            .blocks
            .iter()
            .map(|block| block.content.len() + block.title.as_ref().map_or(0, String::len))
            .sum::<usize>()
            <= MAX_WEB_BLOCK_TOTAL_BYTES + 256
    );
}

#[test]
fn escaped_block_projection_fits_the_server_message_budget() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.blocks.clear();
    for _ in 0..32 {
        app.blocks.push(RenderBlock {
            block_type: BlockType::Text,
            content: "\u{0001}".repeat(MAX_WEB_BLOCK_CONTENT_BYTES),
            title: None,
            success: None,
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        });
    }
    let history = (0..100)
        .map(|index| HistoryEntryView {
            entry_id: format!("history-{index}"),
            label: "\u{0001}".repeat(4096),
        })
        .collect();
    let diagnostics = (0..MAX_WEB_DIAGNOSTICS)
        .map(|_| DiagnosticView {
            code: DiagnosticCode::Unknown,
            severity: DiagnosticSeverity::Warning,
            message: "\u{0001}".repeat(MAX_WEB_DIAGNOSTIC_BYTES),
        })
        .collect();
    let snapshot = IStateSnapshot::new(
        0,
        0,
        project_app(
            &app,
            ProjectionContext {
                panel_data: Some(PanelDataView::InputHistory {
                    entries: history,
                    has_more: false,
                }),
                diagnostics,
                ..ProjectionContext::default()
            },
        ),
    )
    .unwrap();
    let encoded = serde_json::to_vec(&IServerMessage::StateSnapshot {
        snapshot: Box::new(snapshot),
    })
    .unwrap();
    assert!(encoded.len() <= MAX_SERVER_MESSAGE_BYTES);
}

#[test]
fn model_choice_ids_are_opaque_and_stable() {
    let first = model_choice_id("private-connection", "model-name");
    assert_eq!(first, model_choice_id("private-connection", "model-name"));
    assert_ne!(first, model_choice_id("other", "model-name"));
    assert!(first.starts_with("model-"));
    assert!(!first.contains("private-connection"));
    assert!(opaque_choice_id("file", &["/private/path"]).starts_with("file-"));
}

#[test]
fn redacted_fenced_json_reports_redaction_without_fake_truncation() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.blocks.push(RenderBlock {
        block_type: BlockType::Text,
        content: "```json\n{\"path\":\"/etc/private/data\"}\n```".to_string(),
        title: None,
        success: None,
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });

    let snapshot = project_app(&app, ProjectionContext::default());
    let block = snapshot.blocks.blocks.last().unwrap();
    assert!(block.content_loss.redacted);
    assert_eq!(block.content_loss.truncation, None);
    assert!(block.content.contains("[REDACTED-PATH]"));
    assert!(!block.content.contains("/etc/private/data"));
}

#[test]
fn runtime_notice_never_projects_a_raw_podman_container_id() {
    let container_id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let mut app = App::new(&Config::default());
    app.blocks.push(RenderBlock {
        block_type: BlockType::Text,
        content: format!(
            "Podman container {container_id} resumed; network: none; mounted R/W cwd: /private/work"
        ),
        title: None,
        success: None,
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });

    let snapshot = project_app(&app, ProjectionContext::default());
    let block = snapshot.blocks.blocks.last().unwrap();
    assert!(block.content_loss.redacted);
    assert_eq!(block.content_loss.truncation, None);
    assert!(!block.content.contains(container_id));
    assert!(
        block
            .content
            .contains("Podman container [REDACTED-CONTAINER-ID]")
    );
    assert!(block.content.contains("[REDACTED-PATH]"));
}

#[test]
fn rendered_runtime_notice_projects_only_the_validated_operational_name() {
    let container_id = "a".repeat(64);
    let container_name = "lethetic-python-transient-123-4";
    let notice = crate::python::PythonRuntimeNotice {
        container_id: container_id.clone(),
        container_name: container_name.to_string(),
        action: crate::python::RuntimeLaunchAction::Created,
        network: crate::config::NetworkAccess::None,
        mounted_cwd: std::path::PathBuf::from("/private/work"),
    };
    let rendered = notice.render();
    assert!(!rendered.contains(&container_id));
    assert!(rendered.contains(container_name));

    let mut app = App::new(&Config::default());
    app.blocks.push(RenderBlock {
        block_type: BlockType::Text,
        content: rendered,
        title: None,
        success: None,
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });
    let snapshot = project_app(&app, ProjectionContext::default());
    let block = snapshot.blocks.blocks.last().unwrap();
    assert!(block.content.contains(container_name));
    assert!(!block.content.contains(&container_id));
    assert!(block.content.contains("[REDACTED-PATH]"));
}

#[test]
fn complete_python_code_projection_is_valid_json_and_only_filtered() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.blocks.push(RenderBlock {
        block_type: BlockType::ToolCall,
        content: format!(
            "call:python{}",
            serde_json::json!({
                "code": "print('safe')\nharmless_keyword_assignment_identifier = True",
                "description": "Track implementation tasks and verify prerequisites",
            })
        ),
        title: Some("Track implementation tasks and verify prerequisites".to_string()),
        success: None,
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });

    let snapshot = project_app(&app, ProjectionContext::default());
    assert!(
        snapshot
            .blocks
            .blocks
            .iter()
            .all(|block| block.kind != BlockKind::Truncation)
    );
    let block = snapshot.blocks.blocks.last().unwrap();
    assert_eq!(block.content_loss, ProjectionLossView::complete());
    let tool = block.tool.as_ref().unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&tool.payload).is_ok());
    assert!(tool.payload_loss.filtered);
    assert!(!tool.payload_loss.redacted);
    assert_eq!(tool.payload_loss.truncation, None);
    assert!(!tool.payload.contains("description"));
}

#[test]
fn incomplete_python_source_uses_an_explicit_invalid_safe_projection() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.blocks.push(RenderBlock {
        block_type: BlockType::ToolCall,
        content: r#"call:python{"code":42,"description":"not executable"}"#.to_string(),
        title: Some("Python".to_string()),
        success: None,
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });

    let snapshot = project_app(&app, ProjectionContext::default());
    assert!(
        snapshot
            .blocks
            .blocks
            .iter()
            .all(|block| block.kind != BlockKind::Truncation)
    );
    let tool = snapshot
        .blocks
        .blocks
        .last()
        .unwrap()
        .tool
        .as_ref()
        .unwrap();
    assert_eq!(tool.payload, "{}");
    assert_eq!(
        tool.payload_loss,
        ProjectionLossView {
            filtered: false,
            redacted: false,
            truncation: Some(ProjectionTruncationKind::InvalidSource),
        }
    );
}

#[test]
fn tool_blocks_are_structured_and_path_redacted() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.blocks.push(RenderBlock {
        block_type: BlockType::ToolCall,
        content: r#"call:python{"code":"open('/etc/private/data')","description":"Inspect data","tool_call_id":"private-provider-id"}"#.to_string(),
        title: Some("Python".to_string()),
        success: None,
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });
    let snapshot = project_app(&app, ProjectionContext::default());
    let block = snapshot.blocks.blocks.last().unwrap();
    assert!(block.content.is_empty());
    let tool = block.tool.as_ref().unwrap();
    assert_eq!(tool.kind, ToolBlockKind::Call);
    assert_eq!(tool.tool_name, "python");
    assert!(tool.payload_loss.filtered);
    assert!(tool.payload_loss.redacted);
    assert_eq!(tool.payload_loss.truncation, None);
    assert!(serde_json::from_str::<serde_json::Value>(&tool.payload).is_ok());
    assert!(!tool.payload.contains("/etc/private/data"));
    assert!(!tool.payload.contains("description"));
    assert!(!tool.payload.contains("private-provider-id"));
}

#[test]
fn redacted_tool_result_retains_complete_markdown_payload() {
    let config = Config::default();
    let mut app = App::new(&config);
    let private_root = "/etc/private/project";
    app.cwd = private_root.to_string();
    app.current_dir = private_root.to_string();
    app.blocks.push(RenderBlock {
        block_type: BlockType::ToolResult,
        content: format!(
            "# Repository Overview: `{private_root}`\n\n## Directory Structure\n```text\nproject/\n└── src/\n```\n\nTail retained."
        ),
        title: Some("repo_overview: result".to_string()),
        success: Some(true),
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });

    let snapshot = project_app(&app, ProjectionContext::default());
    let block = snapshot.blocks.blocks.last().unwrap();
    assert!(block.content.is_empty());
    assert_eq!(block.content_loss, ProjectionLossView::complete());
    let tool = block.tool.as_ref().unwrap();
    assert_eq!(tool.kind, ToolBlockKind::Result);
    assert!(!tool.payload_loss.filtered);
    assert!(tool.payload_loss.redacted);
    assert_eq!(tool.payload_loss.truncation, None);
    assert!(tool.payload.contains("# Repository Overview"));
    assert!(tool.payload.contains("```text"));
    assert!(tool.payload.contains("Tail retained."));
    assert!(!tool.payload.contains(private_root));
}

#[test]
fn successful_scrubbed_python_result_preserves_harmless_text_and_public_traceback_frames() {
    let config = Config::default();
    let mut app = App::new(&config);
    let private_root = "/home/example/private-workspace";
    app.cwd = private_root.to_string();
    app.current_dir = private_root.to_string();
    app.blocks.push(RenderBlock {
        block_type: BlockType::ToolResult,
        content: format!(
            concat!(
                "harmless_keyword_assignment_identifier = True\n",
                "  File \"/usr/lib/python3.13/pathlib.py\", line 540, in __str__\n",
                "  File \"/home/example/.venv/lib/python3.13/site-packages/pkg/main.py\", line 1, in run\n",
                "workspace={}\n"
            ),
            private_root
        ),
        title: Some("python: result".to_string()),
        success: Some(true),
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });

    let snapshot = project_app(&app, ProjectionContext::default());
    let tool = snapshot
        .blocks
        .blocks
        .last()
        .unwrap()
        .tool
        .as_ref()
        .unwrap();
    assert!(
        tool.payload
            .contains("harmless_keyword_assignment_identifier = True")
    );
    assert!(tool.payload.contains("/usr/lib/python3.13/pathlib.py"));
    assert!(!tool.payload.contains("/home/example"));
    assert!(tool.payload.contains("[REDACTED-PATH]") || tool.payload.contains("[REDACTED]"));
    assert!(tool.payload_loss.redacted);
    assert_eq!(tool.payload_loss.truncation, None);
}

#[test]
fn tool_call_block_never_discloses_a_tail_hidden_by_the_approval_limit() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.show_approval_prompt = true;
    let tail = "tail-marker-should-not-appear";
    let arguments = serde_json::json!({
        "items": vec!["abcd"; 6_000],
        "tail": tail,
    });
    app.blocks.push(RenderBlock {
        block_type: BlockType::ToolCall,
        content: format!(
            "call:write_file{}",
            serde_json::to_string(&arguments).unwrap()
        ),
        title: Some("Write file".to_string()),
        success: None,
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });

    let snapshot = project_app(
        &app,
        ProjectionContext {
            pending_approval: Some(PendingApprovalView {
                approval_id: "approval-1".to_string(),
                session_id: app.session_id.clone(),
                tool_call_id: "tool-call-1".to_string(),
                tool_name: "write_file".to_string(),
                description: "Review write".to_string(),
                preview: approval_preview("write_file", &arguments),
                preview_redacted: false,
                preview_truncated: false,
                can_view_original: false,
                allowed_decisions: vec![
                    ApprovalDecision::ApproveOnce,
                    ApprovalDecision::ApproveAlways,
                    ApprovalDecision::Deny,
                ],
            }),
            ..ProjectionContext::default()
        },
    );

    let approval = snapshot.pending_approval.as_ref().unwrap();
    assert!(approval.preview_truncated);
    assert!(!approval.preview.contains(tail));
    let tool = snapshot
        .blocks
        .blocks
        .last()
        .unwrap()
        .tool
        .as_ref()
        .unwrap();
    assert!(!tool.payload_loss.filtered);
    assert!(!tool.payload_loss.redacted);
    assert_eq!(
        tool.payload_loss.truncation,
        Some(ProjectionTruncationKind::SizeLimit)
    );
    assert!(!tool.payload.contains(tail));
    assert!(!serde_json::to_string(&snapshot).unwrap().contains(tail));
}

#[test]
fn lossy_question_uses_only_fixed_notice_and_remains_cancellable() {
    let mut app = App::new(&Config::default());
    app.add_logical_turn_user_segment("question turn".to_string());
    app.is_asking_user = true;
    let cancel_id = app.active_cancellation_id().unwrap().to_string();
    let raw_secret = "/etc/private/question-secret";
    let snapshot = project_app(
        &app,
        ProjectionContext {
            pending_question: Some(PendingQuestionView {
                form_id: "form-1".to_string(),
                session_id: app.session_id.clone(),
                tool_call_id: "question-call".to_string(),
                questions: vec![QuestionPromptView {
                    question_id: "question-1".to_string(),
                    prompt: format!("{raw_secret} {}", "oversized ".repeat(1_000)),
                    options: Vec::new(),
                    multiple: false,
                    allows_other: true,
                }],
                content_truncated: false,
            }),
            ..ProjectionContext::default()
        },
    );

    let question = snapshot.pending_question.unwrap();
    assert!(question.content_truncated);
    assert_eq!(question.questions.len(), 1);
    assert_eq!(question.questions[0].prompt, QUESTION_PREVIEW_UNAVAILABLE);
    assert!(!question.questions[0].prompt.contains(raw_secret));
    assert!(!question.questions[0].allows_other);
    assert!(snapshot.activity.cancellable);
    assert_eq!(
        snapshot.activity.cancel_id.as_deref(),
        Some(cancel_id.as_str())
    );
}

#[test]
fn one_opaque_cancel_id_spans_turn_phases_and_rotates_after_settlement() {
    let mut app = App::new(&Config::default());
    app.add_logical_turn_user_segment("first turn".to_string());
    let cancel_id = app.active_cancellation_id().unwrap().to_string();

    app.is_processing = true;
    let processing = project_app(&app, ProjectionContext::default()).activity;
    assert!(processing.cancellable);
    assert_eq!(processing.cancel_id.as_deref(), Some(cancel_id.as_str()));

    app.is_processing = false;
    app.show_approval_prompt = true;
    let approval = project_app(&app, ProjectionContext::default()).activity;
    assert_eq!(approval.kind, ActivityKind::AwaitingApproval);
    assert_eq!(approval.cancel_id.as_deref(), Some(cancel_id.as_str()));

    app.show_approval_prompt = false;
    app.is_executing_tool = true;
    let tool = project_app(&app, ProjectionContext::default()).activity;
    assert_eq!(tool.kind, ActivityKind::ExecutingTool);
    assert_eq!(tool.cancel_id.as_deref(), Some(cancel_id.as_str()));

    app.is_executing_tool = false;
    app.is_asking_user = true;
    let question = project_app(&app, ProjectionContext::default()).activity;
    assert_eq!(question.kind, ActivityKind::AwaitingAnswer);
    assert_eq!(question.cancel_id.as_deref(), Some(cancel_id.as_str()));

    app.is_asking_user = false;
    app.settle_logical_turn();
    let idle = project_app(&app, ProjectionContext::default()).activity;
    assert!(!idle.cancellable);
    assert!(idle.cancel_id.is_none());

    app.add_logical_turn_user_segment("second turn".to_string());
    app.is_processing = true;
    let successor = project_app(&app, ProjectionContext::default()).activity;
    assert_ne!(successor.cancel_id.as_deref(), Some(cancel_id.as_str()));
}

#[test]
fn lsp_activity_precedes_generic_tool_activity() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.is_executing_tool = true;
    app.lsp_install_in_progress = true;
    let snapshot = project_app(&app, ProjectionContext::default());
    assert_eq!(snapshot.activity.kind, ActivityKind::ManagingLsp);
}

#[test]
fn loading_progress_projects_the_tui_zero_to_one_hundred_units() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.is_loading_session = true;
    for (input, expected) in [
        (0.0, 0),
        (10.0, 10),
        (49.6, 50),
        (100.0, 100),
        (-4.0, 0),
        (140.0, 100),
        (f32::NAN, 0),
        (f32::INFINITY, 0),
    ] {
        app.load_progress = input;
        let snapshot = project_app(&app, ProjectionContext::default());
        assert_eq!(snapshot.activity.kind, ActivityKind::LoadingSession);
        assert_eq!(snapshot.activity.progress_percent, Some(expected));
    }
}

#[test]
fn panel_data_is_typed_bounded_and_redacted() {
    let config = Config::default();
    let app = App::new(&config);
    let snapshot = project_app(
        &app,
        ProjectionContext {
            panel_data: Some(PanelDataView::InputHistory {
                entries: vec![HistoryEntryView {
                    entry_id: "history-1".to_string(),
                    label: "read /etc/private/data from https://internal.invalid".to_string(),
                }],
                has_more: false,
            }),
            ..ProjectionContext::default()
        },
    );
    assert_eq!(snapshot.overlay.active_panel, Some(PanelId::InputHistory));
    let PanelDataView::InputHistory { entries, .. } = snapshot.overlay.data.unwrap() else {
        panic!("expected history panel data");
    };
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].label.contains("/etc/private/data"));
    assert!(!entries[0].label.contains("internal.invalid"));
}

#[test]
fn system_prompt_editor_marks_redaction_and_truncation_as_lossy() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.show_session_manager = false;
    app.show_prompt_editor = true;
    app.system_prompt = "Keep this private prompt and read /etc/private/system-config".to_string();

    let snapshot = project_app(
        &app,
        ProjectionContext {
            panel_data: Some(PanelDataView::SystemPrompts {
                prompts: Vec::new(),
                editor_content: Some(app.system_prompt.clone()),
                content_truncated: false,
            }),
            ..ProjectionContext::default()
        },
    );
    let PanelDataView::SystemPrompts {
        editor_content,
        content_truncated,
        ..
    } = snapshot.overlay.data.unwrap()
    else {
        panic!("expected system prompt panel data");
    };
    assert!(content_truncated, "redaction must be reported as loss");
    let editor_content = editor_content.unwrap();
    assert!(!editor_content.contains("Keep this private prompt"));
    assert!(!editor_content.contains("/etc/private/system-config"));

    app.system_prompt = "safe text ".repeat(MAX_WEB_SYSTEM_PROMPT_EDITOR_BYTES);
    let snapshot = project_app(
        &app,
        ProjectionContext {
            panel_data: Some(PanelDataView::SystemPrompts {
                prompts: Vec::new(),
                editor_content: Some(app.system_prompt.clone()),
                content_truncated: false,
            }),
            ..ProjectionContext::default()
        },
    );
    let PanelDataView::SystemPrompts {
        editor_content,
        content_truncated,
        ..
    } = snapshot.overlay.data.unwrap()
    else {
        panic!("expected system prompt panel data");
    };
    assert!(content_truncated, "truncation must be reported as loss");
    assert!(editor_content.unwrap().len() <= MAX_WEB_SYSTEM_PROMPT_EDITOR_BYTES);
}

#[test]
fn redacted_approval_preview_keeps_bound_decisions() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.show_session_manager = false;
    app.show_approval_prompt = true;
    let original_path = "/etc/private/approval-secret";
    let snapshot = project_app(
        &app,
        ProjectionContext {
            pending_approval: Some(PendingApprovalView {
                approval_id: "approval-1".to_string(),
                session_id: app.session_id.clone(),
                tool_call_id: "tool-call-1".to_string(),
                tool_name: "python".to_string(),
                description: "Review exact code".to_string(),
                preview: format!("open('{original_path}')"),
                preview_redacted: false,
                preview_truncated: false,
                can_view_original: true,
                allowed_decisions: vec![
                    ApprovalDecision::ApproveAlways,
                    ApprovalDecision::Deny,
                    ApprovalDecision::ApproveOnce,
                ],
            }),
            ..ProjectionContext::default()
        },
    );
    let approval = snapshot.pending_approval.unwrap();
    assert!(approval.preview_redacted, "redaction must be reported");
    assert!(!approval.preview_truncated);
    assert!(!approval.preview.contains(original_path));
    assert!(!approval.can_view_original);
    assert_eq!(
        approval.allowed_decisions,
        vec![
            ApprovalDecision::ApproveOnce,
            ApprovalDecision::ApproveAlways,
            ApprovalDecision::Deny,
        ]
    );
}

#[test]
fn truncated_approval_preview_keeps_bound_decisions() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.show_session_manager = false;
    app.show_approval_prompt = true;
    let snapshot = project_app(
        &app,
        ProjectionContext {
            pending_approval: Some(PendingApprovalView {
                approval_id: "approval-1".to_string(),
                session_id: app.session_id.clone(),
                tool_call_id: "tool-call-1".to_string(),
                tool_name: "write_file".to_string(),
                description: "Review complete content".to_string(),
                preview: "safe text ".repeat(MAX_WEB_APPROVAL_PREVIEW_BYTES),
                preview_redacted: false,
                preview_truncated: false,
                can_view_original: true,
                allowed_decisions: vec![
                    ApprovalDecision::ApproveAlways,
                    ApprovalDecision::Deny,
                    ApprovalDecision::ApproveOnce,
                ],
            }),
            ..ProjectionContext::default()
        },
    );
    let approval = snapshot.pending_approval.unwrap();
    assert!(!approval.preview_redacted);
    assert!(approval.preview_truncated);
    assert!(approval.preview.ends_with("… [truncated]"));
    assert!(!approval.can_view_original);
    assert_eq!(
        approval.allowed_decisions,
        vec![
            ApprovalDecision::ApproveOnce,
            ApprovalDecision::ApproveAlways,
            ApprovalDecision::Deny,
        ]
    );
}

#[test]
fn combined_approval_redaction_and_truncation_are_distinct() {
    let config = Config::default();
    let mut app = App::new(&config);
    app.show_session_manager = false;
    app.show_approval_prompt = true;
    let original_path = "/etc/private/approval-secret";
    let snapshot = project_app(
        &app,
        ProjectionContext {
            pending_approval: Some(PendingApprovalView {
                approval_id: "approval-1".to_string(),
                session_id: app.session_id.clone(),
                tool_call_id: "tool-call-1".to_string(),
                tool_name: "write_file".to_string(),
                description: "Review complete content".to_string(),
                preview: format!(
                    "path={original_path}\n{}",
                    "safe text ".repeat(MAX_WEB_APPROVAL_PREVIEW_BYTES)
                ),
                preview_redacted: false,
                preview_truncated: false,
                can_view_original: true,
                allowed_decisions: vec![
                    ApprovalDecision::ApproveOnce,
                    ApprovalDecision::ApproveAlways,
                    ApprovalDecision::Deny,
                ],
            }),
            ..ProjectionContext::default()
        },
    );
    let approval = snapshot.pending_approval.unwrap();
    assert!(approval.preview_redacted);
    assert!(approval.preview_truncated);
    assert!(!approval.preview.contains(original_path));
    assert!(!approval.can_view_original);
    assert!(
        approval
            .allowed_decisions
            .contains(&ApprovalDecision::ApproveOnce)
    );
}

#[test]
fn noncurrent_delete_confirmation_remains_presentable() {
    const OTHER_SESSION: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

    let config = Config::default();
    let mut app = App::new(&config);
    app.show_session_manager = false;
    let snapshot = project_app(
        &app,
        ProjectionContext {
            panel_data: Some(PanelDataView::Confirmation {
                confirmation: ConfirmationView {
                    confirmation_id: "confirmation-1".to_string(),
                    action: DestructiveActionView::DeleteSession,
                    session_id: Some(OTHER_SESSION.to_string()),
                    title: "Delete session?".to_string(),
                    message: "Delete the exact selected session.".to_string(),
                },
            }),
            ..ProjectionContext::default()
        },
    );
    let PanelDataView::Confirmation { confirmation } = snapshot.overlay.data.unwrap() else {
        panic!("expected confirmation panel data");
    };
    assert_eq!(confirmation.session_id.as_deref(), Some(OTHER_SESSION));

    let rejected = project_app(
        &app,
        ProjectionContext {
            panel_data: Some(PanelDataView::Confirmation {
                confirmation: ConfirmationView {
                    confirmation_id: "confirmation-2".to_string(),
                    action: DestructiveActionView::ClearContext,
                    session_id: Some(OTHER_SESSION.to_string()),
                    title: "Clear context?".to_string(),
                    message: "Clear context.".to_string(),
                },
            }),
            ..ProjectionContext::default()
        },
    );
    assert!(rejected.overlay.data.is_none());
}

#[test]
fn podman_status_exposes_only_validated_retained_or_active_transient_names() {
    use crate::config::{
        AccessMode, NetworkAccess, PackageAccess, PythonExecutionTarget, SandboxBackend,
        ToolProfile,
    };
    use crate::python::{PythonContainerIdentity, PythonContainerKind};

    let mut config = Config::default();
    config.tool_profile = ToolProfile::PythonOnly;
    config.python_runtime.target = Some(PythonExecutionTarget::Sandbox);
    config.python_runtime.sandbox.backend = Some(SandboxBackend::Podman);
    config.python_runtime.sandbox.network = Some(NetworkAccess::Nonlocal);
    config.python_runtime.sandbox.workspace_access = Some(AccessMode::ReadWrite);
    config.python_runtime.sandbox.package_access = PackageAccess::Session;
    let mut app = App::new(&config);
    let runtime_id = "550e8400-e29b-41d4-a716-446655440000";
    let retained_name = format!("lethetic-python-{runtime_id}");
    app.python_runtime_id = Some(runtime_id.to_string());

    assert_eq!(
        project_python_container_with_identity(&app, None),
        Some(PythonContainerView {
            kind: PythonContainerKindView::Retained,
            name: retained_name.clone(),
            active: false,
        })
    );
    assert_eq!(
        project_python_container_with_identity(
            &app,
            PythonContainerIdentity::retained(runtime_id, true),
        ),
        Some(PythonContainerView {
            kind: PythonContainerKindView::Retained,
            name: retained_name,
            active: true,
        })
    );
    app.python_runtime_id = Some("not-a-runtime-id".to_string());
    assert_eq!(project_python_container_with_identity(&app, None), None);

    app.config.python_runtime.sandbox.network = Some(NetworkAccess::Full);
    app.config.python_runtime.sandbox.package_access = PackageAccess::Disabled;
    let transient = PythonContainerIdentity::transient("lethetic-python-transient-123-4").unwrap();
    assert_eq!(
        project_python_container_with_identity(&app, Some(transient.clone())),
        Some(PythonContainerView {
            kind: PythonContainerKindView::Transient,
            name: transient.name.clone(),
            active: true,
        })
    );
    assert_eq!(project_python_container_with_identity(&app, None), None);
    assert_eq!(
        project_python_container_with_identity(
            &app,
            Some(PythonContainerIdentity {
                kind: PythonContainerKind::Retained,
                name: "lethetic-python-550e8400-e29b-41d4-a716-446655440000".to_string(),
                active: true,
            }),
        ),
        None
    );

    app.config.python_runtime.target = Some(PythonExecutionTarget::Host);
    assert_eq!(
        project_python_container_with_identity(&app, Some(transient)),
        None
    );
}

#[test]
fn wfe_status_uses_the_shared_summary_and_cost_markers() {
    let mut app = App::new(&Config::default());
    app.stop_reason = "Ready".to_string();
    app.model_name = "safe-model".to_string();
    app.tokens_per_s = 12.345;
    app.pp_tokens_per_s = 6.789;
    app.max_tokens = 4_096;
    app.memory_usage = 321;
    app.git_status = "2 files dirty".to_string();
    app.server_usage = Some(Usage {
        reported_cost_nanos: None,
        uncached_input_tokens: 11,
        cache_read_input_tokens: 22,
        cache_creation_input_tokens: 33,
        output_tokens: 44,
        total_input_tokens: Some(66),
        breakdown_complete: true,
    });
    app.accounting.latest_logical_turn.estimated_cost = Some(EstimatedCost {
        currency: "USD".to_string(),
        nanos: 12_345,
        incomplete: false,
        mixed_pricing: false,
        long_context_applied: false,
        pricing_effective_as_of: "2026-01-01".to_string(),
        pricing_valid_through: None,
        provenance_kind: "fixture".to_string(),
    });
    app.accounting.latest_logical_turn.unpriced_request_count = 1;

    let summary = crate::status_summary::StatusSummary::from_app(&app);
    let snapshot = project_app(&app, ProjectionContext::default());

    assert_eq!(snapshot.status.stop_reason, summary.stop_reason);
    assert_eq!(snapshot.status.model_label, summary.model_label);
    assert_eq!(snapshot.status.provider_label, summary.provider_label);
    assert_eq!(snapshot.status.tokens_per_second.as_deref(), Some("12.35"));
    assert_eq!(
        snapshot.status.prompt_tokens_per_second.as_deref(),
        Some("6.79")
    );
    assert_eq!(
        snapshot.status.context_tokens,
        summary.context_tokens.to_string()
    );
    assert_eq!(
        snapshot.status.context_limit_tokens,
        summary.context_limit_tokens.to_string()
    );
    assert_eq!(
        snapshot.status.request_usage,
        summary.request_usage.as_ref().map(project_usage)
    );
    assert_eq!(snapshot.status.memory_mebibytes, "321");
    assert_eq!(snapshot.status.git_state, GitStateView::Dirty);

    let projected_cost = snapshot
        .usage
        .latest_turn
        .estimated_cost
        .expect("shared logical-turn cost must be projected");
    let shared_cost = summary
        .latest_turn_cost
        .expect("fixture must have a shared logical-turn cost");
    assert!(shared_cost.incomplete);
    assert_eq!(
        projected_cost.display,
        crate::status_summary::format_estimated_cost(&shared_cost)
    );
}

#[test]
fn wfe_stop_reason_uses_fixed_categories_instead_of_raw_errors_or_prompts() {
    let mut app = App::new(&Config::default());
    app.stop_reason =
        "✗ Server error: quota denied for tenant blue request req-private-42".to_string();
    let snapshot = project_app(&app, ProjectionContext::default());
    assert_eq!(snapshot.status.stop_reason, "Provider request failed.");
    assert!(snapshot.status.stop_reason_loss.filtered);
    let encoded = serde_json::to_string(&snapshot.status).unwrap();
    assert!(!encoded.contains("tenant blue"));
    assert!(!encoded.contains("req-private-42"));

    app.is_asking_user = true;
    app.stop_reason = "⏸ Waiting for your answer: private prompt fragment".to_string();
    let snapshot = project_app(&app, ProjectionContext::default());
    assert_eq!(snapshot.status.stop_reason, "Awaiting user answer.");
    assert!(snapshot.status.stop_reason_loss.filtered);
    assert!(
        !serde_json::to_string(&snapshot.status)
            .unwrap()
            .contains("private prompt fragment")
    );
}

#[test]
fn serialized_snapshot_replaces_provider_and_failed_tool_errors_and_hides_block_costs() {
    let config = Config {
        estimate_cost: Some(false),
        ..Config::default()
    };
    let mut app = App::new(&config);
    app.blocks.clear();
    app.stop_reason =
        "✗ Server error: provider tenant violet rejected request req-provider-7".to_string();
    app.add_segment("safe request".to_string(), BlockType::User);
    app.blocks.last_mut().unwrap().estimated_cost = Some(EstimatedCost {
        currency: "USD".to_string(),
        nanos: 8_765_432_109_876,
        incomplete: false,
        mixed_pricing: false,
        long_context_applied: false,
        pricing_effective_as_of: "2026-01-01".to_string(),
        pricing_valid_through: None,
        provenance_kind: "private-cost-fixture".to_string(),
    });
    let provider_error =
        "provider tenant violet rejected request req-provider-7 after private prompt fragment";
    app.add_segment(
        format!("\nwarning ERROR: {provider_error}\n"),
        BlockType::ProviderError,
    );
    let tool_error = "tool tenant orange failed request req-tool-9 with private runtime diagnostic";
    app.add_segment_with_title(
        tool_error.to_string(),
        BlockType::ToolResult,
        "Run validation".to_string(),
    );
    app.blocks.last_mut().unwrap().success = Some(false);

    assert!(app.blocks.iter().any(|block| {
        block.block_type == BlockType::ProviderError && block.content.contains(provider_error)
    }));
    assert!(app.blocks.iter().any(|block| {
        block.block_type == BlockType::ToolResult && block.content.contains(tool_error)
    }));

    let snapshot = project_app(&app, ProjectionContext::default());
    let encoded = serde_json::to_string(&snapshot).unwrap();
    for private in [
        provider_error,
        tool_error,
        "req-provider-7",
        "req-tool-9",
        "private prompt fragment",
        "private runtime diagnostic",
        "8765432109876",
        "private-cost-fixture",
    ] {
        assert!(!encoded.contains(private), "snapshot leaked {private:?}");
    }
    assert!(
        snapshot
            .blocks
            .blocks
            .iter()
            .all(|block| block.estimated_cost.is_none())
    );

    let provider = snapshot
        .blocks
        .blocks
        .iter()
        .find(|block| block.content == "Provider request failed.")
        .expect("typed provider error must use the fixed browser projection");
    assert_eq!(provider.kind, BlockKind::Text);
    assert!(provider.content_loss.filtered);
    assert!(!provider.content_loss.redacted);
    assert_eq!(provider.content_loss.truncation, None);

    let failed_tool = snapshot
        .blocks
        .blocks
        .iter()
        .find(|block| block.kind == BlockKind::ToolResult)
        .and_then(|block| block.tool.as_ref())
        .expect("failed tool result must remain a typed tool block");
    assert_eq!(failed_tool.payload, "Tool execution failed.");
    assert!(failed_tool.payload_loss.filtered);
    assert!(!failed_tool.payload_loss.redacted);
    assert_eq!(failed_tool.payload_loss.truncation, None);
    assert_eq!(snapshot.status.stop_reason, "Provider request failed.");
}

#[test]
fn legacy_text_error_markers_keep_provider_and_tool_provenance() {
    for (marker, fixed) in [
        ("ERROR:", "Provider request failed."),
        ("PROVIDER CHECKPOINT ERROR:", "Provider request failed."),
        (
            "PROVIDER TERMINAL CHECKPOINT ERROR FOR AN EARLIER REQUEST:",
            "Provider request failed.",
        ),
        (
            "PROVIDER CANCELLATION CHECKPOINT ERROR FOR AN EARLIER REQUEST:",
            "Provider request failed.",
        ),
        (
            "SESSION SAVE ERROR: streaming response was contained:",
            "Provider request failed.",
        ),
        ("SESSION SAVE ERROR:", "Tool execution failed."),
        (
            "SESSION SAVE ERROR: refusing to continue the provider tool call:",
            "Tool execution failed.",
        ),
        ("PYTHON AUDIT ERROR:", "Tool execution failed."),
        ("NONLOCAL PYTHON PREFLIGHT ERROR:", "Tool execution failed."),
    ] {
        let content = format!(
            "Partial assistant response.\n\n{} {marker} private details\n",
            crate::icons::WARNING
        );
        assert_eq!(
            legacy_text_error_projection(&content).as_deref(),
            Some(format!("Partial assistant response.\n\n{fixed}").as_str()),
            "marker {marker:?} was not recognized"
        );
    }
}

#[test]
fn legacy_error_block_shapes_are_filtered_without_losing_safe_partial_text() {
    let mut app = App::new(&Config::default());
    app.blocks.clear();
    let provider_sentinel = "abcabcabcabcabcabcabcabcabcabcabcabc";
    let tool_sentinel = "defdefdefdefdefdefdefdefdefdefdefdef";
    let patch_sentinel = "ghighighighighighighighighighighighi";
    app.blocks.push(RenderBlock {
        block_type: BlockType::Text,
        content: format!(
            "Partial assistant response.\n\n{} ERROR: quota denied tenant violet {provider_sentinel}\n",
            crate::icons::WARNING
        ),
        title: None,
        success: Some(true),
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });
    app.blocks.push(RenderBlock {
        block_type: BlockType::ToolResult,
        content: format!("\nSub-agent failed: quota denied tenant orange {tool_sentinel}\n"),
        title: Some("Run validation".to_string()),
        success: Some(true),
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });
    app.blocks.push(RenderBlock {
        block_type: BlockType::ToolResult,
        content: format!(
            "STDOUT:\npatch diagnostic tenant amber {patch_sentinel}\nSTDERR:\npatch failed"
        ),
        title: Some("Apply patch".to_string()),
        success: Some(true),
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });

    assert!(crate::app::migrate_legacy_error_blocks(&mut app.blocks));
    assert_eq!(app.blocks.len(), 4);
    assert_eq!(app.blocks[0].block_type, BlockType::Text);
    assert_eq!(app.blocks[1].block_type, BlockType::ProviderError);
    assert!(app.blocks[1].content.contains(provider_sentinel));
    assert_eq!(app.blocks[2].block_type, BlockType::ToolError);
    assert!(app.blocks[2].content.contains(tool_sentinel));
    assert_eq!(app.blocks[3].block_type, BlockType::ToolError);
    assert!(app.blocks[3].content.contains(patch_sentinel));

    let snapshot = project_app(&app, ProjectionContext::default());
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(!encoded.contains(provider_sentinel));
    assert!(!encoded.contains(tool_sentinel));
    assert!(!encoded.contains(patch_sentinel));
    assert!(!encoded.contains("quota denied"));
    assert!(!encoded.contains("patch diagnostic"));

    let legacy_partial = &snapshot.blocks.blocks[0];
    assert_eq!(legacy_partial.kind, BlockKind::Text);
    assert_eq!(legacy_partial.content.trim(), "Partial assistant response.");
    assert!(!legacy_partial.content_loss.filtered);

    let legacy_provider = &snapshot.blocks.blocks[1];
    assert_eq!(legacy_provider.kind, BlockKind::Text);
    assert_eq!(legacy_provider.content, "Provider request failed.");
    assert!(legacy_provider.content_loss.filtered);
    assert_eq!(legacy_provider.content_loss.truncation, None);

    let legacy_tool = snapshot.blocks.blocks[2].tool.as_ref().unwrap();
    assert_eq!(legacy_tool.payload, "Tool execution failed.");
    assert!(legacy_tool.payload_loss.filtered);
    assert_eq!(legacy_tool.payload_loss.truncation, None);

    let legacy_patch = snapshot.blocks.blocks[3].tool.as_ref().unwrap();
    assert_eq!(legacy_patch.payload, "Tool execution failed.");
    assert!(legacy_patch.payload_loss.filtered);
    assert_eq!(legacy_patch.payload_loss.truncation, None);
}

#[test]
fn merged_legacy_success_and_failure_keeps_prefix_but_filters_error_tail() {
    let mut app = App::new(&Config::default());
    app.blocks.clear();
    let raw = "validation completed\nERROR: quota denied tenant violet merged-local-42";
    app.blocks.push(RenderBlock {
        block_type: BlockType::ToolResult,
        content: raw.to_string(),
        title: Some("Action".to_string()),
        success: Some(true),
        prompt_tokens: None,
        completion_tokens: None,
        usage: None,
        estimated_cost: None,
        logical_turn_id: None,
        cached_lines: None,
        cached_line_count: None,
    });

    assert!(crate::app::migrate_legacy_error_blocks(&mut app.blocks));
    assert_eq!(app.blocks.len(), 2);
    assert_eq!(app.blocks[0].block_type, BlockType::ToolResult);
    assert_eq!(app.blocks[1].block_type, BlockType::ToolError);
    assert_eq!(
        app.blocks
            .iter()
            .map(|block| block.content.as_str())
            .collect::<String>(),
        raw
    );

    let snapshot = project_app(&app, ProjectionContext::default());
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(!encoded.contains("quota denied"));
    assert!(!encoded.contains("merged-local-42"));
    let payloads = snapshot
        .blocks
        .blocks
        .iter()
        .filter_map(|block| block.tool.as_ref())
        .map(|tool| tool.payload.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        payloads,
        vec!["validation completed\n", "Tool execution failed."]
    );
}

#[test]
fn all_known_legacy_tool_error_families_use_fixed_browser_payloads() {
    let fixtures = [
        (
            "Syntax Error in tool call: malformed syntax syntaxviolet",
            "syntaxviolet",
        ),
        ("LSP error: connection closed lsporange", "lsporange"),
        (
            "[find_symbol fallback]\nERROR: fallback failed fallbackamber",
            "fallbackamber",
        ),
        (
            "source.rs:1:match\nSTDERR: permission denied searchcrimson",
            "searchcrimson",
        ),
        (
            "--- Page 1 Error: PDF is encrypted pdfscarlet ---",
            "pdfscarlet",
        ),
        (
            "EXIT_CODE: 0\nSTDOUT:\nok\nSTDERR:\n\nEXIT_CODE: 1\nSTDOUT:\n\nSTDERR:\nquota denied shellviolet",
            "shellviolet",
        ),
        (
            "EXIT_CODE: 0\nSTDOUT:\nok\nSTDERR:\nSTDOUT:\npatch diagnostic patchorange\nSTDERR:\npatch failed",
            "patchorange",
        ),
        (
            "EXIT_CODE: 0\n... [Output truncated. Full output (25000 characters) saved locally] ...\n\nSTDOUT:\npatch diagnostic truncatedviolet\nSTDERR:\npatch failed",
            "truncatedviolet",
        ),
        (
            "EXIT_CODE: 0\n... [Output truncated. Full output was not saved because secure host storage rejected the path: refusing symlink storageteal] ...",
            "storageteal",
        ),
        (
            "EXIT_CODE: 0\nSTDOUT:\nok\nSTDERR:\nEXIT_CODE: 0\nSTDOUT:\npatch diagnostic stderrheaderteal\nSTDERR:\npatch failed",
            "stderrheaderteal",
        ),
    ];
    let mut app = App::new(&Config::default());
    app.blocks = fixtures
        .iter()
        .map(|(content, _)| RenderBlock {
            block_type: BlockType::ToolResult,
            content: (*content).to_string(),
            title: Some("Action".to_string()),
            success: Some(true),
            prompt_tokens: None,
            completion_tokens: None,
            usage: None,
            estimated_cost: None,
            logical_turn_id: None,
            cached_lines: None,
            cached_line_count: None,
        })
        .collect();

    assert!(crate::app::migrate_legacy_error_blocks(&mut app.blocks));
    for (_, sentinel) in fixtures {
        assert!(
            app.blocks
                .iter()
                .any(|block| block.content.contains(sentinel)),
            "local transcript lost {sentinel}"
        );
    }

    let snapshot = project_app(&app, ProjectionContext::default());
    let encoded = serde_json::to_string(&snapshot).unwrap();
    for (_, sentinel) in fixtures {
        assert!(!encoded.contains(sentinel), "snapshot leaked {sentinel}");
    }
    let failures = snapshot
        .blocks
        .blocks
        .iter()
        .filter_map(|block| block.tool.as_ref())
        .filter(|tool| tool.payload == "Tool execution failed.")
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), fixtures.len());
    assert!(failures.iter().all(|tool| tool.payload_loss.filtered));
}

#[test]
fn typed_tool_error_cannot_merge_into_a_same_title_success_block() {
    let mut app = App::new(&Config::default());
    app.blocks.clear();
    let failed_payload = "private failure request req-failed-11 for tenant crimson";
    app.add_segment_with_title(
        failed_payload.to_string(),
        BlockType::ToolError,
        "Run validation".to_string(),
    );
    app.add_segment_with_title(
        "validation completed".to_string(),
        BlockType::ToolResult,
        "Run validation".to_string(),
    );
    app.blocks.last_mut().unwrap().success = Some(true);

    assert_eq!(app.blocks.len(), 2);
    assert_eq!(app.blocks[0].block_type, BlockType::ToolError);
    assert_eq!(app.blocks[0].success, Some(false));
    assert_eq!(app.blocks[1].block_type, BlockType::ToolResult);
    assert_eq!(app.blocks[1].success, Some(true));

    let snapshot = project_app(&app, ProjectionContext::default());
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(!encoded.contains(failed_payload));
    assert!(!encoded.contains("req-failed-11"));
    let payloads = snapshot
        .blocks
        .blocks
        .iter()
        .filter_map(|block| block.tool.as_ref())
        .map(|tool| (&tool.payload, &tool.payload_loss))
        .collect::<Vec<_>>();
    assert!(
        payloads.iter().any(|(payload, loss)| {
            payload.as_str() == "Tool execution failed." && loss.filtered
        })
    );
    assert!(payloads.iter().any(|(payload, loss)| {
        payload.as_str() == "validation completed" && *loss == &ProjectionLossView::complete()
    }));
}

#[test]
fn disabled_cost_estimation_is_absent_from_wfe_usage() {
    let config = Config {
        estimate_cost: Some(false),
        ..Config::default()
    };
    let mut app = App::new(&config);
    let cost = EstimatedCost {
        currency: "USD".to_string(),
        nanos: 98_765,
        incomplete: false,
        mixed_pricing: false,
        long_context_applied: false,
        pricing_effective_as_of: "2026-01-01".to_string(),
        pricing_valid_through: None,
        provenance_kind: "fixture".to_string(),
    };
    app.accounting.latest_logical_turn.estimated_cost = Some(cost.clone());
    app.accounting.session.estimated_cost = Some(cost);

    let snapshot = project_app(&app, ProjectionContext::default());
    assert_eq!(snapshot.usage.latest_turn.estimated_cost, None);
    assert_eq!(snapshot.usage.session.estimated_cost, None);
    assert_eq!(
        crate::status_summary::StatusSummary::from_app(&app).latest_turn_cost,
        None
    );
}
