//! Merge component assembly — ports the C# provider model
//! (`EmbeddedResourceProvider`, `LoosePresetProvider`, `SkinMergeProvider`).
//!
//! Components and their C# priorities:
//! * `embeddedPresets`  (P1) — presets embedded in paks (`Content/sicario/*.dtp`)
//! * `loosePresets`     (P2) — `*.dtp` files found in the search paths
//! * `sicarioRequests`  (P3) — build requests embedded in paks (`_meta/sicario/*.json`)
//! * `customSkins`      (P4) — PSM skin slot merge (runs last, empirically)
//!
//! Mods run in ascending priority (P1 first). Parameters merge in DESCENDING
//! priority, later-in-sequence overwriting — so embedded presets win, then
//! loose presets, then requests. Within a component the C# quirks are kept:
//! presets fold later-wins, requests fold earlier-wins.

use crate::discovery::{ComponentKind, ScanReport};
use crate::manifest::{parse_preset_json, WingmanMod};
use crate::templating::Vars;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One merge component, mirroring `ModEngine.Merge.MergeComponent`.
#[derive(Debug)]
pub struct MergeComponent {
    pub name: &'static str,
    pub priority: i32,
    pub mods: Vec<WingmanMod>,
    pub params: Vars,
    /// resource key (pak/file path) -> `;`-joined mod labels.
    pub resources: BTreeMap<String, String>,
    pub message: String,
}

/// The C# engine version reported by the loader (`IEngineInfoProvider`).
pub const ENGINE_VERSION: &str = "0.3.0";

/// `IsSupportedBy`: presets declare an engine version; blank on either side
/// passes; otherwise the engine must be >= the requested version.
pub fn preset_supported(preset_engine: Option<&str>, engine: &str) -> bool {
    let (Some(p), e) = (preset_engine, engine) else {
        return true;
    };
    if p.trim().is_empty() || e.trim().is_empty() {
        return true;
    }
    let (Some(pv), Some(ev)) = (parse_version(p), parse_version(e)) else {
        return false;
    };
    ev == (0, 0, 0) || ev >= pv
}

/// Lenient version parse: leading dotted numerics, trailing `+hash` ignored
/// (the C# engine string is `0.3.0+<commit>`).
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    let numeric: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let parts: Vec<u64> = numeric
        .split('.')
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().ok())
        .collect::<Option<Vec<_>>>()?;
    match parts.len() {
        0 => None,
        1 => Some((parts[0], 0, 0)),
        2 => Some((parts[0], parts[1], 0)),
        _ => Some((parts[0], parts[1], parts[2])),
    }
}

fn labels_joined(mods: &[WingmanMod]) -> String {
    mods.iter().map(|m| m.label()).collect::<Vec<_>>().join(";")
}

/// Build the `loosePresets` component from `.dtp` files (path-sorted).
///
/// Returns the component plus warnings for skipped presets.
pub fn loose_component(files: &[PathBuf], engine: &str) -> (MergeComponent, Vec<String>) {
    let mut warnings = Vec::new();
    let mut mods = Vec::new();
    let mut params = Vars::new();
    let mut resources = BTreeMap::new();
    let mut sorted: Vec<&PathBuf> = files.iter().collect();
    sorted.sort();
    let mut loaded = 0usize;
    for path in sorted {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(preset) = parse_preset_json(&text) else {
            continue;
        };
        if preset.mods.is_empty() {
            continue;
        }
        if !preset_supported(preset.engine_version.as_deref(), engine) {
            warnings.push(format!(
                "Incompatible embed! This preset is not supported by the current engine version and will not be loaded: {}",
                path.display()
            ));
            continue;
        }
        loaded += 1;
        resources.insert(
            path.to_string_lossy().to_string(),
            labels_joined(&preset.mods),
        );
        // Preset params fold later-wins (MergeLeft(total, next)).
        for (k, v) in &preset.mod_parameters {
            params.insert(k.clone(), v.clone());
        }
        mods.extend(preset.mods);
    }
    (
        MergeComponent {
            name: "loosePresets",
            priority: 2,
            mods,
            params,
            resources,
            message: format!("Loaded {loaded} loose presets from file."),
        },
        warnings,
    )
}

/// Build the `embeddedPresets` (P1) and `sicarioRequests` (P3) components from
/// a pak scan. A pak containing a preset record never contributes a request
/// (C# precedence).
pub fn embedded_components(scan: &ScanReport, engine: &str) -> (Vec<MergeComponent>, Vec<String>) {
    let mut warnings = Vec::new();

    let mut preset_mods = Vec::new();
    let mut preset_params = Vars::new();
    let mut preset_resources = BTreeMap::new();
    let mut preset_count = 0usize;

    let mut req_mods = Vec::new();
    let mut req_params = Vars::new();
    let mut req_resources = BTreeMap::new();
    let mut req_count = 0usize;

    // Scan order: paks sorted by path (GetAllMods order).
    let mut components: Vec<&crate::discovery::DiscoveredComponent> =
        scan.components.iter().collect();
    components.sort_by(|a, b| a.pak_path.cmp(&b.pak_path));

    let mut paks_with_presets: Vec<PathBuf> = Vec::new();
    for c in &components {
        if c.kind == ComponentKind::Preset {
            paks_with_presets.push(c.pak_path.clone());
        }
    }

    for c in &components {
        let key = c.pak_path.to_string_lossy().to_string();
        match c.kind {
            ComponentKind::Preset => {
                if !preset_supported(c.engine_version.as_deref(), engine) {
                    warnings.push(format!(
                        "Incompatible embed! This preset is not supported by the current engine version and will not be loaded: {key}"
                    ));
                    continue;
                }
                preset_count += 1;
                preset_resources.insert(key, labels_joined(&c.mods));
                // Later pak wins.
                for (k, v) in &c.params {
                    preset_params.insert(k.clone(), v.clone());
                }
                preset_mods.extend(c.mods.clone());
            }
            ComponentKind::BuildRequest => {
                // C# precedence: skip requests from paks that carry a preset.
                if paks_with_presets.contains(&c.pak_path) {
                    continue;
                }
                req_count += 1;
                req_resources.insert(key, labels_joined(&c.mods));
                // Requests fold EARLIER-wins (MergeLeft(next, total)).
                let mut merged = c.params.clone();
                for (k, v) in &req_params {
                    merged.insert(k.clone(), v.clone());
                }
                req_params = merged;
                req_mods.extend(c.mods.clone());
            }
        }
    }

    let embedded_presets = MergeComponent {
        name: "embeddedPresets",
        priority: 1,
        mods: preset_mods,
        params: preset_params,
        resources: preset_resources,
        message: format!("Loaded {preset_count} embedded presets from installed mods"),
    };
    let sicario_requests = MergeComponent {
        name: "sicarioRequests",
        priority: 3,
        mods: req_mods,
        params: req_params,
        resources: req_resources,
        message: format!("Loaded {req_count} Sicario mods for rebuild"),
    };
    (vec![embedded_presets, sicario_requests], warnings)
}

/// The `customSkins` component — ports the C# `SkinSlotLoader` +
/// `SkinMergeProvider`: scans `*_P.pak` files for records under
/// `ProjectWingman/Content/Assets/Skins`, groups them by the directory name
/// (the aircraft row), and synthesizes an `objectRef` mod appending each skin
/// texture to that row's `SkinLibraryLegacy` array.
///
/// Priority note: the C# source leaves this component's priority at the
/// default (0), but the built binary demonstrably runs it **after** every
/// other component (verified via name-append order probes: embeddedPresets,
/// loosePresets and sicarioRequests all append their names first). We use 4
/// to reproduce the observed behavior.
///
/// Returns `None` when no skin records are found.
pub fn skin_component(paks_dir: &Path) -> (Option<MergeComponent>, Vec<String>) {
    let mut paks: Vec<PathBuf> = Vec::new();
    collect_skin_paks(paks_dir, &mut paks);
    paks.sort();

    // dir name -> full record paths (order-preserving grouping).
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for pak in &paks {
        let Ok(archive) = modman_pak::PakArchive::open(pak) else {
            continue;
        };
        for record in archive.files() {
            let norm = record.replace('\\', "/");
            if !norm.starts_with("ProjectWingman/Content/Assets/Skins") {
                continue;
            }
            let dir = std::path::Path::new(&norm)
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match groups.iter_mut().find(|(d, _)| *d == dir) {
                Some((_, paths)) => paths.push(norm),
                None => groups.push((dir, vec![norm])),
            }
        }
    }
    if groups.is_empty() {
        return (None, Vec::new());
    }

    let mut sets: Vec<crate::manifest::PatchSet> = Vec::new();
    let mut resources: BTreeMap<String, String> = BTreeMap::new();
    let mut patch_count = 0usize;
    for (aircraft, paths) in &groups {
        let asset_paths: Vec<&String> = paths
            .iter()
            .filter(|p| p.to_ascii_lowercase().ends_with(".uasset"))
            .collect();
        let patches: Vec<crate::manifest::Patch> = asset_paths
            .iter()
            .map(|p| {
                let stem = std::path::Path::new(p)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let trimmed = match p.split_once("Assets/") {
                    Some((_, rest)) => format!("Assets/{}", rest.trim_end_matches(".uasset")),
                    None => p.to_string(),
                };
                crate::manifest::Patch {
                    description: String::new(),
                    version: None,
                    template: format!("datatable:['{aircraft}'].{{'SkinLibraryLegacy*'}}"),
                    value: format!("'{stem}':'/Game/{trimmed}'"),
                    patch_type: "objectRef".to_string(),
                }
            })
            .collect();
        patch_count += patches.len();
        sets.push(crate::manifest::PatchSet {
            name: format!("Add {} {}", asset_paths.len(), aircraft),
            patches,
        });
        resources.insert(
            aircraft.clone(),
            paths
                .iter()
                .map(|p| p.trim_end_matches(".uasset").to_string())
                .collect::<Vec<_>>()
                .join(";"),
        );
    }

    let mut modm = WingmanMod {
        id: "skinSlots".to_string(),
        ..Default::default()
    };
    modm.asset_patches.insert(
        "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp"
            .to_string(),
        sets,
    );

    (
        Some(MergeComponent {
            name: "customSkins",
            priority: 4,
            mods: vec![modm],
            params: Vars::new(),
            resources,
            message: format!("Successfully compiled skin merge with {patch_count} patches."),
        }),
        Vec::new(),
    )
}

fn collect_skin_paks(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_skin_paks(&path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.to_ascii_lowercase().ends_with("_p.pak"))
        {
            out.push(path);
        }
    }
}

/// `MergeExtensions.GetMods`: ascending priority, concatenated.
pub fn ordered_mods(components: &[MergeComponent]) -> Vec<&WingmanMod> {
    let mut sorted: Vec<&MergeComponent> = components.iter().collect();
    sorted.sort_by_key(|c| c.priority);
    sorted.iter().flat_map(|c| c.mods.iter()).collect()
}

/// Drain mods out of the components in ascending priority order (leaves
/// params/resources intact for the report).
pub fn take_ordered_mods(components: &mut [MergeComponent]) -> Vec<WingmanMod> {
    let mut idx: Vec<usize> = (0..components.len()).collect();
    idx.sort_by_key(|&i| components[i].priority);
    let mut out = Vec::new();
    for i in idx {
        out.append(&mut components[i].mods);
    }
    out
}

/// `MergeExtensions.GetParameters`: descending priority fold, later wins.
pub fn merged_params(components: &[MergeComponent]) -> Vars {
    let mut sorted: Vec<&MergeComponent> = components.iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.priority));
    let mut params = Vars::new();
    for c in sorted {
        for (k, v) in &c.params {
            params.insert(k.clone(), v.clone());
        }
    }
    params
}

/// Collect `.dtp` files from directories (recursive, path-sorted), like the
/// C# `LoosePresetProvider`.
pub fn collect_dtp_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        collect_dtp_recursive(dir, &mut out);
    }
    out.sort();
    out
}

fn collect_dtp_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dtp_recursive(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("dtp"))
        {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(name: &'static str, priority: i32, params: &[(&str, &str)]) -> MergeComponent {
        MergeComponent {
            name,
            priority,
            mods: Vec::new(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            resources: BTreeMap::new(),
            message: String::new(),
        }
    }

    #[test]
    fn params_merge_priority_desc_later_wins() {
        let components = vec![
            comp("sicarioRequests", 3, &[("x", "req"), ("only", "req")]),
            comp("loosePresets", 2, &[("x", "loose")]),
            comp("embeddedPresets", 1, &[("x", "embedded"), ("e", "1")]),
        ];
        let merged = merged_params(&components);
        assert_eq!(merged["x"], "embedded", "lowest priority number wins");
        assert_eq!(merged["only"], "req");
        assert_eq!(merged["e"], "1");
    }

    #[test]
    fn version_parse_and_support() {
        assert!(preset_supported(Some("0.2.0"), "0.3.0+e58d167c"));
        assert!(preset_supported(Some("0.3.0"), "0.3.0"));
        assert!(!preset_supported(Some("0.4.0"), "0.3.0"));
        assert!(preset_supported(None, "0.3.0"));
        assert!(preset_supported(Some(""), "0.3.0"));
        assert!(preset_supported(Some("0.2.0"), "0.0.0"));
        assert!(!preset_supported(Some("garbage"), "0.3.0"));
    }

    #[test]
    fn ordered_mods_by_priority() {
        let mut a = comp("embeddedPresets", 1, &[]);
        let mut b = comp("loosePresets", 2, &[]);
        let mut c = comp("sicarioRequests", 3, &[]);
        a.mods = vec![crate::manifest::parse_mod_json(r#"{"_id":"a"}"#).unwrap()];
        b.mods = vec![crate::manifest::parse_mod_json(r#"{"_id":"b"}"#).unwrap()];
        c.mods = vec![crate::manifest::parse_mod_json(r#"{"_id":"c"}"#).unwrap()];
        let components = vec![c, b, a];
        let ordered = ordered_mods(&components);
        let ids: Vec<&str> = ordered.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }
}
