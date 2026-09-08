//! The database model, plus everything that turns frames into named values.
//!
//! Nothing here does I/O or reads a clock, so it is all directly testable.
//! `can-dbc` types are confined to [`load`] and the rest of the program sees
//! only the types defined here, leaving room for a second parser later.

pub mod codec;
pub mod load;
pub mod mux;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use embedded_can::Id;

use crate::canid;

/// How a signal relates to its message's multiplexor, if at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mux {
    /// Always present.
    #[default]
    Plain,
    /// This signal *is* the multiplexor (`M` in the DBC).
    Selector,
    /// Present only when the multiplexor holds one of these values (`m3`).
    SelectedBy(Vec<u64>),
}

#[derive(Clone, Debug)]
pub struct SignalDef {
    pub name: String,
    /// DBC `start_bit`: LSB position for little-endian, MSB position for big-endian.
    pub start: u16,
    pub len: u16,
    pub big_endian: bool,
    pub signed: bool,
    pub factor: f64,
    pub offset: f64,
    /// Declared limits. Frequently wrong or unset in real files; see
    /// [`mux::effective_range`].
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub unit: String,
    /// `VAL_` table, keyed by *raw* value.
    pub choices: BTreeMap<i64, String>,
    pub mux: Mux,
}

impl SignalDef {
    pub fn choice(&self, raw: i64) -> Option<&str> {
        self.choices.get(&raw).map(String::as_str)
    }
}

#[derive(Clone, Debug)]
pub struct MessageDef {
    pub id: Id,
    pub name: String,
    pub len: usize,
    /// `GenMsgCycleTime`, when the file carries it. Used as the default send period.
    pub cycle_time_ms: Option<u32>,
    pub signals: Vec<SignalDef>,
}

impl MessageDef {
    pub fn is_multiplexed(&self) -> bool {
        self.signals.iter().any(|s| s.mux == Mux::Selector)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Database {
    pub path: PathBuf,
    /// Sorted by name, because that is the order the message pane shows.
    pub messages: Vec<MessageDef>,
    by_id: HashMap<u32, usize>,
}

impl Database {
    pub fn new(path: PathBuf, mut messages: Vec<MessageDef>) -> Self {
        messages.sort_by_key(|m| m.name.to_lowercase());
        let by_id = messages
            .iter()
            .enumerate()
            .map(|(i, m)| (canid::key(m.id), i))
            .collect();
        Self {
            path,
            messages,
            by_id,
        }
    }

    pub fn by_id(&self, id: Id) -> Option<&MessageDef> {
        self.by_id.get(&canid::key(id)).map(|&i| &self.messages[i])
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// What the header shows: the file name alone, not the whole path.
    pub fn label(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(no dbc)".into())
    }
}
