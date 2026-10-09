//! modman-uasset — UE4 .uasset/.uexp binary parser for Project Wingman.
//!
//! Standalone crate with no internal modman dependencies.
//! Handles UE4.27 (v2.1.1A) cooked assets; the DataTable walker + splice editor
//! in [`walk`]/[`edit`] are verified byte-exact against real PW assets.

pub mod edit;
pub mod export;
pub mod hash;
pub mod header;
pub mod names;
pub mod properties;
pub mod rewrite;
pub mod walk;

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
    #[error("Edit error: {0}")]
    Edit(String),
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
        let (chunks, _) = buf[..buf.len().saturating_sub(2)].as_chunks::<2>();
        let u16_data: Vec<u16> = chunks.iter().map(|c| u16::from_le_bytes(*c)).collect();
        let s = String::from_utf16(&u16_data)
            .map_err(|e| Error::Parse(format!("Invalid UCS-2: {e}")))?;
        Ok(Some(s))
    }
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The class name of a uasset's first export (e.g. "DataTable",
/// "Texture2D", "SoundWave"), resolved through the import table.
///
/// Lets callers distinguish real DataTables (field-mergeable) from opaque
/// assets (textures, audio, meshes — single-winner by nature).
pub fn asset_class(uasset: &[u8]) -> Result<String, Error> {
    use std::io::{Cursor, Seek, SeekFrom};
    let mut c = Cursor::new(uasset);
    let header = PackageHeader::read(&mut c)?;
    if header.export_count < 1 {
        return Err(Error::Parse("no exports".into()));
    }
    c.seek(SeekFrom::Start(header.export_offset as u64))?;
    let exports = export::read_export_map(&mut c, header.export_count)?;
    let ex = exports
        .first()
        .ok_or_else(|| Error::Parse("no exports".into()))?;
    if !ex.class_index.is_import() {
        return Err(Error::Parse("export class is not an import".into()));
    }
    let imp_idx = (-ex.class_index.index - 1) as usize;
    if imp_idx >= header.import_count.max(0) as usize {
        return Err(Error::Parse("export class import out of range".into()));
    }
    let entry_off = header.import_offset as usize + imp_idx * 28;
    if entry_off + 28 > uasset.len() {
        return Err(Error::Parse("import entry out of range".into()));
    }
    // FObjectImport: [ClassPackage FName 8][ClassName FName 8][Outer i32][ObjectName FName 8]
    let name_idx = i32::from_le_bytes(uasset[entry_off + 20..entry_off + 24].try_into().unwrap());
    let mut nc = Cursor::new(uasset);
    nc.seek(SeekFrom::Start(header.name_offset as u64))?;
    let names = names::read_name_table(&mut nc, header.name_count)?;
    names
        .get(name_idx.max(0) as usize)
        .cloned()
        .ok_or_else(|| Error::Parse(format!("class name index {name_idx} out of range")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_returns_string() {
        assert!(!version().is_empty());
    }
}
