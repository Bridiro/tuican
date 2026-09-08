//! Bit-level encode and decode. `can-dbc` parses; none of this comes with it.
//!
//! Extraction walks one bit at a time rather than shifting a `u64`. It is
//! slower, but Motorola's sawtooth bit numbering falls out of the index
//! arithmetic instead of a shift that is easy to get off by one, and nothing
//! assumes an 8-byte payload, so CAN FD works unchanged.

use std::collections::HashMap;

use super::{MessageDef, SignalDef, mux};

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("{0}: no value")]
    Missing(String),
    #[error("{sig} = {value} is outside [{lo}, {hi}]")]
    OutOfRange {
        sig: String,
        value: f64,
        lo: f64,
        hi: f64,
    },
    #[error("{sig} does not fit in {len} bits of a {size}-byte message")]
    Overrun { sig: String, len: u16, size: usize },
}

/// Read `len` bits starting at the DBC `start` bit.
fn extract(data: &[u8], start: u16, len: u16, big_endian: bool) -> u64 {
    let bit_at =
        |byte: usize, bit: u32| -> u64 { data.get(byte).map_or(0, |b| ((b >> bit) & 1) as u64) };
    let mut v = 0u64;
    for i in 0..len as u32 {
        if big_endian {
            // A big-endian start_bit names the signal's MSB, numbered LSB-first
            // within its byte; the signal then walks forward in MSB-first order.
            let m = (start / 8) as u32 * 8 + (7 - (start % 8) as u32) + i;
            v = (v << 1) | bit_at((m / 8) as usize, 7 - (m % 8));
        } else {
            let idx = start as u32 + i;
            v |= bit_at((idx / 8) as usize, idx % 8) << i;
        }
    }
    v
}

/// The inverse of [`extract`]; bits outside `data` are dropped.
fn insert(data: &mut [u8], start: u16, len: u16, big_endian: bool, raw: u64) {
    for i in 0..len as u32 {
        let (byte, bit, src) = if big_endian {
            let m = (start / 8) as u32 * 8 + (7 - (start % 8) as u32) + i;
            (
                (m / 8) as usize,
                7 - (m % 8),
                (raw >> (len as u32 - 1 - i)) & 1,
            )
        } else {
            let idx = start as u32 + i;
            ((idx / 8) as usize, idx % 8, (raw >> i) & 1)
        };
        if let Some(b) = data.get_mut(byte) {
            *b = (*b & !(1 << bit)) | ((src as u8) << bit);
        }
    }
}

/// The highest byte index this signal touches, for bounds checking.
fn last_byte(sig: &SignalDef) -> usize {
    if sig.len == 0 {
        return 0;
    }
    let last = sig.len as u32 - 1;
    if sig.big_endian {
        let m = (sig.start / 8) as u32 * 8 + (7 - (sig.start % 8) as u32) + last;
        (m / 8) as usize
    } else {
        ((sig.start as u32 + last) / 8) as usize
    }
}

/// The raw field value, sign-extended if the signal is signed.
pub fn decode_raw(sig: &SignalDef, data: &[u8]) -> i64 {
    let raw = extract(data, sig.start, sig.len, sig.big_endian);
    if sig.signed && sig.len > 0 && sig.len < 64 && (raw >> (sig.len - 1)) & 1 == 1 {
        (raw | (u64::MAX << sig.len)) as i64
    } else {
        raw as i64
    }
}

pub fn decode_signal(sig: &SignalDef, data: &[u8]) -> f64 {
    decode_raw(sig, data) as f64 * sig.factor + sig.offset
}

#[derive(Clone, Debug)]
pub struct Decoded {
    pub name: String,
    /// The physical value, for anything that needs to compare rather than show.
    pub value: f64,
    /// Enum name where the `VAL_` table has one, else the number and its unit.
    pub text: String,
}

pub fn format_value(sig: &SignalDef, raw: i64, value: f64) -> String {
    if let Some(name) = sig.choice(raw) {
        return format!("{name} ({})", trim(value));
    }
    match sig.unit.as_str() {
        "" | "U" | "enum" => trim(value),
        unit => format!("{} {unit}", trim(value)),
    }
}

/// `%g`-ish: keep it short, drop a trailing `.0`.
fn trim(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Decode every signal the current multiplexor value selects.
pub fn decode_message(msg: &MessageDef, data: &[u8]) -> Vec<Decoded> {
    // Resolve the multiplexor from the frame itself, not from any UI state.
    let mut selected: HashMap<String, f64> = HashMap::new();
    if let Some(sel) = msg.signals.iter().find(|s| s.mux == super::Mux::Selector) {
        selected.insert(sel.name.clone(), decode_signal(sel, data));
    }
    mux::active_signals(msg, &selected)
        .into_iter()
        .map(|sig| {
            let raw = decode_raw(sig, data);
            let value = raw as f64 * sig.factor + sig.offset;
            Decoded {
                name: sig.name.clone(),
                value,
                text: format_value(sig, raw, value),
            }
        })
        .collect()
}

/// Build the payload for a message from physical signal values.
pub fn encode(msg: &MessageDef, values: &HashMap<String, f64>) -> Result<Vec<u8>, CodecError> {
    let mut data = vec![0u8; msg.len];
    for sig in mux::active_signals(msg, values) {
        let value = *values
            .get(&sig.name)
            .ok_or_else(|| CodecError::Missing(sig.name.clone()))?;
        if last_byte(sig) >= msg.len {
            return Err(CodecError::Overrun {
                sig: sig.name.clone(),
                len: sig.len,
                size: msg.len,
            });
        }
        let (lo, hi) = mux::representable_range(sig);
        if value < lo || value > hi {
            return Err(CodecError::OutOfRange {
                sig: sig.name.clone(),
                value,
                lo,
                hi,
            });
        }
        let raw = ((value - sig.offset) / sig.factor).round() as i64;
        insert(&mut data, sig.start, sig.len, sig.big_endian, raw as u64);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn sig(name: &str, start: u16, len: u16, big_endian: bool, signed: bool) -> SignalDef {
        SignalDef {
            name: name.into(),
            start,
            len,
            big_endian,
            signed,
            factor: 1.0,
            offset: 0.0,
            min: None,
            max: None,
            unit: String::new(),
            choices: BTreeMap::new(),
            mux: super::super::Mux::Plain,
        }
    }

    #[test]
    fn little_endian_spans_a_byte_boundary() {
        // 12 bits starting at bit 4 of byte 0: nibble of byte 0, then all of byte 1.
        let s = sig("x", 4, 12, false, false);
        assert_eq!(decode_raw(&s, &[0xF0, 0xAB]), 0xABF);
        let mut buf = [0u8; 2];
        insert(&mut buf, 4, 12, false, 0xABF);
        assert_eq!(buf, [0xF0, 0xAB]);
    }

    #[test]
    fn big_endian_matches_a_hand_computed_vector() {
        // Motorola, start_bit 7 (MSB of byte 0), 16 bits => the first two bytes,
        // most significant first.
        let s = sig("x", 7, 16, true, false);
        assert_eq!(decode_raw(&s, &[0x12, 0x34]), 0x1234);

        // start_bit 3 (bit 3 of byte 0), 8 bits: the low nibble of byte 0 becomes
        // the high nibble of the value, the high nibble of byte 1 the low one.
        let s = sig("y", 3, 8, true, false);
        assert_eq!(decode_raw(&s, &[0x0A, 0xB0]), 0xAB);
    }

    #[test]
    fn signed_values_sign_extend() {
        let s = sig("t", 0, 8, false, true);
        assert_eq!(decode_raw(&s, &[0xFF]), -1);
        assert_eq!(decode_raw(&s, &[0x80]), -128);
        let s = sig("t", 0, 4, false, true);
        assert_eq!(decode_raw(&s, &[0x0F]), -1);
    }

    #[test]
    fn round_trips_through_encode_and_decode() {
        for &big in &[false, true] {
            for &signed in &[false, true] {
                for start in [0u16, 3, 7, 8, 12] {
                    let mut s = sig("v", if big { start.max(7) } else { start }, 10, big, signed);
                    s.factor = 0.5;
                    s.offset = -100.0;
                    let msg = MessageDef {
                        id: crate::canid::make(0x123, false).unwrap(),
                        name: "M".into(),
                        len: 8,
                        cycle_time_ms: None,
                        signals: vec![s.clone()],
                    };
                    let want = -50.5;
                    let data = encode(&msg, &HashMap::from([("v".into(), want)])).unwrap();
                    assert!(
                        (decode_signal(&s, &data) - want).abs() < 1e-9,
                        "big={big} signed={signed} start={start}"
                    );
                }
            }
        }
    }

    #[test]
    fn fd_payloads_beyond_eight_bytes_decode() {
        let s = sig("late", 480, 16, false, false); // byte 60
        let mut data = vec![0u8; 64];
        data[60] = 0xCD;
        data[61] = 0xAB;
        assert_eq!(decode_raw(&s, &data), 0xABCD);
    }

    #[test]
    fn a_signal_past_the_end_is_rejected_not_truncated() {
        let s = sig("wide", 56, 16, false, false);
        let msg = MessageDef {
            id: crate::canid::make(1, false).unwrap(),
            name: "M".into(),
            len: 8,
            cycle_time_ms: None,
            signals: vec![s],
        };
        let err = encode(&msg, &HashMap::from([("wide".into(), 1.0)])).unwrap_err();
        assert!(matches!(err, CodecError::Overrun { .. }));
    }
}
