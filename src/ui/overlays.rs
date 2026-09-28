use crate::app::App;
use crate::icons;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block as UIBlock, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use super::block::{render_json_highlighted, render_python_call};
use super::layout::centered_rect;

fn python_approval_policy_lines(app: &App) -> Vec<String> {
    use crate::config::{AccessMode, NetworkAccess, PythonExecutionTarget, SandboxBackend};

    let mut lines = vec![format!(
        "Policy source: {}",
        app.python_policy.effective_source().label()
    )];
    match app.config.python_runtime.target {
        Some(PythonExecutionTarget::Host) => {
            lines.push(format!(
                "Target: Host ({})",
                app.config.python_runtime.python_executable
            ));
            lines.push(
                "WARNING: Host Python is unrestricted: full host files, environment, network, and subprocesses."
                    .to_string(),
            );
        }
        Some(PythonExecutionTarget::Sandbox) => {
            let backend = match app.config.python_runtime.sandbox.backend {
                Some(SandboxBackend::Bubblewrap) => "Bubblewrap".to_string(),
                Some(SandboxBackend::Podman) => format!(
                    "Podman (image {}, --pull=never)",
                    app.config.python_runtime.sandbox.podman_image
                ),
                None => "unresolved".to_string(),
            };
            lines.push(format!("Target: Sandbox / {backend}"));
            lines.push(match app.config.python_runtime.sandbox.network {
                Some(NetworkAccess::None) => "Network: None (private network namespace)".to_string(),
                Some(NetworkAccess::Nonlocal) => {
                    "Network: Public packages only (host, LAN, and direct container networking blocked)"
                        .to_string()
                }
                Some(NetworkAccess::Full) => {
                    "Network: Full (host localhost, LAN, and Internet are reachable)".to_string()
                }
                None => "Network: unresolved".to_string(),
            });
            let workspace_access = match app.config.python_runtime.sandbox.workspace_access {
                Some(AccessMode::ReadOnly) => "read-only",
                Some(AccessMode::ReadWrite) => "read/write",
                None => "unresolved",
            };
            lines.push(format!(
                "Workspace: {} ({workspace_access})",
                app.tool_runtime.workspace_root().display()
            ));
            const MAX_DISPLAYED_GRANTS: usize = 8;
            for grant in app
                .config
                .python_runtime
                .sandbox
                .grants
                .iter()
                .take(MAX_DISPLAYED_GRANTS)
            {
                lines.push(format!(
                    "Grant: {:?} {}",
                    grant.access,
                    grant.path.display()
                ));
            }
            let hidden = app
                .config
                .python_runtime
                .sandbox
                .grants
                .len()
                .saturating_sub(MAX_DISPLAYED_GRANTS);
            if hidden > 0 {
                lines.push(format!(
                    "… {hidden} additional grants omitted from display (included in approval fingerprint)"
                ));
            }
            lines.push(
                "Isolation: namespace/container only; not a VM or CPU, memory, or kernel guarantee."
                    .to_string(),
            );
        }
        None => lines.push("Target: unresolved".to_string()),
    }
    lines.push(
        "todowrite remains host-side and may write .lethetic/todos.json; the LLM transport is outside the sandbox."
            .to_string(),
    );
    lines
}

pub(super) fn render_primary(f: &mut ratatui::Frame, app: &mut App) -> bool {
    render_palette(f, app);
    render_model_switcher(f, app);
    render_lsp_manager(f, app);
    render_theme_menu(f, app);
    render_history(f, app);
    if render_loading(f, app) {
        return true;
    }
    render_prompt_manager(f, app);
    render_session_manager(f, app);
    render_latest_files(f, app);
    false
}

pub(super) fn render_secondary(f: &mut ratatui::Frame, app: &mut App) {
    render_approval(f, app);
    render_prompt_editor(f, app);
    render_hotkeys(f, app);
    render_session_name(f, app);
}

fn render_palette(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_palette {
        return;
    }
    let area = centered_rect(70, 70, f.area());
    f.render_widget(Clear, area);
    let matches = app.palette_matches();
    let items: Vec<ListItem> = matches
        .iter()
        .map(|command| {
            let view = app.command_view(*command);
            let title = Line::from(format!("{} {}", view.icon.glyph(), view.label));
            let detail = view
                .disabled_reason
                .clone()
                .filter(|_| !view.enabled)
                .unwrap_or_else(|| view.description.clone());
            let description = Line::from(Span::styled(
                format!("    {detail}"),
                Style::default()
                    .fg(app.theme.system_fg)
                    .add_modifier(Modifier::DIM),
            ));
            let item = ListItem::new(vec![title, description]);
            if view.enabled {
                item
            } else {
                item.style(Style::default().fg(app.theme.system_fg))
            }
        })
        .collect();
    f.render_stateful_widget(
        List::new(items)
            .block(
                UIBlock::default()
                    .title(if app.palette_query.is_empty() {
                        format!(
                            "{} Command Palette  (type to filter · ↑↓ · Enter · Esc)",
                            icons::COMMAND
                        )
                    } else {
                        format!(
                            "{} Command Palette › {}▏  ({} match{})",
                            icons::COMMAND,
                            app.palette_query,
                            matches.len(),
                            if matches.len() == 1 { "" } else { "es" }
                        )
                    })
                    .borders(Borders::ALL)
                    .style(Style::default().bg(app.theme.terminal_bg)),
            )
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(app.theme.highlight_fg),
            )
            .highlight_symbol("> "),
        area,
        &mut app.palette_state,
    );
}

fn render_model_switcher(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_model_switcher {
        return;
    }
    let area = centered_rect(65, 50, f.area());
    f.render_widget(Clear, area);

    let items: Vec<ListItem> = if app.available_models.is_empty() {
        vec![ListItem::new("Fetching models…")]
    } else {
        app.available_models
            .iter()
            .map(|choice| {
                let same_connection = app
                    .config
                    .active_connection_id()
                    .is_some_and(|id| id == choice.connection_id.as_str())
                    || (app.config.active_connection_id().is_none()
                        && choice.connection_id == "__active"
                        && choice.url == app.server_url);
                let active = same_connection && choice.model_id == app.model_name;
                let label = if active {
                    format!("▶ {} [active]", choice.display)
                } else {
                    format!("  {}", choice.display)
                };
                ListItem::new(label)
            })
            .collect()
    };

    let title = if app.compact_model_picker_src.is_some() {
        format!(
            "{} Pick compaction model  (↑↓ · Enter: select · q: cancel)",
            icons::MODEL
        )
    } else {
        format!(
            "{} Models  (↑↓ navigate · Enter: switch · s: scan for more · q: close)",
            icons::MODEL
        )
    };
    f.render_stateful_widget(
        List::new(items)
            .block(
                UIBlock::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .style(Style::default().bg(app.theme.terminal_bg)),
            )
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(app.theme.highlight_fg),
            )
            .highlight_symbol("> "),
        area,
        &mut app.model_switcher_state,
    );
}

fn render_lsp_manager(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_lsp_manager {
        return;
    }
    use crate::lsp::registry::{SERVERS, check_installed};
    use ratatui::text::{Line as RLine, Span};
    use ratatui::widgets::Paragraph;

    let area = centered_rect(72, 60, f.area());
    f.render_widget(Clear, area);

    let items: Vec<ListItem> = SERVERS
        .iter()
        .map(|def| {
            let installed = check_installed(def);
            let status = if installed {
                Span::styled("✓ ", Style::default().fg(app.theme.success_fg))
            } else {
                Span::styled("✗ ", Style::default().fg(app.theme.warning_fg))
            };
            let label = Span::raw(format!(
                "{:<26} {:<28} {}",
                def.display_name,
                def.binary,
                if installed {
                    "installed"
                } else {
                    def.install_note
                }
            ));
            ListItem::new(RLine::from(vec![status, label]))
        })
        .collect();

    let inner_area = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Min(1),
        ratatui::layout::Constraint::Length(2),
    ])
    .split(area);

    f.render_stateful_widget(
        List::new(items)
            .block(UIBlock::default()
                .title(format!("{} LSP Servers  (↑↓ navigate · Enter/i: install when available · q: close)", icons::SEARCH))
                .borders(Borders::ALL)
                .style(Style::default().bg(app.theme.terminal_bg)))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(app.theme.highlight_fg))
            .highlight_symbol("> "),
        inner_area[0],
        &mut app.lsp_server_list_state,
    );

    // Show install guidance for selected entry
    if let Some(i) = app.lsp_server_list_state.selected()
        && let Some(def) = SERVERS.get(i)
    {
        let guidance = def
            .install_cmd
            .map(|command| format!("  install: {command}"))
            .unwrap_or_else(|| format!("  manual install: {}", def.install_note));
        let cmd_line = Paragraph::new(guidance).style(Style::default().fg(app.theme.system_fg));
        f.render_widget(cmd_line, inner_area[1]);
    }
}

fn render_theme_menu(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_theme_menu {
        return;
    }
    let area = centered_rect(60, 60, f.area());
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = app
        .themes
        .iter()
        .map(|t| ListItem::new(t.name.as_str()))
        .collect();
    f.render_stateful_widget(
        List::new(items)
            .block(
                UIBlock::default()
                    .title(format!("{} Themes", icons::THEME))
                    .borders(Borders::ALL)
                    .style(Style::default().bg(app.theme.terminal_bg)),
            )
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(app.theme.highlight_fg),
            )
            .highlight_symbol("> "),
        area,
        &mut app.theme_state,
    );
}

fn render_history(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_history {
        return;
    }
    let area = centered_rect(80, 50, f.area());
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = app
        .history
        .iter()
        .rev()
        .enumerate()
        .map(|(i, s)| {
            let line = if i == 0 {
                format!("{} (Latest)", s)
            } else {
                s.clone()
            };
            ListItem::new(line)
        })
        .collect();
    f.render_stateful_widget(
        List::new(items)
            .block(
                UIBlock::default()
                    .title(format!(
                        "{} Input History (ESC to cancel, Enter to paste)",
                        icons::COMMAND
                    ))
                    .borders(Borders::ALL)
                    .style(Style::default().bg(app.theme.terminal_bg)),
            )
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(app.theme.highlight_fg),
            )
            .highlight_symbol("> "),
        area,
        &mut app.history_state,
    );
}

fn render_loading(f: &mut ratatui::Frame, app: &App) -> bool {
    if !app.is_loading_session {
        return false;
    }
    let area = centered_rect(50, 10, f.area());
    f.render_widget(Clear, area);

    let block = UIBlock::default()
        .title(format!("{} Loading Session...", icons::PROCESSING))
        .borders(Borders::ALL)
        .style(Style::default().bg(app.theme.terminal_bg))
        .border_style(Style::default().fg(app.theme.thought_fg));

    let progress = (app.load_progress as u16).min(100);
    let filled = (progress as usize * 40) / 100;
    let empty = 40_usize.saturating_sub(filled);
    let bar = format!(
        "[{}{}] {}%",
        "█".repeat(filled),
        "░".repeat(empty),
        progress
    );

    let text = format!("\n  {}\n\n  {}", bar, app.load_status);
    f.render_widget(
        Paragraph::new(text)
            .block(block)
            .alignment(ratatui::layout::Alignment::Center),
        area,
    );
    true // Don't render the rest of the UI while loading
}

fn render_prompt_manager(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_prompt_manager {
        return;
    }
    let area = centered_rect(60, 60, f.area());
    f.render_widget(Clear, area);

    let mut items: Vec<ListItem> = vec![ListItem::new("  + Create New Prompt")];
    items.extend(app.prompt_files.iter().map(|name| {
        let label = if name == "compaction" {
            format!("{name} [compaction]")
        } else {
            name.clone()
        };
        ListItem::new(label)
    }));

    let block = UIBlock::default()
        .title(format!("{} Prompt Manager", icons::MODEL))
        .borders(Borders::ALL)
        .style(Style::default().bg(app.theme.terminal_bg))
        .border_style(Style::default().fg(app.theme.warning_fg));

    let inner_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(area);

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(app.theme.highlight_fg),
            )
            .highlight_symbol("> "),
        inner_layout[0],
        &mut app.prompt_list_state,
    );

    let help_text = "(Enter) Select/Create | (Esc) Close";
    f.render_widget(
        Paragraph::new(help_text)
            .block(
                UIBlock::default()
                    .borders(Borders::TOP)
                    .style(Style::default().bg(app.theme.terminal_bg)),
            )
            .style(Style::default().fg(app.theme.system_fg)),
        inner_layout[1],
    );
}

fn render_session_manager(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_session_manager {
        return;
    }
    let area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = app
        .session_summaries
        .iter()
        .map(|summary| {
            let mut lines = vec![Line::from(summary.label(&app.session_id))];
            if !summary.details.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("    {}", summary.details),
                    Style::default()
                        .fg(app.theme.system_fg)
                        .add_modifier(Modifier::DIM),
                )));
            }
            ListItem::new(lines)
        })
        .collect();

    let block = UIBlock::default()
        .title(format!("{} Session Manager", icons::COMMAND))
        .borders(Borders::ALL)
        .style(Style::default().bg(app.theme.terminal_bg))
        .border_style(Style::default().fg(app.theme.thought_fg));

    let inner_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(area);

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(app.theme.highlight_fg),
            )
            .highlight_symbol("> "),
        inner_layout[0],
        &mut app.session_list_state,
    );

    let help_text =
        "(Enter) Resume | (C) Compact | (N) New | (D) Delete | (X) Wipe All | (Esc) Close";
    f.render_widget(
        Paragraph::new(help_text)
            .block(
                UIBlock::default()
                    .borders(Borders::TOP)
                    .style(Style::default().bg(app.theme.terminal_bg)),
            )
            .style(Style::default().fg(app.theme.system_fg)),
        inner_layout[1],
    );
}

fn render_latest_files(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_latest_files {
        return;
    }
    let area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, area);

    let mut total_tokens = 0;
    let all_files = app.context_manager.all_cached_files();
    let items: Vec<ListItem> = all_files
        .iter()
        .map(|(path, cached, is_active)| {
            total_tokens += cached.tokens;
            let elapsed = cached.timestamp.elapsed().as_secs();
            let time_str = if elapsed < 60 {
                format!("{} sec ago", elapsed)
            } else if elapsed < 3600 {
                format!("{} min ago", elapsed / 60)
            } else {
                format!("{} hours ago", elapsed / 3600)
            };

            let display_path = if path.len() > 38 {
                format!("...{}", &path[path.len() - 35..])
            } else {
                path.clone()
            };

            let tier = if *is_active { "● active" } else { "  latest" };
            let content = format!(
                "{} {:<38} {:>6} tok ({})",
                tier, display_path, cached.tokens, time_str
            );
            ListItem::new(content)
        })
        .collect();

    let block = UIBlock::default()
        .title(format!("{} Files in Context", icons::COMMAND))
        .borders(Borders::ALL)
        .style(Style::default().bg(app.theme.terminal_bg))
        .border_style(Style::default().fg(app.theme.thought_fg));

    let inner_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(area);

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(app.theme.highlight_fg),
            )
            .highlight_symbol("> "),
        inner_layout[0],
        &mut app.latest_files_state,
    );

    let help_text = format!(
        "(R) Remove from Context | (Esc) Close | Total Tokens: {}",
        total_tokens
    );
    f.render_widget(
        Paragraph::new(help_text)
            .block(
                UIBlock::default()
                    .borders(Borders::TOP)
                    .style(Style::default().bg(app.theme.terminal_bg)),
            )
            .style(Style::default().fg(app.theme.system_fg)),
        inner_layout[1],
    );
}

fn render_approval(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_approval_prompt {
        return;
    }
    let area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, area);
    if let Some(tc) = &app.pending_tool_call {
        let is_python = tc.function.name == "python";
        let mut display_text = Text::from(vec![Line::from(vec![
            Span::styled("Tool: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(&tc.function.name, Style::default().fg(app.theme.success_fg)),
        ])]);

        if is_python {
            display_text.lines.extend(render_python_call(
                &tc.function.arguments,
                &app.theme,
                app.python_approval_show_original,
            ));
        } else {
            display_text.lines.push(Line::from("Params:"));
            let json_text = render_json_highlighted(&tc.function.arguments, &app.theme);
            let total_lines = json_text.lines.len();
            display_text
                .lines
                .extend(json_text.lines.into_iter().take(15));
            if total_lines > 15 {
                display_text.lines.push(Line::from(Span::styled(
                    "... [Truncated for display]",
                    Style::default().fg(app.theme.system_fg),
                )));
            }
        }
        if is_python {
            display_text.lines.push(Line::from(""));
            display_text.lines.push(Line::from(Span::styled(
                "Effective Python policy",
                Style::default()
                    .fg(app.theme.warning_fg)
                    .add_modifier(Modifier::BOLD),
            )));
            for line in python_approval_policy_lines(app) {
                let style = if line.starts_with("WARNING:") {
                    Style::default()
                        .fg(app.theme.warning_fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(app.theme.system_fg)
                };
                display_text
                    .lines
                    .push(Line::from(Span::styled(line, style)));
            }
        }

        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(if is_python { 4 } else { 3 }),
            ])
            .split(area);
        let block = UIBlock::default()
            .title(format!("{} Security Confirmation", icons::WARNING))
            .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
            .style(Style::default().bg(app.theme.terminal_bg))
            .border_style(Style::default().fg(app.theme.warning_fg));
        let scroll = if is_python {
            app.python_approval_scroll
        } else {
            0
        };
        f.render_widget(
            Paragraph::new(display_text)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            sections[0],
        );

        let mut help = vec![Line::from(vec![
            Span::styled(
                "(A)",
                Style::default()
                    .fg(app.theme.warning_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("lways Allow | "),
            Span::styled(
                "(O)",
                Style::default()
                    .fg(app.theme.warning_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("nce | "),
            Span::styled(
                "(D)",
                Style::default()
                    .fg(app.theme.error_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("eny"),
        ])];
        if is_python {
            help.push(Line::from(format!(
                "(V) View {} | ↑/↓/PgUp/PgDn scroll",
                if app.python_approval_show_original {
                    "formatted preview"
                } else {
                    "exact original source"
                }
            )));
        }
        let help_block = UIBlock::default()
            .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
            .style(Style::default().bg(app.theme.terminal_bg))
            .border_style(Style::default().fg(app.theme.warning_fg));
        f.render_widget(
            Paragraph::new(Text::from(help))
                .block(help_block)
                .style(Style::default().fg(app.theme.system_fg))
                .wrap(Wrap { trim: false }),
            sections[1],
        );
    } else {
        app.show_approval_prompt = false;
        app.python_approval_show_original = false;
        app.python_approval_scroll = 0;
    }
}

fn render_prompt_editor(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_prompt_editor {
        return;
    }
    let full_area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, full_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
        .split(full_area);

    let header_block = UIBlock::default()
        .title(format!("{} System Prompt Editor", icons::MODEL))
        .borders(Borders::ALL)
        .style(if app.is_editing_prompt {
            Style::default()
                .fg(app.theme.warning_fg)
                .bg(app.theme.terminal_bg)
        } else {
            Style::default().bg(app.theme.terminal_bg)
        });

    let instructions = if app.is_editing_prompt {
        format!(
            "EDITING MODE | Cursor: {} | (ESC) Finish",
            app.prompt_cursor_pos
        )
    } else {
        "(M)odify | (S)ave & Use | Save for Later (N) | (UP/DN) Scroll | (ESC) Close".to_string()
    };
    f.render_widget(Paragraph::new(instructions).block(header_block), chunks[0]);

    let editor_block = UIBlock::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .style(Style::default().bg(app.theme.terminal_bg));

    let mut display_spans = Vec::new();
    if app.is_editing_prompt {
        let mut current_pos = 0;
        let mut cursor_seen = false;

        for c in app.system_prompt.chars() {
            if current_pos == app.prompt_cursor_pos {
                display_spans.push(Span::styled(
                    c.to_string(),
                    Style::default()
                        .add_modifier(Modifier::REVERSED)
                        .fg(app.theme.warning_fg),
                ));
                cursor_seen = true;
            } else {
                display_spans.push(Span::raw(c.to_string()));
            }
            current_pos += c.len_utf8();
        }

        if !cursor_seen {
            display_spans.push(Span::styled("█", Style::default().fg(app.theme.warning_fg)));
        }
    } else {
        display_spans.push(Span::raw(app.system_prompt.clone()));
    }

    f.render_widget(
        Paragraph::new(Line::from(display_spans))
            .block(editor_block)
            .wrap(Wrap { trim: false })
            .scroll((app.prompt_scroll as u16, 0)),
        chunks[1],
    );

    if app.show_prompt_save_dialog {
        let dialog_area = centered_rect(50, 20, f.area());
        f.render_widget(Clear, dialog_area);

        let dialog_block = UIBlock::default()
            .title(format!("{} Save Prompt As", icons::WARNING))
            .borders(Borders::ALL)
            .style(Style::default().bg(app.theme.terminal_bg))
            .border_style(Style::default().fg(app.theme.warning_fg));

        let text = format!(
            "Enter filename (without .md):\n> {}\n\n(ENTER) Save | (ESC) Cancel",
            app.prompt_save_name
        );
        f.render_widget(
            Paragraph::new(text)
                .block(dialog_block)
                .wrap(Wrap { trim: true }),
            dialog_area,
        );
    }
}

/// Every terminal shortcut, grouped. Keep in sync with the README table.
const HOTKEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Commands",
        &[
            (
                "Ctrl+P / Esc",
                "Command palette: type to fuzzy-filter, ↑↓ move, Enter run, Esc close",
            ),
            ("F1", "This hotkey reference"),
            ("Enter", "Send prompt / confirm selection"),
            ("Alt+Enter", "New line in the prompt"),
            ("Up (empty prompt)", "Input history"),
        ],
    ),
    (
        "Stopping and quitting",
        &[
            (
                "Esc Esc",
                "Stop the running reply or tool (two presses within 0.8 s)",
            ),
            (
                "Ctrl+C",
                "Cancel active work; press again when idle to quit",
            ),
        ],
    ),
    (
        "Output",
        &[
            ("Tab", "Switch focus between input and output"),
            (
                "Up / Down",
                "Scroll output (at the input edge, or when output is focused)",
            ),
            ("Alt+Up / Alt+Down", "Scroll output one line at any time"),
            ("PgUp / PgDn", "Scroll output 20 lines"),
            ("Ctrl+Home / Ctrl+End", "Jump to the top / bottom"),
            ("Mouse wheel", "Scroll output"),
            ("Click 󰇻", "Copy a block's content (wl-copy)"),
            ("Ctrl+L", "Clear the screen, keep the context"),
        ],
    ),
    (
        "Panes and view",
        &[
            ("F9", "Todo list pane (the model's remaining todos)"),
            ("F8", "Background tasks pane (progress bars, last output)"),
            ("F12", "Debugger pane"),
            (
                "F10",
                "Mouse capture off/on: off lets you select text; Shift+drag also works",
            ),
            ("Ctrl+O", "Hide / show thinking blocks"),
        ],
    ),
    (
        "In lists and dialogs",
        &[
            (
                "Sessions: Enter / C / N / D / X",
                "Resume / compact / new / delete / wipe all",
            ),
            (
                "Models: s",
                "Scan the connection's catalog; type to filter, Enter adds",
            ),
            ("Approval: A / O / D", "Always allow / allow once / deny"),
        ],
    ),
];

fn render_hotkeys(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_hotkeys {
        return;
    }
    let area = centered_rect(80, 85, f.area());
    f.render_widget(Clear, area);
    let key_width = HOTKEYS
        .iter()
        .flat_map(|(_, keys)| keys.iter().map(|(key, _)| key.chars().count()))
        .max()
        .unwrap_or(10);
    let mut lines = Vec::new();
    for (section, keys) in HOTKEYS {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            *section,
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(app.theme.highlight_fg),
        )));
        for (key, action) in *keys {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {key:<key_width$}  "),
                    Style::default().fg(app.theme.tool_fg),
                ),
                Span::styled(*action, Style::default().fg(app.theme.output_fg)),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Esc or Enter closes",
        Style::default()
            .add_modifier(Modifier::ITALIC)
            .fg(app.theme.system_fg),
    )));
    f.render_widget(
        Paragraph::new(lines)
            .block(
                UIBlock::default()
                    .title(format!("{} Hotkeys", icons::COMMAND))
                    .borders(Borders::ALL)
                    .style(Style::default().bg(app.theme.terminal_bg)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_session_name(f: &mut ratatui::Frame, app: &mut App) {
    if !app.show_session_name_dialog {
        return;
    }
    let area = centered_rect(60, 24, f.area());
    f.render_widget(Clear, area);
    let title = if app.display_name.is_some() {
        "Rename Session"
    } else {
        "Name Session"
    };
    let block = UIBlock::default()
        .title(format!("{} {title}", icons::COMMAND))
        .borders(Borders::ALL)
        .style(Style::default().bg(app.theme.terminal_bg))
        .border_style(Style::default().fg(app.theme.highlight_fg));
    let mut lines = vec![
        Line::from("Display-only name; session identity and files do not move."),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(app.theme.highlight_fg)),
            Span::styled(
                app.session_name_input.clone(),
                Style::default().fg(app.theme.input_fg),
            ),
        ]),
        Line::from(""),
        Line::from("Enter: save · empty clears · Esc: cancel"),
    ];
    if let Some(error) = &app.session_name_error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(app.theme.error_fg),
        )));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}
