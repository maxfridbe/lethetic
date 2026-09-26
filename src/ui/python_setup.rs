use crate::app::App;
use crate::icons;
use crate::python_setup::PythonSetupStage;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block as UIBlock, Borders, Clear, Paragraph, Wrap},
};

use super::layout::centered_rect;

pub(super) fn render(f: &mut ratatui::Frame, app: &App) {
    use crate::config::{
        AccessMode, NetworkAccess, PythonExecutionTarget, SandboxBackend, ToolProfile,
    };
    let Some(setup) = app.python_setup.as_ref() else {
        return;
    };
    let area = centered_rect(86, 78, f.area());
    f.render_widget(Clear, area);
    let mut lines = vec![Line::from(Span::styled(
        setup.stage.title(),
        Style::default()
            .fg(app.theme.highlight_fg)
            .add_modifier(Modifier::BOLD),
    ))];
    lines.push(Line::from(""));

    match setup.stage {
        PythonSetupStage::Profile => {
            lines.push(Line::from(format!(
                "Choose:  {} General    {} Python-only",
                if setup.profile == ToolProfile::General {
                    "▶"
                } else {
                    " "
                },
                if setup.profile == ToolProfile::PythonOnly {
                    "▶"
                } else {
                    " "
                }
            )));
            lines.push(Line::from(
                "General keeps the existing tools. Python-only exposes only python; task tracking uses import lethetic_todo.",
            ));
        }
        PythonSetupStage::Target => {
            lines.push(Line::from(format!(
                "Choose:  {} Host    {} Sandbox",
                if setup.runtime.target == Some(PythonExecutionTarget::Host) {
                    "▶"
                } else {
                    " "
                },
                if setup.runtime.target == Some(PythonExecutionTarget::Sandbox) {
                    "▶"
                } else {
                    " "
                }
            )));
            lines.push(Line::from(Span::styled(
                "Host runs with your full filesystem, environment, network, and subprocess permissions.",
                Style::default().fg(app.theme.warning_fg),
            )));
        }
        PythonSetupStage::Backend => {
            for choice in [
                crate::python::backend::PythonBackendChoice::Bubblewrap,
                crate::python::backend::PythonBackendChoice::Podman,
            ] {
                let selected = matches!(
                    (choice, setup.runtime.sandbox.backend),
                    (
                        crate::python::backend::PythonBackendChoice::Bubblewrap,
                        Some(SandboxBackend::Bubblewrap)
                    ) | (
                        crate::python::backend::PythonBackendChoice::Podman,
                        Some(SandboxBackend::Podman)
                    )
                );
                let capability = setup.capabilities.iter().find(|item| item.choice == choice);
                lines.push(Line::from(format!(
                    "{} {} — {}",
                    if selected { "▶" } else { " " },
                    choice.label(),
                    capability
                        .map(|item| item.reason.as_str())
                        .unwrap_or("not probed")
                )));
            }
            lines.push(Line::from(
                "Unavailable backends never fall back to another backend or Host.",
            ));
        }
        PythonSetupStage::PodmanImage => {
            lines.push(Line::from(format!(
                "Image: {}",
                setup.runtime.sandbox.podman_image
            )));
            lines.push(Line::from(
                "Enter: continue · F5/Ctrl-P: explicit Pull · Esc: back",
            ));
            lines.push(Line::from("Worker launch always uses --pull=never."));
        }
        PythonSetupStage::Network => {
            lines.push(Line::from(format!(
                "Choose:  {} None    {} Public packages only; blocks host/LAN    {} Full",
                if setup.runtime.sandbox.network == Some(NetworkAccess::None) {
                    "▶"
                } else {
                    " "
                },
                if setup.runtime.sandbox.network == Some(NetworkAccess::Nonlocal) {
                    "▶"
                } else {
                    " "
                },
                if setup.runtime.sandbox.network == Some(NetworkAccess::Full) {
                    "▶"
                } else {
                    " "
                }
            )));
            lines.push(Line::from("Public-packages mode requires Podman and keeps direct container networking disabled."));
            lines.push(Line::from(
                "Full reaches host localhost, LAN, VPN/local routes, and the Internet.",
            ));
            lines.push(Line::from(
                "The LLM connection itself always remains outside this sandbox.",
            ));
        }
        PythonSetupStage::WorkspaceAccess => {
            lines.push(Line::from(format!(
                "Workspace {}:  {} Read-only    {} Read/write",
                setup.workspace_root.display(),
                if setup.runtime.sandbox.workspace_access == Some(AccessMode::ReadOnly) {
                    "▶"
                } else {
                    " "
                },
                if setup.runtime.sandbox.workspace_access == Some(AccessMode::ReadWrite) {
                    "▶"
                } else {
                    " "
                }
            )));
        }
        PythonSetupStage::Grants => {
            lines.push(Line::from(format!(
                "Browser: {}  ({:?})  hidden:{}",
                setup.path_picker.directory.display(),
                setup.path_picker.access,
                setup.path_picker.show_hidden
            )));
            let entry_window = crate::python_setup::visible_window(
                setup.path_picker.entries.len(),
                setup.path_picker.selected,
                8,
            );
            if entry_window.start > 0 {
                lines.push(Line::from("  …"));
            }
            for (index, entry) in setup
                .path_picker
                .entries
                .iter()
                .enumerate()
                .skip(entry_window.start)
                .take(entry_window.len())
            {
                lines.push(Line::from(format!(
                    "{} {}{}",
                    if index == setup.path_picker.selected {
                        "▶"
                    } else {
                        " "
                    },
                    entry.name,
                    if entry.is_directory { "/" } else { "" }
                )));
            }
            if entry_window.end < setup.path_picker.entries.len() {
                lines.push(Line::from("  …"));
            }
            lines.push(Line::from(format!(
                "Pasted/typed path: {}",
                setup.path_picker.typed_path
            )));
            lines.push(Line::from("Ctrl-A add selected · Ctrl-T add typed · Ctrl-R next-add RO/RW · Ctrl-E edit selected RO/RW · Delete remove · N next"));
            lines.push(Line::from(
                "Ctrl-H hidden · PageUp/PageDown select an existing grant",
            ));
            lines.push(Line::from("Grants:"));
            let grant_window = crate::python_setup::visible_window(
                setup.runtime.sandbox.grants.len(),
                setup.selected_grant,
                6,
            );
            if grant_window.start > 0 {
                lines.push(Line::from("  …"));
            }
            for (index, grant) in setup
                .runtime
                .sandbox
                .grants
                .iter()
                .enumerate()
                .skip(grant_window.start)
                .take(grant_window.len())
            {
                lines.push(Line::from(format!(
                    "{} {:?} {}",
                    if index == setup.selected_grant {
                        "▶"
                    } else {
                        " "
                    },
                    grant.access,
                    grant.path.display()
                )));
            }
            if grant_window.end < setup.runtime.sandbox.grants.len() {
                lines.push(Line::from("  …"));
            }
            let warnings = setup.security_warnings();
            for warning in warnings.iter().take(4) {
                lines.push(Line::from(Span::styled(
                    warning.clone(),
                    Style::default()
                        .fg(app.theme.warning_fg)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            if warnings.len() > 4 {
                lines.push(Line::from(Span::styled(
                    format!("… {} additional security warnings", warnings.len() - 4),
                    Style::default().fg(app.theme.warning_fg),
                )));
            }
        }
        PythonSetupStage::Confirm => {
            for line in setup.summary_lines() {
                lines.push(Line::from(line));
            }
            let warnings = setup.security_warnings();
            for warning in warnings.iter().take(8) {
                lines.push(Line::from(Span::styled(
                    warning.clone(),
                    Style::default()
                        .fg(app.theme.warning_fg)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            if warnings.len() > 8 {
                lines.push(Line::from(Span::styled(
                    format!("… {} additional security warnings", warnings.len() - 8),
                    Style::default().fg(app.theme.warning_fg),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(format!(
                "Persistence: {} (←/→ change)",
                setup.persistence.label()
            )));
            lines.push(Line::from("Enter applies atomically · Esc goes back"));
        }
        PythonSetupStage::PullConfirm => {
            lines.push(Line::from(Span::styled(
                format!("Pull exactly: {}", setup.runtime.sandbox.podman_image),
                Style::default()
                    .fg(app.theme.warning_fg)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(
                "This downloads data from a registry and consumes host disk/network.",
            ));
            lines.push(Line::from("Y/Enter: pull · N/Esc: cancel"));
        }
        PythonSetupStage::Probing => lines.push(Line::from(
            "Probing Host, Bubblewrap, and Podman with hardened smoke checks…",
        )),
        PythonSetupStage::Pulling => {
            lines.push(Line::from("Pulling the explicitly confirmed Podman image…"))
        }
        PythonSetupStage::Applying => lines.push(Line::from(
            "Validating, persisting, and applying policy atomically…",
        )),
    }
    if let Some(error) = &setup.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Error: {error}"),
            Style::default().fg(app.theme.error_fg),
        )));
    }
    if !setup.is_busy()
        && !matches!(
            setup.stage,
            PythonSetupStage::Confirm
                | PythonSetupStage::PullConfirm
                | PythonSetupStage::Grants
                | PythonSetupStage::PodmanImage
        )
    {
        lines.push(Line::from(""));
        lines.push(Line::from("←/→ choose · Enter continue · Esc back/cancel"));
    }
    let block = UIBlock::default()
        .title(format!("{} Python Runspace Policy", icons::COMMAND))
        .borders(Borders::ALL)
        .style(Style::default().bg(app.theme.terminal_bg))
        .border_style(Style::default().fg(app.theme.highlight_fg));
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}
