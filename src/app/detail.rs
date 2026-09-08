//! The overlay that shows one row in full.
//!
//! Panes are single-line and column-aligned, so a decoded message or a rule
//! wider than the terminal gets cut at the right edge. This is where the whole
//! thing can be read, wrapped over as many lines as it needs.

use std::time::Instant;

use super::action::{Action, Pane};
use super::{App, Mode, trim};
use crate::canid;
use crate::dbc::{codec, mux};

pub enum DetailRow {
    /// The subject of the overlay, shown once at the top.
    Heading(String),
    /// A label and its value, wrapped with a hanging indent.
    Pair(String, String),
    /// A full-width line, wrapped.
    Text(String),
    Blank,
}

pub struct Detail {
    pub title: String,
    pub rows: Vec<DetailRow>,
    pub scroll: usize,
}

impl Detail {
    fn new(title: impl Into<String>, rows: Vec<DetailRow>) -> Self {
        Self {
            title: title.into(),
            rows,
            scroll: 0,
        }
    }
}

impl App {
    /// Open the overlay on whatever the focused pane has under its cursor.
    pub(super) fn inspect(&mut self) {
        let detail = match self.pane {
            Pane::Messages | Pane::Signals => self.message_detail(),
            Pane::Rx => self.rx_detail(),
            Pane::Cyclic => self.cyclic_detail(),
            Pane::Rules => self.rule_detail(),
        };
        match detail {
            Some(d) => self.mode = Mode::Detail(d),
            None => self.status = "nothing under the cursor to show".into(),
        }
    }

    fn rx_detail(&self) -> Option<Detail> {
        let row = self.rx.row(self.rx_index)?;
        let mut rows = vec![
            DetailRow::Heading(format!("{}  {}", row.name, canid::display(row.id))),
            DetailRow::Blank,
        ];
        let rate = if row.period_ms > 0.0 {
            format!("{} frames, every {:.1} ms", row.count, row.period_ms)
        } else {
            format!("{} frame", row.count)
        };
        rows.push(DetailRow::Pair("received".into(), rate));
        let ago = Instant::now().duration_since(row.last_seen).as_secs_f64();
        rows.push(DetailRow::Pair(
            "last seen".into(),
            format!("{ago:.2} s ago"),
        ));
        rows.push(DetailRow::Pair(
            "data".into(),
            format!("{} · {} bytes", row.data.hex(), row.data.len()),
        ));
        rows.push(DetailRow::Blank);

        match self.db.by_id(row.id) {
            Some(msg) => {
                let decoded = codec::decode_message(msg, &row.data);
                if decoded.is_empty() {
                    rows.push(DetailRow::Text("no signals in this multiplex".into()));
                }
                for d in decoded {
                    rows.push(DetailRow::Pair(d.name, d.text));
                }
            }
            None => rows.push(DetailRow::Text(
                "this id is not in the loaded DBC, so only the raw bytes are known".into(),
            )),
        }
        Some(Detail::new("received message", rows))
    }

    fn cyclic_detail(&self) -> Option<Detail> {
        let name = self.selected_cyclic()?;
        let entry = self.cyclic.get(&name)?;
        let sent = self
            .stats
            .cyclic
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, c)| *c)
            .unwrap_or(0);

        let mut rows = vec![
            DetailRow::Heading(format!("{name}  {}", canid::display(entry.id))),
            DetailRow::Blank,
            DetailRow::Pair(
                "period".into(),
                format!("{} ms", trim(entry.period.as_secs_f64() * 1000.0)),
            ),
            DetailRow::Pair("sent".into(), sent.to_string()),
            DetailRow::Pair(
                "data".into(),
                format!("{} · {} bytes", entry.data.hex(), entry.data.len()),
            ),
            DetailRow::Blank,
        ];
        // Decode the bytes actually going out, rather than the values we think
        // we set: if the two ever disagree, this is where it shows.
        if let Some(msg) = self.db.by_id(entry.id) {
            for d in codec::decode_message(msg, &entry.data) {
                rows.push(DetailRow::Pair(d.name, d.text));
            }
        }
        Some(Detail::new("periodic send", rows))
    }

    fn rule_detail(&self) -> Option<Detail> {
        let rule = self.rules.rules.get(self.rules_index)?;
        let state = if rule.warning.is_some() {
            "needs attention"
        } else if !rule.enabled {
            "disabled"
        } else if rule.holding {
            "condition currently true, waiting for it to fall"
        } else {
            "armed"
        };
        let mut rows = vec![
            DetailRow::Heading(rule.name.clone()),
            DetailRow::Blank,
            DetailRow::Pair("state".into(), state.into()),
            DetailRow::Pair("fired".into(), rule.fired.to_string()),
        ];
        if let Some(warning) = &rule.warning {
            rows.push(DetailRow::Blank);
            rows.push(DetailRow::Text(warning.clone()));
        }
        rows.push(DetailRow::Blank);
        rows.push(DetailRow::Text("when".into()));
        for c in &rule.all {
            rows.push(DetailRow::Text(format!("  {}", c.describe())));
        }
        if !rule.any.is_empty() {
            rows.push(DetailRow::Text("  and any of".into()));
            for c in &rule.any {
                rows.push(DetailRow::Text(format!("    {}", c.describe())));
            }
        }
        rows.push(DetailRow::Blank);
        rows.push(DetailRow::Text("then".into()));
        for a in &rule.then {
            rows.push(DetailRow::Text(format!("  {}", a.describe())));
        }
        Some(Detail::new("rule", rows))
    }

    fn message_detail(&self) -> Option<Detail> {
        let msg = self.current_message()?;
        let values = self.values_of(&msg.name);
        let active: Vec<&str> = mux::active_signals(msg, &values)
            .iter()
            .map(|s| s.name.as_str())
            .collect();

        let mut rows = vec![
            DetailRow::Heading(format!("{}  {}", msg.name, canid::display(msg.id))),
            DetailRow::Blank,
            DetailRow::Pair("length".into(), format!("{} bytes", msg.len)),
        ];
        if let Some(ms) = msg.cycle_time_ms {
            rows.push(DetailRow::Pair("cycle time".into(), format!("{ms} ms")));
        }
        rows.push(DetailRow::Blank);

        for sig in &msg.signals {
            let value = values.get(&sig.name).copied().unwrap_or(0.0);
            let raw = ((value - sig.offset) / sig.factor).round() as i64;
            let (lo, hi) = mux::effective_range(sig);
            // Wrapping collapses runs of spaces, so separate the fields with
            // something that survives it.
            let mut line = format!(
                "{} · [{} .. {}] · bit {}+{}{}",
                codec::format_value(sig, raw, value),
                trim(lo),
                trim(hi),
                sig.start,
                sig.len,
                if sig.big_endian { ", big-endian" } else { "" },
            );
            if !active.contains(&sig.name.as_str()) {
                line.push_str(" · not in this multiplex");
            }
            rows.push(DetailRow::Pair(sig.name.clone(), line));
        }
        Some(Detail::new("message", rows))
    }

    pub(super) fn update_detail(&mut self, action: Action) {
        let Mode::Detail(detail) = &mut self.mode else {
            return;
        };
        match action {
            Action::Move(step) => {
                detail.scroll = detail.scroll.saturating_add_signed(step as isize);
            }
            Action::Page(step) | Action::HalfPage(step) => {
                detail.scroll = detail.scroll.saturating_add_signed(step as isize * 10);
            }
            Action::Home => detail.scroll = 0,
            Action::End => detail.scroll = usize::MAX,
            // Anything else closes it, so it never traps the keyboard.
            _ => self.mode = Mode::Normal,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::config::Config;
    use crate::dbc::{Database, MessageDef, Mux, SignalDef};
    use crate::transport::Frame;

    fn signal(name: &str, start: u16) -> SignalDef {
        SignalDef {
            name: name.into(),
            start,
            len: 8,
            big_endian: false,
            signed: false,
            factor: 1.0,
            offset: 0.0,
            min: None,
            max: None,
            unit: "A".into(),
            choices: BTreeMap::new(),
            mux: Mux::Plain,
        }
    }

    /// The overlay exists so nothing is cut off, so it has to list every
    /// signal, however many there are and however long the line would be.
    #[test]
    fn the_overlay_lists_every_signal_of_a_received_message() {
        let id = canid::make(0x123, false).unwrap();
        let names = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"];
        let msg = MessageDef {
            id,
            name: "Wide".into(),
            len: 6,
            cycle_time_ms: None,
            signals: names
                .iter()
                .enumerate()
                .map(|(i, n)| signal(n, i as u16 * 8))
                .collect(),
        };
        let db = Database::new("test.dbc".into(), vec![msg]);

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new(tx, Config::default());
        app.db = Arc::new(db);
        let frame = Frame {
            id,
            data: crate::transport::Payload::new(&[1, 2, 3, 4, 5, 6]),
            echo: false,
        };
        app.rx.record(&frame, Instant::now(), &app.db);
        app.pane = Pane::Rx;

        app.inspect();

        let Mode::Detail(detail) = &app.mode else {
            panic!("inspect did not open the overlay");
        };
        let labels: Vec<&str> = detail
            .rows
            .iter()
            .filter_map(|r| match r {
                DetailRow::Pair(label, _) => Some(label.as_str()),
                _ => None,
            })
            .collect();
        for name in names {
            assert!(labels.contains(&name), "{name} missing from {labels:?}");
        }
        assert!(labels.contains(&"data"), "raw bytes should be shown too");
    }

    #[test]
    fn any_ordinary_key_closes_the_overlay() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new(tx, Config::default());
        app.mode = Mode::Detail(Detail::new("x", vec![DetailRow::Text("y".into())]));
        app.update_detail(Action::Cancel);
        assert!(matches!(app.mode, Mode::Normal));
    }
}
