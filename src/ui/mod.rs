//! Drawing. A pure function of `&App` and the terminal's current size: nothing
//! here mutates state except the scroll offsets, which depend on the height we
//! were actually given this frame.

pub mod input;
pub mod layout;
pub mod mouse;
pub mod panes;
pub mod theme;

use ratatui::Frame;
use ratatui::widgets::{Clear, Paragraph};

use crate::app::{App, Mode};

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();

    if area.width < layout::MIN_WIDTH || area.height < layout::MIN_HEIGHT {
        let msg = format!(
            "terminal too small\nneed {}x{}, have {}x{}",
            layout::MIN_WIDTH,
            layout::MIN_HEIGHT,
            area.width,
            area.height
        );
        f.render_widget(Paragraph::new(msg).centered(), area);
        return;
    }

    // The panels borrow their rows from the receive table; with both closed the
    // band is the one-line summary strip instead.
    let stacked = app.show_cyclic_panel && app.show_rules_panel && area.width < 120;
    let wanted = |open: bool, rows: usize| if open { layout::panel_height(rows) } else { 0 };
    let cyclic_rows = wanted(app.show_cyclic_panel, app.cyclic.len());
    let rules_rows = wanted(app.show_rules_panel, app.rules.rules.len());
    let band_height = match (app.show_cyclic_panel, app.show_rules_panel) {
        (false, false) => 1,
        _ if stacked => (cyclic_rows + rules_rows).min(12),
        _ => cyclic_rows.max(rules_rows),
    };
    let frames = layout::split(area, band_height);
    let (cyclic_area, rules_area) =
        layout::split_band(frames.band, app.show_cyclic_panel, app.show_rules_panel);
    let visible_count = app.visible_messages().len();
    let rx_count = app.rx.len();
    let cyclic_count = app.cyclic.len();
    let rules_count = app.rules.rules.len();
    app.layout = layout::LayoutMap {
        messages: frames.messages,
        signals: frames.signals,
        rx: frames.rx,
        cyclic: cyclic_area,
        rules: rules_area,
    };

    // Reconcile the scroll offsets against the height this frame actually has.
    // Doing it here rather than on resize is why there is no resize handler.
    let signal_count = app.current_message().map_or(0, |m| m.signals.len());
    for (scroll, cursor, rows, total) in [
        (
            &mut app.msg_scroll,
            app.msg_index,
            frames.messages.height.saturating_sub(1) as usize,
            visible_count,
        ),
        (
            &mut app.sig_scroll,
            app.sig_index,
            frames.signals.height.saturating_sub(1) as usize,
            signal_count,
        ),
        (
            &mut app.rx_scroll,
            app.rx_index,
            frames.rx.height.saturating_sub(2) as usize,
            rx_count,
        ),
        (
            &mut app.cyclic_scroll,
            app.cyclic_index,
            cyclic_area.height.saturating_sub(2) as usize,
            cyclic_count,
        ),
        (
            &mut app.rules_scroll,
            app.rules_index,
            rules_area.height.saturating_sub(2) as usize,
            rules_count,
        ),
    ] {
        scroll.offset = if scroll.follow {
            layout::scroll_to_show(scroll.offset, cursor, rows, total)
        } else {
            // Detached by the wheel: leave it where the user put it, but never
            // past the end of a list that has since shrunk.
            scroll.offset.min(total.saturating_sub(rows))
        };
    }

    panes::header(f, frames.header, app);
    panes::messages(f, frames.messages, app);
    panes::signals(f, frames.signals, app);
    panes::rx(f, frames.rx, app);
    if app.show_cyclic_panel {
        panes::cyclic_panel(f, cyclic_area, app);
    }
    if app.show_rules_panel {
        panes::rules_panel(f, rules_area, app);
    }
    if !app.show_cyclic_panel && !app.show_rules_panel {
        panes::cyclic_strip(f, frames.band, app);
    }
    panes::status(f, frames.status, app);
    panes::hints(f, frames.hints, app);

    match &app.mode {
        Mode::Prompt(p) => panes::prompt(f, frames.status, p),
        Mode::Picker(p) => {
            let rect = layout::centred(area, 72, p.rows.len() as u16 + 2);
            f.render_widget(Clear, rect);
            panes::picker(f, rect, p);
        }
        Mode::Help => {
            let (w, h) = panes::help_size();
            let rect = layout::centred(area, w, h);
            f.render_widget(Clear, rect);
            panes::help(f, rect);
        }
        Mode::Normal => {}
    }
}
