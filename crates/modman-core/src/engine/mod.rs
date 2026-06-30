//! Patch application engine — matches fragments to uasset properties
//! and applies modifications.
//!
//! Pipeline:
//! 1. Parse fragment chain from template string
//! 2. Apply fragments to filter/select properties from a uasset export
//! 3. For each matching property, apply the patch operation
//! 4. Return modified property data ready for serialization

use crate::fragment::Fragment;
use crate::manifest::WingmanMod;
use crate::patch::{parse_patch_value, PatchOp};
use modman_uasset::properties::{Property, PropertyValue};
use std::collections::HashMap;

/// A single patch to apply with its fragments and operation
#[derive(Debug, Clone)]
pub struct CompiledPatch {
    pub fragments: Vec<Fragment>,
    pub operation: PatchOp,
    pub target_file: String,
}

/// The result of applying all patches to a set of properties
#[derive(Debug)]
pub struct PatchResult {
    pub modified_properties: Vec<Modification>,
}

/// A single property modification
#[derive(Debug)]
pub struct Modification {
    pub property_name: String,
    pub original_value: String,
    pub new_value: String,
}

/// Compile a WingmanMod into a list of ready-to-apply patches
pub fn compile_mod(modm: &WingmanMod) -> Result<Vec<CompiledPatch>, String> {
    let mut patches = Vec::new();

    for (target_file, patch_sets) in &modm.asset_patches {
        for set in patch_sets {
            for patch_def in &set.patches {
                // Parse the template into fragments
                let ctx = crate::fragment::parse_template(&patch_def.template).map_err(|e| {
                    format!("Template parse error in '{}': {e}", patch_def.description)
                })?;

                // Parse the value into a patch operation
                let operation = parse_patch_value(&patch_def.patch_type, &patch_def.value)
                    .map_err(|e| {
                        format!("Value parse error in '{}': {e}", patch_def.description)
                    })?;

                patches.push(CompiledPatch {
                    fragments: ctx.fragments,
                    operation,
                    target_file: target_file.clone(),
                });
            }
        }
    }

    Ok(patches)
}

/// Apply a fragment chain to a list of properties to find matching ones.
/// Each fragment narrows the result set.
pub fn match_fragments<'a>(
    properties: &'a [Property],
    fragments: &[Fragment],
) -> Vec<&'a Property> {
    if fragments.is_empty() {
        return properties.iter().collect();
    }

    let mut current: Vec<&'a Property> = properties.iter().collect();

    for fragment in fragments {
        current = apply_fragment(&current, fragment);
        if current.is_empty() {
            break;
        }
    }

    current
}

fn apply_fragment<'a>(input: &[&'a Property], fragment: &Fragment) -> Vec<&'a Property> {
    match fragment {
        Fragment::StructName { name, invert } => {
            let pattern = name.trim_end_matches('*');
            let is_wildcard = name.ends_with('*');
            input
                .iter()
                .filter(|p| {
                    let matches = if is_wildcard {
                        p.name.starts_with(pattern)
                    } else {
                        p.name == *name
                    };
                    matches ^ invert
                })
                .copied()
                .collect()
        }
        Fragment::ArrayIndex(index) => input.get(*index).map(|p| vec![*p]).unwrap_or_default(),
        Fragment::StructProperty(name) => {
            // Return children of struct properties matching the name
            let pattern = name.trim_end_matches('*');
            let is_wildcard = name.ends_with('*');
            input
                .iter()
                .filter_map(|p| match &p.value {
                    PropertyValue::Struct { properties, .. } => Some(
                        properties
                            .iter()
                            .filter(|child| {
                                if is_wildcard {
                                    child.name.starts_with(pattern)
                                } else {
                                    child.name == *name
                                }
                            })
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                })
                .flatten()
                .collect()
        }
        Fragment::PropertyType(type_name) => input
            .iter()
            .filter(|p| p.type_name == *type_name)
            .copied()
            .collect(),
        Fragment::PropertyValue { prop_type, value } => {
            input
                .iter()
                .filter(|p| {
                    if p.type_name != *prop_type {
                        return false;
                    }
                    // Match value
                    match &p.value {
                        PropertyValue::Int(v) => value.parse::<i32>().ok() == Some(*v),
                        PropertyValue::Float(v) => {
                            (v - value.parse::<f32>().unwrap_or(f32::MAX)).abs() < 0.001
                        }
                        PropertyValue::Bool(v) => value == "true" && *v || value == "false" && !*v,
                        PropertyValue::Str(s) => s == value,
                        _ => false,
                    }
                })
                .copied()
                .collect()
        }
        Fragment::Any => input.to_vec(),
        _ => {
            // Other fragment types (EnumValue, StructMatch, Flatten, ArrayPropertyIndex, ArrayFlatten)
            // are not yet implemented in the matcher
            input.to_vec()
        }
    }
}

/// Apply a patch operation to matching properties.
/// Returns a list of modifications made.
pub fn apply_patch(matches: &[&Property], operation: &PatchOp) -> Vec<Modification> {
    let mut modifications = Vec::new();

    for prop in matches {
        let original = format!("{:?}", prop.value);
        let new_value = compute_new_value(&prop.value, operation);

        modifications.push(Modification {
            property_name: prop.name.clone(),
            original_value: original,
            new_value,
        });
    }

    modifications
}

fn compute_new_value(_current: &PropertyValue, _operation: &PatchOp) -> String {
    // TODO: Actually modify the property value
    // For now, report what would be done
    match _operation {
        PatchOp::PropertyValue { value_type, value } => {
            format!("set {} = {}", value_type, value)
        }
        PatchOp::ModifyPropertyValue { operation, .. } => {
            format!("modify with {:?}", operation)
        }
        _ => "patch applied (stub)".to_string(),
    }
}

/// Build a patch execution plan from a WingmanMod
pub fn plan_build(modm: &WingmanMod) -> Result<BuildPlan, String> {
    let patches = compile_mod(modm)?;

    // Group patches by target file
    let mut file_patches: HashMap<String, Vec<CompiledPatch>> = HashMap::new();
    for patch in patches {
        file_patches
            .entry(patch.target_file.clone())
            .or_default()
            .push(patch);
    }

    Ok(BuildPlan { file_patches })
}

/// A complete build plan describing what needs to be done
#[derive(Debug)]
pub struct BuildPlan {
    pub file_patches: HashMap<String, Vec<CompiledPatch>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::WingmanMod;

    #[test]
    fn test_compile_simple_mod() {
        let json = r#"{
            "_id": "test",
            "assetPatches": {
                "test.uasset": [{
                    "name": "test",
                    "patches": [{
                        "template": "datatable:{'BaseStats*'}.{'CanUseAoA*'}",
                        "value": "BoolProperty:true",
                        "type": "propertyValue"
                    }]
                }]
            },
            "filePatches": {}
        }"#;
        let m: WingmanMod = serde_json::from_str(json).unwrap();
        let plan = plan_build(&m).unwrap();
        assert_eq!(plan.file_patches.len(), 1);
        let patches = &plan.file_patches["test.uasset"];
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].fragments.len(), 2);
    }

    #[test]
    fn test_match_struct_name() {
        let props = vec![
            Property {
                name: "F-15C".into(),
                type_name: "StructProperty".into(),
                value: PropertyValue::Struct {
                    struct_type: "RowStruct".into(),
                    properties: vec![],
                },
                size: 0,
            },
            Property {
                name: "T-21".into(),
                type_name: "StructProperty".into(),
                value: PropertyValue::Struct {
                    struct_type: "RowStruct".into(),
                    properties: vec![],
                },
                size: 0,
            },
        ];

        let matches = match_fragments(
            &props,
            &[Fragment::StructName {
                name: "F-15C".into(),
                invert: false,
            }],
        );
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "F-15C");
    }

    #[test]
    fn test_match_property_type() {
        let props = vec![
            Property {
                name: "MaxSpeed".into(),
                type_name: "FloatProperty".into(),
                value: PropertyValue::Float(2500.0),
                size: 4,
            },
            Property {
                name: "Count".into(),
                type_name: "IntProperty".into(),
                value: PropertyValue::Int(5),
                size: 4,
            },
        ];

        let matches = match_fragments(&props, &[Fragment::PropertyType("FloatProperty".into())]);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "MaxSpeed");
    }
}
