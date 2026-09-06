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
    Connected { spec: TransportSpec, who: String },
    ConnectFailed(String),
    Disconnected,
    BusError(String),
    Stats(Stats),
}
