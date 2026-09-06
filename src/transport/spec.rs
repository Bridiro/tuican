//! A serialisable description of "how to connect", so the picker, the config
//! file and the CLI all speak the same language.

use serde::{Deserialize, Serialize};

use super::{Transport, TransportError};

/// Offered in the picker; the first is the default for a fresh install.
pub const COMMON_BITRATES: &[u32] = &[1_000_000, 500_000, 250_000, 125_000];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransportSpec {
    /// candleLight and friends. `bus`/`address` locate the device; `vid`/`pid`
    /// are kept only so the label survives a replug onto a different address.
    GsUsb { bus: u8, address: u8, vid: u16, pid: u16, bitrate: u32 },
    #[cfg(target_os = "linux")]
    SocketCan { iface: String },
    Slcan { port: String, bitrate: u32 },
    Virtual,
}

impl TransportSpec {
    /// Whether the picker should ask for a bitrate before connecting.
    pub fn needs_bitrate(&self) -> bool {
        match self {
            Self::GsUsb { .. } | Self::Slcan { .. } => true,
            #[cfg(target_os = "linux")]
            Self::SocketCan { .. } => false, // set with `ip link`, not by us
            Self::Virtual => false,
        }
    }

    pub fn with_bitrate(mut self, rate: u32) -> Self {
        match &mut self {
            Self::GsUsb { bitrate, .. } | Self::Slcan { bitrate, .. } => *bitrate = rate,
            _ => {}
        }
        self
    }

    pub fn bitrate(&self) -> Option<u32> {
        match self {
            Self::GsUsb { bitrate, .. } | Self::Slcan { bitrate, .. } => Some(*bitrate),
            _ => None,
        }
    }

    /// Identity ignoring the bitrate, for "is this the one I used last time?".
    pub fn same_device(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::GsUsb { bus: a, address: b, .. }, Self::GsUsb { bus: c, address: d, .. }) => {
                a == c && b == d
            }
            #[cfg(target_os = "linux")]
            (Self::SocketCan { iface: a }, Self::SocketCan { iface: b }) => a == b,
            (Self::Slcan { port: a, .. }, Self::Slcan { port: b, .. }) => a == b,
            (Self::Virtual, Self::Virtual) => true,
            _ => false,
        }
    }
}

/// Build the one live transport. The caller drops the previous one first.
pub fn open(spec: &TransportSpec) -> Result<Box<dyn Transport>, TransportError> {
    Ok(match spec {
        TransportSpec::GsUsb { bus, address, vid, pid, bitrate } => {
            Box::new(super::gs_usb::GsUsb::open(*bus, *address, *vid, *pid, *bitrate)?)
        }
        #[cfg(target_os = "linux")]
        TransportSpec::SocketCan { iface } => Box::new(super::socketcan::SocketCan::open(iface)?),
        TransportSpec::Slcan { port, bitrate } => {
            Box::new(super::slcan::Slcan::open(port, *bitrate)?)
        }
        TransportSpec::Virtual => Box::new(super::virt::Loopback::new()),
    })
}
