//! What the receive pane shows. Decoding happens here, on the UI side, which is
//! what makes swapping the DBC at runtime a pointer swap instead of a reconnect.

use std::collections::BTreeMap;
use std::time::Instant;

use embedded_can::Id;

use crate::canid;
use crate::dbc::{Database, codec};
use crate::transport::{Frame, Payload};

pub struct RxRow {
    pub id: Id,
    /// The DBC name, or the hex id when nothing matches.
    pub name: String,
    pub known: bool,
    pub count: u64,
    pub last_seen: Instant,
    /// Smoothed inter-arrival time; 0 until the second frame arrives.
    pub period_ms: f64,
    pub data: Payload,
    pub decoded: String,
}

impl RxRow {
    /// A node that has stopped talking, judged against its own observed rate.
    pub fn stale(&self, now: Instant) -> bool {
        self.period_ms > 0.0
            && now.duration_since(self.last_seen).as_secs_f64() * 1000.0 > self.period_ms * 3.0
    }
}

/// Keyed so that standard `0x123` and extended `0x123` stay distinct, and so
/// iteration is in id order without a sort on every frame.
#[derive(Default)]
pub struct RxTable {
    rows: BTreeMap<u32, RxRow>,
}

impl RxTable {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &RxRow> {
        self.rows.values()
    }

    pub fn clear(&mut self) {
        self.rows.clear();
    }

    pub fn record(&mut self, frame: &Frame, at: Instant, db: &Database) {
        let (name, known, decoded) = describe(frame, db);
        match self.rows.get_mut(&canid::key(frame.id)) {
            Some(row) => {
                let delta = at.duration_since(row.last_seen).as_secs_f64() * 1000.0;
                // Light smoothing, so the number does not jitter on every frame.
                row.period_ms = if row.period_ms == 0.0 {
                    delta
                } else {
                    row.period_ms * 0.8 + delta * 0.2
                };
                row.last_seen = at;
                row.count += 1;
                row.data = frame.data;
                row.decoded = decoded;
                row.name = name;
                row.known = known;
            }
            None => {
                self.rows.insert(
                    canid::key(frame.id),
                    RxRow {
                        id: frame.id,
                        name,
                        known,
                        count: 1,
                        last_seen: at,
                        period_ms: 0.0,
                        data: frame.data,
                        decoded,
                    },
                );
            }
        }
    }

    /// Re-run every row against a newly loaded database, keeping the counters.
    /// Losing the rate history on a DBC swap would be a needless annoyance.
    pub fn redecode(&mut self, db: &Database) {
        for row in self.rows.values_mut() {
            let frame = Frame { id: row.id, data: row.data, echo: false };
            let (name, known, decoded) = describe(&frame, db);
            row.name = name;
            row.known = known;
            row.decoded = decoded;
        }
    }
}

fn describe(frame: &Frame, db: &Database) -> (String, bool, String) {
    match db.by_id(frame.id) {
        Some(msg) => {
            let decoded = codec::decode_message(msg, &frame.data)
                .iter()
                .map(|d| format!("{}={}", d.name, d.text))
                .collect::<Vec<_>>()
                .join("  ");
            (msg.name.clone(), true, decoded)
        }
        // An unknown id is still traffic worth seeing; show the bytes.
        None => (canid::display(frame.id), false, frame.data.hex()),
    }
}
