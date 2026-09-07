//! Bus → UI. Note there is no "decoded" anything here: the bus never sees a DBC.

use std::time::Instant;

use crate::transport::{Frame, TransportSpec};

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub tx: u64,
    pub tx_errors: u64,
    /// Frames the adapter handed back to us as our own. Counted, not shown as traffic.
    pub tx_echo: u64,
    pub rx: u64,
    /// Message name → frames sent, for the cyclic strip.
    pub cyclic: Vec<(String, u64)>,
}

#[derive(Clone, Debug)]
pub enum Event {
    Rx(Frame, Instant),
    Connected {
        spec: TransportSpec,
        who: String,
        /// True when this is the link coming back by itself, not a fresh
        /// connect. The periodic sends were kept and are already running again.
        resumed: bool,
    },
    ConnectFailed(String),
    Disconnected {
        /// True while the bus is retrying on its own, so the UI can say so and
        /// keep showing the periodic sends that will resume.
        retrying: bool,
    },
    /// A retry attempt that did not succeed.
    Reconnecting {
        attempt: u32,
    },
    BusError(String),
    Stats(Stats),
}
