//! Export map entries for UE4 uasset files.
//!
//! UE4.27 cooked entries are **104 bytes**; this layout is verified against both
//! UAssetAPI's `Export.ReadExportMapEntry` and real PW assets (see [`crate::walk`]).

use crate::names::FName;
use crate::Error;
use std::io::Read;

/// Size of a cooked UE4.27 export-map entry in bytes.
pub const EXPORT_ENTRY_SIZE: usize = 104;

/// An entry in the export map.
#[derive(Debug, Clone)]
pub struct ExportEntry {
    pub class_index: FPackageIndex,
    pub super_index: FPackageIndex,
    pub template_index: FPackageIndex,
    pub outer_index: FPackageIndex,
    pub object_name: FName,
    pub object_flags: u32,
    pub serial_size: i64,
    pub serial_offset: i64,
    pub b_forced_export: bool,
    pub b_not_for_client: bool,
    pub b_not_for_server: bool,
    pub package_flags: u32,
    pub b_not_always_loaded_for_editor_game: bool,
    pub b_is_asset: bool,
    pub first_export_dependency_offset: i32,
    pub serialization_before_serialization_dependencies_size: i32,
    pub create_before_serialization_dependencies_size: i32,
    pub serialization_before_create_dependencies_size: i32,
    pub create_before_create_dependencies_size: i32,
}

impl ExportEntry {
    /// Read one export entry from the current stream position (UE4.27 layout, 104 bytes).
    pub fn read<R: Read>(reader: &mut R) -> Result<Self, Error> {
        Ok(Self {
            class_index: FPackageIndex::read(reader)?,
            super_index: FPackageIndex::read(reader)?,
            template_index: FPackageIndex::read(reader)?,
            outer_index: FPackageIndex::read(reader)?,
            object_name: FName::read(reader)?,
            object_flags: read_u32(reader)?,
            serial_size: read_i64(reader)?,
            serial_offset: read_i64(reader)?,
            b_forced_export: read_bool_int(reader)?,
            b_not_for_client: read_bool_int(reader)?,
            b_not_for_server: read_bool_int(reader)?,
            // 16-byte PackageGuid is not needed for navigation
            package_flags: {
                skip(reader, 16)?;
                read_u32(reader)?
            },
            b_not_always_loaded_for_editor_game: read_bool_int(reader)?,
            b_is_asset: read_bool_int(reader)?,
            first_export_dependency_offset: read_i32(reader)?,
            serialization_before_serialization_dependencies_size: read_i32(reader)?,
            create_before_serialization_dependencies_size: read_i32(reader)?,
            serialization_before_create_dependencies_size: read_i32(reader)?,
            create_before_create_dependencies_size: read_i32(reader)?,
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

fn read_i32<R: Read>(reader: &mut R) -> Result<i32, Error> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_i64<R: Read>(reader: &mut R) -> Result<i64, Error> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(i64::from_le_bytes(buf))
}

fn read_bool_int<R: Read>(reader: &mut R) -> Result<bool, Error> {
    match read_i32(reader)? {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(Error::Parse(format!("invalid boolean-int value {other}"))),
    }
}

fn skip<R: Read>(reader: &mut R, n: usize) -> Result<(), Error> {
    let mut buf = vec![0u8; n];
    reader.read_exact(&mut buf)?;
    Ok(())
}
