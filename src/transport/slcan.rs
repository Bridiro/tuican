//! Lawicel/slcan ASCII adapters over a serial port (CANable in slcan firmware,
//! USBtin, and most cheap clones).

use std::io::{Read, Write};
use std::time::Duration;

use super::{Frame, Payload, Transport, TransportError};
use crate::canid;

/// slcan only offers this fixed ladder of bitrates.
const RATES: &[(u32, u8)] = &[
    (10_000, b'0'),
    (20_000, b'1'),
    (50_000, b'2'),
    (100_000, b'3'),
    (125_000, b'4'),
    (250_000, b'5'),
    (500_000, b'6'),
    (800_000, b'7'),
    (1_000_000, b'8'),
];

pub struct Slcan {
    port: Box<dyn serialport::SerialPort>,
    name: String,
    bitrate: u32,
    /// Partial line carried between reads; frames end at `\r`.
    pending: Vec<u8>,
}

impl Slcan {
    pub fn open(path: &str, bitrate: u32) -> Result<Self, TransportError> {
        let code = RATES
            .iter()
            .find(|(r, _)| *r == bitrate)
            .map(|(_, c)| *c)
            .ok_or(TransportError::Bitrate(bitrate))?;

        let mut port = serialport::new(path, 115_200)
            .timeout(Duration::from_millis(10))
            .open()
            .map_err(|e| match e.kind {
                serialport::ErrorKind::NoDevice => TransportError::NotFound(path.into()),
                serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => {
                    TransportError::Permission(format!(
                        "no access to {path} (Linux: add yourself to the `dialout` group)"
                    ))
                }
                _ => TransportError::Other(e.description),
            })?;

        // Close first: the adapter may still be open from a previous run, and it
        // rejects a bitrate change while it is on the bus.
        for cmd in [&b"C\r"[..], &[b'S', code, b'\r'], b"O\r"] {
            port.write_all(cmd).map_err(io_err)?;
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = port.clear(serialport::ClearBuffer::Input);

        Ok(Self { port, name: path.to_string(), bitrate, pending: Vec::new() })
    }

    /// Pull one complete `\r`-terminated line out of the buffer.
    fn take_line(&mut self) -> Option<Vec<u8>> {
        let end = self.pending.iter().position(|&b| b == b'\r')?;
        let line: Vec<u8> = self.pending.drain(..=end).take(end).collect();
        Some(line)
    }
}

impl Transport for Slcan {
    fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        if frame.data.len() > 8 {
            return Err(TransportError::Other("slcan carries at most 8 bytes".into()));
        }
        let mut cmd = String::with_capacity(24);
        if canid::is_extended(frame.id) {
            cmd.push('T');
            cmd.push_str(&format!("{:08X}", canid::raw(frame.id)));
        } else {
            cmd.push('t');
            cmd.push_str(&format!("{:03X}", canid::raw(frame.id)));
        }
        cmd.push_str(&format!("{}", frame.data.len()));
        for b in frame.data.iter() {
            cmd.push_str(&format!("{b:02X}"));
        }
        cmd.push('\r');
        self.port.write_all(cmd.as_bytes()).map_err(io_err)
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, TransportError> {
        if let Some(line) = self.take_line() {
            return Ok(parse_line(&line));
        }
        self.port.set_timeout(timeout.max(Duration::from_millis(1))).ok();
        let mut buf = [0u8; 256];
        match self.port.read(&mut buf) {
            Ok(0) => Ok(None),
            Ok(n) => {
                self.pending.extend_from_slice(&buf[..n]);
                Ok(self.take_line().and_then(|l| parse_line(&l)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                Err(TransportError::Disconnected)
            }
            Err(e) => Err(io_err(e)),
        }
    }

    fn describe(&self) -> String {
        format!("slcan {} @ {}", self.name, crate::fmt_bitrate(self.bitrate))
    }
}

impl Drop for Slcan {
    fn drop(&mut self) {
        let _ = self.port.write_all(b"C\r");
    }
}

/// `t1238AABBCC…` — kind, id, dlc, then the payload as hex.
fn parse_line(line: &[u8]) -> Option<Frame> {
    let text = std::str::from_utf8(line).ok()?;
    let (kind, rest) = text.split_at_checked(1)?;
    let (id_len, extended, remote) = match kind {
        "t" => (3, false, false),
        "T" => (8, true, false),
        "r" => (3, false, true),
        "R" => (8, true, true),
        _ => return None, // status replies, bell, version strings
    };
    let (id_hex, rest) = rest.split_at_checked(id_len)?;
    let id = canid::make(u32::from_str_radix(id_hex, 16).ok()?, extended)?;
    let (dlc, rest) = rest.split_at_checked(1)?;
    let dlc = dlc.parse::<usize>().ok()?.min(8);

    let mut data = [0u8; 8];
    if !remote {
        let hex = rest.as_bytes();
        for (i, byte) in data.iter_mut().enumerate().take(dlc) {
            let pair = hex.get(i * 2..i * 2 + 2)?;
            *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
    }
    Some(Frame {
        id,
        data: Payload::new(if remote { &[] } else { &data[..dlc] }),
        echo: false,
    })
}

fn io_err(e: std::io::Error) -> TransportError {
    TransportError::Other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_standard_data_frame() {
        let f = parse_line(b"t1233112233").unwrap();
        assert_eq!(canid::raw(f.id), 0x123);
        assert_eq!(&*f.data, &[0x11, 0x22, 0x33]);
    }

    #[test]
    fn parses_an_extended_frame() {
        let f = parse_line(b"T18FF12342AABB").unwrap();
        assert_eq!(canid::raw(f.id), 0x18FF_1234);
        assert!(canid::is_extended(f.id));
        assert_eq!(&*f.data, &[0xAA, 0xBB]);
    }

    #[test]
    fn ignores_status_chatter() {
        assert!(parse_line(b"V1013").is_none());
        assert!(parse_line(b"").is_none());
    }

    #[test]
    fn a_truncated_line_does_not_panic() {
        assert!(parse_line(b"t123").is_none());
        assert!(parse_line(b"t1238AA").is_none());
    }
}
