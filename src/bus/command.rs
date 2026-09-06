//! UI → bus. Everything the user can ask the adapter to do.

use std::time::Duration;

use embedded_can::Id;

use crate::transport::{Payload, TransportSpec};

#[derive(Clone, Debug)]
pub enum Command {
    /// Switch interfaces. Implies disconnecting whatever is live.
    Connect(TransportSpec),
    Send {
        id: Id,
        data: Payload,
    },
    /// `key` is the message name, so re-arming the same message replaces it
    /// rather than stacking a second sender.
    SetCyclic {
        key: String,
        id: Id,
        data: Payload,
        period: Duration,
    },
    ClearCyclic(String),
    ClearAllCyclic,
    Shutdown,
}
