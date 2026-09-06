//! CAN identifier helpers.
//!
//! We use `embedded_can::Id` everywhere so that "standard or extended" is a type
//! distinction rather than a `bool` argument that eventually gets passed wrong.

use embedded_can::{ExtendedId, Id, StandardId};

/// Bit 31 marks an extended identifier. This is the convention DBC files
/// themselves use in `BO_` lines, so it doubles as our map key.
pub const EXTENDED_FLAG: u32 = 0x8000_0000;

pub fn raw(id: Id) -> u32 {
    match id {
        Id::Standard(s) => s.as_raw() as u32,
        Id::Extended(e) => e.as_raw(),
    }
}

pub fn is_extended(id: Id) -> bool {
    matches!(id, Id::Extended(_))
}

/// A key that cannot confuse standard `0x123` with extended `0x123`.
pub fn key(id: Id) -> u32 {
    raw(id) | if is_extended(id) { EXTENDED_FLAG } else { 0 }
}

/// Build an `Id`, falling back to extended when the value cannot be standard.
pub fn make(raw: u32, extended: bool) -> Option<Id> {
    if extended || raw > StandardId::MAX.as_raw() as u32 {
        ExtendedId::new(raw & 0x1FFF_FFFF).map(Id::Extended)
    } else {
        StandardId::new(raw as u16).map(Id::Standard)
    }
}

/// Render as a DBC-style hex id: three digits for standard, eight for extended.
pub fn display(id: Id) -> String {
    match id {
        Id::Standard(s) => format!("0x{:03X}", s.as_raw()),
        Id::Extended(e) => format!("0x{:08X}", e.as_raw()),
    }
}
