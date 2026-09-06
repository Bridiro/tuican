//! Mouse → [`Action`], under one rule: the mouse may only do what a key can
//! already do, and never anything destructive. No click sends a frame, and
//! nothing in the tool is mouse-only.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use crate::app::Mode;
use crate::app::action::{Action, Pane};
use crate::ui::layout::LayoutMap;

pub fn map(event: MouseEvent, layout: &LayoutMap, mode: &Mode) -> Action {
    // A modal owns the keyboard; let it own the mouse too rather than have a
    // click quietly change the selection behind an open prompt.
    if !matches!(mode, Mode::Normal) {
        return Action::None;
    }
    let at = Position::new(event.column, event.row);
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => match hit(at, layout) {
            Some((pane, row)) => Action::FocusAndSelect(pane, row),
            None => Action::None,
        },
        // Scrolls the pane under the cursor, not the focused one, and leaves
        // focus alone — so the wheel never displaces keyboard navigation.
        MouseEventKind::ScrollDown => Action::ScrollIn(pane_at(at, layout), 3),
        MouseEventKind::ScrollUp => Action::ScrollIn(pane_at(at, layout), -3),
        _ => Action::None,
    }
}

fn pane_at(at: Position, layout: &LayoutMap) -> Option<Pane> {
    Pane::ALL.into_iter().find(|p| layout.rect_of(*p).contains(at))
}

/// Which pane, and which visible row within it, the pointer is over.
fn hit(at: Position, layout: &LayoutMap) -> Option<(Pane, usize)> {
    let pane = pane_at(at, layout)?;
    let rect = layout.rect_of(pane);
    let first_row = rect.y + pane.header_rows();
    if at.y < first_row || at.y >= rect.y + rect.height {
        return None; // the title bar or the bottom border
    }
    Some((pane, (at.y - first_row) as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn layout() -> LayoutMap {
        LayoutMap {
            messages: Rect::new(0, 1, 38, 10),
            signals: Rect::new(38, 1, 42, 10),
            rx: Rect::new(0, 11, 80, 10),
        }
    }

    #[test]
    fn a_click_lands_on_the_row_under_the_pointer() {
        // Row 1 of the messages pane is the first item (row 0 is its title).
        assert_eq!(hit(Position::new(4, 2), &layout()), Some((Pane::Messages, 0)));
        assert_eq!(hit(Position::new(4, 5), &layout()), Some((Pane::Messages, 3)));
        // The receive pane has a column header, so its items start a row later.
        assert_eq!(hit(Position::new(4, 13), &layout()), Some((Pane::Rx, 0)));
    }

    #[test]
    fn clicking_a_title_bar_does_nothing() {
        assert_eq!(hit(Position::new(4, 1), &layout()), None);
        assert_eq!(hit(Position::new(4, 12), &layout()), None);
    }

    #[test]
    fn modals_swallow_the_mouse_entirely() {
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 2,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        assert_eq!(map(click, &layout(), &Mode::Help), Action::None);
    }
}
