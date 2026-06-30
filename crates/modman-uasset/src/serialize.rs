//! Binary serialization for UE4 uasset files.
//!
//! Mirrors the reader: write_fstring, FName, header, name table, export map, and properties.

use crate::Error;
use std::io::Write;

/// Write a serialized FString (length-prefixed string with null terminator)
pub fn write_fstring<W: Write>(writer: &mut W, s: Option<&str>) -> Result<(), Error> {
    match s {
        None => {
            // Zero length = null string
            writer.write_all(&0i32.to_le_bytes())?;
        }
        Some(text) => {
            // Positive length = UTF-8 with null terminator
            let len = text.len() as i32 + 1; // +1 for null terminator
            writer.write_all(&len.to_le_bytes())?;
            writer.write_all(text.as_bytes())?;
            writer.write_all(&[0u8])?; // null terminator
        }
    }
    Ok(())
}

/// Write an FName (index + number)
pub fn write_fname<W: Write>(writer: &mut W, index: i32, number: i32) -> Result<(), Error> {
    writer.write_all(&index.to_le_bytes())?;
    writer.write_all(&number.to_le_bytes())?;
    Ok(())
}
pub fn write_fstring_ue4<W: Write>(writer: &mut W, s: &str) -> Result<(), Error> {
    // Check if the string needs UCS-2 encoding (has non-ASCII chars)
    if s.is_ascii() {
        write_fstring(writer, Some(s))?;
    } else {
        // UCS-2 encoding (negative length)
        let u16_data: Vec<u16> = s.encode_utf16().collect();
        let byte_len = (u16_data.len() + 1) * 2; // +1 for null terminator
        let neg_len = -(byte_len as i32 / 2);
        writer.write_all(&neg_len.to_le_bytes())?;
        for &code in &u16_data {
            writer.write_all(&code.to_le_bytes())?;
        }
        writer.write_all(&0u16.to_le_bytes())?; // null terminator
    }
    Ok(())
}

/// Write property tag (name FName + type FName + size + array index)
pub fn write_property_tag<W: Write>(
    writer: &mut W,
    name_idx: i32,
    name_num: i32,
    type_idx: i32,
    type_num: i32,
    size: i32,
    array_index: i32,
) -> Result<(), Error> {
    write_fname(writer, name_idx, name_num)?;
    write_fname(writer, type_idx, type_num)?;
    writer.write_all(&size.to_le_bytes())?;
    writer.write_all(&array_index.to_le_bytes())?;
    Ok(())
}

/// Write the None terminator (8 zero bytes)
pub fn write_none_terminator<W: Write>(writer: &mut W) -> Result<(), Error> {
    writer.write_all(&[0u8; 8])?; // FName with index=0, number=0
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_fstring() {
        let test_strings = vec![
            "Hello",
            "None",
            "",
            "/Script/CoreUObject",
            "test with spaces",
        ];
        for s in test_strings {
            let mut buf = Vec::new();
            write_fstring(&mut buf, Some(s)).unwrap();
            let mut cursor = std::io::Cursor::new(&buf);
            let result = crate::read_fstring(&mut cursor).unwrap();
            assert_eq!(result, Some(s.to_string()), "roundtrip failed for '{}'", s);
        }
    }

    #[test]
    fn roundtrip_null_fstring() {
        let mut buf = Vec::new();
        write_fstring(&mut buf, None).unwrap();
        let mut cursor = std::io::Cursor::new(&buf);
        let result = crate::read_fstring(&mut cursor).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn roundtrip_fname() {
        let mut buf = Vec::new();
        write_fname(&mut buf, 42, 1).unwrap();
        let mut cursor = std::io::Cursor::new(&buf);
        let name = crate::names::FName::read(&mut cursor).unwrap();
        assert_eq!(name.index, 42);
        assert_eq!(name.number, 1);
    }
}
