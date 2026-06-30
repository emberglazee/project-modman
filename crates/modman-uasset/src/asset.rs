//! Full uasset writer — serializes a complete uasset file from parsed components.
//!
//! Reads the original binary, parses it, allows modifications, then writes back.
//! For properties that don't change size (Int, Float, Bool), we can patch in-place.
//! For variable-size changes, we rebuild the .uexp from scratch.

use crate::export::ExportEntry;
use crate::header::PackageHeader;
use crate::properties::{Property, PropertyValue};
use crate::serialize;
use crate::Error;
use std::io::{Seek, Write};

/// A fully parsed uasset + uexp pair ready for modification
pub struct AssetFile {
    pub header: PackageHeader,
    pub names: Vec<String>,
    pub exports: Vec<ExportEntry>,
    pub properties: Vec<Property>,
    pub uasset_data: Vec<u8>,
    pub uexp_data: Vec<u8>,
}

impl AssetFile {
    /// Open and parse a .uasset + .uexp pair
    pub fn open(uasset_path: &str, uexp_path: &str) -> Result<Self, Error> {
        let uasset_data = std::fs::read(uasset_path)?;
        let uexp_data = std::fs::read(uexp_path)?;

        let mut cursor = std::io::Cursor::new(&uasset_data);
        let header = PackageHeader::read(&mut cursor)?;

        // Read name table
        let name_offset = header.name_offset as u64;
        cursor.seek(std::io::SeekFrom::Start(name_offset))?;
        let names = crate::names::read_name_table(&mut cursor, header.name_count)?;

        // Read export map
        let export_offset = header.export_offset as u64;
        cursor.seek(std::io::SeekFrom::Start(export_offset))?;
        let exports = crate::export::read_export_map(&mut cursor, header.export_count)?;

        // Read properties from .uexp
        let mut uexp_cursor = std::io::Cursor::new(&uexp_data);
        let properties = crate::properties::read_properties(&mut uexp_cursor, &names)?;

        Ok(Self {
            header,
            names,
            exports,
            properties,
            uasset_data,
            uexp_data,
        })
    }

    /// Apply patches and write the modified files back
    pub fn apply_and_write(&mut self, output_uasset: &str, output_uexp: &str) -> Result<(), Error> {
        // For now, write the uasset as-is (header + name table + export map unchanged)
        std::fs::write(output_uasset, &self.uasset_data)?;

        // Serialize modified properties to .uexp
        let mut uexp_out = Vec::new();
        self.serialize_properties(&mut uexp_out)?;
        std::fs::write(output_uexp, uexp_out)?;

        Ok(())
    }

    /// Serialize properties back to .uexp binary format
    fn serialize_properties<W: Write>(&self, writer: &mut W) -> Result<(), Error> {
        for prop in &self.properties {
            self.write_property(writer, prop)?;
        }
        // Write None terminator
        serialize::write_none_terminator(writer)?;
        Ok(())
    }

    /// Write a single property tag + value
    fn write_property<W: Write>(&self, writer: &mut W, prop: &Property) -> Result<(), Error> {
        if prop.name.is_empty() && prop.type_name.is_empty() {
            serialize::write_none_terminator(writer)?;
            return Ok(());
        }

        // Find name and type indices
        let name_idx = self.find_name_index(&prop.name).unwrap_or(0);
        let name_num = 0;
        let type_idx = self.find_name_index(&prop.type_name).unwrap_or(0);
        let type_num = 0;

        // Compute serialized size of property value
        let value_size = self.compute_property_size(prop);

        // Write property tag
        serialize::write_property_tag(
            writer, name_idx, name_num, type_idx, type_num, value_size, 0,
        )?;

        // Write property value
        self.write_property_value(writer, &prop.value)?;

        Ok(())
    }

    fn find_name_index(&self, name: &str) -> Option<i32> {
        self.names.iter().position(|n| n == name).map(|i| i as i32)
    }

    fn compute_property_size(&self, prop: &Property) -> i32 {
        match &prop.value {
            PropertyValue::Int(_) => 4,
            PropertyValue::Float(_) => 4,
            PropertyValue::Bool(_) => 1,
            PropertyValue::Str(s) => {
                // FString: 4 bytes length + string bytes + null terminator
                let len = s.len() as i32;
                if len > 0 {
                    4 + len + 1
                } else {
                    4 // just the zero length field
                }
            }
            PropertyValue::Name(_) => 8, // FName: 8 bytes
            PropertyValue::Struct {
                struct_type: _,
                properties,
            } => {
                // 8 bytes subtype FName + 16 bytes GUID + inner properties
                let mut size: i32 = 8 + 16;
                for inner in properties {
                    size += 24 + self.compute_property_size(inner); // tag + value
                }
                size += 8; // None terminator for inner props
                size
            }
            PropertyValue::Array {
                element_type: _,
                elements,
            } => {
                // FName type + i32 inner_size + i32 count + elements
                let mut size: i32 = 8 + 4 + 4;
                for elem in elements {
                    match elem {
                        PropertyValue::Int(_) => size += 4,
                        PropertyValue::Float(_) => size += 4,
                        PropertyValue::Bool(_) => size += 1,
                        PropertyValue::Str(s) => {
                            let len = s.len() as i32;
                            size += 4 + if len > 0 { len + 1 } else { 0 };
                        }
                        PropertyValue::Name(_) => size += 8,
                        _ => size += 4,
                    }
                }
                size
            }
            PropertyValue::None => 0,
        }
    }

    fn write_property_value<W: Write>(
        &self,
        writer: &mut W,
        value: &PropertyValue,
    ) -> Result<(), Error> {
        match value {
            PropertyValue::Int(v) => {
                writer.write_all(&v.to_le_bytes())?;
            }
            PropertyValue::Float(v) => {
                writer.write_all(&v.to_le_bytes())?;
            }
            PropertyValue::Bool(v) => {
                writer.write_all(&[*v as u8])?;
            }
            PropertyValue::Str(s) => {
                serialize::write_fstring(writer, Some(s))?;
            }
            PropertyValue::Name(fname) => {
                let idx = fname.index.max(0);
                let num = fname.number.max(0);
                serialize::write_fname(writer, idx.max(0), num)?;
            }
            PropertyValue::Struct {
                struct_type,
                properties,
            } => {
                // Write subtype FName
                let sub_idx = self.find_name_index(struct_type).unwrap_or(0);
                serialize::write_fname(writer, sub_idx, 0)?;
                // Write empty GUID (16 zeros)
                writer.write_all(&[0u8; 16])?;
                // Write inner properties
                for inner in properties {
                    self.write_property(writer, inner)?;
                }
                serialize::write_none_terminator(writer)?;
            }
            PropertyValue::Array {
                element_type,
                elements,
            } => {
                let elem_idx = self.find_name_index(element_type).unwrap_or(0);
                serialize::write_fname(writer, elem_idx, 0)?;
                // inner_size (0 = auto)
                writer.write_all(&0i32.to_le_bytes())?;
                // count
                writer.write_all(&(elements.len() as i32).to_le_bytes())?;
                for elem in elements {
                    self.write_property_value(writer, elem)?;
                }
            }
            PropertyValue::None => {}
        }
        Ok(())
    }
}
