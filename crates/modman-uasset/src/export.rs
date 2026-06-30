//! Export map entries for UE4 uasset files.
//!
//! Each export describes an object serialized within the uasset/uexp.

use crate::names::FName;
use crate::Error;
use std::io::Read;

/// An entry in the export map
#[derive(Debug, Clone)]
pub struct ExportEntry {
    pub object_name: FName,
    pub outer_index: FPackageIndex,
    pub class_index: FPackageIndex,
    pub super_index: FPackageIndex,
    pub template_index: FPackageIndex,
    pub object_flags: u32,
    pub serial_size: i64,
    pub serial_offset: i64,
}

impl ExportEntry {
    /// Read an export entry from the current stream position
    pub fn read<R: Read>(reader: &mut R) -> Result<Self, Error> {
        let object_name = FName::read(reader)?;
        let outer_index = FPackageIndex::read(reader)?;
        let class_index = FPackageIndex::read(reader)?;
        let super_index = FPackageIndex::read(reader)?;
        let template_index = FPackageIndex::read(reader)?;
        let object_flags = read_u32(reader)?;
        let serial_size = read_i64(reader)?;
        let serial_offset = read_i64(reader)?;
        // Skip optional fields (script serial offsets, forced export flags, etc.)
        // For cooked UE4 assets, these may not be present or use defaults

        Ok(Self {
            object_name,
            outer_index,
            class_index,
            super_index,
            template_index,
            object_flags,
            serial_size,
            serial_offset,
        })
    }
}

/// A reference to an import or export entry
#[derive(Debug, Clone, Copy)]
pub struct FPackageIndex {
    /// Negative = import (1-indexed), Positive = export (0-indexed, +1)
    pub index: i32,
}

impl FPackageIndex {
    pub fn read<R: Read>(reader: &mut R) -> Result<Self, Error> {
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf)?;
        Ok(Self {
            index: i32::from_le_bytes(buf),
        })
    }

    pub fn is_import(&self) -> bool {
        self.index < 0
    }

    pub fn is_export(&self) -> bool {
        self.index > 0
    }

    pub fn is_null(&self) -> bool {
        self.index == 0
    }

    /// Get the 0-based index into the import/export table
    pub fn table_index(&self) -> usize {
        if self.index < 0 {
            (-self.index - 1) as usize
        } else {
            (self.index - 1) as usize
        }
    }
}

/// Read the export map from the current stream position
pub fn read_export_map<R: Read>(reader: &mut R, count: i32) -> Result<Vec<ExportEntry>, Error> {
    let mut exports = Vec::with_capacity(count as usize);
    for _ in 0..count {
        exports.push(ExportEntry::read(reader)?);
    }
    Ok(exports)
}

fn read_u32<R: Read>(reader: &mut R) -> Result<u32, Error> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_i64<R: Read>(reader: &mut R) -> Result<i64, Error> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(i64::from_le_bytes(buf))
}
