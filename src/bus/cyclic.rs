//! The periodic-send table.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use embedded_can::Id;

use crate::transport::Payload;

struct Entry {
    id: Id,
    data: Payload,
    period: Duration,
    next: Instant,
    count: u64,
}

#[derive(Default)]
pub struct Cyclic {
    entries: BTreeMap<String, Entry>,
}

impl Cyclic {
    pub fn set(&mut self, key: String, id: Id, data: Payload, period: Duration) {
        let count = self.entries.get(&key).map_or(0, |e| e.count);
        self.entries.insert(
            key,
            Entry {
                id,
                data,
                period,
                next: Instant::now(),
                count,
            },
        );
    }

    pub fn remove(&mut self, key: &str) {
        self.entries.remove(key);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Restart every schedule from `now`. Used when a dropped link comes back:
    /// the stale deadlines would otherwise all fire at once.
    pub fn rearm(&mut self, now: Instant) {
        for entry in self.entries.values_mut() {
            entry.next = now;
        }
    }

    pub fn next_due(&self) -> Option<Instant> {
        self.entries.values().map(|e| e.next).min()
    }

    /// Everything due at `now`, rescheduled from `now` rather than from its
    /// previous deadline: after a stall we want the next frame one period away,
    /// not a burst catching up on missed ones.
    pub fn take_due(&mut self, now: Instant) -> Vec<(Id, Payload)> {
        let mut due = Vec::new();
        for entry in self.entries.values_mut() {
            if entry.next <= now {
                entry.next = now + entry.period;
                entry.count += 1;
                due.push((entry.id, entry.data));
            }
        }
        due
    }

    pub fn counts(&self) -> Vec<(String, u64)> {
        self.entries
            .iter()
            .map(|(k, e)| (k.clone(), e.count))
            .collect()
    }
}
