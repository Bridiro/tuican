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

    let frames = layout::split(area);
    let visible_count = app.visible_messages().len();
    let rx_count = app.rx.len();
    app.layout = layout::LayoutMap {
        messages: frames.messages,
        signals: frames.signals,
        rx: frames.rx,
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
    panes::cyclic(f, frames.cyclic, app);
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
