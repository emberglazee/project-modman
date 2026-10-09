//! Multi-mod merge (M5) — sequential application with later mods' edits
//! winning, matching the C# merger's observed semantics (verified with oracle
//! conflict experiments: `false`→`true` and `true`→`false` both resolve to the
//! last-applied value).
//!
//! Merge order follows the C# component order: embedded build-request mods
//! (Priority 1) → loose presets (2) → embedded presets (3).

use crate::apply::{self, ApplyError};
use crate::hexpatch;
use crate::manifest::WingmanMod;
use crate::rowops;
use modman_uasset::walk::DataTable;
use std::collections::BTreeMap;

/// The merged asset pair.
#[derive(Debug)]
pub struct MergedOutput {
    pub uasset: Vec<u8>,
    pub uexp: Vec<u8>,
}

/// A virtual build filesystem: target path (game-root relative, forward
/// slashes) -> raw bytes.
pub type FileMap = BTreeMap<String, Vec<u8>>;

/// Resolve a patch target key against the map (exact, then case-insensitive
/// suffix match).
pub fn resolve_target<'a>(files: &'a FileMap, target: &str) -> Option<&'a String> {
    let norm = target.trim_start_matches('/').replace('\\', "/");
    if files.contains_key(&norm) {
        return files.get_key_value(&norm).map(|(k, _)| k);
    }
    let t = norm.to_lowercase();
    files.keys().find(|k| {
        let kl = k.to_lowercase();
        kl == t || kl.ends_with(&t) || t.ends_with(&kl)
    })
}

/// Engine-major phase 1: apply every mod's `filePatches` (hex engine) in mod
/// order, including the C# `HexPatchEngine` length auto-correct: when a
/// `.uexp` changes size, its sibling `.uasset`'s serial size (old `len - 4`)
/// is hex-swapped for the new value.
pub fn apply_hex_phase(files: &mut FileMap, mods: &[&WingmanMod]) -> Result<(), ApplyError> {
    for modm in mods {
        let mut targets: Vec<&String> = modm.file_patches.keys().collect();
        targets.sort();
        for target in targets {
            let sets = &modm.file_patches[target];
            let key = resolve_target(files, target)
                .ok_or_else(|| {
                    ApplyError::Asset(format!("filePatches target not found: {target}"))
                })?
                .clone();
            let src = files.get(&key).expect("resolved key exists").clone();
            let out = hexpatch::run_file_patches(&src, sets)
                .map_err(|e| ApplyError::Asset(e.to_string()))?;
            if key.ends_with(".uexp") && out.len() != src.len() {
                let sibling = format!("{}.uasset", &key[..key.len() - 5]);
                if let Some(uasset) = files.get(&sibling).cloned() {
                    let fixed = hexpatch::apply_length_fixup(&uasset, src.len(), out.len())
                        .map_err(|e| ApplyError::Asset(e.to_string()))?;
                    files.insert(sibling, fixed);
                }
            }
            files.insert(key, out);
        }
    }
    Ok(())
}

/// Apply a sequence of mods (in merge order) to a vanilla asset pair.
///
/// Each mod is planned against the *current* accumulated state (the C# merger
/// re-reads the file between mods), so value-matching in later mods sees
/// earlier mods' edits.
pub fn merge_mods(
    vanilla_uasset: &[u8],
    vanilla_uexp: &[u8],
    mods: &[&WingmanMod],
    target: &str,
) -> Result<MergedOutput, ApplyError> {
    let mut uasset = vanilla_uasset.to_vec();
    let mut uexp = vanilla_uexp.to_vec();
    for modm in mods {
        let dt =
            DataTable::walk_bytes(&uasset, &uexp).map_err(|e| ApplyError::Asset(e.to_string()))?;
        // Same-size path first; anything that doesn't fit falls back to the
        // length-changing machinery (which also handles pure in-place edits).
        match apply::plan_same_size_edits(&dt, modm, target) {
            Ok(edits) => {
                uexp = apply::apply_edits(&uexp, &edits)?;
            }
            Err(_) => {
                let res = rowops::apply_length_changing(&dt, &uexp, modm, target)?;
                if res.uexp_delta != 0 || !res.name_append.is_empty() {
                    uasset = modman_uasset::rewrite::rewrite_uasset(
                        &uasset,
                        &modman_uasset::rewrite::RewritePlan {
                            name_append: res.name_append,
                            uexp_delta: res.uexp_delta,
                        },
                    )
                    .map_err(|e| ApplyError::Asset(e.to_string()))?;
                }
                uexp = res.uexp;
            }
        }
    }
    Ok(MergedOutput { uasset, uexp })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{parse_meta_request_json, parse_preset_json};

    const TARGET: &str =
        "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp";

    fn pair() -> (Vec<u8>, Vec<u8>) {
        let ua = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modman-uasset/tests/fixtures/DB_Aircraft.uasset"
        ))
        .unwrap();
        let ue = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../modman-uasset/tests/fixtures/DB_Aircraft.uexp"
        ))
        .unwrap();
        (ua, ue)
    }

    fn fixed_loadout_mod(display: &str, value: &str) -> WingmanMod {
        let json = format!(
            r#"{{
                "version": 1,
                "mods": [{{
                    "_id": "",
                    "_vars": {{}},
                    "assetPatches": {{
                        "{TARGET}": [{{
                            "name": "t",
                            "patches": [{{
                                "description": "set FixedLoadout",
                                "template": "datatable:['SPEAR'].{{'FixedLoadout*'}}",
                                "value": "BoolProperty:{value}",
                                "type": "propertyValue"
                            }}]
                        }}]
                    }},
                    "_meta": {{ "displayName": "{display}" }},
                    "filePatches": {{}}
                }}]
            }}"#
        );
        parse_preset_json(&json).unwrap().mods.remove(0)
    }

    /// Oracle-verified conflict semantics: the later mod wins.
    #[test]
    fn conflict_later_mod_wins() {
        let (ua, ue) = pair();
        let a = fixed_loadout_mod("A", "false");
        let b = fixed_loadout_mod("B", "true");
        let out = merge_mods(&ua, &ue, &[&a, &b], TARGET).unwrap();
        assert_eq!(out.uexp[83882], 1, "false then true -> true");
        let out2 = merge_mods(&ua, &ue, &[&b, &a], TARGET).unwrap();
        assert_eq!(out2.uexp[83882], 0, "true then false -> false");
    }

    /// Two-mod merge (embedded request + loose preset) vs the banked oracle
    /// output: uasset identical; uexp identical modulo regenerated FText keys.
    #[test]
    fn merge_spear_and_chimera_matches_oracle() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let spear = std::path::Path::new(&home)
            .join("modding/project-wingman/sicario-corpus/553-spear-unlocker/spearunlock.dtp");
        let chimera = std::path::Path::new(&home).join(
            "modding/project-wingman/sicario-corpus/256-improved-chimera/ImprovedChimera_P-sicario-meta.json",
        );
        let oracle_pak = std::path::Path::new(&home).join(
            "modding/project-wingman/sicario-oracle/run3-merge-spear-chimera/SicarioMerge_P.pak",
        );
        if !spear.exists() || !chimera.exists() || !oracle_pak.exists() {
            return;
        }

        let mut spear_mod = parse_preset_json(&std::fs::read_to_string(&spear).unwrap())
            .unwrap()
            .mods
            .remove(0);
        let mut chimera_mod = parse_meta_request_json(&std::fs::read_to_string(&chimera).unwrap())
            .unwrap()
            .request
            .mods
            .remove(0);
        // The pipeline renders before merging (C# `PatchTemplateBehaviour`).
        let inputs = crate::templating::Vars::new();
        crate::template::apply_variables_to_mod(&mut spear_mod, &inputs);
        crate::template::apply_variables_to_mod(&mut chimera_mod, &inputs);

        // Component order (C# priorities): embedded presets (P1) → loose
        // presets (P2) → sicario requests (P3). No conflicts here, but the
        // order should be faithful.
        let (ua, ue) = pair();
        let out = merge_mods(&ua, &ue, &[&spear_mod, &chimera_mod], TARGET).unwrap();

        // Extract the oracle entries from the banked pak.
        let pak = modman_pak::PakArchive::open(&oracle_pak).unwrap();
        let find = |suffix: &str| {
            pak.files()
                .into_iter()
                .find(|f| f.ends_with(suffix))
                .unwrap()
        };
        let oracle_uasset = pak.read_entry(&find("DB_Aircraft.uasset")).unwrap();
        let oracle_uexp = pak.read_entry(&find("DB_Aircraft.uexp")).unwrap();

        assert_eq!(out.uasset, oracle_uasset, "uasset differs from oracle");
        assert_eq!(out.uexp.len(), oracle_uexp.len());
        let diffs: Vec<usize> = (0..out.uexp.len())
            .filter(|&i| out.uexp[i] != oracle_uexp[i])
            .collect();
        if !diffs.is_empty() {
            let mut ranges: Vec<(usize, usize)> = Vec::new();
            for &i in &diffs {
                if let Some(last) = ranges.last_mut() {
                    // FText keys are random hex strings; coincidental byte
                    // matches split a key's diff into sub-ranges, so merge
                    // across small gaps.
                    if i <= last.1 + 4 {
                        last.1 = i;
                        continue;
                    }
                }
                ranges.push((i, i));
            }
            assert!(
                ranges.len() <= 4,
                "too many diff ranges: {ranges:?} ({} bytes)",
                diffs.len()
            );
            let ok = |c: u8| c.is_ascii_digit() || (b'A'..=b'F').contains(&c);
            for (a, b) in &ranges {
                assert!(b - a <= 36, "diff range too large: {a}..{b}");
                for (off, (&x, &y)) in out.uexp[*a..=*b]
                    .iter()
                    .zip(oracle_uexp[*a..=*b].iter())
                    .enumerate()
                {
                    assert!(
                        ok(x) && ok(y),
                        "non-key diff byte at {}: {:02x} vs {:02x}",
                        a + off,
                        x,
                        y
                    );
                }
            }
        }
    }

    struct HexCase {
        uasset: Vec<u8>,
        uexp: Vec<u8>,
        oracle_uasset: Vec<u8>,
        oracle_uexp: Vec<u8>,
    }

    fn oracle_hex_case(preset_file: &str, oracle_pak: &str) -> Option<HexCase> {
        let home = std::env::var("HOME").ok()?;
        let base =
            std::path::Path::new(&home).join("modding/project-wingman/sicario-oracle/run5-hex");
        let preset = base.join(preset_file);
        let pak_path = base.join(oracle_pak);
        if !preset.exists() || !pak_path.exists() {
            return None;
        }

        let (ua, ue) = pair();
        let uasset_key =
            "ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uasset"
                .to_string();
        let mut files = FileMap::new();
        files.insert(uasset_key.clone(), ua);
        files.insert(TARGET.to_string(), ue);

        let mut mods = parse_preset_json(&std::fs::read_to_string(&preset).unwrap())
            .unwrap()
            .mods;
        for m in &mut mods {
            crate::template::apply_variables_to_mod(m, &crate::templating::Vars::new());
        }
        let refs: Vec<&WingmanMod> = mods.iter().collect();
        apply_hex_phase(&mut files, &refs).unwrap();

        let pak = modman_pak::PakArchive::open(&pak_path).unwrap();
        let find = |suffix: &str| {
            pak.files()
                .into_iter()
                .find(|f| f.ends_with(suffix))
                .unwrap()
        };
        let oracle_uexp = pak.read_entry(&find("DB_Aircraft.uexp")).unwrap();
        let oracle_uasset = pak.read_entry(&find("DB_Aircraft.uasset")).unwrap();
        Some(HexCase {
            uasset: files[&uasset_key].clone(),
            uexp: files[TARGET].clone(),
            oracle_uasset,
            oracle_uexp,
        })
    }

    /// Hex engine parity vs the C# merger (clean path): `row`/`word` filters,
    /// `before` type, ignored `none` type, cross-mod growth and the `.uexp`
    /// length auto-correct in the sibling `.uasset`.
    #[test]
    fn hex_phase_matches_oracle() {
        let Some(case) = oracle_hex_case("hex-test2.dtp", "oracle-hex8-pak") else {
            return;
        };
        assert_eq!(
            case.uexp, case.oracle_uexp,
            "hex-phase uexp differs from oracle"
        );
        assert_eq!(
            case.uasset, case.oracle_uasset,
            "hex-phase uasset (length auto-correct) differs from oracle"
        );
    }

    /// Faithful reproduction of the C# "absent value" behavior: doc-style
    /// `substitution` fields do not bind to `value`, and matching templates
    /// get dropped by the InPlace stream-walk (the destructive path).
    #[test]
    fn hex_absent_value_damage_matches_oracle() {
        let Some(case) = oracle_hex_case("hex-test.dtp", "oracle-hex7-pak") else {
            return;
        };
        assert_eq!(
            case.uexp, case.oracle_uexp,
            "damage-path uexp differs from oracle"
        );
        assert_eq!(
            case.uasset, case.oracle_uasset,
            "damage-path uasset differs from oracle"
        );
    }
}
