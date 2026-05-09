use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block as UIBlock, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use crate::app::{App, BlockType, RenderBlock};
use crate::icons;
use crate::markdown;

fn render_json_highlighted(json_val: &serde_json::Value, theme: &Theme) -> Text<'static> {
    let pretty = match serde_json::to_string_pretty(json_val) {
        Ok(s) => s,
        Err(_) => format!("{:?}", json_val),
    };

    let mut lines = Vec::new();
    for line in pretty.lines() {
        let mut spans = Vec::new();
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        
        if !indent.is_empty() {
            spans.push(Span::raw(indent.to_string()));
        }

        if trimmed.starts_with('"') {
            if let Some(colon_pos) = trimmed.find(':') {
                // It's a key
                let key = &trimmed[..colon_pos];
                let rest = &trimmed[colon_pos..];
                spans.push(Span::styled(key.to_string(), Style::default().fg(theme.json_key_fg)));
                
                // Colorize the value part
                let value_part = rest.trim_start_matches(':').trim();
                spans.push(Span::raw(": "));
                if value_part.starts_with('"') {
                    spans.push(Span::styled(value_part.to_string(), Style::default().fg(theme.json_val_fg)));
                } else if value_part == "true" || value_part == "false" || value_part == "null" {
                    spans.push(Span::styled(value_part.to_string(), Style::default().fg(theme.error_fg)));
                } else if value_part.chars().next().map_or(false, |c| c.is_ascii_digit() || c == '-') {
                    spans.push(Span::styled(value_part.to_string(), Style::default().fg(theme.thought_fg)));
                } else {
                    spans.push(Span::raw(value_part.to_string()));
                }
            } else {
                // Just a string (maybe in an array)
                spans.push(Span::styled(trimmed.to_string(), Style::default().fg(theme.json_val_fg)));
            }
        } else if trimmed == "{" || trimmed == "}" || trimmed == "[" || trimmed == "]" || trimmed == "}," || trimmed == "]," {
            spans.push(Span::styled(trimmed.to_string(), Style::default().fg(theme.system_fg)));
        } else {
            spans.push(Span::raw(trimmed.to_string()));
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

#[derive(Clone, PartialEq, Debug)]
pub struct Theme {
    pub name: String,
    pub output_fg: Color,
    pub input_fg: Color,
    pub highlight_fg: Color,
    pub system_fg: Color,
    pub thought_fg: Color,
    pub tool_fg: Color,
    pub success_fg: Color,
    pub error_fg: Color,
    pub warning_fg: Color,
    pub json_key_fg: Color,
    pub json_val_fg: Color,
    pub input_bg: Color,
    pub thought_bg: Color,
    pub tool_bg: Color,
    pub terminal_bg: Color,
}

impl Theme {
    pub fn default() -> Self {
        Self {
            name: "Default".to_string(),
            output_fg: Color::Green,
            input_fg: Color::Yellow,
            highlight_fg: Color::Cyan,
            system_fg: Color::DarkGray,
            thought_fg: Color::Cyan,
            tool_fg: Color::LightBlue,
            success_fg: Color::Green,
            error_fg: Color::Red,
            warning_fg: Color::Yellow,
            json_key_fg: Color::Cyan,
            json_val_fg: Color::Yellow,
            input_bg: Color::Rgb(20, 20, 30),
            thought_bg: Color::Rgb(25, 45, 45),
            tool_bg: Color::Rgb(45, 45, 30),
            terminal_bg: Color::Rgb(15, 15, 20),
        }
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::default(),
            Self { 
                name: "Matrix".to_string(), 
                output_fg: Color::Green, 
                input_fg: Color::LightGreen, 
                highlight_fg: Color::White,
                system_fg: Color::DarkGray,
                thought_fg: Color::Green,
                tool_fg: Color::LightGreen,
                success_fg: Color::LightGreen,
                error_fg: Color::Red,
                warning_fg: Color::Yellow,
                json_key_fg: Color::Green,
                json_val_fg: Color::LightGreen,
                input_bg: Color::Black,
                thought_bg: Color::Black,
                tool_bg: Color::Black,
                terminal_bg: Color::Black,
            },
            Self { 
                name: "Cyberpunk".to_string(), 
                output_fg: Color::Magenta, 
                input_fg: Color::LightCyan, 
                highlight_fg: Color::LightMagenta,
                system_fg: Color::DarkGray,
                thought_fg: Color::LightMagenta,
                tool_fg: Color::LightCyan,
                success_fg: Color::Green,
                error_fg: Color::Red,
                warning_fg: Color::Yellow,
                json_key_fg: Color::LightCyan,
                json_val_fg: Color::Magenta,
                input_bg: Color::Rgb(30, 0, 30),
                thought_bg: Color::Rgb(0, 30, 30),
                tool_bg: Color::Rgb(30, 30, 0),
                terminal_bg: Color::Rgb(10, 0, 10),
            },
            Self { 
                name: "Ocean".to_string(), 
                output_fg: Color::Blue, 
                input_fg: Color::Cyan, 
                highlight_fg: Color::White,
                system_fg: Color::DarkGray,
                thought_fg: Color::Cyan,
                tool_fg: Color::LightBlue,
                success_fg: Color::Green,
                error_fg: Color::Red,
                warning_fg: Color::Yellow,
                json_key_fg: Color::LightBlue,
                json_val_fg: Color::Cyan,
                input_bg: Color::Rgb(0, 20, 40),
                thought_bg: Color::Rgb(0, 30, 50),
                tool_bg: Color::Rgb(0, 40, 60),
                terminal_bg: Color::Rgb(0, 10, 20),
            },
            Self { 
                name: "Sunset".to_string(), 
                output_fg: Color::Red, 
                input_fg: Color::LightYellow, 
                highlight_fg: Color::Yellow,
                system_fg: Color::DarkGray,
                thought_fg: Color::Yellow,
                tool_fg: Color::LightRed,
                success_fg: Color::Green,
                error_fg: Color::LightRed,
                warning_fg: Color::Yellow,
                json_key_fg: Color::LightYellow,
                json_val_fg: Color::Red,
                input_bg: Color::Rgb(40, 10, 0),
                thought_bg: Color::Rgb(50, 20, 0),
                tool_bg: Color::Rgb(60, 30, 0),
                terminal_bg: Color::Rgb(20, 5, 0),
            },
            Self { 
                name: "Forest".to_string(), 
                output_fg: Color::Green, 
                input_fg: Color::LightGreen, 
                highlight_fg: Color::White,
                system_fg: Color::DarkGray,
                thought_fg: Color::Green,
                tool_fg: Color::Green,
                success_fg: Color::LightGreen,
                error_fg: Color::Red,
                warning_fg: Color::Yellow,
                json_key_fg: Color::Green,
                json_val_fg: Color::LightGreen,
                input_bg: Color::Rgb(0, 20, 0),
                thought_bg: Color::Rgb(0, 30, 0),
                tool_bg: Color::Rgb(0, 40, 0),
                terminal_bg: Color::Rgb(0, 10, 0),
            },
            Self { 
                name: "Lavender".to_string(), 
                output_fg: Color::Magenta, 
                input_fg: Color::White, 
                highlight_fg: Color::LightMagenta,
                system_fg: Color::DarkGray,
                thought_fg: Color::Magenta,
                tool_fg: Color::LightMagenta,
                success_fg: Color::Green,
                error_fg: Color::Red,
                warning_fg: Color::Yellow,
                json_key_fg: Color::LightMagenta,
                json_val_fg: Color::White,
                input_bg: Color::Rgb(20, 0, 40),
                thought_bg: Color::Rgb(30, 0, 50),
                tool_bg: Color::Rgb(40, 0, 60),
                terminal_bg: Color::Rgb(10, 0, 20),
            },
            Self { 
                name: "Mono".to_string(), 
                output_fg: Color::White, 
                input_fg: Color::Gray, 
                highlight_fg: Color::DarkGray,
                system_fg: Color::DarkGray,
                thought_fg: Color::White,
                tool_fg: Color::Gray,
                success_fg: Color::White,
                error_fg: Color::White,
                warning_fg: Color::White,
                json_key_fg: Color::White,
                json_val_fg: Color::Gray,
                input_bg: Color::Black,
                thought_bg: Color::Black,
                tool_bg: Color::Black,
                terminal_bg: Color::Black,
            },
            Self { 
                name: "Gold".to_string(), 
                output_fg: Color::Yellow, 
                input_fg: Color::White, 
                highlight_fg: Color::LightYellow,
                system_fg: Color::DarkGray,
                thought_fg: Color::Yellow,
                tool_fg: Color::LightYellow,
                success_fg: Color::Green,
                error_fg: Color::Red,
                warning_fg: Color::Yellow,
                json_key_fg: Color::LightYellow,
                json_val_fg: Color::White,
                input_bg: Color::Rgb(30, 20, 0),
                thought_bg: Color::Rgb(40, 30, 0),
                tool_bg: Color::Rgb(50, 40, 0),
                terminal_bg: Color::Rgb(15, 10, 0),
            },
            Self {
                name: "Deep Sea".to_string(),
                output_fg: Color::DarkGray,
                input_fg: Color::Blue,
                highlight_fg: Color::LightBlue,
                system_fg: Color::DarkGray,
                thought_fg: Color::Blue,
                tool_fg: Color::LightBlue,
                success_fg: Color::Green,
                error_fg: Color::Red,
                warning_fg: Color::Yellow,
                json_key_fg: Color::Blue,
                json_val_fg: Color::LightBlue,
                input_bg: Color::Rgb(0, 10, 30),
                thought_bg: Color::Rgb(0, 20, 40),
                tool_bg: Color::Rgb(0, 30, 50),
                terminal_bg: Color::Rgb(0, 5, 15),
            },
            // ── 5 additional dark themes ────────────────────────────────────
            Self {
                name: "Dracula".to_string(),
                output_fg: Color::Rgb(248, 248, 242),
                input_fg: Color::Rgb(80, 250, 123),
                highlight_fg: Color::Rgb(189, 147, 249),
                system_fg: Color::Rgb(98, 114, 164),
                thought_fg: Color::Rgb(139, 233, 253),
                tool_fg: Color::Rgb(255, 184, 108),
                success_fg: Color::Rgb(80, 250, 123),
                error_fg: Color::Rgb(255, 85, 85),
                warning_fg: Color::Rgb(241, 250, 140),
                json_key_fg: Color::Rgb(189, 147, 249),
                json_val_fg: Color::Rgb(248, 248, 242),
                input_bg: Color::Rgb(40, 42, 54),
                thought_bg: Color::Rgb(44, 46, 60),
                tool_bg: Color::Rgb(48, 50, 66),
                terminal_bg: Color::Rgb(30, 31, 41),
            },
            Self {
                name: "Nord".to_string(),
                output_fg: Color::Rgb(216, 222, 233),
                input_fg: Color::Rgb(136, 192, 208),
                highlight_fg: Color::Rgb(129, 161, 193),
                system_fg: Color::Rgb(76, 86, 106),
                thought_fg: Color::Rgb(143, 188, 187),
                tool_fg: Color::Rgb(163, 190, 140),
                success_fg: Color::Rgb(163, 190, 140),
                error_fg: Color::Rgb(191, 97, 106),
                warning_fg: Color::Rgb(235, 203, 139),
                json_key_fg: Color::Rgb(136, 192, 208),
                json_val_fg: Color::Rgb(216, 222, 233),
                input_bg: Color::Rgb(46, 52, 64),
                thought_bg: Color::Rgb(49, 56, 69),
                tool_bg: Color::Rgb(52, 59, 73),
                terminal_bg: Color::Rgb(36, 40, 49),
            },
            Self {
                name: "Gruvbox".to_string(),
                output_fg: Color::Rgb(235, 219, 178),
                input_fg: Color::Rgb(184, 187, 38),
                highlight_fg: Color::Rgb(250, 189, 47),
                system_fg: Color::Rgb(102, 92, 84),
                thought_fg: Color::Rgb(131, 165, 152),
                tool_fg: Color::Rgb(214, 93, 14),
                success_fg: Color::Rgb(184, 187, 38),
                error_fg: Color::Rgb(204, 36, 29),
                warning_fg: Color::Rgb(250, 189, 47),
                json_key_fg: Color::Rgb(131, 165, 152),
                json_val_fg: Color::Rgb(235, 219, 178),
                input_bg: Color::Rgb(50, 48, 47),
                thought_bg: Color::Rgb(55, 53, 52),
                tool_bg: Color::Rgb(60, 58, 57),
                terminal_bg: Color::Rgb(29, 32, 33),
            },
            Self {
                name: "Tokyo Night".to_string(),
                output_fg: Color::Rgb(169, 177, 214),
                input_fg: Color::Rgb(125, 207, 255),
                highlight_fg: Color::Rgb(187, 154, 247),
                system_fg: Color::Rgb(86, 95, 137),
                thought_fg: Color::Rgb(125, 207, 255),
                tool_fg: Color::Rgb(247, 118, 142),
                success_fg: Color::Rgb(158, 206, 106),
                error_fg: Color::Rgb(247, 118, 142),
                warning_fg: Color::Rgb(224, 175, 104),
                json_key_fg: Color::Rgb(187, 154, 247),
                json_val_fg: Color::Rgb(169, 177, 214),
                input_bg: Color::Rgb(31, 35, 53),
                thought_bg: Color::Rgb(36, 40, 59),
                tool_bg: Color::Rgb(41, 46, 66),
                terminal_bg: Color::Rgb(26, 27, 38),
            },
            Self {
                name: "Monokai".to_string(),
                output_fg: Color::Rgb(248, 248, 242),
                input_fg: Color::Rgb(166, 226, 46),
                highlight_fg: Color::Rgb(102, 217, 239),
                system_fg: Color::Rgb(117, 113, 94),
                thought_fg: Color::Rgb(102, 217, 239),
                tool_fg: Color::Rgb(253, 151, 31),
                success_fg: Color::Rgb(166, 226, 46),
                error_fg: Color::Rgb(249, 38, 114),
                warning_fg: Color::Rgb(230, 219, 116),
                json_key_fg: Color::Rgb(102, 217, 239),
                json_val_fg: Color::Rgb(248, 248, 242),
                input_bg: Color::Rgb(45, 42, 38),
                thought_bg: Color::Rgb(50, 47, 43),
                tool_bg: Color::Rgb(55, 52, 48),
                terminal_bg: Color::Rgb(39, 40, 34),
            },
            Self {
                name: "Obsidian".to_string(),
                output_fg: Color::Rgb(240, 240, 255),
                input_fg: Color::Rgb(80, 180, 255),
                highlight_fg: Color::Rgb(120, 210, 255),
                system_fg: Color::Rgb(80, 90, 110),
                thought_fg: Color::Rgb(100, 160, 240),
                tool_fg: Color::Rgb(60, 220, 180),
                success_fg: Color::Rgb(60, 220, 120),
                error_fg: Color::Rgb(255, 80, 100),
                warning_fg: Color::Rgb(255, 200, 60),
                json_key_fg: Color::Rgb(80, 180, 255),
                json_val_fg: Color::Rgb(240, 240, 255),
                input_bg: Color::Rgb(15, 18, 28),
                thought_bg: Color::Rgb(12, 18, 32),
                tool_bg: Color::Rgb(10, 24, 28),
                terminal_bg: Color::Rgb(8, 10, 18),
            },
            Self {
                name: "Ash".to_string(),
                output_fg: Color::Rgb(200, 204, 212),
                input_fg: Color::Rgb(140, 160, 190),
                highlight_fg: Color::Rgb(160, 180, 210),
                system_fg: Color::Rgb(80, 88, 100),
                thought_fg: Color::Rgb(130, 150, 175),
                tool_fg: Color::Rgb(120, 175, 155),
                success_fg: Color::Rgb(120, 180, 130),
                error_fg: Color::Rgb(200, 100, 100),
                warning_fg: Color::Rgb(190, 170, 100),
                json_key_fg: Color::Rgb(140, 160, 190),
                json_val_fg: Color::Rgb(200, 204, 212),
                input_bg: Color::Rgb(38, 42, 50),
                thought_bg: Color::Rgb(40, 44, 54),
                tool_bg: Color::Rgb(36, 46, 48),
                terminal_bg: Color::Rgb(30, 33, 40),
            },
            Self {
                name: "Infrared".to_string(),
                output_fg: Color::Rgb(255, 220, 180),
                input_fg: Color::Rgb(255, 100, 50),
                highlight_fg: Color::Rgb(255, 140, 60),
                system_fg: Color::Rgb(100, 70, 50),
                thought_fg: Color::Rgb(240, 160, 80),
                tool_fg: Color::Rgb(255, 80, 80),
                success_fg: Color::Rgb(180, 220, 80),
                error_fg: Color::Rgb(255, 60, 60),
                warning_fg: Color::Rgb(255, 180, 60),
                json_key_fg: Color::Rgb(255, 120, 60),
                json_val_fg: Color::Rgb(255, 220, 180),
                input_bg: Color::Rgb(35, 15, 8),
                thought_bg: Color::Rgb(38, 18, 5),
                tool_bg: Color::Rgb(40, 10, 10),
                terminal_bg: Color::Rgb(20, 8, 4),
            },
            // ── 10 light themes ──────────────────────────────────────────────
            Self {
                name: "Paper".to_string(),
                output_fg: Color::Rgb(40, 40, 40),
                input_fg: Color::Rgb(0, 100, 200),
                highlight_fg: Color::Rgb(0, 120, 220),
                system_fg: Color::Rgb(140, 140, 140),
                thought_fg: Color::Rgb(80, 80, 160),
                tool_fg: Color::Rgb(0, 130, 80),
                success_fg: Color::Rgb(0, 130, 80),
                error_fg: Color::Rgb(180, 30, 30),
                warning_fg: Color::Rgb(160, 100, 0),
                json_key_fg: Color::Rgb(0, 100, 200),
                json_val_fg: Color::Rgb(40, 40, 40),
                input_bg: Color::Rgb(230, 235, 245),
                thought_bg: Color::Rgb(225, 225, 240),
                tool_bg: Color::Rgb(225, 240, 230),
                terminal_bg: Color::Rgb(245, 245, 250),
            },
            Self {
                name: "Solarized Light".to_string(),
                output_fg: Color::Rgb(88, 110, 117),
                input_fg: Color::Rgb(38, 139, 210),
                highlight_fg: Color::Rgb(108, 113, 196),
                system_fg: Color::Rgb(147, 161, 161),
                thought_fg: Color::Rgb(42, 161, 152),
                tool_fg: Color::Rgb(133, 153, 0),
                success_fg: Color::Rgb(133, 153, 0),
                error_fg: Color::Rgb(220, 50, 47),
                warning_fg: Color::Rgb(181, 137, 0),
                json_key_fg: Color::Rgb(38, 139, 210),
                json_val_fg: Color::Rgb(88, 110, 117),
                input_bg: Color::Rgb(238, 232, 213),
                thought_bg: Color::Rgb(234, 228, 209),
                tool_bg: Color::Rgb(230, 224, 205),
                terminal_bg: Color::Rgb(253, 246, 227),
            },
            Self {
                name: "GitHub Light".to_string(),
                output_fg: Color::Rgb(36, 41, 47),
                input_fg: Color::Rgb(5, 80, 174),
                highlight_fg: Color::Rgb(9, 105, 218),
                system_fg: Color::Rgb(110, 119, 129),
                thought_fg: Color::Rgb(130, 80, 223),
                tool_fg: Color::Rgb(5, 122, 85),
                success_fg: Color::Rgb(26, 127, 55),
                error_fg: Color::Rgb(207, 34, 46),
                warning_fg: Color::Rgb(154, 103, 0),
                json_key_fg: Color::Rgb(5, 80, 174),
                json_val_fg: Color::Rgb(36, 41, 47),
                input_bg: Color::Rgb(230, 236, 242),
                thought_bg: Color::Rgb(234, 228, 250),
                tool_bg: Color::Rgb(222, 244, 228),
                terminal_bg: Color::Rgb(255, 255, 255),
            },
            Self {
                name: "Ivory".to_string(),
                output_fg: Color::Rgb(50, 30, 10),
                input_fg: Color::Rgb(120, 60, 0),
                highlight_fg: Color::Rgb(160, 80, 0),
                system_fg: Color::Rgb(160, 140, 120),
                thought_fg: Color::Rgb(80, 100, 140),
                tool_fg: Color::Rgb(60, 120, 60),
                success_fg: Color::Rgb(60, 120, 60),
                error_fg: Color::Rgb(180, 40, 40),
                warning_fg: Color::Rgb(160, 110, 0),
                json_key_fg: Color::Rgb(120, 60, 0),
                json_val_fg: Color::Rgb(50, 30, 10),
                input_bg: Color::Rgb(240, 232, 215),
                thought_bg: Color::Rgb(235, 228, 210),
                tool_bg: Color::Rgb(230, 240, 225),
                terminal_bg: Color::Rgb(255, 252, 240),
            },
            Self {
                name: "Rose".to_string(),
                output_fg: Color::Rgb(87, 70, 75),
                input_fg: Color::Rgb(180, 60, 100),
                highlight_fg: Color::Rgb(200, 80, 120),
                system_fg: Color::Rgb(160, 140, 145),
                thought_fg: Color::Rgb(130, 80, 160),
                tool_fg: Color::Rgb(60, 130, 120),
                success_fg: Color::Rgb(60, 140, 80),
                error_fg: Color::Rgb(200, 50, 60),
                warning_fg: Color::Rgb(180, 120, 0),
                json_key_fg: Color::Rgb(180, 60, 100),
                json_val_fg: Color::Rgb(87, 70, 75),
                input_bg: Color::Rgb(245, 232, 236),
                thought_bg: Color::Rgb(240, 228, 245),
                tool_bg: Color::Rgb(225, 242, 238),
                terminal_bg: Color::Rgb(255, 245, 248),
            },
            Self {
                name: "Mint".to_string(),
                output_fg: Color::Rgb(30, 60, 50),
                input_fg: Color::Rgb(0, 130, 100),
                highlight_fg: Color::Rgb(0, 160, 120),
                system_fg: Color::Rgb(130, 160, 150),
                thought_fg: Color::Rgb(0, 100, 160),
                tool_fg: Color::Rgb(100, 0, 160),
                success_fg: Color::Rgb(0, 140, 80),
                error_fg: Color::Rgb(200, 50, 50),
                warning_fg: Color::Rgb(160, 120, 0),
                json_key_fg: Color::Rgb(0, 130, 100),
                json_val_fg: Color::Rgb(30, 60, 50),
                input_bg: Color::Rgb(220, 245, 238),
                thought_bg: Color::Rgb(215, 235, 250),
                tool_bg: Color::Rgb(235, 220, 250),
                terminal_bg: Color::Rgb(240, 255, 250),
            },
            Self {
                name: "Sky".to_string(),
                output_fg: Color::Rgb(30, 50, 80),
                input_fg: Color::Rgb(0, 100, 180),
                highlight_fg: Color::Rgb(0, 130, 220),
                system_fg: Color::Rgb(120, 150, 180),
                thought_fg: Color::Rgb(70, 100, 160),
                tool_fg: Color::Rgb(0, 140, 100),
                success_fg: Color::Rgb(0, 140, 80),
                error_fg: Color::Rgb(200, 50, 50),
                warning_fg: Color::Rgb(180, 120, 0),
                json_key_fg: Color::Rgb(0, 100, 180),
                json_val_fg: Color::Rgb(30, 50, 80),
                input_bg: Color::Rgb(215, 232, 250),
                thought_bg: Color::Rgb(220, 228, 248),
                tool_bg: Color::Rgb(215, 245, 238),
                terminal_bg: Color::Rgb(235, 245, 255),
            },
            Self {
                name: "Linen".to_string(),
                output_fg: Color::Rgb(60, 55, 50),
                input_fg: Color::Rgb(110, 80, 40),
                highlight_fg: Color::Rgb(140, 100, 50),
                system_fg: Color::Rgb(170, 160, 150),
                thought_fg: Color::Rgb(80, 100, 130),
                tool_fg: Color::Rgb(80, 130, 80),
                success_fg: Color::Rgb(70, 130, 70),
                error_fg: Color::Rgb(170, 50, 50),
                warning_fg: Color::Rgb(150, 100, 0),
                json_key_fg: Color::Rgb(110, 80, 40),
                json_val_fg: Color::Rgb(60, 55, 50),
                input_bg: Color::Rgb(238, 232, 222),
                thought_bg: Color::Rgb(230, 232, 242),
                tool_bg: Color::Rgb(225, 238, 225),
                terminal_bg: Color::Rgb(250, 245, 238),
            },
            Self {
                name: "Chalk".to_string(),
                output_fg: Color::Rgb(45, 45, 45),
                input_fg: Color::Rgb(80, 80, 200),
                highlight_fg: Color::Rgb(100, 100, 220),
                system_fg: Color::Rgb(160, 160, 160),
                thought_fg: Color::Rgb(120, 60, 160),
                tool_fg: Color::Rgb(0, 140, 140),
                success_fg: Color::Rgb(0, 140, 80),
                error_fg: Color::Rgb(180, 40, 40),
                warning_fg: Color::Rgb(150, 100, 0),
                json_key_fg: Color::Rgb(80, 80, 200),
                json_val_fg: Color::Rgb(45, 45, 45),
                input_bg: Color::Rgb(232, 232, 242),
                thought_bg: Color::Rgb(235, 228, 242),
                tool_bg: Color::Rgb(222, 240, 240),
                terminal_bg: Color::Rgb(245, 245, 248),
            },
            Self {
                name: "Parchment".to_string(),
                output_fg: Color::Rgb(55, 40, 20),
                input_fg: Color::Rgb(100, 50, 10),
                highlight_fg: Color::Rgb(140, 80, 20),
                system_fg: Color::Rgb(170, 150, 120),
                thought_fg: Color::Rgb(60, 80, 120),
                tool_fg: Color::Rgb(40, 100, 60),
                success_fg: Color::Rgb(50, 110, 50),
                error_fg: Color::Rgb(160, 40, 40),
                warning_fg: Color::Rgb(150, 90, 0),
                json_key_fg: Color::Rgb(100, 50, 10),
                json_val_fg: Color::Rgb(55, 40, 20),
                input_bg: Color::Rgb(235, 220, 190),
                thought_bg: Color::Rgb(228, 218, 200),
                tool_bg: Color::Rgb(220, 235, 210),
                terminal_bg: Color::Rgb(250, 240, 215),
            },
            Self {
                name: "Clay".to_string(),
                output_fg: Color::Rgb(60, 35, 20),
                input_fg: Color::Rgb(160, 70, 30),
                highlight_fg: Color::Rgb(190, 90, 40),
                system_fg: Color::Rgb(175, 150, 135),
                thought_fg: Color::Rgb(90, 100, 140),
                tool_fg: Color::Rgb(50, 120, 80),
                success_fg: Color::Rgb(60, 130, 70),
                error_fg: Color::Rgb(180, 50, 50),
                warning_fg: Color::Rgb(170, 100, 0),
                json_key_fg: Color::Rgb(160, 70, 30),
                json_val_fg: Color::Rgb(60, 35, 20),
                input_bg: Color::Rgb(240, 225, 210),
                thought_bg: Color::Rgb(232, 228, 218),
                tool_bg: Color::Rgb(222, 235, 218),
                terminal_bg: Color::Rgb(252, 242, 230),
            },
            Self {
                name: "Fog".to_string(),
                output_fg: Color::Rgb(55, 60, 70),
                input_fg: Color::Rgb(80, 110, 150),
                highlight_fg: Color::Rgb(100, 130, 170),
                system_fg: Color::Rgb(160, 165, 175),
                thought_fg: Color::Rgb(100, 120, 155),
                tool_fg: Color::Rgb(70, 140, 130),
                success_fg: Color::Rgb(70, 140, 90),
                error_fg: Color::Rgb(170, 70, 70),
                warning_fg: Color::Rgb(150, 120, 50),
                json_key_fg: Color::Rgb(80, 110, 150),
                json_val_fg: Color::Rgb(55, 60, 70),
                input_bg: Color::Rgb(228, 232, 238),
                thought_bg: Color::Rgb(225, 230, 240),
                tool_bg: Color::Rgb(220, 236, 234),
                terminal_bg: Color::Rgb(242, 244, 248),
            },
        ]
    }
}

pub fn ui(f: &mut ratatui::Frame, app: &mut App) {
    // Solid theme background for the entire terminal
    f.render_widget(
        UIBlock::default().style(Style::default().bg(app.theme.terminal_bg)),
        f.area(),
    );

    let main_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(if app.show_debug { [Constraint::Percentage(50), Constraint::Percentage(50)] } else { [Constraint::Percentage(100), Constraint::Min(0)] }.as_ref())
        .split(f.area());

    let inner_width = main_layout[0].width.saturating_sub(4);
    let prefix_len = 2; // "> "
    
    let input_height = (((app.input.len() + prefix_len) as u16 / inner_width.max(1)) + 3).min(10);
    
    let left_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(input_height),
            Constraint::Length(2),
        ].as_ref())
        .split(main_layout[0]);

    let title = if app.show_approval_prompt { format!("{} Approval Required", icons::WARNING) } 
                else if app.is_executing_tool { format!("{} {} Executing Tool...", icons::TOOL_SPINNER[app.tool_spinner_index], icons::COMMAND) }
                else { format!("{} Output", icons::OUTPUT) };

    let terminal_width = left_layout[0].width.saturating_sub(2) as usize;
    let terminal_height = left_layout[0].height.saturating_sub(2) as usize;
    
    if terminal_width != app.last_rendered_width {
        for block in &mut app.blocks {
            block.cached_lines = None;
        }
        app.last_rendered_width = terminal_width;
    }

    // --- VIRTUALIZATION LOGIC START ---
    // Instead of rendering all lines, we first count them and then only render the ones that would be visible.
    let mut total_lines = 0;
    let num_blocks = app.blocks.len();
    let mut block_line_counts = Vec::with_capacity(num_blocks);

    for (i, block) in app.blocks.iter_mut().enumerate() {
        let is_last = i == num_blocks - 1;
        let count = if let Some(ref cached) = block.cached_lines {
            if is_last && (app.is_executing_tool || app.is_processing) {
                // Bypass cache for live streaming/preview
                let rendered = render_block_to_lines(block, terminal_width, &app.theme, if app.is_executing_tool { Some(&app.tool_output_preview) } else { None });
                let len = rendered.len();
                len
            } else {
                cached.len()
            }
        } else {
            let rendered = render_block_to_lines(block, terminal_width, &app.theme, None);
            let len = rendered.len();
            block.cached_lines = Some(rendered);
            len
        };
        block_line_counts.push(count);
        total_lines += count;
    }
    app.total_line_count = total_lines;

    let mut selected_line = app.output_state.selected().unwrap_or(0);
    if app.auto_scroll && total_lines > 0 {
        selected_line = total_lines.saturating_sub(1);
        app.output_state.select(Some(selected_line));
    }
    
    // Calculate the window of lines to actually create ListItems for.
    let half_height = terminal_height / 2;
    let mut start_line = selected_line.saturating_sub(half_height);
    
    // Ensure we fill the terminal as much as possible if we are near the end
    if start_line + terminal_height > total_lines {
        start_line = total_lines.saturating_sub(terminal_height);
    }
    
    let end_line = (start_line + terminal_height + 2).min(total_lines);

    let mut list_items = Vec::new();
    let mut current_line_idx = 0;

    for (block_idx, count) in block_line_counts.iter().enumerate() {
        let block_end = current_line_idx + count;
        let is_last = block_idx == app.blocks.len() - 1;

        // If this block is within or partially within our visible window
        if block_end > start_line && current_line_idx < end_line {
            // Re-render if it's the live block, otherwise use cache
            let lines_to_render = if is_last && (app.is_executing_tool || app.is_processing) {
                render_block_to_lines(&app.blocks[block_idx], terminal_width, &app.theme, if app.is_executing_tool { Some(&app.tool_output_preview) } else { None })
            } else {
                app.blocks[block_idx].cached_lines.as_ref().cloned().unwrap_or_default()
            };

            for (i, line) in lines_to_render.iter().enumerate() {
                let absolute_idx = current_line_idx + i;
                if absolute_idx >= start_line && absolute_idx < end_line {
                    let rendered_line = line.clone();
                    list_items.push(ListItem::new(rendered_line));
                }
            }
        }
        current_line_idx += count;
    }


    // Adjust the list state to point to the correct relative item in our virtualized list
    let mut virtual_state = ListState::default();
    let relative_selected = selected_line.saturating_sub(start_line);
    virtual_state.select(Some(relative_selected));

    let output_block = UIBlock::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if app.is_output_focused { Style::default().fg(app.theme.highlight_fg) } else { Style::default() });

    f.render_stateful_widget(
        List::new(list_items).block(output_block),
        left_layout[0],
        &mut virtual_state
    );

    // Render Scrollbar
    f.render_stateful_widget(
        Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓")),
        left_layout[0],
        &mut ScrollbarState::new(total_lines.saturating_sub(terminal_height)).position(start_line),
    );
    // --- VIRTUALIZATION LOGIC END ---
    
    let input_style = Style::default().bg(app.theme.input_bg).fg(app.theme.input_fg);
    let input_title = if app.is_asking_user {
        format!("{} Waiting for your answer...", icons::WARNING)
    } else {
        format!("{} Input", icons::INPUT)
    };
    let input_block = UIBlock::default()
        .title(input_title)
        .borders(Borders::ALL)
        .style(if !app.is_output_focused { input_style.fg(app.theme.highlight_fg) } else { input_style });
    
    let prefix = Span::styled("> ", Style::default().fg(app.theme.highlight_fg).add_modifier(Modifier::BOLD));
    let input_text = Line::from(vec![prefix, Span::raw(&app.input)]);
    f.render_widget(Paragraph::new(input_text).block(input_block).wrap(Wrap { trim: false }), left_layout[2]);

    if !app.is_output_focused {
        let cursor_x = left_layout[2].x + 1 + prefix_len as u16 + (app.cursor_pos as u16 % inner_width.max(1));
        let cursor_y = left_layout[2].y + 1 + (app.cursor_pos as u16 / inner_width.max(1));
        f.set_cursor_position((cursor_x, cursor_y));
    }

    let line2_spans = vec![
        Span::styled(format!("{} Path: ", icons::PATH), Style::default().fg(app.theme.system_fg)),
        Span::styled(format!("{} ", app.current_dir), Style::default().fg(app.theme.tool_fg)),
        Span::styled(format!("| {} Git: ", icons::GIT), Style::default().fg(app.theme.system_fg)),
        Span::styled(format!("{} ", app.git_status), Style::default().fg(if app.git_status.contains("dirty") { app.theme.error_fg } else { app.theme.success_fg })),
    ];

    let processing_text = if app.show_approval_prompt {
        vec![Line::from(vec![Span::styled(format!("  {} {} Awaiting Permission For Tool Call...", icons::SPINNER[app.spinner_index], icons::WARNING), Style::default().fg(app.theme.warning_fg).add_modifier(Modifier::BOLD))])]
    } else if app.is_executing_tool {
        let preview = if app.tool_output_preview.is_empty() {
             "Executing Tool...".to_string()
        } else {
            let first_line = app.tool_output_preview.lines().next().unwrap_or("...");
            if first_line.len() > 50 {
                format!("{}...", &first_line[..47])
            } else {
                first_line.to_string()
            }
        };
        vec![Line::from(vec![Span::styled(format!("  {} {} {}", icons::TOOL_SPINNER[app.tool_spinner_index], icons::COMMAND, preview), Style::default().fg(app.theme.tool_fg))])]
    } else if app.is_asking_user {
        vec![Line::from(vec![Span::styled(format!("  {} {} Waiting for User Input...", icons::SPINNER[app.spinner_index], icons::INPUT), Style::default().fg(app.theme.warning_fg))])]
    } else if app.is_processing {
        vec![Line::from(vec![Span::styled(format!("  {} {} Lethetic Intelligence Engine Processing...", icons::SPINNER[app.spinner_index], icons::PROCESSING), Style::default().fg(app.theme.warning_fg))])]
    } else {
        let reason = &app.stop_reason;
        let color = if reason.starts_with('⚠') || reason.starts_with('✗') {
            app.theme.warning_fg
        } else if reason.starts_with('⏸') || reason.starts_with('→') {
            app.theme.highlight_fg
        } else {
            app.theme.system_fg
        };
        vec![Line::from(vec![Span::styled(format!("  {} {}", icons::SUCCESS, reason), Style::default().fg(color))])]
    };
    f.render_widget(Paragraph::new(processing_text), left_layout[1]);

    let status_text = vec![
        Line::from(vec![
            Span::styled(format!("{} tg: ", icons::TOKENS), Style::default().fg(app.theme.system_fg)),
            Span::styled(format!("{:.1} ", app.tokens_per_s), Style::default().fg(app.theme.thought_fg)),
            Span::styled("pp: ", Style::default().fg(app.theme.system_fg)),
            Span::styled(format!("{:.1} ", app.pp_tokens_per_s), Style::default().fg(app.theme.thought_fg)),
            Span::styled(format!("| {} Model: ", icons::MODEL), Style::default().fg(app.theme.system_fg)),
            Span::styled(format!("{} ", app.model_name), Style::default().fg(app.theme.success_fg)),
            Span::styled(format!("| {} Server: ", icons::SERVER), Style::default().fg(app.theme.system_fg)),
            Span::styled(format!("{} ", app.server_url), Style::default().fg(app.theme.warning_fg)),
            Span::styled(format!("| {} Context: ", icons::TOKENS), Style::default().fg(app.theme.system_fg)),
            Span::styled(
                {
                    let used = app.server_prompt_tokens.unwrap_or(app.context_manager.get_token_count() as u32);
                    format!("{}/{} ", used, app.max_tokens)
                },
                Style::default().fg(app.theme.thought_fg)
            ),
            Span::styled("| Mem: ", Style::default().fg(app.theme.system_fg)),
            Span::styled(format!("{}MB ", app.memory_usage), Style::default().fg(app.theme.thought_fg)),
            Span::styled("| Files: ", Style::default().fg(app.theme.system_fg)),
            Span::styled(format!("{} ", app.context_manager.active_files.len() + app.context_manager.latest_files.len()), Style::default().fg(app.theme.thought_fg)),
        ]),
        Line::from(line2_spans),
    ];
    f.render_widget(Paragraph::new(status_text).wrap(Wrap { trim: true }), left_layout[3]);

    if app.show_debug {
        let items: Vec<ListItem> = app.debug_log.iter().rev().take(50).map(|s| ListItem::new(s.as_str())).collect();
        f.render_widget(List::new(items).block(UIBlock::default().title(format!("{} Debugger", icons::DEBUG)).borders(Borders::ALL)).style(Style::default().fg(app.theme.system_fg)), main_layout[1]);
    }

    if app.show_palette {
        let area = centered_rect(60, 25, f.area());
        f.render_widget(Clear, area);
        let items: Vec<ListItem> = app.palette_items.iter().map(|i| ListItem::new(i.as_str())).collect();
        f.render_stateful_widget(List::new(items).block(UIBlock::default().title(format!("{} Command Palette", icons::COMMAND)).borders(Borders::ALL).style(Style::default().bg(app.theme.terminal_bg))).highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(app.theme.highlight_fg)).highlight_symbol("> "), area, &mut app.palette_state);
    }

    if app.show_theme_menu {
        let area = centered_rect(60, 60, f.area());
        f.render_widget(Clear, area);
        let items: Vec<ListItem> = app.themes.iter().map(|t| ListItem::new(t.name.as_str())).collect();
        f.render_stateful_widget(List::new(items).block(UIBlock::default().title(format!("{} Themes", icons::THEME)).borders(Borders::ALL).style(Style::default().bg(app.theme.terminal_bg))).highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(app.theme.highlight_fg)).highlight_symbol("> "), area, &mut app.theme_state);
    }

    if app.show_history {
        let area = centered_rect(80, 50, f.area());
        f.render_widget(Clear, area);
        let items: Vec<ListItem> = app.history.iter().rev().enumerate().map(|(i, s)| {
            let line = if i == 0 { format!("{} (Latest)", s) } else { s.clone() };
            ListItem::new(line)
        }).collect();
        f.render_stateful_widget(List::new(items).block(UIBlock::default().title(format!("{} Input History (ESC to cancel, Enter to paste)", icons::COMMAND)).borders(Borders::ALL).style(Style::default().bg(app.theme.terminal_bg))).highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(app.theme.highlight_fg)).highlight_symbol("> "), area, &mut app.history_state);
    }

    if app.is_loading_session {
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
        let bar = format!("[{}{}] {}%", "█".repeat(filled), "░".repeat(empty), progress);
        
        let text = format!("\n  {}\n\n  {}", bar, app.load_status);
        f.render_widget(Paragraph::new(text).block(block).alignment(ratatui::layout::Alignment::Center), area);
        return; // Don't render the rest of the UI while loading
    }

    if app.show_prompt_manager {
        let area = centered_rect(60, 60, f.area());
        f.render_widget(Clear, area);
        
        let mut items: Vec<ListItem> = vec![ListItem::new("  + Create New Prompt")];
        items.extend(app.prompt_files.iter().map(|f| ListItem::new(f.clone())));

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
                .highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(app.theme.highlight_fg))
                .highlight_symbol("> "),
            inner_layout[0],
            &mut app.prompt_list_state
        );
        
        let help_text = "(Enter) Select/Create | (Esc) Close";
        f.render_widget(Paragraph::new(help_text).block(UIBlock::default().borders(Borders::TOP).style(Style::default().bg(app.theme.terminal_bg))).style(Style::default().fg(app.theme.system_fg)), inner_layout[1]);
    }

    if app.show_session_manager {
        let area = centered_rect(80, 80, f.area());
        f.render_widget(Clear, area);
        let items: Vec<ListItem> = app.session_files.iter().map(|f| {
            let name = std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy();
            ListItem::new(name.to_string())
        }).collect();
        
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
                .highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(app.theme.highlight_fg))
                .highlight_symbol("> "),
            inner_layout[0],
            &mut app.session_list_state
        );
        
        let help_text = "(Enter) Resume | (N) New | (D) Delete | (X) Wipe All | (Esc) Close";
        f.render_widget(Paragraph::new(help_text).block(UIBlock::default().borders(Borders::TOP).style(Style::default().bg(app.theme.terminal_bg))).style(Style::default().fg(app.theme.system_fg)), inner_layout[1]);
    }

    if app.show_latest_files {
        let area = centered_rect(80, 80, f.area());
        f.render_widget(Clear, area);
        
        let mut total_tokens = 0;
        let all_files = app.context_manager.all_cached_files();
        let items: Vec<ListItem> = all_files.iter().map(|(path, cached, is_active)| {
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
            let content = format!("{} {:<38} {:>6} tok ({})", tier, display_path, cached.tokens, time_str);
            ListItem::new(content)
        }).collect();

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
                .highlight_style(Style::default().add_modifier(Modifier::BOLD).fg(app.theme.highlight_fg))
                .highlight_symbol("> "),
            inner_layout[0],
            &mut app.latest_files_state
        );
        
        let help_text = format!("(R) Remove from Context | (Esc) Close | Total Tokens: {}", total_tokens);
        f.render_widget(Paragraph::new(help_text).block(UIBlock::default().borders(Borders::TOP).style(Style::default().bg(app.theme.terminal_bg))).style(Style::default().fg(app.theme.system_fg)), inner_layout[1]);
    }

    if app.show_approval_prompt {
        let area = centered_rect(70, 60, f.area());
        f.render_widget(Clear, area);
        if let Some(tc) = &app.pending_tool_call {
            let json_text = render_json_highlighted(&tc.function.arguments, &app.theme);

            let mut display_text = Text::from(vec![
                Line::from(vec![
                    Span::styled("Tool: ", Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(&tc.function.name, Style::default().fg(app.theme.success_fg)),
                ]),
                Line::from("Params:"),
            ]);

            // Add JSON lines with possible truncation
            let max_lines = 15;
            let mut lines_added = 0;
            for line in json_text.lines {
                if lines_added >= max_lines {
                    display_text.lines.push(Line::from(Span::styled("... [Truncated for display]", Style::default().fg(app.theme.system_fg))));
                    break;
                }
                display_text.lines.push(line);
                lines_added += 1;
            }

            display_text.lines.push(Line::from(""));
            display_text.lines.push(Line::from(vec![
                Span::styled("(A)", Style::default().fg(app.theme.warning_fg).add_modifier(Modifier::BOLD)),
                Span::raw("lways Allow | "),
                Span::styled("(O)", Style::default().fg(app.theme.warning_fg).add_modifier(Modifier::BOLD)),
                Span::raw("nce | "),
                Span::styled("(D)", Style::default().fg(app.theme.error_fg).add_modifier(Modifier::BOLD)),
                Span::raw("eny"),
            ]));

            let block = UIBlock::default()
                .title(format!("{} Security Confirmation", icons::WARNING))
                .borders(Borders::ALL)
                .style(Style::default().bg(app.theme.terminal_bg))
                .border_style(Style::default().fg(app.theme.warning_fg));

            f.render_widget(Paragraph::new(display_text).block(block).wrap(Wrap { trim: false }), area);
        } else {
            app.show_approval_prompt = false;
        }
    }
    if app.show_prompt_editor {
        let full_area = centered_rect(80, 80, f.area());
        f.render_widget(Clear, full_area);
        
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
            .split(full_area);

        let header_block = UIBlock::default()
            .title(format!("{} System Prompt Editor", icons::MODEL))
            .borders(Borders::ALL)
            .style(if app.is_editing_prompt { Style::default().fg(app.theme.warning_fg).bg(app.theme.terminal_bg) } else { Style::default().bg(app.theme.terminal_bg) });
        
        let instructions = if app.is_editing_prompt { 
            format!("EDITING MODE | Cursor: {} | (ESC) Finish", app.prompt_cursor_pos) 
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
                    display_spans.push(Span::styled(c.to_string(), Style::default().add_modifier(Modifier::REVERSED).fg(app.theme.warning_fg)));
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
            chunks[1]
        );

        if app.show_prompt_save_dialog {
            let dialog_area = centered_rect(50, 20, f.area());
            f.render_widget(Clear, dialog_area);
            
            let dialog_block = UIBlock::default()
                .title(format!("{} Save Prompt As", icons::WARNING))
                .borders(Borders::ALL)
                .style(Style::default().bg(app.theme.terminal_bg))
                .border_style(Style::default().fg(app.theme.warning_fg));
                
            let text = format!("Enter filename (without .md):\n> {}\n\n(ENTER) Save | (ESC) Cancel", app.prompt_save_name);
            f.render_widget(Paragraph::new(text).block(dialog_block).wrap(Wrap { trim: true }), dialog_area);
        }
    }

    if app.show_hotkeys {
        let area = centered_rect(70, 70, f.area());
        f.render_widget(Clear, area);
        let hotkeys_text = vec![
            Line::from(vec![Span::styled("Navigation", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))]),
            Line::from(vec![Span::raw("  TAB       : Toggle Focus between Input and Output")]),
            Line::from(vec![Span::raw("  UP / DOWN : Scroll Output (when focused)")]),
            Line::from(vec![Span::raw("  PGUP/PGDN : Fast Scroll Output")]),
            Line::from(vec![Span::raw("  ESC       : Open Command Palette / Stop Output")]),
            Line::from(vec![]),
            Line::from(vec![Span::styled("Global Toggles", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))]),
            Line::from(vec![Span::raw("  F12       : Toggle Debugger Pane")]),
            Line::from(vec![Span::raw("  F10       : Toggle Mouse (for terminal selection)")]),
            Line::from(vec![Span::raw("  CTRL + P  : Command Palette")]),
            Line::from(vec![Span::raw("  CTRL + L  : Clear UI (Keep Context)")]),
            Line::from(vec![]),
            Line::from(vec![Span::styled("General", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))]),
            Line::from(vec![Span::raw("  ENTER     : Send Prompt / Confirm Selection")]),
            Line::from(vec![Span::raw("  CTRL + C  : Stop Output (1st) / Quit (2nd)")]),
            Line::from(vec![]),
            Line::from(vec![Span::styled("Press ESC or ENTER to close", Style::default().add_modifier(Modifier::ITALIC).fg(Color::DarkGray))]),
        ];
        f.render_widget(Paragraph::new(hotkeys_text).block(UIBlock::default().title(format!("{} Hotkeys Reference", icons::COMMAND)).borders(Borders::ALL).style(Style::default().bg(app.theme.terminal_bg))).wrap(Wrap { trim: false }), area);
    }
}

pub fn render_block_to_lines(block: &RenderBlock, width: usize, theme: &Theme, tool_preview: Option<&str>) -> Vec<Line<'static>> {
    let block_color = match block.block_type {
        BlockType::User => theme.input_fg,
        BlockType::Thought => theme.thought_fg,
        BlockType::Formulating => theme.warning_fg,
        BlockType::ToolCall => theme.tool_fg,
        BlockType::ToolResult => {
            match block.success {
                Some(true) => theme.success_fg,
                Some(false) => theme.error_fg,
                None => theme.system_fg,
            }
        }
        BlockType::Divider => theme.system_fg,
        _ => theme.output_fg,
    };

    let (bg_color, mut header) = match block.block_type {
        BlockType::User => (theme.input_bg, Some(format!("{} User Request", icons::INPUT))),
        BlockType::Thought => (theme.thought_bg, Some(format!("{} Engine Thinking...", icons::PROCESSING))),
        BlockType::Formulating => (theme.thought_bg, Some(format!("{} Formulating tool request...", icons::SPINNER[0]))),
        BlockType::ToolCall => (theme.tool_bg, Some(format!("{} Engine Tool Request", icons::COMMAND))),
        BlockType::ToolResult => (theme.terminal_bg, Some(format!("{} Agent, Tool Output", icons::SUCCESS))),
        BlockType::Divider => (Color::Reset, None),
        _ => (Color::Reset, None),
    };

    if let Some(ref t) = block.title {
        header = match block.block_type {
            BlockType::ToolCall => header, // Keep generic "Engine Tool Request"
            BlockType::ToolResult => Some(format!("{} Agent, {}", icons::SUCCESS, t)),
            _ => Some(t.clone()),
        };
    }

    let status_block = Span::styled("█ ", Style::default().fg(block_color));
    let base_style = Style::default().bg(bg_color).fg(theme.output_fg);
    let mut lines_output: Vec<Line<'static>> = Vec::new();

    if block.block_type == BlockType::Divider {
        lines_output.push(Line::from(vec![
            Span::styled("─".repeat(width), Style::default().fg(theme.system_fg))
        ]));
        return lines_output;
    }

    if let Some(h) = header {
        let mut header_spans = vec![
            status_block.clone(),
            Span::styled(format!(" {} ", h), base_style.add_modifier(Modifier::BOLD).fg(Color::White)),
        ];
        let current_len = 2 + h.len() + 2;
        if width > current_len {
            header_spans.push(Span::styled(" ".repeat(width - current_len), base_style));
        }
        lines_output.push(Line::from(header_spans));
    }

    // Advanced rendering for specialized blocks
    let content_lines: Vec<Line<'static>> = if block.block_type == BlockType::Formulating {
        let block_lines: Vec<&str> = block.content.lines().collect();
        let mut formatted = vec![Line::from(Span::styled("(Engine is preparing the tool payload...)", base_style.add_modifier(Modifier::ITALIC)))];

        let last_lines = if block_lines.len() > 3 {
            &block_lines[block_lines.len() - 3..]
        } else {
            &block_lines[..]
        };

        // Always show 3 lines to prevent bouncing
        for i in 0..3 {
            if i < last_lines.len() {
                let line_content = last_lines[i];
                let max_line_len = width.saturating_sub(10);
                let display_line = if line_content.len() > max_line_len {
                    format!("  {}...", &line_content[..max_line_len.saturating_sub(3)])
                } else {
                    format!("  {}", line_content)
                };
                formatted.push(Line::from(Span::styled(display_line, base_style.add_modifier(Modifier::DIM))));
            } else {
                formatted.push(Line::from(Span::styled("  ", base_style)));
            }
        }
        formatted
    }
 else if block.block_type == BlockType::ToolCall {
        if let Some(brace_pos) = block.content.find('{') {
            let func_name_part = &block.content[..brace_pos];
            let json_part = &block.content[brace_pos..];
            
            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(json_part) {
                let json_text = render_json_highlighted(&json_val, theme);
                let mut formatted = Vec::new();
                
                if let Some(first_line) = json_text.lines.first() {
                    let mut spans = vec![Span::styled(func_name_part.to_string(), Style::default().fg(theme.tool_fg))];
                    spans.extend(first_line.spans.clone());
                    formatted.push(Line::from(spans));
                }
                for line in json_text.lines.iter().skip(1) {
                    formatted.push(line.clone());
                }
                
                if let Some(preview) = tool_preview {
                    formatted.push(Line::from(vec![Span::styled("--- Live Output Preview ---", Style::default().fg(theme.system_fg).add_modifier(Modifier::ITALIC))]));
                    let lines: Vec<&str> = preview.lines().collect();
                    // Always show 5 lines to prevent bouncing
                    for i in 0..5 {
                        if i < lines.len() {
                            let line_content = lines[i];
                            // Truncate to width to prevent wrapping which causes bouncing
                            let max_line_len = width.saturating_sub(10);
                            let display_line = if line_content.len() > max_line_len {
                                format!("> {}...", &line_content[..max_line_len.saturating_sub(3)])
                            } else {
                                format!("> {}", line_content)
                            };
                            formatted.push(Line::from(vec![Span::styled(display_line, Style::default().fg(theme.system_fg).add_modifier(Modifier::ITALIC))]));
                        } else {
                            // Spacer line
                            formatted.push(Line::from(vec![Span::styled(">", Style::default().fg(theme.system_fg).add_modifier(Modifier::ITALIC))]));
                        }
                    }
                }
                
                formatted
            } else {
                block.content.lines().map(|l| Line::from(Span::styled(l.to_string(), base_style))).collect()
            }
        } else {
            block.content.lines().map(|l| Line::from(Span::styled(l.to_string(), base_style))).collect()
        }
    } else if block.block_type == BlockType::Text || block.block_type == BlockType::ToolResult || block.block_type == BlockType::Markdown || block.block_type == BlockType::Thought || block.content.contains("```") {
        markdown::render_markdown(&block.content, theme).lines
    } else {
        block.content.lines().map(|l| Line::from(Span::styled(l.to_string(), base_style))).collect()
    };

    let wrapped = wrap_lines(content_lines, width.saturating_sub(2));

    for mut line in wrapped {
        let mut spans = vec![status_block.clone()];
        
        for span in line.spans.iter_mut() {
            if block.block_type == BlockType::Thought {
                span.style = span.style.add_modifier(Modifier::ITALIC).fg(theme.thought_fg);
            } else if block.block_type == BlockType::ToolCall && span.style.fg.is_none() {
                span.style = span.style.fg(theme.warning_fg);
            }
        }
        
        spans.append(&mut line.spans);
        
        let current_len = 2 + line.width();
        if width > current_len {
            spans.push(Span::styled(" ".repeat(width - current_len), base_style));
        }
        lines_output.push(Line::from(spans));
    }

    let is_final = block.block_type == BlockType::User || block.block_type == BlockType::Divider;
    if is_final {
        lines_output.push(Line::from(vec![status_block, Span::styled(" ".repeat(width.saturating_sub(2)), base_style)]));
    }

    lines_output
}

fn wrap_lines(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 { return lines; }
    let mut wrapped_lines = Vec::new();
    for line in lines {
        if line.spans.is_empty() {
            wrapped_lines.push(Line::from(vec![]));
            continue;
        }

        // Detect line number prefix: 6 chars + tab
        let mut indent_width = 0;
        let mut indent_style = Style::default();
        if let Some(first_span) = line.spans.first() {
            if first_span.content.len() >= 7 
               && first_span.content.chars().take(6).all(|c| c.is_whitespace() || c.is_ascii_digit()) 
               && first_span.content.chars().nth(6) == Some('\t') {
                indent_width = 7;
                indent_style = first_span.style;
            }
        }

        let mut current_line_spans = Vec::new();
        let mut current_width = 0;

        for span in line.spans {
            let text = span.content.to_string();
            let style = span.style;
            
            // Split into words, preserving spaces
            let mut words = Vec::new();
            let mut current_word = String::new();
            for c in text.chars() {
                current_word.push(c);
                if c.is_whitespace() {
                    words.push(current_word);
                    current_word = String::new();
                }
            }
            if !current_word.is_empty() {
                words.push(current_word);
            }

            for word in words {
                let word_width = word.chars().count();
                
                if current_width + word_width <= max_width {
                    current_line_spans.push(Span::styled(word, style));
                    current_width += word_width;
                } else {
                    if !current_line_spans.is_empty() {
                        wrapped_lines.push(Line::from(std::mem::take(&mut current_line_spans)));
                    }
                    
                    // Start new line with indent if needed
                    if indent_width > 0 {
                        current_line_spans.push(Span::styled(" ".repeat(indent_width), indent_style));
                        current_width = indent_width;
                    } else {
                        current_width = 0;
                    }

                    if word_width + current_width > max_width {
                        // Word is longer than remaining line, split it
                        let mut remaining = word;
                        let available = max_width.saturating_sub(current_width);
                        
                        if available > 0 {
                            let head: String = remaining.chars().take(available).collect();
                            let tail: String = remaining.chars().skip(available).collect();
                            current_line_spans.push(Span::styled(head, style));
                            wrapped_lines.push(Line::from(std::mem::take(&mut current_line_spans)));
                            remaining = tail;
                        } else {
                            // No room even for one char, just wrap and start fresh (with indent)
                            if !current_line_spans.is_empty() {
                                wrapped_lines.push(Line::from(std::mem::take(&mut current_line_spans)));
                            }
                        }

                        while remaining.chars().count() > max_width.saturating_sub(indent_width) {
                            let chunk_size = max_width.saturating_sub(indent_width);
                            if chunk_size == 0 { break; } // Safety
                            
                            let mut next_line = Vec::new();
                            if indent_width > 0 {
                                next_line.push(Span::styled(" ".repeat(indent_width), indent_style));
                            }
                            let head: String = remaining.chars().take(chunk_size).collect();
                            let tail: String = remaining.chars().skip(chunk_size).collect();
                            next_line.push(Span::styled(head, style));
                            wrapped_lines.push(Line::from(next_line));
                            remaining = tail;
                        }
                        
                        // Final piece of the split word
                        if !remaining.is_empty() {
                            if indent_width > 0 {
                                current_line_spans.push(Span::styled(" ".repeat(indent_width), indent_style));
                            }
                            current_line_spans.push(Span::styled(remaining.clone(), style));
                            current_width = indent_width + remaining.chars().count();
                        }
                    } else {
                        current_line_spans.push(Span::styled(word, style));
                        current_width += word_width;
                    }
                }
            }
        }
        if !current_line_spans.is_empty() {
            wrapped_lines.push(Line::from(current_line_spans));
        }
    }
    wrapped_lines
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage((100 - percent_y) / 2), Constraint::Percentage(percent_y), Constraint::Percentage((100 - percent_y) / 2)].as_ref()).split(r);
    Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage((100 - percent_x) / 2), Constraint::Percentage(percent_x), Constraint::Percentage((100 - percent_x) / 2)].as_ref()).split(popup_layout[1])[1]
}
