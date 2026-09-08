//! Adapters. Everything here turns bytes on a wire into [`Frame`]s and back.
//! Nothing here knows what a DBC is.

pub mod discover;
pub mod gs_usb;
pub mod slcan;
pub mod spec;
pub mod virt;

#[cfg(target_os = "linux")]
pub mod socketcan;

use std::fmt;
use std::ops::Deref;
use std::time::Duration;

use embedded_can::Id;

pub use spec::TransportSpec;

/// A CAN payload, inline. 64 bytes so classic and FD frames share one type and
/// the receive path allocates nothing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Payload {
    bytes: [u8; 64],
    len: u8,
}

impl Payload {
    pub const MAX: usize = 64;

    pub fn new(src: &[u8]) -> Self {
        let len = src.len().min(Self::MAX);
        let mut bytes = [0u8; Self::MAX];
        bytes[..len].copy_from_slice(&src[..len]);
        Self {
            bytes,
            len: len as u8,
        }
    }

    pub fn hex(&self) -> String {
        self.iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Deref for Payload {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

impl fmt::Debug for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.hex())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub id: Id,
    pub data: Payload,
    /// The adapter handing us back a frame *we* transmitted.
    ///
    /// gs_usb does this for every send. Without the distinction, everything you
    /// transmit shows up in the receive table as though the bus had answered.
    pub echo: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("device not found: {0}")]
    NotFound(String),
    /// Carries the remedy, not just the errno. See `gs_usb::PERMISSION_HINT`.
    #[error("{0}")]
    Permission(String),
    #[error("this adapter cannot do {0} bit/s")]
    Bitrate(u32),
    #[error("link is down")]
    Disconnected,
    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for TransportError {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(format!("{e:#}"))
    }
}

/// One adapter, driven exclusively by the bus thread.
///
/// `Send` but deliberately not `Sync` or `Clone`. The bus thread owns the only
/// handle, so "exactly one interface is live" holds by construction rather than
/// by a runtime check.
pub trait Transport: Send {
    fn send(&mut self, frame: &Frame) -> Result<(), TransportError>;

    /// `Ok(None)` on timeout. That is the normal quiet-bus case, not an error.
    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, TransportError>;

    /// One line for the header bar, e.g. `candleLight 1d50:606f @ 1 Mbit/s`.
    fn describe(&self) -> String;
}
