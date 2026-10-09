//! Same-size patch application (M3) — resolve asset patches against a walked
//! DataTable, compute the replacement bytes, and splice them into the uexp.
//!
//! Same-size only: length-changing operations (row duplication, text edits,
//! array inserts) are deliberately rejected here and handled by the
//! length-changing milestone. Rejecting is safe — nothing is written unless
//! every edit fits its existing byte span exactly.

use crate::manifest::{Patch, WingmanMod};
use crate::patch::{parse_patch_value, PatchOp};
use crate::resolver::{resolve, Node};
use modman_uasset::edit::splice;
use modman_uasset::walk::DataTable;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("template parse error: {0}")]
    Parse(String),
    #[error("value parse error: {0}")]
    Value(String),
    #[error("unsupported patch (not same-size yet): {0}")]
    Unsupported(String),
    #[error("splice error: {0}")]
    Splice(String),
    #[error("asset error: {0}")]
    Asset(String),
}

/// One planned same-size edit: overwrite `span` with `bytes`.
#[derive(Debug, Clone)]
pub struct PlannedEdit {
    pub description: String,
    pub patch_type: String,
    pub target: String,
    pub span: (usize, usize),
    pub bytes: Vec<u8>,
}

/// Plan every same-size edit a mod applies to one target file key
/// (e.g. `ProjectWingman/Content/.../DB_Aircraft.uexp`).
pub fn plan_same_size_edits(
    dt: &DataTable,
    modm: &WingmanMod,
    target: &str,
) -> Result<Vec<PlannedEdit>, ApplyError> {
    // The C# applies Liquid templating at load time; mirror that here.
    let mut substituted = modm.clone();
    crate::template::apply_variables_to_mod(&mut substituted);
    let mut out = Vec::new();
    let Some(sets) = substituted.asset_patches.get(target) else {
        return Ok(out);
    };
    for set in sets {
        for patch in &set.patches {
            plan_patch(dt, patch, &mut out)?;
        }
    }
    Ok(out)
}

fn plan_patch(dt: &DataTable, patch: &Patch, out: &mut Vec<PlannedEdit>) -> Result<(), ApplyError> {
    let ctx = crate::fragment::parse_template(&patch.template)
        .map_err(|e| ApplyError::Parse(e.to_string()))?;
    let nodes = resolve(dt, &ctx.fragments);
    let op = parse_patch_value(&patch.patch_type, &patch.value)
        .map_err(|e| ApplyError::Value(e.to_string()))?;

    match op {
        PatchOp::PropertyValue { value_type, value } => {
            for node in nodes {
                let Some(span) = node.value_span() else {
                    return Err(ApplyError::Unsupported(format!(
                        "no editable span for {} (patch: {})",
                        node.describe(),
                        patch.description
                    )));
                };
                let bytes = encode_value(&value_type, &value, &node, span)?;
                out.push(PlannedEdit {
                    description: patch.description.clone(),
                    patch_type: patch.patch_type.clone(),
                    target: node.describe(),
                    span,
                    bytes,
                });
            }
        }
        _ => {
            return Err(ApplyError::Unsupported(format!(
                "patch type '{}' ({})",
                patch.patch_type, patch.description
            )))
        }
    }
    Ok(())
}

/// Encode a `propertyValue` replacement and enforce the same-size constraint.
fn encode_value(
    value_type: &str,
    value: &str,
    node: &Node<'_>,
    span: (usize, usize),
) -> Result<Vec<u8>, ApplyError> {
    if node.type_name() != value_type {
        return Err(ApplyError::Value(format!(
            "value type {value_type} does not match target type {} ({})",
            node.type_name(),
            node.describe()
        )));
    }
    let bytes = encode_scalar(value_type, value)?;
    let span_len = span.1 - span.0;
    if bytes.len() != span_len {
        return Err(ApplyError::Unsupported(format!(
            "length-changing edit on {}: span is {span_len} bytes, replacement is {} bytes",
            node.describe(),
            bytes.len()
        )));
    }
    Ok(bytes)
}

/// Encode a scalar patch value into its raw bytes (no size checks).
pub(crate) fn encode_scalar(value_type: &str, value: &str) -> Result<Vec<u8>, ApplyError> {
    Ok(match value_type {
        "BoolProperty" => match value.to_ascii_lowercase().as_str() {
            "true" => vec![1u8],
            "false" => vec![0u8],
            other => return Err(ApplyError::Value(format!("invalid bool value '{other}'"))),
        },
        "IntProperty" => value
            .parse::<i32>()
            .map_err(|e| ApplyError::Value(format!("invalid int '{value}': {e}")))?
            .to_le_bytes()
            .to_vec(),
        "FloatProperty" => value
            .parse::<f32>()
            .map_err(|e| ApplyError::Value(format!("invalid float '{value}': {e}")))?
            .to_le_bytes()
            .to_vec(),
        "StrProperty" => {
            let mut v = value.as_bytes().to_vec();
            v.push(0);
            v
        }
        "ByteProperty" => vec![value
            .parse::<u8>()
            .map_err(|e| ApplyError::Value(format!("invalid byte '{value}': {e}")))?],
        other => {
            return Err(ApplyError::Unsupported(format!(
                "value type '{other}' (same-size encoder)"
            )))
        }
    })
}

/// Apply planned edits to a copy of the uexp payload.
pub fn apply_edits(uexp: &[u8], edits: &[PlannedEdit]) -> Result<Vec<u8>, ApplyError> {
    let mut out = uexp.to_vec();
    for e in edits {
        splice(&mut out, e.span, &e.bytes).map_err(|err| ApplyError::Splice(err.to_string()))?;
    }
    Ok(out)
}

/// Plan + apply a mod's same-size edits for one target file.
pub fn apply_mod_same_size(
    dt: &DataTable,
    uexp: &[u8],
    modm: &WingmanMod,
    target: &str,
) -> Result<(Vec<u8>, Vec<PlannedEdit>), ApplyError> {
    let edits = plan_same_size_edits(dt, modm, target)?;
    let out = apply_edits(uexp, &edits)?;
    Ok((out, edits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse_preset_json;

    const TARGET: &str =
        "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp";

    fn stem() -> &'static str {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modman-uasset/tests/fixtures/DB_Aircraft"
        )
    }

    fn vanilla() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modman-uasset/tests/fixtures/DB_Aircraft.uexp"
        ))
        .unwrap()
    }

    fn spear_mod() -> WingmanMod {
        let json = r#"{
            "version": 1,
            "mods": [{
                "_id": "",
                "_sicario": { "private": false, "preview": false, "overwrites": false, "enableSteps": {} },
                "_inputs": [],
                "_vars": {},
                "assetPatches": {
                    "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp": [{
                        "name": "SPEAR loadout",
                        "patches": [
                            {
                                "version": 1,
                                "description": "Unfix the SPEAR's loadout",
                                "template": "datatable:['SPEAR'].{'FixedLoadout*'}",
                                "value": "BoolProperty:false",
                                "type": "propertyValue"
                            },
                            {
                                "version": 1,
                                "description": "Change third slot to visible railgun",
                                "template": "datatable:['SPEAR'].{'HardpointCompatibilityList*'}.[[3]].<StrProperty='rgps'>",
                                "value": "StrProperty:'rgpd'",
                                "type": "propertyValue"
                            }
                        ]
                    }]
                },
                "_meta": { "displayName": "SPEAR Unlock" },
                "filePatches": {}
            }]
        }"#;
        parse_preset_json(json).unwrap().mods.remove(0)
    }

    #[test]
    fn spear_same_size_edits_match_expected_bytes() {
        let dt = DataTable::load(stem()).unwrap();
        let van = vanilla();
        let (out, edits) = apply_mod_same_size(&dt, &van, &spear_mod(), TARGET).unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].span, (83882, 83883));
        assert_eq!(edits[0].bytes, vec![0u8]);
        assert_eq!(edits[1].span, (83714, 83719));
        assert_eq!(edits[1].bytes, b"rgpd\0");

        let diffs: Vec<usize> = (0..van.len()).filter(|&i| van[i] != out[i]).collect();
        assert_eq!(diffs, vec![83717, 83882]);
        assert_eq!(out[83717], b'd');
        assert_eq!(out[83882], 0);
    }

    /// Full byte-parity against the banked C# merger output (when present).
    #[test]
    fn spear_matches_oracle_bytes() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let oracle = std::path::Path::new(&home)
            .join("modding/project-wingman/sicario-oracle/run1-553-spear/oracle-DB_Aircraft.uexp");
        if !oracle.exists() {
            return;
        }
        let oracle_bytes = std::fs::read(&oracle).unwrap();
        let dt = DataTable::load(stem()).unwrap();
        let van = vanilla();
        let (out, _) = apply_mod_same_size(&dt, &van, &spear_mod(), TARGET).unwrap();
        assert_eq!(out.len(), oracle_bytes.len());
        assert!(
            out == oracle_bytes,
            "our output differs from the C# merger output"
        );
    }

    #[test]
    fn length_changing_is_rejected() {
        let dt = DataTable::load(stem()).unwrap();
        let van = vanilla();
        let json = r#"{
            "version": 1,
            "mods": [{
                "_id": "",
                "_vars": {},
                "assetPatches": {
                    "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp": [{
                        "name": "bad",
                        "patches": [{
                            "description": "grow the string",
                            "template": "datatable:['SPEAR'].{'HardpointCompatibilityList*'}.[[3]]",
                            "value": "StrProperty:'rgpd-extra'",
                            "type": "propertyValue"
                        }]
                    }]
                },
                "filePatches": {}
            }]
        }"#;
        let m = parse_preset_json(json).unwrap().mods.remove(0);
        let err = apply_mod_same_size(&dt, &van, &m, TARGET).unwrap_err();
        assert!(
            matches!(err, ApplyError::Unsupported(_)),
            "expected Unsupported, got {err:?}"
        );
    }

    #[test]
    fn unsupported_patch_types_rejected() {
        let dt = DataTable::load(stem()).unwrap();
        let van = vanilla();
        let json = r#"{
            "version": 1,
            "mods": [{
                "_id": "",
                "_vars": {},
                "assetPatches": {
                    "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp": [{
                        "name": "dup",
                        "patches": [{
                            "description": "duplicate a row",
                            "template": "datatable:[*]",
                            "value": "'ACG-01'>'ACG-01X'",
                            "type": "duplicateEntry"
                        }]
                    }]
                },
                "filePatches": {}
            }]
        }"#;
        let m = parse_preset_json(json).unwrap().mods.remove(0);
        let err = apply_mod_same_size(&dt, &van, &m, TARGET).unwrap_err();
        assert!(matches!(err, ApplyError::Unsupported(_)));
    }
}
