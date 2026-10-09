//! Multi-mod merge (M5) — sequential application with later mods' edits
//! winning, matching the C# merger's observed semantics (verified with oracle
//! conflict experiments: `false`→`true` and `true`→`false` both resolve to the
//! last-applied value).
//!
//! Merge order follows the C# component order: embedded build-request mods
//! (Priority 1) → loose presets (2) → embedded presets (3).

use crate::apply::{self, ApplyError};
use crate::manifest::WingmanMod;
use crate::rowops;
use modman_uasset::walk::DataTable;

/// The merged asset pair.
#[derive(Debug)]
pub struct MergedOutput {
    pub uasset: Vec<u8>,
    pub uexp: Vec<u8>,
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

        let spear_mod = parse_preset_json(&std::fs::read_to_string(&spear).unwrap())
            .unwrap()
            .mods
            .remove(0);
        let chimera_mod = parse_meta_request_json(&std::fs::read_to_string(&chimera).unwrap())
            .unwrap()
            .request
            .mods
            .remove(0);

        // Component order: embedded requests (chimera) before loose presets (spear).
        let (ua, ue) = pair();
        let out = merge_mods(&ua, &ue, &[&chimera_mod, &spear_mod], TARGET).unwrap();

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
                    if i <= last.1 + 2 {
                        last.1 = i;
                        continue;
                    }
                }
                ranges.push((i, i));
            }
            assert!(
                ranges.len() <= 3,
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
}
