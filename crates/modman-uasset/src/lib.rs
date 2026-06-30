//! modman-uasset — UE4 .uasset/.uexp binary parser for Project Wingman.
//!
//! Standalone crate with no internal modman dependencies.
//! Handles UE4.24 (v1.0.4d) and UE4.27 (v2.1.1A) uasset formats.

pub mod header;
pub mod name_table;
use std::io::Read;

pub use header::PackageHeader;

/// Errors from uasset parsing
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid magic number: 0x{0:08X}")]
    InvalidMagic(u32),
    #[error("Invalid string length: {0}")]
    InvalidStringLength(i32),
    #[error("Parse error: {0}")]
    Parse(String),
}

/// Helper: read a null-terminated or length-prefixed FString
pub fn read_fstring<R: Read>(reader: &mut R) -> Result<Option<String>, Error> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let length = i32::from_le_bytes(len_buf);

    if length == 0 {
        return Ok(None);
    }

    let abs_len = length.unsigned_abs() as usize;

    if length > 0 {
        // UTF-8 string
        let mut buf = vec![0u8; abs_len];
        reader.read_exact(&mut buf)?;
        let s = String::from_utf8(buf[..buf.len().saturating_sub(1)].to_vec())
            .map_err(|e| Error::Parse(format!("Invalid UTF-8: {e}")))?;
        Ok(Some(s))
    } else {
        // UCS-2 string (negative length = 2 bytes per char)
        let mut buf = vec![0u8; abs_len];
        reader.read_exact(&mut buf)?;
        let u16_data: Vec<u16> = buf[..buf.len().saturating_sub(2)]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let s = String::from_utf16(&u16_data)
            .map_err(|e| Error::Parse(format!("Invalid UCS-2: {e}")))?;
        Ok(Some(s))
    }
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_returns_string() {
        assert!(!version().is_empty());
    }
}
