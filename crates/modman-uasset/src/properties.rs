//! Property types and tagged property serialization for UE4 uasset files.
//!
//! After the export map, each export's data contains a list of tagged properties.
//! Each property has a tag (name + type + size) followed by type-specific data.

use crate::names::FName;
use crate::Error;
use std::io::Read;

/// A tagged property read from an export's data
#[derive(Debug, Clone)]
pub enum PropertyValue {
    Int(i32),
    Float(f32),
    Bool(bool),
    Str(String),
    Name(FName),
    Struct {
        struct_type: String,
        properties: Vec<Property>,
    },
    Array {
        element_type: String,
        elements: Vec<PropertyValue>,
    },
    None,
}

/// A tagged property with its name and type
#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    pub type_name: String,
    pub value: PropertyValue,
}

/// Read all properties from an export's data stream
pub fn read_properties<R: Read>(reader: &mut R, names: &[String]) -> Result<Vec<Property>, Error> {
    let mut properties = Vec::new();

    loop {
        let prop = read_single_property(reader, names)?;
        match &prop.value {
            PropertyValue::None => break,
            _ => properties.push(prop),
        }
    }

    Ok(properties)
}

fn read_single_property<R: Read>(reader: &mut R, names: &[String]) -> Result<Property, Error> {
    // Read property tag
    let mut name_fname = FName::read(reader)?;
    name_fname.resolve(names);

    // None name (index 0, number 0) terminates property list
    if name_fname.index == 0 && name_fname.number == 0 {
        return Ok(Property {
            name: String::new(),
            type_name: String::new(),
            value: PropertyValue::None,
        });
    }

    let mut type_fname = FName::read(reader)?;
    type_fname.resolve(names);

    let size = read_i32(reader)?;
    let _array_index = read_i32(reader)?;

    let type_name = type_fname.value.unwrap_or_default();

    // Read property data based on type
    let value = read_property_value(reader, &type_name, size, names)?;

    Ok(Property {
        name: name_fname.value.unwrap_or_default(),
        type_name,
        value,
    })
}

fn read_property_value<R: Read>(
    reader: &mut R,
    type_name: &str,
    _size: i32,
    names: &[String],
) -> Result<PropertyValue, Error> {
    match type_name {
        "IntProperty" => {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            Ok(PropertyValue::Int(i32::from_le_bytes(buf)))
        }
        "FloatProperty" => {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            Ok(PropertyValue::Float(f32::from_le_bytes(buf)))
        }
        "BoolProperty" => {
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            Ok(PropertyValue::Bool(buf[0] != 0))
        }
        "StrProperty" => {
            let s = crate::read_fstring(reader)?;
            Ok(PropertyValue::Str(s.unwrap_or_default()))
        }
        "NameProperty" => {
            let mut name = FName::read(reader)?;
            name.resolve(names);
            Ok(PropertyValue::Name(name))
        }
        "TextProperty" => {
            // TextProperty has flags + culture invariant + source + key + namespace + value
            // Skip for now - read the size bytes
            let mut fname = FName::read(reader)?;
            fname.resolve(names);
            let _text_flags = read_i32(reader)?;
            // Read the text value (simplified)
            let s = crate::read_fstring(reader)?;
            Ok(PropertyValue::Str(s.unwrap_or_default()))
        }
        "StructProperty" => {
            // Struct property has a type name FName before the data
            let mut struct_type_fname = FName::read(reader)?;
            struct_type_fname.resolve(names);
            let struct_type = struct_type_fname.value.unwrap_or_default();
            let _struct_guid = read_i64(reader)?; // guid (first 8 bytes) - skip for now
            let _struct_guid_2 = read_i64(reader)?; // guid (second 8 bytes)

            let inner_props = read_properties(reader, names)?;
            Ok(PropertyValue::Struct {
                struct_type,
                properties: inner_props,
            })
        }
        "ArrayProperty" => {
            // Array property has type name + element count + elements
            let mut elem_type_fname = FName::read(reader)?;
            elem_type_fname.resolve(names);
            let element_type = elem_type_fname.value.unwrap_or_default();
            let _inner_size = read_i32(reader)?;
            let count = read_i32(reader)?;

            let mut elements = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let elem = read_property_value(reader, &element_type, 0, names)?;
                elements.push(elem);
            }

            Ok(PropertyValue::Array {
                element_type,
                elements,
            })
        }
        "ObjectProperty" | "SoftObjectProperty" => {
            // Object reference - just an FPackageIndex
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            // We don't need the value for now
            Ok(PropertyValue::Int(i32::from_le_bytes(buf)))
        }
        "ByteProperty" => {
            // ByteProperty: FName enum type + int8 value
            let mut enum_type = FName::read(reader)?;
            enum_type.resolve(names);
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            Ok(PropertyValue::Int(buf[0] as i32))
        }
        _ => {
            // Unknown type - skip by reading size bytes
            let skip = _size.max(0) as usize;
            let mut skip_buf = vec![0u8; skip.min(1024 * 1024)]; // cap at 1MB
            let to_read = skip_buf.len().min(skip);
            reader.read_exact(&mut skip_buf[..to_read])?;
            Ok(PropertyValue::Str(format!("<skipped {} bytes>", skip)))
        }
    }
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
