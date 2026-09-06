//! The individual panes. Each one renders from `&App` and never writes to it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::app::action::Pane;
use crate::app::{App, Link, Picker, PickerKind, Prompt};
use crate::canid;
use crate::dbc::mux;
use crate::ui::layout::{clip, elide};
use crate::ui::theme;

fn block(title: String, active: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::TOP)
        .border_style(theme::border(active))
        .title(Span::styled(format!(" {title} "), theme::title(active)))
}

pub fn header(f: &mut Frame, area: Rect, app: &App) {
    let link = match app.link {
        Link::Up => format!(" ● {} ", app.link_label),
        Link::Down => " ○ not connected ".to_string(),
    };
    // Least important last: a narrow terminal drops fields off the end rather
    // than clipping one mid-word.
    let fields = [
        format!("{} · {} msgs", app.db.label(), app.db.messages.len()),
        format!("tx {} rx {}", app.stats.tx, app.stats.rx),
        format!("ids {}", app.rx.len()),
        format!("cyclic {}", app.cyclic.len()),
    ];
    let mut text = link;
    for field in fields {
        if text.chars().count() + field.chars().count() + 3 > area.width as usize {
            break;
        }
        text.push_str(&format!("│ {field} "));
    }
    f.render_widget(
        Paragraph::new(format!("{text:<width$}", width = area.width as usize))
            .style(theme::header()),
        area,
    );
}

pub fn messages(f: &mut Frame, area: Rect, app: &App) {
    let active = app.pane == Pane::Messages;
    let title = if app.filter.is_empty() {
        "messages".to_string()
    } else {
        format!("messages  /{}", app.filter)
    };
    let inner = area.height.saturating_sub(1) as usize;
    let shown = app.visible_messages();
    let width = area.width.saturating_sub(2) as usize;

    let items: Vec<ListItem> = shown
        .iter()
        .enumerate()
        .skip(app.msg_scroll.offset)
        .take(inner)
        .map(|(i, m)| {
            let live = app.cyclic.contains_key(&m.name);
            let name_width = width.saturating_sub(canid::display(m.id).len() + 3);
            let text = format!(
                "{}{} {}",
                if live { "~" } else { " " },
                canid::display(m.id),
                elide(&m.name, name_width.max(4)),
            );
            let style = if i == app.msg_index {
                theme::selected(active)
            } else if live {
                theme::accent()
            } else {
                theme::normal()
            };
            ListItem::new(Line::from(Span::styled(text, style)))
        })
        .collect();

    let empty = shown.is_empty();
    f.render_widget(List::new(items).block(block(title, active)), area);
    if empty {
        let hint = if app.db.is_empty() {
            "no DBC loaded — press F3"
        } else {
            "nothing matches this filter"
        };
        let row = Rect { y: area.y + 1, height: 1, ..area };
        f.render_widget(Paragraph::new(hint).style(theme::dim()), row);
    }
}

pub fn signals(f: &mut Frame, area: Rect, app: &App) {
    let active = app.pane == Pane::Signals;
    let Some(msg) = app.current_message() else {
        f.render_widget(block("signals".into(), active), area);
        return;
    };
    let title = format!("signals  {} ({} B)", msg.name, msg.len);
    let values = app.values_of(&msg.name);
    let live: Vec<&str> = mux::active_signals(msg, &values)
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    let inner = area.height.saturating_sub(1) as usize;
    let width = area.width.saturating_sub(2) as usize;
    let name_width = (width / 2).clamp(8, 24);

    let items: Vec<ListItem> = msg
        .signals
        .iter()
        .enumerate()
        .skip(app.sig_scroll.offset)
        .take(inner)
        .map(|(i, sig)| {
            let value = values.get(&sig.name).copied().unwrap_or(0.0);
            let raw = ((value - sig.offset) / sig.factor).round() as i64;
            let shown = crate::dbc::codec::format_value(sig, raw, value);
            let included = live.contains(&sig.name.as_str());
            let mut text = format!(
                "{:<name_width$} {shown}",
                elide(&sig.name, name_width),
                name_width = name_width
            );
            if !included {
                // Say why it is greyed out rather than leaving the user guessing.
                text.push_str("   (not in this mux)");
            }
            let style = if i == app.sig_index {
                theme::selected(active)
            } else if included {
                theme::normal()
            } else {
                theme::dim()
            };
            ListItem::new(Line::from(Span::styled(clip(&text, width), style)))
        })
        .collect();

    f.render_widget(List::new(items).block(block(title, active)), area);
}

pub fn rx(f: &mut Frame, area: Rect, app: &App) {
    let active = app.pane == Pane::Rx;
    let width = area.width as usize;
    let now = app.now();

    // Shed columns rather than truncate: the decoded text is the point of the
    // pane, so it is the last thing to lose room.
    let show_period = width >= 100;
    let show_hex = width >= 80;

    let mut header = format!("  {:>10} {:<24} {:>8}", "id", "name", "count");
    if show_period {
        header.push_str(&format!(" {:>7}", "ms"));
    }
    if show_hex {
        header.push_str(&format!("  {:<23}", "data"));
    }
    header.push_str("  decoded");

    let inner = area.height.saturating_sub(2) as usize;
    let items: Vec<ListItem> = app
        .rx
        .iter()
        .enumerate()
        .skip(app.rx_scroll.offset)
        .take(inner)
        .map(|(i, row)| {
            let mut text = format!(
                "  {:>10} {:<24} {:>8}",
                canid::display(row.id),
                elide(&row.name, 24),
                row.count
            );
            if show_period {
                text.push_str(&format!(" {:>7.1}", row.period_ms));
            }
            if show_hex {
                text.push_str(&format!("  {:<23}", elide(&row.data.hex(), 23)));
            }
            text.push_str(&format!("  {}", row.decoded));

            let style = if i == app.rx_index {
                theme::selected(active)
            } else if row.stale(now) {
                // A node that stopped talking, judged against its own rate.
                theme::dim()
            } else if row.known {
                theme::normal()
            } else {
                theme::accent()
            };
            ListItem::new(Line::from(Span::styled(clip(&text, width), style)))
        })
        .collect();

    // Three separate widgets rather than one List with a block: the block owns
    // row 0, the column header owns row 1, and the rows start at row 2.
    let title = format!("receive  {} ids", app.rx.len());
    f.render_widget(block(title, active), area);
    f.render_widget(
        Paragraph::new(header).style(theme::dim()),
        Rect { y: area.y + 1, height: 1, ..area },
    );
    f.render_widget(
        List::new(items),
        Rect {
            y: area.y + 2,
            height: area.height.saturating_sub(2),
            ..area
        },
    );
}

pub fn cyclic(f: &mut Frame, area: Rect, app: &App) {
    let text = if app.stats.cyclic.is_empty() {
        " cyclic: none".to_string()
    } else {
        let parts: Vec<String> = app
            .stats
            .cyclic
            .iter()
            .map(|(name, count)| format!("{name}({count})"))
            .collect();
        format!(" cyclic: {}", parts.join(", "))
    };
    f.render_widget(Paragraph::new(clip(&text, area.width as usize)).style(theme::dim()), area);
}

pub fn status(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
        Span::styled(format!(" [{}] ", app.pane.name()), theme::dim()),
        Span::raw(app.status.clone()),
    ];
    if let Some(err) = &app.last_error {
        spans.push(Span::styled(format!("   ⚠ {err}"), theme::error()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn hints(f: &mut Frame, area: Rect, app: &App) {
    let mouse = if app.mouse { "mouse:on" } else { "mouse:off" };
    let full = format!(
        " tab pane · / filter · ⏎ edit · s send · p cyclic · x stop · r raw · c clear · \
         F3 dbc · F4 iface · F2 {mouse} · ? help · q quit "
    );
    let short = format!(" tab · / · ⏎ · s · p · x · r · c · F3 · F4 · ? · q  [{mouse}] ");
    let text = if full.len() <= area.width as usize { full } else { short };
    f.render_widget(
        Paragraph::new(format!("{text:<width$}", width = area.width as usize))
            .style(theme::hints()),
        area,
    );
}

pub fn prompt(f: &mut Frame, area: Rect, prompt: &Prompt) {
    // The prompt replaces the status line, so wipe what was there first.
    f.render_widget(Clear, area);
    let mut spans = vec![
        Span::styled(format!(" {} ", prompt.label), theme::accent()),
        Span::raw(prompt.buffer.clone()),
        Span::styled("▏", theme::accent()),
    ];
    // The default sits after the cursor, greyed: visible, but plainly not text
    // you are about to append to.
    if prompt.buffer.is_empty()
        && let Some(default) = &prompt.default
    {
        spans.push(Span::styled(format!("{default}  (enter keeps this)"), theme::dim()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn picker(f: &mut Frame, area: Rect, picker: &Picker) {
    let hint = match picker.kind {
        PickerKind::Interface(_) => "↑↓ choose · ⏎ connect · esc skip",
        PickerKind::Bitrate(_) => "↑↓ choose · ⏎ connect · esc back",
        PickerKind::Dbc(_) => "↑↓ choose · ⏎ load · esc skip",
    };
    let width = area.width.saturating_sub(2) as usize;
    let label_width = width / 2;

    let items: Vec<ListItem> = picker
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let text = format!(
                " {:<label_width$}  {}",
                elide(&row.label, label_width),
                elide(&row.detail, width.saturating_sub(label_width + 3)),
                label_width = label_width
            );
            let style = if i == picker.index {
                theme::selected(true)
            } else {
                theme::normal()
            };
            ListItem::new(Line::from(Span::styled(text, style)))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border(true))
        .title(Span::styled(format!(" {} ", picker.title), theme::title(true)))
        .title_bottom(Span::styled(format!(" {hint} "), theme::dim()));
    f.render_widget(List::new(items).block(block), area);
}

const KEYS: &[(&str, &str)] = &[
    ("tab / shift-tab", "move between panes"),
    ("↑ ↓  k j", "move within a pane"),
    ("pgup / pgdn, g / G", "page, top, bottom"),
    ("/", "filter messages by name or hex id"),
    ("enter", "edit the selected signal"),
    ("s", "send the selected message once"),
    ("p", "start or stop sending it periodically"),
    ("x", "stop every periodic send"),
    ("r", "raw send, e.g.  4E5 11 22 33"),
    ("c", "clear the receive table"),
    ("F2", "mouse on/off (off restores text selection)"),
    ("F3", "load a different DBC, keeping the link"),
    ("F4", "connect to a different interface"),
    ("q", "quit"),
];

/// Sized from the content, so the box has no dead rows and clips nothing.
pub fn help_size() -> (u16, u16) {
    const KEY_COLUMN: usize = 20;
    let widest = KEYS.iter().map(|(_, what)| what.chars().count()).max().unwrap_or(0);
    ((KEY_COLUMN + widest + 4) as u16, KEYS.len() as u16 + 2)
}

pub fn help(f: &mut Frame, area: Rect) {
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(format!(" {key:<20}"), theme::accent()),
                Span::raw((*what).to_string()),
            ])
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border(true))
        .title(Span::styled(" keys ", theme::title(true)))
        .title_bottom(Span::styled(" any key closes ", theme::dim()));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

