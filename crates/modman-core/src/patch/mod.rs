//! Patch types and value parsing for Project Modman.
//!
//! Each patch has a `type` (like "propertyValue", "modifyPropertyValue")
//! and a `value` string that's parsed into a typed modification.

use crate::fragment::Fragment;

/// A parsed patch operation
#[derive(Debug, Clone)]
pub enum PatchOp {
    /// Set a property to a specific value
    PropertyValue {
        value_type: String,
        value: String,
    },
    /// Modify a numeric property with arithmetic
    ModifyPropertyValue {
        value_type: Option<String>,
        operation: ArithmeticOp,
        range: Option<ValueRange>,
    },
    /// Set values in an array property
    ArrayPropertyValue {
        value_type: String,
        elements: Vec<String>,
        mode: ArrayMode,
    },
    /// Set a TextProperty value
    TextProperty {
        key: Option<String>,
        value: String,
    },
    /// Duplicate a datatable row or property
    DuplicateEntry {
        source: String,
        target: String,
    },
    DuplicateProperty {
        source: String,
        target: String,
    },
    /// Clone an array item
    DuplicateArrayItem {
        source_index: usize,
        target_index: Option<usize>,
    },
    /// Delete matching entries
    DeleteEntry(Vec<String>),
    /// Change object references
    ObjectRef {
        object_name: String,
        object_path: String,
    },
}

impl PatchOp {
    /// Short label matching the Sicario patch `type` string.
    pub fn label(&self) -> &'static str {
        match self {
            PatchOp::PropertyValue { .. } => "propertyValue",
            PatchOp::ModifyPropertyValue { .. } => "modifyPropertyValue",
            PatchOp::ArrayPropertyValue { .. } => "arrayPropertyValue",
            PatchOp::TextProperty { .. } => "textProperty",
            PatchOp::DuplicateEntry { .. } => "duplicateEntry",
            PatchOp::DuplicateProperty { .. } => "duplicateProperty",
            PatchOp::DuplicateArrayItem { .. } => "duplicateArrayItem",
            PatchOp::DeleteEntry(_) => "deleteEntry",
            PatchOp::ObjectRef { .. } => "objectRef",
        }
    }
}

/// Arithmetic operation for modifyPropertyValue
#[derive(Debug, Clone, Copy)]
pub enum ArithmeticOp {
    Add(f64),
    Subtract(f64),
    Multiply(f64),
    Divide(f64),
}

/// Numeric clamping range
#[derive(Debug, Clone, Copy)]
pub struct ValueRange {
    pub min: f64,
    pub max: f64,
}

/// Array modification mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArrayMode {
    Replace,
    Append,
    Insert,
}

/// A fully parsed patch ready for execution
#[derive(Debug, Clone)]
pub struct ParsedPatch {
    /// The fragments from the template string
    pub fragments: Vec<Fragment>,
    /// The patch type and its parsed value
    pub operation: PatchOp,
}

/// Parse errors for patch values
#[derive(Debug, thiserror::Error)]
pub enum PatchParseError {
    #[error("Invalid patch type: {0}")]
    InvalidType(String),
    #[error("Invalid value format: {0}")]
    InvalidValue(String),
    #[error("Unknown patch type '{0}'")]
    UnknownType(String),
}

/// Parse a complete patch (template + type + value) into a ParsedPatch
pub fn parse_patch(
    template: &str,
    patch_type: &str,
    value: &str,
) -> Result<ParsedPatch, PatchParseError> {
    let ctx = crate::fragment::parse_template(template)
        .map_err(|e| PatchParseError::InvalidValue(format!("Template parse error: {e}")))?;

    let operation = parse_patch_value(patch_type, value)?;

    Ok(ParsedPatch {
        fragments: ctx.fragments,
        operation,
    })
}

/// Parse a patch value string based on patch type
pub fn parse_patch_value(patch_type: &str, value: &str) -> Result<PatchOp, PatchParseError> {
    match patch_type {
        "propertyValue" => parse_property_value(value),
        "modifyPropertyValue" => parse_modify_property_value(value),
        "arrayPropertyValue" => parse_array_property_value(value),
        "textProperty" => parse_text_property(value),
        "duplicateEntry" => parse_duplicate(value),
        "duplicateProperty" => parse_duplicate(value),
        "duplicateArrayItem" => parse_duplicate_array_item(value),
        "deleteEntry" => Ok(PatchOp::DeleteEntry(
            value.split(',').map(|s| s.trim().to_string()).collect(),
        )),
        "objectRef" => parse_object_ref(value),
        _ => Err(PatchParseError::UnknownType(patch_type.to_string())),
    }
}

fn parse_property_value(value: &str) -> Result<PatchOp, PatchParseError> {
    // Format: "Type:value" or "Type:'value with spaces'"
    let (type_name, val) = split_value(value)?;
    Ok(PatchOp::PropertyValue {
        value_type: type_name,
        value: val.trim_matches('\'').to_string(),
    })
}

fn parse_modify_property_value(value: &str) -> Result<PatchOp, PatchParseError> {
    // Format: "Type:op value(range)" e.g. "FloatProperty:*2", "IntProperty:+6(0-10)"
    let (type_part, rest) = split_value(value)?;

    let value_type = if type_part == "*" {
        None
    } else {
        Some(type_part)
    };

    let (op_str, range_str) = if let Some(paren_start) = rest.find('(') {
        let range_s = rest[paren_start..].to_string();
        let op_s = rest[..paren_start].to_string();
        (op_s, Some(range_s))
    } else {
        (rest.to_string(), None)
    };

    let range = match range_str.as_deref() {
        Some(r) => {
            let parsed = parse_range(r)
                .map_err(|e| PatchParseError::InvalidValue(format!("Invalid range: {e}")))?;
            parsed
        }
        None => None,
    };

    let operation = parse_arithmetic(&op_str)?;

    Ok(PatchOp::ModifyPropertyValue {
        value_type,
        operation,
        range,
    })
}

fn parse_array_property_value(value: &str) -> Result<PatchOp, PatchParseError> {
    // Format: "Type:[items]" or "Type:+[items]"
    let (type_name, rest) = split_value(value)?;
    let rest = rest.trim();

    let mode = if rest.starts_with('+') {
        ArrayMode::Append
    } else if rest.starts_with('-') {
        ArrayMode::Insert
    } else {
        ArrayMode::Replace
    };

    let inner = rest
        .trim_start_matches('+')
        .trim_start_matches('-')
        .trim_start_matches('[')
        .trim_end_matches(']');

    let elements: Vec<String> = inner
        .split(',')
        .map(|s| s.trim().trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect();

    Ok(PatchOp::ArrayPropertyValue {
        value_type: type_name,
        elements,
        mode,
    })
}

fn parse_text_property(value: &str) -> Result<PatchOp, PatchParseError> {
    // Format: `'key':'value'` or `*:'value'` (key `*` = all parts).
    let value = value.trim();
    if let Some(colon_pos) = value.find(':') {
        let key_raw = value[..colon_pos].trim().trim_matches('\'');
        let key = if key_raw == "*" {
            None
        } else {
            Some(key_raw.to_string())
        };
        let val = value[colon_pos + 1..].trim().trim_matches('\'').to_string();
        Ok(PatchOp::TextProperty { key, value: val })
    } else {
        Err(PatchParseError::InvalidValue(format!(
            "Invalid textProperty format: {value}"
        )))
    }
}

fn parse_duplicate(value: &str) -> Result<PatchOp, PatchParseError> {
    // Format: "'Source'>'Target'"
    let value = value.trim();
    if let Some(greater_pos) = value.find('>') {
        let source = value[..greater_pos].trim().trim_matches('\'').to_string();
        let target = value[greater_pos + 1..]
            .trim()
            .trim_matches('\'')
            .to_string();
        Ok(PatchOp::DuplicateEntry { source, target })
    } else {
        Err(PatchParseError::InvalidValue(format!(
            "Invalid duplicate format: {value}"
        )))
    }
}

fn parse_duplicate_array_item(value: &str) -> Result<PatchOp, PatchParseError> {
    // Format: "SrcIndex>DstIndex"
    let value = value.trim();
    if let Some(greater_pos) = value.find('>') {
        let src = value[..greater_pos]
            .trim()
            .parse::<usize>()
            .map_err(|_| PatchParseError::InvalidValue(format!("Invalid index: {value}")))?;
        let dst_str = value[greater_pos + 1..].trim();
        let dst =
            if dst_str.is_empty() {
                None
            } else {
                Some(dst_str.parse::<usize>().map_err(|_| {
                    PatchParseError::InvalidValue(format!("Invalid index: {value}"))
                })?)
            };
        Ok(PatchOp::DuplicateArrayItem {
            source_index: src,
            target_index: dst,
        })
    } else {
        Err(PatchParseError::InvalidValue(format!(
            "Invalid duplicateArrayItem format: {value}"
        )))
    }
}

fn parse_object_ref(value: &str) -> Result<PatchOp, PatchParseError> {
    // Format: "'Name':'Path'"
    let value = value.trim();
    if let Some(colon_pos) = value.find("':'") {
        let obj_name = value[1..colon_pos].to_string();
        let obj_path = value[colon_pos + 2..].trim_end_matches('\'').to_string();
        Ok(PatchOp::ObjectRef {
            object_name: obj_name,
            object_path: obj_path,
        })
    } else {
        Err(PatchParseError::InvalidValue(format!(
            "Invalid objectRef format: {value}"
        )))
    }
}

fn split_value(value: &str) -> Result<(String, String), PatchParseError> {
    let value = value.trim();
    if let Some(colon_pos) = value.find(':') {
        Ok((
            value[..colon_pos].to_string(),
            value[colon_pos + 1..].trim().to_string(),
        ))
    } else {
        Err(PatchParseError::InvalidValue(format!(
            "Expected 'Type:value' format, got: {value}"
        )))
    }
}

fn parse_arithmetic(s: &str) -> Result<ArithmeticOp, PatchParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(PatchParseError::InvalidValue("Empty arithmetic".into()));
    }
    let op_char = s.chars().next().unwrap();
    let num_str = s[1..].trim();
    let num: f64 = num_str
        .parse()
        .map_err(|_| PatchParseError::InvalidValue(format!("Invalid number: {num_str}")))?;

    match op_char {
        '+' => Ok(ArithmeticOp::Add(num)),
        '-' => Ok(ArithmeticOp::Subtract(num)),
        '*' => Ok(ArithmeticOp::Multiply(num)),
        '/' => Ok(ArithmeticOp::Divide(num)),
        _ => Err(PatchParseError::InvalidValue(format!(
            "Unknown operator '{op_char}' in {s}"
        ))),
    }
}

fn parse_range(s: &str) -> Result<Option<ValueRange>, String> {
    let s = s.trim().trim_start_matches('(').trim_end_matches(')');
    if s.is_empty() {
        return Ok(None);
    }
    if let Some(dash_pos) = s.find('-') {
        let min: f64 = s[..dash_pos]
            .parse()
            .map_err(|e| format!("Invalid range min: {e}"))?;
        let max: f64 = s[dash_pos + 1..]
            .parse()
            .map_err(|e| format!("Invalid range max: {e}"))?;
        Ok(Some(ValueRange { min, max }))
    } else {
        Err("Range must have min-max format".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_property_value_bool() {
        let op = parse_patch_value("propertyValue", "BoolProperty:true").unwrap();
        match op {
            PatchOp::PropertyValue { value_type, value } => {
                assert_eq!(value_type, "BoolProperty");
                assert_eq!(value, "true");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_modify_multiply() {
        let op = parse_patch_value("modifyPropertyValue", "FloatProperty:*2").unwrap();
        match op {
            PatchOp::ModifyPropertyValue {
                value_type,
                operation,
                range,
            } => {
                assert_eq!(value_type.unwrap(), "FloatProperty");
                assert!(matches!(operation, ArithmeticOp::Multiply(v) if v == 2.0));
                assert!(range.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_modify_with_range() {
        let op = parse_patch_value("modifyPropertyValue", "IntProperty:+6(0-10)").unwrap();
        match op {
            PatchOp::ModifyPropertyValue { range, .. } => {
                let r = range.unwrap();
                assert_eq!(r.min, 0.0);
                assert_eq!(r.max, 10.0);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_array_property() {
        let op = parse_patch_value("arrayPropertyValue", "IntProperty:[2,2,4,1]").unwrap();
        match op {
            PatchOp::ArrayPropertyValue {
                value_type,
                elements,
                mode,
            } => {
                assert_eq!(value_type, "IntProperty");
                assert_eq!(elements, vec!["2", "2", "4", "1"]);
                assert_eq!(mode, ArrayMode::Replace);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_array_append() {
        let op = parse_patch_value("arrayPropertyValue", "IntProperty:+[2]").unwrap();
        match op {
            PatchOp::ArrayPropertyValue { mode, .. } => {
                assert_eq!(mode, ArrayMode::Append);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_full_patch() {
        let parsed = parse_patch(
            "datatable:{'BaseStats*'}.{'CanUseAoA*'}",
            "propertyValue",
            "BoolProperty:true",
        )
        .unwrap();
        assert_eq!(parsed.fragments.len(), 2);
        match parsed.operation {
            PatchOp::PropertyValue { value_type, value } => {
                assert_eq!(value_type, "BoolProperty");
                assert_eq!(value, "true");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_duplicate_entry() {
        let op = parse_patch_value("duplicateEntry", "'MSSL'>'MSTM'").unwrap();
        match op {
            PatchOp::DuplicateEntry { source, target } => {
                assert_eq!(source, "MSSL");
                assert_eq!(target, "MSTM");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_parse_object_ref() {
        let op = parse_patch_value(
            "objectRef",
            "'F16Custom_01':'/Game/Assets/Objects/Aircraft/F16C/Textures/Skin/F16Custom_01'",
        )
        .unwrap();
        match op {
            PatchOp::ObjectRef {
                object_name,
                object_path,
            } => {
                assert_eq!(object_name, "F16Custom_01");
                assert!(object_path.contains("F16Custom_01"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_unknown_type() {
        let result = parse_patch_value("unknownType", "foo:bar");
        assert!(result.is_err());
    }
}
