use crate::app::{App, RcSetupStage};
use crate::icons;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block as UIBlock, Borders, Clear, Paragraph, Wrap},
};

use super::centered_rect;

fn option_line(selected: bool, label: String, style: Style, highlight: Style) -> Line<'static> {
    if selected {
        Line::from(Span::styled(format!("> {label}"), highlight))
    } else {
        Line::from(Span::styled(format!("  {label}"), style))
    }
}

pub(super) fn render(f: &mut ratatui::Frame, app: &App) {
    let theme = &app.theme;
    let normal = Style::default().fg(theme.output_fg);
    let dim = Style::default().fg(theme.system_fg);
    let highlight = Style::default()
        .fg(theme.highlight_fg)
        .add_modifier(Modifier::BOLD);

    if let Some(setup) = &app.rc_setup {
        let mut lines: Vec<Line> = Vec::new();
        let (title, hint) = match setup.stage {
            RcSetupStage::Address => {
                lines.push(Line::from(Span::styled(
                    "Which address should the browser connect to?",
                    normal,
                )));
                for (index, choice) in setup.choices.iter().enumerate() {
                    lines.push(option_line(
                        index == setup.selected,
                        format!("{}   ({})", choice.host, choice.note),
                        normal,
                        highlight,
                    ));
                }
                let custom = setup.selected == setup.choices.len();
                lines.push(option_line(
                    custom,
                    format!(
                        "custom: {}{}",
                        setup.custom_host,
                        if custom { "▏" } else { "" }
                    ),
                    normal,
                    highlight,
                ));
                (
                    "1/4 Address",
                    "↑↓ choose · type on the custom row · Enter next · Esc cancel",
                )
            }
            RcSetupStage::Port => {
                lines.push(Line::from(Span::styled(
                    format!("Host: {}", setup.host()),
                    dim,
                )));
                lines.push(Line::from(vec![
                    Span::styled("Port: ", normal),
                    Span::styled(format!("{}▏", setup.port), highlight),
                ]));
                ("2/4 Port", "digits · Enter next · Esc back")
            }
            RcSetupStage::Auth => {
                lines.push(Line::from(Span::styled(
                    "How should browsers authenticate?",
                    normal,
                )));
                lines.push(option_line(
                    !setup.open,
                    "Private token URL shown after start (recommended)".to_string(),
                    normal,
                    highlight,
                ));
                lines.push(option_line(
                    setup.open,
                    "Open: anyone who can reach the address gets full control".to_string(),
                    normal,
                    highlight,
                ));
                ("3/4 Authentication", "↑↓ choose · Enter next · Esc back")
            }
            RcSetupStage::Files => {
                lines.push(Line::from(Span::styled(
                    "Share the launch directory read-only in the browser?",
                    normal,
                )));
                lines.push(option_line(
                    !setup.files,
                    "No".to_string(),
                    normal,
                    highlight,
                ));
                lines.push(option_line(
                    setup.files,
                    "Yes (protected files and links are excluded)".to_string(),
                    normal,
                    highlight,
                ));
                ("4/4 File sharing", "↑↓ choose · Enter next · Esc back")
            }
            RcSetupStage::Confirm => {
                lines.push(Line::from(Span::styled(
                    format!("Target: {}", setup.target()),
                    normal,
                )));
                lines.push(Line::from(Span::styled(
                    format!(
                        "Authentication: {}",
                        if setup.open {
                            "OPEN (no token)"
                        } else {
                            "private token URL"
                        }
                    ),
                    normal,
                )));
                lines.push(Line::from(Span::styled(
                    format!("File sharing: {}", if setup.files { "yes" } else { "no" }),
                    normal,
                )));
                lines.push(Line::from(""));
                if setup.open {
                    lines.push(Line::from(Span::styled(
                        "WARNING: every peer that can reach this address gets the same control as this terminal. Restrict it with a firewall or VPN.",
                        Style::default().fg(theme.error_fg).add_modifier(Modifier::BOLD),
                    )));
                }
                lines.push(Line::from(Span::styled(
                    "A browser can send prompts, approve tools, switch models, and quit.",
                    dim,
                )));
                lines.push(Line::from(Span::styled(
                    format!("Same as launching with: lethetic {}", setup.flags()),
                    dim,
                )));
                ("Confirm", "Enter start · Esc back")
            }
        };
        if let Some(error) = &setup.error {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                error.clone(),
                Style::default().fg(theme.error_fg),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(hint, dim)));
        let area = centered_rect(70, 60, f.area());
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                UIBlock::default()
                    .title(format!("{} Remote Control · {title}", icons::SERVER))
                    .borders(Borders::ALL)
                    .style(Style::default().bg(theme.terminal_bg))
                    .border_style(Style::default().fg(theme.highlight_fg)),
            ),
            area,
        );
    }

    if let Some(info) = &app.rc_info {
        let mut lines: Vec<Line> = info
            .lines
            .iter()
            .map(|line| Line::from(Span::styled(line.clone(), normal)))
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "c: copy URL (wl-copy) · F10 toggles mouse capture for terminal selection · Enter/Esc close",
            dim,
        )));
        let area = centered_rect(80, 45, f.area());
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                UIBlock::default()
                    .title(format!("{} Remote Control running", icons::SUCCESS))
                    .borders(Borders::ALL)
                    .style(Style::default().bg(theme.terminal_bg))
                    .border_style(Style::default().fg(theme.success_fg)),
            ),
            area,
        );
    }
}
