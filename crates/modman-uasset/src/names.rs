//! Name table and FName type for UE4 uasset files.
//!
//! FNames are stored as a table of (string, flags) pairs at the start
//! of the file after the header. Properties reference names by index
//! into this table, combined with a "number" suffix.

use crate::Error;
use std::io::Read;

/// A name in the uasset name map, referenced by index + number
#[derive(Debug, Clone)]
pub struct FName {
    /// Index into the name map (0-based)
    pub index: i32,
    /// Number suffix (used for duplicate names)
    pub number: i32,
    /// The resolved string value (None if index is out of range)
    pub value: Option<String>,
}

impl FName {
    pub fn new(index: i32, number: i32) -> Self {
        Self {
            index,
            number,
            value: None,
        }
    }

    /// Read an FName from the current stream position.
    /// The name map must be provided to resolve index → string.
    pub fn read<R: Read>(reader: &mut R) -> Result<Self, Error> {
        let mut buf = [0u8; 8];
        reader.read_exact(&mut buf)?;
        let index = i32::from_le_bytes(buf[0..4].try_into().unwrap());
        let number = i32::from_le_bytes(buf[4..8].try_into().unwrap());
        Ok(Self {
            index,
            number,
            value: None,
        })
    }

    /// Resolve this FName's value from a name table
    pub fn resolve(&mut self, names: &[String]) {
        if self.index >= 0 && (self.index as usize) < names.len() {
            let base = &names[self.index as usize];
            if self.number > 0 {
                self.value = Some(format!("{}_{}", base, self.number - 1));
            } else {
                self.value = Some(base.clone());
            }
        }
    }
}

/// Read the name table from a stream
pub fn read_name_table<R: Read>(reader: &mut R, count: i32) -> Result<Vec<String>, Error> {
    let mut names = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = crate::read_fstring(reader)?.unwrap_or_default();
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf)?;
        let _flags = u32::from_le_bytes(buf);
        names.push(name);
    }
    Ok(names)
}
