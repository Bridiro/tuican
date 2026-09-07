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

/// Draw a pane that has a column header: the title bar, the header, and the
/// body, each in its own rect.
///
/// Rendering a header *over* a list that already occupies the row silently
/// hides the list's first item, so the three never share a row here.
fn with_header(f: &mut Frame, area: Rect, title: String, active: bool, header: &str) -> Rect {
    f.render_widget(block(title, active), area);
    if area.height < 2 {
        return Rect { height: 0, ..area };
    }
    let header_row = Rect { y: area.y + 1, height: 1, ..area };
    f.render_widget(
        Paragraph::new(clip(header, area.width as usize)).style(theme::dim()),
        header_row,
    );
    Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area }
}

fn block(title: String, active: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::TOP)
        .border_style(theme::border(active))
        .title(Span::styled(format!(" {title} "), theme::title(active)))
}

pub fn header(f: &mut Frame, area: Rect, app: &App) {
    let link = match app.link {
        Link::Up => format!(" ● {} ", app.link_label),
        Link::Retrying => format!(" ◐ {} ", app.link_label),
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

pub fn cyclic_strip(f: &mut Frame, area: Rect, app: &App) {
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

/// The periodic sends in full: what is going out, how fast, and how many have
/// gone. The strip above only has room for names.
pub fn cyclic_panel(f: &mut Frame, area: Rect, app: &App) {
    let active = app.pane == Pane::Cyclic;
    let width = area.width as usize;
    let counts: std::collections::HashMap<&str, u64> = app
        .stats
        .cyclic
        .iter()
        .map(|(name, count)| (name.as_str(), *count))
        .collect();

    let header = format!(
        "  {:<28} {:>10} {:>9} {:>8}  {}",
        "message", "id", "period", "sent", "data"
    );
    let inner = area.height.saturating_sub(2) as usize;
    let items: Vec<ListItem> = app
        .cyclic
        .iter()
        .enumerate()
        .skip(app.cyclic_scroll.offset)
        .take(inner)
        .map(|(i, (name, entry))| {
            let text = format!(
                "  {:<28} {:>10} {:>7.0}ms {:>8}  {}",
                elide(name, 28),
                canid::display(entry.id),
                entry.period.as_secs_f64() * 1000.0,
                counts.get(name.as_str()).copied().unwrap_or(0),
                entry.data.hex(),
            );
            let style = if i == app.cyclic_index {
                theme::selected(active)
            } else {
                theme::accent()
            };
            ListItem::new(Line::from(Span::styled(clip(&text, width), style)))
        })
        .collect();

    let title = format!("cyclic  {} running", app.cyclic.len());
    let body = with_header(f, area, title, active, &header);
    if app.cyclic.is_empty() {
        f.render_widget(
            Paragraph::new("  nothing running — select a message and press p").style(theme::dim()),
            body,
        );
    } else {
        f.render_widget(List::new(items), body);
    }
}

/// The reaction rules: what each one watches, whether it is armed, and how
/// many times it has fired.
pub fn rules_panel(f: &mut Frame, area: Rect, app: &App) {
    let active = app.pane == Pane::Rules;
    let width = area.width as usize;
    let header = format!("  {:<3} {:<26} {:>7} {:>6}  {}", "on", "rule", "state", "fired", "when");

    let inner = area.height.saturating_sub(2) as usize;
    let items: Vec<ListItem> = app
        .rules
        .rules
        .iter()
        .enumerate()
        .skip(app.rules_scroll.offset)
        .take(inner)
        .map(|(i, rule)| {
            let state = if rule.warning.is_some() {
                "bad"
            } else if !rule.enabled {
                "off"
            } else if rule.holding {
                // Condition currently true: it has fired and is waiting for the
                // condition to fall before it can fire again.
                "held"
            } else {
                "armed"
            };
            let text = format!(
                "  {:<3} {:<26} {:>7} {:>6}  {}",
                if rule.enabled { "[x]" } else { "[ ]" },
                elide(&rule.name, 26),
                state,
                rule.fired,
                rule.warning.clone().unwrap_or_else(|| rule.summary()),
            );
            let style = if i == app.rules_index {
                theme::selected(active)
            } else if rule.warning.is_some() {
                theme::error()
            } else if !rule.enabled {
                theme::dim()
            } else if rule.holding {
                theme::accent()
            } else {
                theme::normal()
            };
            ListItem::new(Line::from(Span::styled(clip(&text, width), style)))
        })
        .collect();

    let bad = app.rules.rules.iter().filter(|r| r.warning.is_some()).count();
    let title = if bad > 0 {
        format!(
            "rules  {}  ({bad} need{} attention)",
            app.rules.label(),
            if bad == 1 { "s" } else { "" }
        )
    } else {
        format!("rules  {}", app.rules.label())
    };
    let body = with_header(f, area, title, active, &header);
    if app.rules.rules.is_empty() {
        f.render_widget(
            Paragraph::new("  no rules loaded — start with --rules FILE").style(theme::dim()),
            body,
        );
    } else {
        f.render_widget(List::new(items), body);
    }
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
         F3 dbc · F4 iface · F5 cyclic · F6 rules · F2 {mouse} · ? help · q quit "
    );
    let short = format!(
        " tab · / · ⏎ · s · p · x · r · c · F3 · F4 · F5 · F6 · ? · q  [{mouse}] "
    );
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
    ("tab / shift-tab, h l", "move between panes"),
    ("j k, ↑ ↓", "move within a pane"),
    ("5j, 12k", "repeat a motion"),
    ("gg / G / 42G", "top, bottom, that row"),
    ("ctrl-d / ctrl-u", "half page down / up"),
    ("ctrl-f / ctrl-b", "page down / up"),
    ("H / M / L", "top, middle, bottom of the screen"),
    ("/", "filter messages (starts empty each time)"),
    ("enter", "edit signal, or act on the panel row"),
    ("s", "send the selected message once"),
    ("p", "start or stop sending it periodically"),
    ("d", "stop the periodic send under the cursor"),
    ("x", "stop every periodic send"),
    ("r", "raw send, e.g.  4E5 11 22 33"),
    ("c", "clear the receive table"),
    ("F2", "mouse on/off (off restores text selection)"),
    ("F3 / F4", "change DBC / change interface"),
    ("F5 / F6", "cyclic panel / rules panel"),
    ("F7", "reload the rules file"),
    ("q", "quit"),
];

/// Width of the key column: the widest key plus a gap, so a long binding
/// cannot run into its own description.
fn key_column() -> usize {
    KEYS.iter().map(|(key, _)| key.chars().count()).max().unwrap_or(0) + 2
}

/// Sized from the content, so the box has no dead rows and clips nothing.
pub fn help_size() -> (u16, u16) {
    let widest = KEYS.iter().map(|(_, what)| what.chars().count()).max().unwrap_or(0);
    ((key_column() + widest + 4) as u16, KEYS.len() as u16 + 2)
}

pub fn help(f: &mut Frame, area: Rect) {
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(format!(" {key:<width$}", width = key_column()), theme::accent()),
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

