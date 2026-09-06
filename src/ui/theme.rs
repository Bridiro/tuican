//! Every style in one place, so the panes stay consistent.

use ratatui::style::{Color, Modifier, Style};

pub fn header() -> Style {
    Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
}

pub fn hints() -> Style {
    Style::default().fg(Color::Black).bg(Color::DarkGray)
}

pub fn title(active: bool) -> Style {
    if active {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

pub fn border(active: bool) -> Style {
    Style::default().fg(if active { Color::Cyan } else { Color::DarkGray })
}

/// The cursor row. Dimmed when its pane is not focused, so it is always clear
/// which pane the keyboard is driving.
pub fn selected(focused: bool) -> Style {
    if focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)
    }
}

pub fn normal() -> Style {
    Style::default()
}

pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn error() -> Style {
    Style::default().fg(Color::Red)
}

pub fn accent() -> Style {
    Style::default().fg(Color::Yellow)
}
