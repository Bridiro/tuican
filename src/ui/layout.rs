//! Where things go. Every frame is laid out from the *current* terminal size,
//! so there is no resize handler anywhere in the program — a `Resize` event is
//! just a redraw trigger.

use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::action::Pane;

/// The rects of the last frame, kept solely so the mouse can hit-test them.
#[derive(Clone, Copy, Default)]
pub struct LayoutMap {
    pub messages: Rect,
    pub signals: Rect,
    pub rx: Rect,
}

impl LayoutMap {
    pub fn rect_of(&self, pane: Pane) -> Rect {
        match pane {
            Pane::Messages => self.messages,
            Pane::Signals => self.signals,
            Pane::Rx => self.rx,
        }
    }
}

/// Below this, panes cannot hold a useful number of rows and we say so rather
/// than rendering unreadable slivers.
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 12;

pub struct Frames {
    pub header: Rect,
    pub messages: Rect,
    pub signals: Rect,
    pub rx: Rect,
    pub cyclic: Rect,
    pub status: Rect,
    pub hints: Rect,
}

/// Split the terminal. The top two panes sit side by side when there is room
/// and stack when there is not, rather than being squeezed into uselessness.
pub fn split(area: Rect) -> Frames {
    let rows = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Percentage(45), // messages | signals
        Constraint::Min(4),    // receive — absorbs the slack
        Constraint::Length(1), // cyclic strip
        Constraint::Length(1), // status / last error
        Constraint::Length(1), // key hints
    ])
    .split(area);

    let top = if area.width >= 110 {
        // Wide: give the message list a fixed, comfortable column.
        Layout::horizontal([Constraint::Length(38), Constraint::Min(40)]).split(rows[1])
    } else if area.width >= 80 {
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(rows[1])
    } else {
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1])
    };

    Frames {
        header: rows[0],
        messages: top[0],
        signals: top[1],
        rx: rows[2],
        cyclic: rows[3],
        status: rows[4],
        hints: rows[5],
    }
}

/// A centred box for modals, clamped so it always fits.
pub fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2)).max(1);
    let h = height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// Keep a cursor visible in a window of `height` rows without jumping around:
/// scroll only when the cursor would otherwise leave the window.
pub fn scroll_to_show(scroll: usize, cursor: usize, height: usize, total: usize) -> usize {
    if height == 0 {
        return 0;
    }
    let max_scroll = total.saturating_sub(height);
    let mut s = scroll.min(max_scroll);
    if cursor < s {
        s = cursor;
    } else if cursor >= s + height {
        s = cursor + 1 - height;
    }
    s.min(max_scroll)
}

/// Elide in the middle. DBC names within a family share a long prefix
/// (`TsacCellboard1Voltage`, `TsacCellboard2Temperature`), so cutting the tail
/// loses the part that says what the signal *is*.
///
/// This is readability, not identity: at a narrow width two names in the same
/// family can still render alike. The id column beside them is what actually
/// distinguishes rows, and it is never elided.
pub fn elide(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".repeat(width);
    }
    let keep = width - 1;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    chars[..head].iter().collect::<String>() + "…" + &chars[chars.len() - tail..].iter().collect::<String>()
}

/// Cut a composed row at the right edge. Rows are column-aligned, so trimming
/// the tail preserves the layout; [`elide`] would cut through the columns.
pub fn clip(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    chars[..width - 1].iter().collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_a_row_keeps_the_leading_columns_intact() {
        assert_eq!(clip("0x123  Name  42", 20), "0x123  Name  42");
        assert_eq!(clip("0x123  Name  42", 9), "0x123  N…");
        assert_eq!(clip("abc", 0), "");
    }

    #[test]
    fn scrolling_only_moves_when_the_cursor_would_leave() {
        assert_eq!(scroll_to_show(0, 5, 10, 100), 0); // already visible
        assert_eq!(scroll_to_show(0, 10, 10, 100), 1); // one past the bottom
        assert_eq!(scroll_to_show(20, 3, 10, 100), 3); // jumped above
        assert_eq!(scroll_to_show(95, 99, 10, 100), 90); // clamped to the end
    }

    #[test]
    fn eliding_keeps_both_ends() {
        assert_eq!(elide("short", 10), "short");
        assert_eq!(elide("TsacCellboard1Voltage", 12), "TsacCe…ltage");
        assert!(elide("TsacCellboard1Voltage", 12).chars().count() <= 12);
        // The suffix survives, which is what says whether a signal is a
        // voltage or a temperature.
        assert!(elide("TsacCellboard1Voltage", 14).ends_with("oltage"));
        assert!(elide("TsacCellboard1Temperature", 14).ends_with("rature"));
    }

    #[test]
    fn a_tiny_terminal_still_produces_valid_rects() {
        let f = split(Rect::new(0, 0, MIN_WIDTH, MIN_HEIGHT));
        assert!(f.rx.height >= 1);
        assert!(f.messages.width > 0 && f.signals.width > 0);
    }
}
