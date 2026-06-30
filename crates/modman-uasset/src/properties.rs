//! Property types and tagged property serialization for UE4 uasset files.
//!
//! After the export map, each export's data contains a list of tagged properties.
//! Each property has a tag (name + type + size) followed by type-specific data.

use crate::names::FName;
use crate::Error;
use std::io::{Read, Seek};

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
    pub size: i32,
}

/// Read all properties from a stream, bounded by end_pos (0 = unlimited)
pub fn read_properties_bounded<R: Read + Seek>(
    reader: &mut R,
    names: &[String],
    end_pos: u64,
) -> Result<Vec<Property>, Error> {
    let mut properties = Vec::new();
    let mut last_pos;

    loop {
        last_pos = reader.stream_position()?;
        if end_pos > 0 && last_pos >= end_pos {
            break;
        }

        let prop = read_single_property(reader, names)?;
        match &prop.value {
            PropertyValue::None => break,
            _ => properties.push(prop),
        }
    }

    Ok(properties)
}

/// Read all properties from a stream (unbounded, reads until None or EOF)
pub fn read_properties<R: Read + Seek>(
    reader: &mut R,
    names: &[String],
) -> Result<Vec<Property>, Error> {
    let mut properties = Vec::new();

    loop {
        // Check if we can read at least 8 bytes (a None terminator)
        let pos = reader.stream_position()?;
        let remaining = reader
            .seek(std::io::SeekFrom::End(0))
            .map(|len| len.saturating_sub(pos))
            .unwrap_or(0);
        reader.seek(std::io::SeekFrom::Start(pos))?;

        if remaining < 8 {
            break;
        }

        let prop = read_single_property(reader, names)?;
        match &prop.value {
            PropertyValue::None => break,
            _ => properties.push(prop),
        }
    }

    Ok(properties)
}

/// Known UE4 property type names used to validate tags
const KNOWN_PROPERTY_TYPES: &[&str] = &[
    "IntProperty",
    "FloatProperty",
    "BoolProperty",
    "StrProperty",
    "NameProperty",
    "TextProperty",
    "StructProperty",
    "ArrayProperty",
    "ObjectProperty",
    "SoftObjectProperty",
    "ByteProperty",
    "EnumProperty",
    "InterfaceProperty",
    "FieldPathProperty",
    "MapProperty",
    "SetProperty",
    "MulticastDelegateProperty",
    "DelegateProperty",
    "ClassProperty",
    "ObjectProperty",
];

fn read_single_property<R: Read + Seek>(
    reader: &mut R,
    names: &[String],
) -> Result<Property, Error> {
    let tag_start = reader.stream_position()?;

    // Read property tag: Name FName + Type FName + Size + ArrayIndex
    let mut name_fname = FName::read(reader)?;
    name_fname.resolve(names);

    // None name (index 0, number 0) terminates property list
    if name_fname.index == 0 && name_fname.number == 0 {
        return Ok(Property {
            name: String::new(),
            type_name: String::new(),
            value: PropertyValue::None,
            size: 0,
        });
    }

    let mut type_fname = FName::read(reader)?;
    type_fname.resolve(names);

    // Validate that the type name looks like a UE4 property type
    let type_name = type_fname.value.as_deref().unwrap_or("");
    let is_known_type = KNOWN_PROPERTY_TYPES.contains(&type_name);

    // Read size and array index (needed even for unknown types to skip)
    let size = read_i32(reader)?;

    // If the type is not recognized, this is likely not a valid property tag.
    // Rewind and treat the remaining data as finished.
    if !is_known_type && !type_name.is_empty() {
        reader.seek(std::io::SeekFrom::Start(tag_start))?;
        return Ok(Property {
            name: String::new(),
            type_name: String::new(),
            value: PropertyValue::None,
            size: 0,
        });
    }

    let _array_index = read_i32(reader)?;

    // Read property data based on type
    let value = read_property_value(reader, type_name, size, names)?;

    Ok(Property {
        name: name_fname.value.unwrap_or_default(),
        type_name: type_name.to_string(),
        value,
        size,
    })
}

fn read_property_value<R: Read + Seek>(
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
            let _flags = FName::read(reader)?;
            let _text_flags = read_i32(reader)?;
            let s = crate::read_fstring(reader)?;
            Ok(PropertyValue::Str(s.unwrap_or_default()))
        }
        "StructProperty" => {
            // StructProperty: subtype FName + 16 byte GUID + raw struct data
            let mut struct_type_fname = FName::read(reader)?;
            struct_type_fname.resolve(names);
            let struct_type = struct_type_fname.value.unwrap_or_default();
            let mut guid = [0u8; 16];
            reader.read_exact(&mut guid)?;

            // Cooked PW assets use raw struct serialization (not tagged properties)
            // Skip the remaining struct data based on the size field
            let consumed_so_far = 8 + 16; // subtype FName + GUID
            let remaining = (_size as u64).saturating_sub(consumed_so_far);
            if remaining > 0 {
                let mut skip = vec![0u8; remaining as usize];
                reader.read_exact(&mut skip)?;
            }

            Ok(PropertyValue::Struct {
                struct_type,
                properties: Vec::new(), // Raw data, no parsed sub-properties
            })
        }
        "ArrayProperty" => {
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
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            Ok(PropertyValue::Int(i32::from_le_bytes(buf)))
        }
        "ByteProperty" => {
            let mut enum_type = FName::read(reader)?;
            enum_type.resolve(names);
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            Ok(PropertyValue::Int(buf[0] as i32))
        }
        "EnumProperty" => {
            let _enum_type = FName::read(reader)?;
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf)?;
            Ok(PropertyValue::Int(buf[0] as i32))
        }
        _ => {
            // Unknown type - skip by reading size bytes
            let skip = _size.max(0) as usize;
            let mut skip_buf = vec![0u8; skip.min(1024 * 1024)];
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
