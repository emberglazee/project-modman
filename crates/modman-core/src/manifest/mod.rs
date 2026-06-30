use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root mod manifest — matches Sicario's .dtm JSON format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WingmanMod {
    #[serde(default, rename = "_id")]
    pub id: String,

    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<ModMeta>,

    #[serde(default, rename = "_sicario")]
    pub sicario: SicarioMetadata,

    #[serde(default, rename = "_vars")]
    pub variables: HashMap<String, String>,

    #[serde(default, rename = "_inputs")]
    pub inputs: Vec<PatchParameter>,

    #[serde(default, rename = "assetPatches")]
    pub asset_patches: HashMap<String, Vec<PatchSet>>,

    #[serde(default, rename = "filePatches")]
    pub file_patches: HashMap<String, Vec<PatchSet>>,
}

/// User-facing mod metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModMeta {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
}

/// Sicario-specific metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SicarioMetadata {
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub overwrites: bool,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub preview: bool,
    #[serde(default, rename = "enableSteps")]
    pub enable_steps: HashMap<String, String>,
}

/// A user-facing parameter for the mod
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchParameter {
    pub key: String,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    #[serde(rename = "type")]
    pub param_type: String,
}

/// A named set of patches targeting one file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchSet {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub patches: Vec<Patch>,
}

/// A single patch operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Patch {
    #[serde(default)]
    pub description: String,
    pub template: String,
    pub value: String,
    #[serde(rename = "type")]
    pub patch_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_mod() {
        let json = r#"{
            "_id": "test-mod",
            "assetPatches": {}
        }"#;
        let m: WingmanMod = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "test-mod");
        assert!(m.asset_patches.is_empty());
    }

    #[test]
    fn parse_sicario_example() {
        let json = r#"{
            "_id": "aoa-unlocker",
            "_meta": { "displayName": "AoA for All" },
            "filePatches": {},
            "assetPatches": {
                "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp": [{
                    "name": "AoA Unlock",
                    "patches": [{
                        "description": "Set CanUseAoA to true",
                        "template": "datatable:{'BaseStats*'}.{'CanUseAoA*'}",
                        "value": "BoolProperty:true",
                        "type": "propertyValue"
                    }]
                }]
            }
        }"#;
        let m: WingmanMod = serde_json::from_str(json).unwrap();
        assert_eq!(m.id, "aoa-unlocker");
        assert_eq!(m.meta.unwrap().display_name, "AoA for All");
        let patches = &m.asset_patches.values().next().unwrap()[0].patches;
        assert_eq!(patches[0].patch_type, "propertyValue");
        assert_eq!(patches[0].value, "BoolProperty:true");
    }

    #[test]
    fn round_trip_serde() {
        let m = WingmanMod {
            id: "roundtrip".into(),
            meta: None,
            sicario: SicarioMetadata::default(),
            variables: [("foo".into(), "bar".into())].into(),
            inputs: vec![],
            asset_patches: HashMap::new(),
            file_patches: HashMap::new(),
        };
        let json = serde_json::to_string_pretty(&m).unwrap();
        let back: WingmanMod = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "roundtrip");
        assert_eq!(back.variables.get("foo").unwrap(), "bar");
    }
}
