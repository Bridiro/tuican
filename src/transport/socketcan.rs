//! Native SocketCAN. Linux only: this module is `cfg`'d out everywhere else,
//! which is why the trait exists in the first place.
//!
//! The bitrate is not ours to set here. A `can0` interface is configured with
//! `ip link set can0 up type can bitrate 500000` before the program runs, so we
//! report what the link is actually using rather than pretend to control it.

use std::time::Duration;

// `EmbeddedFrame` is `embedded_can::Frame`. socketcan aliases it internally but
// does not re-export it, so take it from the crate that defines it.
use embedded_can::Frame as EmbeddedFrame;
use socketcan::{CanFrame, CanSocket, Socket};

use super::{Frame, Payload, Transport, TransportError};

pub struct SocketCan {
    socket: CanSocket,
    iface: String,
}

impl SocketCan {
    pub fn open(iface: &str) -> Result<Self, TransportError> {
        let socket = CanSocket::open(iface).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => TransportError::NotFound(format!(
                "{iface} does not exist (bring it up: \
                 sudo ip link set {iface} up type can bitrate 500000)"
            )),
            std::io::ErrorKind::PermissionDenied => {
                TransportError::Permission(format!("no permission to open {iface}"))
            }
            _ => TransportError::Other(e.to_string()),
        })?;
        socket
            .set_read_timeout(Duration::from_millis(5))
            .map_err(|e| TransportError::Other(e.to_string()))?;
        Ok(Self {
            socket,
            iface: iface.to_string(),
        })
    }
}

impl Transport for SocketCan {
    fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        let out = CanFrame::new(frame.id, &frame.data)
            .ok_or_else(|| TransportError::Other("frame too long for classic CAN".into()))?;
        self.socket.write_frame(&out).map_err(|e| match e.kind() {
            std::io::ErrorKind::NetworkDown => TransportError::Disconnected,
            _ => TransportError::Other(e.to_string()),
        })
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, TransportError> {
        self.socket
            .set_read_timeout(timeout.max(Duration::from_millis(1)))
            .ok();
        match self.socket.read_frame() {
            Ok(CanFrame::Data(f)) => Ok(Some(Frame {
                id: f.id(),
                data: Payload::new(f.data()),
                echo: false,
            })),
            // Remote frames carry no payload; error frames are bus state, not traffic.
            Ok(CanFrame::Remote(f)) => Ok(Some(Frame {
                id: f.id(),
                data: Payload::new(&[]),
                echo: false,
            })),
            // `into_error` decodes the error bits into something readable;
            // the raw frame's Debug output is not worth showing a user.
            Ok(CanFrame::Error(e)) => Err(TransportError::Other(format!(
                "bus error: {}",
                e.into_error()
            ))),
            Err(e) => match e.kind() {
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Ok(None),
                std::io::ErrorKind::NetworkDown => Err(TransportError::Disconnected),
                _ => Err(TransportError::Other(e.to_string())),
            },
        }
    }

    fn describe(&self) -> String {
        match bitrate_of(&self.iface) {
            Some(b) => format!("socketcan {} @ {}", self.iface, crate::fmt_bitrate(b)),
            None => format!("socketcan {}", self.iface),
        }
    }
}

/// Read the configured bitrate out of sysfs rather than guessing at it.
pub fn bitrate_of(iface: &str) -> Option<u32> {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/can_bittiming/bitrate"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// `up`, `down`, or whatever the kernel reports.
pub fn state_of(iface: &str) -> String {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/operstate"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}
