//! The only module that knows `can-dbc` exists.
//!
//! Everything crosses into our own model here, so a parser upgrade, or a
//! second format later, is a change to this file and nothing else.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use can_dbc::{
    AttributeValue, ByteOrder, Dbc, MessageId, MultiplexIndicator, NumericValue, ValueDescription,
    ValueType,
};

use super::{Database, MessageDef, Mux, SignalDef};
use crate::canid;

/// Vector's conventional attribute for a message's send period.
const CYCLE_TIME_ATTR: &str = "GenMsgCycleTime";

pub fn load(path: &Path) -> anyhow::Result<Database> {
    let text = read_lossy(path)?;
    let dbc = Dbc::try_from(text.as_str())
        .map_err(|e| anyhow!("{}: not a valid DBC ({e})", path.display()))?;

    let messages = dbc
        .messages
        .iter()
        .filter_map(|m| {
            // The independent-signals pseudo-message carries no real frame.
            let id = message_id(m.id)?;
            Some(MessageDef {
                id,
                name: m.name.clone(),
                len: m.size as usize,
                cycle_time_ms: cycle_time(&dbc, m.id),
                signals: m
                    .signals
                    .iter()
                    .map(|s| SignalDef {
                        name: s.name.clone(),
                        start: s.start_bit as u16,
                        len: s.size as u16,
                        big_endian: matches!(s.byte_order, ByteOrder::BigEndian),
                        signed: matches!(s.value_type, ValueType::Signed),
                        // A zero factor would make every encode a division by zero.
                        factor: if s.factor == 0.0 { 1.0 } else { s.factor },
                        offset: s.offset,
                        min: numeric(&s.min),
                        max: numeric(&s.max),
                        unit: s.unit.clone(),
                        choices: choices(&dbc, m.id, &s.name),
                        mux: match &s.multiplexer_indicator {
                            MultiplexIndicator::Plain => Mux::Plain,
                            MultiplexIndicator::Multiplexor => Mux::Selector,
                            MultiplexIndicator::MultiplexedSignal(v)
                            | MultiplexIndicator::MultiplexorAndMultiplexedSignal(v) => {
                                Mux::SelectedBy(vec![*v])
                            }
                        },
                    })
                    .collect(),
            })
        })
        .collect::<Vec<_>>();

    if messages.is_empty() {
        return Err(anyhow!(
            "{}: parsed, but defines no messages",
            path.display()
        ));
    }
    Ok(Database::new(path.to_path_buf(), messages))
}

/// DBC files are commonly Latin-1 (degree signs in units, names with umlauts),
/// which is not valid UTF-8. Refusing to open one over a unit string would be
/// a poor trade, so fall back to a byte-wise Latin-1 decode.
fn read_lossy(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

fn message_id(id: MessageId) -> Option<embedded_can::Id> {
    match id {
        MessageId::Standard(v) => canid::make(v as u32, false),
        MessageId::Extended(v) => canid::make(v, true),
    }
}

fn numeric(v: &NumericValue) -> Option<f64> {
    Some(match v {
        NumericValue::Uint(u) => *u as f64,
        NumericValue::Int(i) => *i as f64,
        NumericValue::Double(d) => *d,
    })
}

fn choices(dbc: &Dbc, id: MessageId, signal: &str) -> BTreeMap<i64, String> {
    dbc.value_descriptions
        .iter()
        .find_map(|v| match v {
            ValueDescription::Signal {
                message_id,
                name,
                value_descriptions,
            } if *message_id == id && name == signal => Some(
                value_descriptions
                    .iter()
                    .map(|d| (d.id, d.description.clone()))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn cycle_time(dbc: &Dbc, id: MessageId) -> Option<u32> {
    dbc.attribute_values_message.iter().find_map(|a| {
        (a.name == CYCLE_TIME_ATTR && a.message_id == id)
            .then(|| match &a.value {
                AttributeValue::Uint(v) => Some(*v as u32),
                AttributeValue::Int(v) if *v > 0 => Some(*v as u32),
                AttributeValue::Double(v) if *v > 0.0 => Some(*v as u32),
                AttributeValue::String(s) => s.parse().ok(),
                _ => None,
            })
            .flatten()
            .filter(|&ms| ms > 0)
    })
}

/// Files offered in the DBC picker: every `*.dbc` under `root`, a few levels
/// deep. Three levels because the common layout is `dbc/<network>/<name>.dbc`,
/// which a one-level scan misses entirely.
pub fn discover(root: &Path) -> Vec<PathBuf> {
    const MAX_DEPTH: usize = 3;
    const MAX_RESULTS: usize = 200;
    /// Directories that never hold a hand-written DBC but can hold thousands
    /// of files.
    const SKIP: &[&str] = &[
        "target",
        "build",
        "dist",
        "node_modules",
        "venv",
        ".venv",
        "__pycache__",
    ];

    fn is_dbc(p: &Path) -> bool {
        p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dbc"))
    }

    let mut found = Vec::new();
    let mut frontier = vec![root.to_path_buf()];
    for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for dir in frontier {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if path.is_dir() {
                    if !name.starts_with('.') && !SKIP.contains(&name.as_ref()) {
                        next.push(path);
                    }
                } else if is_dbc(&path) && found.len() < MAX_RESULTS {
                    found.push(path);
                }
            }
        }
        if found.len() >= MAX_RESULTS {
            break;
        }
        frontier = next;
    }
    found.sort();
    found.dedup();
    found
}
