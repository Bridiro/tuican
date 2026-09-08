//! Looking up messages and signals, and turning them into frames.

use std::collections::HashMap;
use std::sync::Arc;

use super::{App, Scroll};
use crate::dbc::{self, MessageDef, codec, mux};
use crate::transport::Payload;

impl App {
    pub fn visible_messages(&self) -> Vec<&MessageDef> {
        if self.filter.is_empty() {
            return self.db.messages.iter().collect();
        }
        let needle = self.filter.to_lowercase();
        self.db
            .messages
            .iter()
            .filter(|m| {
                m.name.to_lowercase().contains(&needle)
                    || crate::canid::display(m.id).to_lowercase().contains(&needle)
            })
            .collect()
    }

    pub fn current_message(&self) -> Option<&MessageDef> {
        let shown = self.visible_messages();
        shown
            .get(self.msg_index.min(shown.len().saturating_sub(1)))
            .copied()
    }

    /// Values for a message, seeded on first use with something that encodes.
    pub(super) fn values_for(&mut self, name: &str) -> &mut HashMap<String, f64> {
        if !self.values.contains_key(name) {
            let seed = self
                .db
                .messages
                .iter()
                .find(|m| m.name == name)
                .map(mux::initial_values)
                .unwrap_or_default();
            self.values.insert(name.to_string(), seed);
        }
        self.values.get_mut(name).expect("just inserted")
    }

    pub fn values_of(&self, name: &str) -> HashMap<String, f64> {
        self.values.get(name).cloned().unwrap_or_else(|| {
            self.db
                .messages
                .iter()
                .find(|m| m.name == name)
                .map(mux::initial_values)
                .unwrap_or_default()
        })
    }

    pub(super) fn encode_current(&mut self) -> Option<(String, embedded_can::Id, Payload)> {
        let name = self.current_message()?.name.clone();
        let (id, data) = self.encode_named(&name)?;
        Some((name, id, data))
    }

    /// Encode any message by name. Rules need this: they act on messages the
    /// cursor is nowhere near.
    pub(super) fn encode_named(&mut self, name: &str) -> Option<(embedded_can::Id, Payload)> {
        let msg = self.db.messages.iter().find(|m| m.name == name)?.clone();
        let values = self.values_of(name);
        match codec::encode(&msg, &values) {
            Ok(data) => Some((msg.id, Payload::new(&data))),
            Err(e) => {
                self.status = format!("cannot encode {name}: {e}");
                None
            }
        }
    }

    /// Swap the loaded file. On failure the old database stays live, so a typo
    /// cannot leave you with nothing loaded.
    pub fn load_dbc(&mut self, path: &std::path::Path) {
        match dbc::load::load(path) {
            Ok(db) => {
                let count = db.messages.len();
                self.db = Arc::new(db);
                self.values.clear(); // signal edits belonged to the old file
                self.rx.redecode(&self.db); // keep the counters, redo the names
                self.msg_index = 0;
                self.sig_index = 0;
                self.msg_scroll = Scroll {
                    offset: 0,
                    follow: true,
                };
                self.sig_scroll = Scroll {
                    offset: 0,
                    follow: true,
                };
                self.config.remember_dbc(path);
                self.status = format!("loaded {}, {count} messages", self.db.label());
                // Rule names were validated against the old database.
                if let Some(rules) = self.rules.path.clone() {
                    self.load_rules(&rules);
                    self.status = format!("loaded {}, {count} messages", self.db.label());
                }
            }
            Err(e) => {
                self.status = format!("{e:#}");
                self.last_error = Some(format!("{e:#}"));
            }
        }
    }
}
