//! Signal ranges, starting values, and multiplexor resolution.
//!
//! Two things here are easy to omit and both make messages silently
//! unsendable: ranges are computed from the bit layout rather than trusted from
//! the file, and a multiplexor is seeded with a value the message defines.

use std::collections::HashMap;

use super::{MessageDef, Mux, SignalDef};

/// What the field can physically hold, from its width, factor and offset.
///
/// Computed rather than taken from the declared min/max, which disagree in
/// real files. A 7-bit signal at factor 0.1, offset -20 tops out at -7.3
/// however large a maximum the DBC claims.
pub fn representable_range(sig: &SignalDef) -> (f64, f64) {
    if sig.len == 0 || sig.len > 64 {
        return (f64::NEG_INFINITY, f64::INFINITY);
    }
    let (lo_raw, hi_raw): (i128, i128) = if sig.signed {
        (-(1i128 << (sig.len - 1)), (1i128 << (sig.len - 1)) - 1)
    } else {
        (0, (1i128 << sig.len) - 1)
    };
    let a = lo_raw as f64 * sig.factor + sig.offset;
    let b = hi_raw as f64 * sig.factor + sig.offset;
    if a <= b { (a, b) } else { (b, a) } // a negative factor flips the ends
}

/// The representable range, narrowed by the declared limits only where those
/// fall inside it. `min == max == 0` is the usual DBC spelling of "unspecified".
pub fn effective_range(sig: &SignalDef) -> (f64, f64) {
    let (rlo, rhi) = representable_range(sig);
    let (mut lo, mut hi) = (rlo, rhi);
    let unspecified = matches!((sig.min, sig.max), (Some(a), Some(b)) if a == 0.0 && b == 0.0);
    if !unspecified {
        if let Some(m) = sig.min
            && (rlo..=rhi).contains(&m)
        {
            lo = m;
        }
        if let Some(m) = sig.max
            && (rlo..=rhi).contains(&m)
        {
            hi = m;
        }
    }
    if lo <= hi { (lo, hi) } else { (rlo, rhi) }
}

/// A starting value that is guaranteed to encode.
pub fn default_value(sig: &SignalDef) -> f64 {
    if let Some((&raw, _)) = sig.choices.iter().next() {
        return raw as f64 * sig.factor + sig.offset;
    }
    let (lo, hi) = effective_range(sig);
    if !lo.is_finite() || !hi.is_finite() {
        return 0.0;
    }
    0.0f64.max(lo).min(hi)
}

/// The multiplexor values this message actually defines, ascending.
pub fn defined_selectors(msg: &MessageDef) -> Vec<u64> {
    let mut v: Vec<u64> = msg
        .signals
        .iter()
        .filter_map(|s| match &s.mux {
            Mux::SelectedBy(ids) => Some(ids.iter().copied()),
            _ => None,
        })
        .flatten()
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Starting values for every signal of a message, sendable as-is.
///
/// The multiplexor gets the lowest id the message actually defines: 0 is not
/// always one of them, and an undefined selector makes the message unencodable.
pub fn initial_values(msg: &MessageDef) -> HashMap<String, f64> {
    let mut values: HashMap<String, f64> = msg
        .signals
        .iter()
        .map(|s| (s.name.clone(), default_value(s)))
        .collect();

    let defined = defined_selectors(msg);
    if let Some(&first) = defined.first() {
        for sel in msg.signals.iter().filter(|s| s.mux == Mux::Selector) {
            let want = first as f64 * sel.factor + sel.offset;
            let current = values[&sel.name];
            let raw = ((current - sel.offset) / sel.factor).round();
            if raw < 0.0 || !defined.contains(&(raw as u64)) {
                values.insert(sel.name.clone(), want);
            }
        }
    }
    values
}

/// The raw multiplexor value currently selected, if the message has one.
pub fn selector_raw(msg: &MessageDef, values: &HashMap<String, f64>) -> Option<u64> {
    let sel = msg.signals.iter().find(|s| s.mux == Mux::Selector)?;
    let v = values
        .get(&sel.name)
        .copied()
        .unwrap_or_else(|| default_value(sel));
    let raw = ((v - sel.offset) / sel.factor).round();
    (raw >= 0.0).then_some(raw as u64)
}

/// The signals that belong in this encode.
///
/// A multiplexed message only carries the signals selected by the current
/// multiplexor value, and including the others produces a frame the receiver
/// will misread. Cell voltages and temperatures commonly use this.
pub fn active_signals<'a>(
    msg: &'a MessageDef,
    values: &HashMap<String, f64>,
) -> Vec<&'a SignalDef> {
    if !msg.is_multiplexed() {
        return msg.signals.iter().collect();
    }
    let selected = selector_raw(msg, values);
    msg.signals
        .iter()
        .filter(|s| match &s.mux {
            Mux::Plain | Mux::Selector => true,
            Mux::SelectedBy(ids) => selected.is_some_and(|v| ids.contains(&v)),
        })
        .collect()
}
