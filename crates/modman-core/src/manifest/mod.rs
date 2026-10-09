//! Sicario mod/preset JSON models — mirrors the C# `WingmanMod` / `WingmanPreset`
//! / build-request ("_meta/sicario/*.json") model set, plus the lenient parsing
//! layer the C# loader applies (`PropertyNameCaseInsensitive = true`,
//! `ReadCommentHandling.Skip`, `AllowTrailingCommas = true`).
//!
//! Key handling: keys are normalized to lowercase before typed deserialization,
//! EXCEPT inside data maps (asset/file patch target paths, `_vars`,
//! `modParameters`, `enableSteps`) whose keys are data and must be preserved.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Deserialize helper: treat explicit `null` as `Default` — the wild corpus
/// contains `null` for optional string fields (`group`, `description`, ...).
fn de_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Root mod document — matches Sicario's `.dtm` / embedded mod format.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WingmanMod {
    #[serde(default, rename = "_id", deserialize_with = "de_null_default")]
    pub id: String,

    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<ModMeta>,

    #[serde(default, rename = "_sicario")]
    pub sicario: SicarioMetadata,

    #[serde(default, rename = "_vars")]
    pub variables: HashMap<String, String>,

    #[serde(default, rename = "_inputs")]
    pub inputs: Vec<PatchParameter>,

    /// DataTable/asset patches: target file path → patch sets.
    #[serde(default, rename = "assetpatches")]
    pub asset_patches: HashMap<String, Vec<PatchSet>>,

    /// Raw/hex file patches: target file path → hex patch sets.
    #[serde(default, rename = "filepatches")]
    pub file_patches: HashMap<String, Vec<FilePatchSet>>,
}

impl WingmanMod {
    /// Total number of patches across asset + file patch sets.
    pub fn patch_count(&self) -> usize {
        let a: usize = self
            .asset_patches
            .values()
            .flat_map(|sets| sets.iter())
            .map(|s| s.patches.len())
            .sum();
        let f: usize = self
            .file_patches
            .values()
            .flat_map(|sets| sets.iter())
            .map(|s| s.patches.len())
            .sum();
        a + f
    }

    /// C# `ModParser.IsValid`: has at least one asset or file patch.
    pub fn is_valid(&self) -> bool {
        !self.asset_patches.is_empty() || !self.file_patches.is_empty()
    }

    /// Human label: `_meta.displayName`, else `_id`, else "(unnamed)".
    pub fn label(&self) -> String {
        if let Some(meta) = &self.meta {
            if !meta.display_name.is_empty() {
                return meta.display_name.clone();
            }
        }
        if !self.id.is_empty() {
            return self.id.clone();
        }
        "(unnamed)".to_string()
    }
}

/// User-facing mod metadata (`_meta`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModMeta {
    #[serde(default, rename = "displayname", deserialize_with = "de_null_default")]
    pub display_name: String,
    #[serde(default, deserialize_with = "de_null_default")]
    pub author: String,
    #[serde(default, deserialize_with = "de_null_default")]
    pub description: String,
}

/// Sicario-specific metadata (`_sicario`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SicarioMetadata {
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub overwrites: bool,
    #[serde(default, deserialize_with = "de_null_default")]
    pub group: String,
    #[serde(default)]
    pub preview: bool,
    /// Step name → Liquid expression evaluated against `inputs.*`
    /// (e.g. `{{ inputs.payoutOverwrite }}`).
    #[serde(default, rename = "enablesteps")]
    pub enable_steps: HashMap<String, String>,
}

/// A user-facing parameter for the mod (`_inputs` entry).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PatchParameter {
    #[serde(default, deserialize_with = "de_null_default")]
    pub id: String,
    #[serde(default, deserialize_with = "de_null_default")]
    pub message: String,
    #[serde(default, rename = "type", deserialize_with = "de_null_default")]
    pub param_type: String,
    #[serde(default, deserialize_with = "de_null_default")]
    pub default: String,
    /// Optional input constraints (hosted-app UI metadata; tolerated, not used).
    #[serde(default)]
    pub range: Option<Value>,
    #[serde(default)]
    pub pattern: Option<Value>,
}

/// A named set of asset patches targeting one file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PatchSet {
    #[serde(default, deserialize_with = "de_null_default")]
    pub name: String,
    #[serde(default)]
    pub patches: Vec<Patch>,
}

/// A single asset patch operation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Patch {
    #[serde(default, deserialize_with = "de_null_default")]
    pub description: String,
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default, deserialize_with = "de_null_default")]
    pub template: String,
    #[serde(default, deserialize_with = "de_null_default")]
    pub value: String,
    #[serde(default, rename = "type", deserialize_with = "de_null_default")]
    pub patch_type: String,
}

/// A named set of raw/hex file patches targeting one file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FilePatchSet {
    #[serde(default, deserialize_with = "de_null_default")]
    pub name: String,
    #[serde(default)]
    pub patches: Vec<FilePatch>,
}

/// A single hex patch (HexPatch format).
///
/// NOTE: the local C# merger binds the substitution from the JSON field
/// `value` (ModEngine.Core `Patch.Value`); doc/hosted-style `"substitution"`
/// fields are silently ignored, which turns the replacement into an empty
/// write. This model mirrors that contract exactly.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FilePatch {
    #[serde(default, deserialize_with = "de_null_default")]
    pub description: String,
    /// Hex string to search for (e.g. `"00 48 02"`).
    #[serde(default)]
    pub template: Option<String>,
    /// Hex string to substitute (e.g. `"01"`). Bound from the `value` field.
    #[serde(default)]
    pub value: Option<String>,
    /// Patch mode: `before`, `inPlace`, `valueBefore`; anything else (e.g.
    /// `none`) is ignored by the engine.
    #[serde(default, rename = "type")]
    pub patch_type: Option<String>,
    /// Optional window anchors for scoped matching.
    #[serde(default)]
    pub window: Option<HexWindow>,
}

/// HexPatch match window anchors.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HexWindow {
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default, rename = "maxMatches", alias = "maxmatches")]
    pub max_matches: Option<i64>,
}

/// A preset file (`.dtp`): parameters + one or more mods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WingmanPreset {
    #[serde(default = "default_version")]
    pub version: i64,
    #[serde(default, rename = "engineversion")]
    pub engine_version: Option<String>,
    #[serde(default, rename = "modparameters")]
    pub mod_parameters: HashMap<String, String>,
    #[serde(default)]
    pub mods: Vec<WingmanMod>,
}

impl Default for WingmanPreset {
    fn default() -> Self {
        Self {
            version: 1,
            engine_version: None,
            mod_parameters: HashMap::new(),
            mods: Vec::new(),
        }
    }
}

fn default_version() -> i64 {
    1
}

/// A built-mod request file (`ProjectWingman/_meta/sicario/<id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetaRequest {
    #[serde(default)]
    pub app: AppInfo,
    #[serde(default)]
    pub request: BuildRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppInfo {
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuildRequest {
    #[serde(default, deserialize_with = "de_null_default")]
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "username")]
    pub user_name: Option<String>,
    #[serde(default)]
    pub mods: Vec<WingmanMod>,
}

// ---------------------------------------------------------------------------
// Lenient parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum KeyMode {
    /// Lowercase this object's keys; decide child mode per key.
    Normalize,
    /// Preserve this object's keys (data map); children are structs again.
    PreserveKeys,
    /// Preserve everything below (leaf data maps whose values are strings).
    PreserveAll,
}

/// Normalize JSON keys for case-insensitive typed parsing.
///
/// Struct keys are lowercased; keys of data maps (patch target paths,
/// `_vars`, `modParameters`, `enableSteps`) are preserved verbatim.
pub fn normalize_json_keys(value: &mut Value) {
    normalize_value(value, KeyMode::Normalize);
}

fn normalize_value(value: &mut Value, mode: KeyMode) {
    match value {
        Value::Object(map) => {
            let old: Map<String, Value> = std::mem::take(map);
            for (k, mut v) in old {
                match mode {
                    KeyMode::PreserveAll => {
                        normalize_value(&mut v, KeyMode::PreserveAll);
                        map.insert(k, v);
                    }
                    KeyMode::PreserveKeys => {
                        normalize_value(&mut v, KeyMode::Normalize);
                        map.insert(k, v);
                    }
                    KeyMode::Normalize => {
                        let lk = k.to_lowercase();
                        let child_mode = match lk.as_str() {
                            "assetpatches" | "filepatches" => KeyMode::PreserveKeys,
                            "_vars" | "modparameters" | "enablesteps" | "variables" => {
                                KeyMode::PreserveAll
                            }
                            _ => KeyMode::Normalize,
                        };
                        normalize_value(&mut v, child_mode);
                        map.insert(lk, v);
                    }
                }
            }
        }
        Value::Array(items) => {
            for it in items.iter_mut() {
                normalize_value(it, mode);
            }
        }
        _ => {}
    }
}

/// Strip `//` and `/* */` comments and trailing commas (outside strings),
/// matching the C# loader's JSON options.
pub fn sanitize_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
                i += 1;
            }
            '/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            '/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            ',' => {
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
                    // trailing comma — drop it
                    i += 1;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn value_from_json(raw: &str) -> Result<Value, serde_json::Error> {
    let mut v: Value = serde_json::from_str(&sanitize_json(raw))?;
    normalize_json_keys(&mut v);
    Ok(v)
}

/// Parse a single mod document (`.dtm` or an embedded mod).
pub fn parse_mod_json(raw: &str) -> Result<WingmanMod, serde_json::Error> {
    serde_json::from_value(value_from_json(raw)?)
}

/// Parse a preset document (`.dtp`).
pub fn parse_preset_json(raw: &str) -> Result<WingmanPreset, serde_json::Error> {
    serde_json::from_value(value_from_json(raw)?)
}

/// Parse a built-mod request document (`_meta/sicario/*.json`).
pub fn parse_meta_request_json(raw: &str) -> Result<MetaRequest, serde_json::Error> {
    serde_json::from_value(value_from_json(raw)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_mod() {
        let m = parse_mod_json(r#"{ "_id": "test-mod", "assetPatches": {} }"#).unwrap();
        assert_eq!(m.id, "test-mod");
        assert!(m.asset_patches.is_empty());
        assert!(!m.is_valid());
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
        let m = parse_mod_json(json).unwrap();
        assert_eq!(m.id, "aoa-unlocker");
        assert_eq!(m.meta.as_ref().unwrap().display_name, "AoA for All");
        let sets = m.asset_patches.values().next().unwrap();
        assert_eq!(sets[0].patches[0].patch_type, "propertyValue");
        assert_eq!(sets[0].patches[0].value, "BoolProperty:true");
        assert!(m.is_valid());
        assert_eq!(m.patch_count(), 1);
    }

    #[test]
    fn case_insensitive_keys_in_the_wild() {
        // The hex-patch docs use `_meta.DisplayName`, `Author`, `FilePatches`.
        let json = r#"{
            "_meta": { "DisplayName": "AoA for All", "Author": "someone", "Description": "d" },
            "FilePatches": {
                "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp": [{
                    "name": "AoA Unlock",
                    "patches": [{
                        "description": "Set CanUseAoA",
                        "template": "00 48 02",
                        "substitution": "01",
                        "type": "before"
                    }]
                }]
            }
        }"#;
        let m = parse_mod_json(json).unwrap();
        assert_eq!(m.meta.as_ref().unwrap().display_name, "AoA for All");
        let sets = m.file_patches.values().next().unwrap();
        // `substitution` does not bind — the C# reads `value` only.
        assert_eq!(sets[0].patches[0].value, None);
        assert_eq!(sets[0].patches[0].template.as_deref(), Some("00 48 02"));
        assert_eq!(sets[0].patches[0].patch_type.as_deref(), Some("before"));
        assert!(m.is_valid());
    }

    #[test]
    fn data_map_keys_are_preserved() {
        let json = r#"{
            "_vars": { "MyVar": "5", "otherVar": "x" },
            "assetPatches": {
                "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp": [{
                    "name": "n",
                    "patches": [{ "template": "datatable:[*]", "value": "IntProperty:1", "type": "propertyValue" }]
                }]
            }
        }"#;
        let m = parse_mod_json(json).unwrap();
        assert!(m.variables.contains_key("MyVar"));
        assert!(m.variables.contains_key("otherVar"));
        assert!(m.asset_patches.contains_key(
            "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp"
        ));
    }

    #[test]
    fn parse_preset() {
        let json = r#"{
            "version": 1,
            "engineVersion": "0.2.0",
            "modParameters": { "payoutFactor": "5" },
            "mods": [{ "_id": "m1", "assetPatches": {} }]
        }"#;
        let p = parse_preset_json(json).unwrap();
        assert_eq!(p.version, 1);
        assert_eq!(p.engine_version.as_deref(), Some("0.2.0"));
        assert_eq!(p.mod_parameters.get("payoutFactor").unwrap(), "5");
        assert_eq!(p.mods.len(), 1);
    }

    #[test]
    fn parse_meta_request() {
        let json = r#"{
            "app": { "owner": "agc93", "name": "SicarioPatch", "version": "1.0.0.0" },
            "request": {
                "id": "63947024efce48c29504727073fc4023",
                "mods": [{ "_id": "", "_vars": { "aircraftName": "ACG-01X" }, "assetPatches": {} }]
            }
        }"#;
        let r = parse_meta_request_json(json).unwrap();
        assert_eq!(r.app.name, "SicarioPatch");
        assert_eq!(r.request.mods.len(), 1);
        assert_eq!(
            r.request.mods[0].variables.get("aircraftName").unwrap(),
            "ACG-01X"
        );
    }

    #[test]
    fn parse_enable_steps_and_inputs() {
        let json = r#"{
            "_inputs": [
                { "id": "payoutFactor", "type": "number", "message": "Payout Multiplier", "default": "10" },
                { "id": "payoutOverwrite", "type": "boolean", "message": "Overwrite?", "default": "true" }
            ],
            "_sicario": {
                "enableSteps": {
                    "ReplaceBase": "{{ inputs.payoutOverwrite | not }}",
                    "ReplaceAny": "{{ inputs.payoutOverwrite }}"
                }
            },
            "assetPatches": {}
        }"#;
        let m = parse_mod_json(json).unwrap();
        assert_eq!(m.inputs.len(), 2);
        assert_eq!(m.inputs[0].id, "payoutFactor");
        assert_eq!(m.inputs[0].param_type, "number");
        assert_eq!(
            m.sicario.enable_steps.get("ReplaceAny").unwrap(),
            "{{ inputs.payoutOverwrite }}"
        );
    }

    #[test]
    fn sanitize_handles_comments_and_trailing_commas() {
        let raw = r#"{
            // line comment
            "a": 1, /* block
            comment */
            "b": [1, 2,],
        }"#;
        let v: Value = serde_json::from_str(&sanitize_json(raw)).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"][1], 2);
    }

    #[test]
    fn sanitize_keeps_comment_like_strings() {
        let raw = r#"{ "url": "https://example.com/path", "s": "a // b /* c */" }"#;
        let v: Value = serde_json::from_str(&sanitize_json(raw)).unwrap();
        assert_eq!(v["url"], "https://example.com/path");
        assert_eq!(v["s"], "a // b /* c */");
    }

    #[test]
    fn round_trip_serde() {
        let m = WingmanMod {
            id: "roundtrip".into(),
            variables: [("foo".into(), "bar".into())].into(),
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&m).unwrap();
        let back: WingmanMod = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "roundtrip");
        assert_eq!(back.variables.get("foo").unwrap(), "bar");
    }
}
