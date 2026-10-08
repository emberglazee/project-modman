//! Same-size splice editing for cooked uexp payloads.
//!
//! Same-size edits overwrite ONLY the value bytes — no size fields, offsets, or
//! other structures move, which is why they can be applied to a copy of the
//! original bytes with zero re-serialization risk (verified against the
//! UAssetAPI reader for real PW DataTables).

use crate::walk::{ByteValue, Prop, PropValue};
use crate::Error;

/// The byte span a same-size splice may overwrite for this property, if any.
pub fn edit_span(prop: &Prop) -> Option<(usize, usize)> {
    match &prop.value {
        PropValue::Int(_)
        | PropValue::Float(_)
        | PropValue::Object(_)
        | PropValue::Name(_)
        | PropValue::Bool(_) => Some((prop.vstart, prop.vend)),
        PropValue::Byte {
            value: ByteValue::Byte(_),
            ..
        } => Some((prop.vstart, prop.vstart + 1)),
        PropValue::Byte {
            value: ByteValue::FName(_),
            ..
        } => Some((prop.vstart, prop.vstart + 8)),
        _ => None,
    }
}

/// Overwrite `span` in `uexp` with `bytes` (must match the span length exactly).
pub fn splice(uexp: &mut [u8], span: (usize, usize), bytes: &[u8]) -> Result<(), Error> {
    let (a, b) = span;
    if a > b || b > uexp.len() {
        return Err(Error::Edit(format!(
            "splice span {a}..{b} out of range (len {})",
            uexp.len()
        )));
    }
    if b - a != bytes.len() {
        return Err(Error::Edit(format!(
            "splice length mismatch: span is {} bytes, replacement is {}",
            b - a,
            bytes.len()
        )));
    }
    uexp[a..b].copy_from_slice(bytes);
    Ok(())
}

/// Set a float value (`FloatProperty`).
pub fn set_f32(uexp: &mut [u8], prop: &Prop, v: f32) -> Result<(), Error> {
    match &prop.value {
        PropValue::Float(_) => splice(uexp, edit_span(prop).unwrap(), &v.to_le_bytes()),
        _ => Err(Error::Edit(format!("not a FloatProperty: {}", prop.name))),
    }
}

/// Set an int value (`IntProperty`).
pub fn set_i32(uexp: &mut [u8], prop: &Prop, v: i32) -> Result<(), Error> {
    match &prop.value {
        PropValue::Int(_) => splice(uexp, edit_span(prop).unwrap(), &v.to_le_bytes()),
        _ => Err(Error::Edit(format!("not an IntProperty: {}", prop.name))),
    }
}

/// Set a bool value (`BoolProperty`; the value lives inside the tag).
pub fn set_bool(uexp: &mut [u8], prop: &Prop, v: bool) -> Result<(), Error> {
    match &prop.value {
        PropValue::Bool(_) => splice(uexp, edit_span(prop).unwrap(), &[v as u8]),
        _ => Err(Error::Edit(format!("not a BoolProperty: {}", prop.name))),
    }
}

/// Set a raw `ByteProperty` byte value (1-byte byte-kind only).
pub fn set_byte(uexp: &mut [u8], prop: &Prop, v: u8) -> Result<(), Error> {
    match &prop.value {
        PropValue::Byte {
            value: ByteValue::Byte(_),
            ..
        } => splice(uexp, edit_span(prop).unwrap(), &[v]),
        _ => Err(Error::Edit(format!(
            "not a byte-valued ByteProperty: {}",
            prop.name
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_bounds_and_lengths() {
        let mut buf = vec![0u8; 8];
        assert!(splice(&mut buf, (2, 6), &[1, 2, 3, 4]).is_ok());
        assert_eq!(&buf[2..6], &[1, 2, 3, 4]);
        assert!(splice(&mut buf, (2, 6), &[1, 2]).is_err());
        assert!(splice(&mut buf, (6, 9), &[1, 2, 3]).is_err());
        assert!(splice(&mut buf, (7, 3), &[]).is_err());
    }
}
