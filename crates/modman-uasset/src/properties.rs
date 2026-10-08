//! Legacy property shapes used by the pre-parity merge engine.
//!
//! NOTE: superseded by [`crate::walk`] for real asset work — the walker produces
//! the full navigable tree with byte ranges. These types remain only so
//! `modman-core`'s in-progress patch engine keeps compiling while the A-layer
//! (fragments/merge) is rebuilt on top of the walker.

use crate::names::FName;

/// A tagged property with its name and type
#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    pub type_name: String,
    pub value: PropertyValue,
    pub size: i32,
}

/// Value variants for the legacy property shape.
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
