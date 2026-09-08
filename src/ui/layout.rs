//! Where things go. Every frame is laid out from the *current* terminal size,
//! so there is no resize handler anywhere in the program. A `Resize` event is
//! just a redraw trigger.

use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::action::Pane;

/// The rects of the last frame, kept solely so the mouse can hit-test them.
#[derive(Clone, Copy, Default)]
pub struct LayoutMap {
    pub messages: Rect,
    pub signals: Rect,
    pub rx: Rect,
    pub cyclic: Rect,
    pub rules: Rect,
}

impl LayoutMap {
    pub fn rect_of(&self, pane: Pane) -> Rect {
        match pane {
            Pane::Messages => self.messages,
            Pane::Signals => self.signals,
            Pane::Rx => self.rx,
            Pane::Cyclic => self.cyclic,
            Pane::Rules => self.rules,
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
    /// Hosts the periodic-send and rules panels when either is open, otherwise
    /// it is the one-line summary strip.
    pub band: Rect,
    pub status: Rect,
    pub hints: Rect,
}

/// Height a bottom panel wants for `rows` entries: a title, a column header,
/// and the rows, capped so it never crowds out the receive table.
pub fn panel_height(rows: usize) -> u16 {
    (rows as u16 + 2).clamp(3, 9)
}

/// Share the bottom band between the two panels. Side by side when there is
/// room for both, stacked when there is not.
pub fn split_band(band: Rect, cyclic: bool, rules: bool) -> (Rect, Rect) {
    match (cyclic, rules) {
        (true, true) if band.width >= 120 => {
            let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(band);
            (cols[0], cols[1])
        }
        (true, true) => {
            let rows = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(band);
            (rows[0], rows[1])
        }
        (true, false) => (band, Rect::ZERO),
        (false, true) => (Rect::ZERO, band),
        (false, false) => (Rect::ZERO, Rect::ZERO),
    }
}

/// Split the terminal. The top two panes sit side by side when there is room
/// and stack when there is not, rather than being squeezed into uselessness.
pub fn split(area: Rect, band_height: u16) -> Frames {
    let rows = Layout::vertical([
        Constraint::Length(1),           // header
        Constraint::Percentage(45),      // messages | signals
        Constraint::Min(4),              // receive, absorbs the slack
        Constraint::Length(band_height), // one-line strip, or the panels
        Constraint::Length(1),           // status / last error
        Constraint::Length(1),           // key hints
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
        band: rows[3],
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
/// This is readability, not identity. At a narrow width two names in the same
/// family can still render alike; the id column beside them distinguishes the
/// rows, and it is never elided.
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
    chars[..head].iter().collect::<String>()
        + "…"
        + &chars[chars.len() - tail..].iter().collect::<String>()
}

/// Make a composed row exactly `width` columns: cut at the right edge, or pad.
///
/// Rows are column-aligned, so trimming the tail preserves the layout where
/// [`elide`] would cut through the columns. Padding matters too: without it a
/// highlighted row only covers its own text instead of the whole pane.
pub fn fit(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() == width {
        return text.to_string();
    }
    if chars.len() < width {
        return text.to_string() + &" ".repeat(width - chars.len());
    }
    if width == 0 {
        return String::new();
    }
    chars[..width - 1].iter().collect::<String>() + "…"
}

/// Break text into lines of at most `width` columns, splitting on spaces and
/// only mid-word when a single word is itself too long.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut len = 0;
    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        if wlen > width {
            if len > 0 {
                lines.push(std::mem::take(&mut line));
                len = 0;
            }
            let mut chars = word.chars().peekable();
            while chars.peek().is_some() {
                let chunk: String = chars.by_ref().take(width).collect();
                lines.push(chunk);
            }
            continue;
        }
        if len > 0 && len + 1 + wlen > width {
            lines.push(std::mem::take(&mut line));
            len = 0;
        }
        if len > 0 {
            line.push(' ');
            len += 1;
        }
        line.push_str(word);
        len += wlen;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_breaks_on_spaces_and_never_exceeds_the_width() {
        let lines = wrap(
            "state=Running (2)  ready=Ready (1)  torqueActual=125.4 Nm",
            24,
        );
        assert!(lines.len() > 1);
        for l in &lines {
            assert!(l.chars().count() <= 24, "{l:?}");
        }
        assert!(lines.join(" ").contains("torqueActual=125.4"));
    }

    #[test]
    fn a_word_longer_than_the_width_is_split_rather_than_lost() {
        let lines = wrap("0123456789abcdefghij", 8);
        assert_eq!(lines, vec!["01234567", "89abcdef", "ghij"]);
    }

    #[test]
    fn empty_input_still_yields_one_line() {
        assert_eq!(wrap("", 10), vec![String::new()]);
        assert!(wrap("anything", 0).is_empty());
    }

    #[test]
    fn fitting_a_row_keeps_the_leading_columns_and_fills_the_width() {
        // Padded, so a selected row highlights across the whole pane.
        assert_eq!(fit("0x123  Name  42", 20), "0x123  Name  42     ");
        assert_eq!(fit("0x123  Name  42", 9), "0x123  N…");
        assert_eq!(fit("abc", 0), "");
        for width in [0, 1, 8, 40] {
            assert_eq!(fit("0x123  Name", width).chars().count(), width);
        }
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
        // The suffix survives, so a voltage still reads differently from a
        // temperature.
        assert!(elide("TsacCellboard1Voltage", 14).ends_with("oltage"));
        assert!(elide("TsacCellboard1Temperature", 14).ends_with("rature"));
    }

    #[test]
    fn a_tiny_terminal_still_produces_valid_rects() {
        let f = split(Rect::new(0, 0, MIN_WIDTH, MIN_HEIGHT), 1);
        assert!(f.rx.height >= 1);
        assert!(f.messages.width > 0 && f.signals.width > 0);
    }

    #[test]
    fn both_panels_share_the_band_and_stack_when_narrow() {
        let wide = Rect::new(0, 20, 160, 8);
        let (c, r) = split_band(wide, true, true);
        assert_eq!(c.height, 8);
        assert_eq!(r.height, 8);
        assert!(c.width > 0 && r.width > 0 && c.x != r.x);

        let narrow = Rect::new(0, 20, 90, 8);
        let (c, r) = split_band(narrow, true, true);
        assert_eq!(c.width, 90);
        assert!(c.y != r.y, "narrow terminals stack instead of squeezing");

        let (c, r) = split_band(wide, true, false);
        assert_eq!(c, wide);
        assert_eq!(r, Rect::ZERO);
    }

    #[test]
    fn the_cyclic_panel_takes_its_space_from_the_receive_table() {
        let area = Rect::new(0, 0, 120, 30);
        let strip = split(area, 1);
        let panel = split(area, panel_height(4));
        assert_eq!(panel.band.height, 6);
        assert!(panel.rx.height < strip.rx.height);
        // The message panes keep their share either way.
        assert_eq!(panel.messages.height, strip.messages.height);
    }
}
