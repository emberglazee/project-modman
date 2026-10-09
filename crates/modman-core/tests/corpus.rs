//! Corpus validation: parse every real Sicario specimen (presets + embedded
//! build-request metas) with the M1 model layer. Skips cleanly when the local
//! corpus is not present (e.g. CI).

use modman_core::manifest::{parse_meta_request_json, parse_mod_json, parse_preset_json};
use std::path::{Path, PathBuf};

fn corpus_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let p = Path::new(&home).join("modding/project-wingman/sicario-corpus");
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

fn collect(root: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some(ext) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn parse_all_corpus_presets() {
    let Some(root) = corpus_dir() else {
        eprintln!("corpus not present, skipping");
        return;
    };
    let presets = collect(&root, "dtp");
    assert!(presets.len() >= 5, "expected several .dtp specimens");

    for path in &presets {
        let raw = std::fs::read_to_string(path).unwrap();
        let preset = parse_preset_json(&raw)
            .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
        assert!(
            !preset.mods.is_empty(),
            "{} parsed with zero mods",
            path.display()
        );
        assert!(preset.version >= 1);
        for m in &preset.mods {
            assert!(
                m.is_valid(),
                "{}: mod {} has no patches",
                path.display(),
                m.label()
            );
        }
    }
}

#[test]
fn parse_all_corpus_meta_requests() {
    let Some(root) = corpus_dir() else {
        return;
    };
    let metas = collect(&root, "json");
    assert!(metas.len() >= 8, "expected several meta specimens");

    let mut parsed = 0;
    for path in &metas {
        let raw = std::fs::read_to_string(path).unwrap();
        let req = parse_meta_request_json(&raw)
            .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
        if !req.request.mods.is_empty() {
            parsed += 1;
            assert!(
                !req.request.id.is_empty(),
                "{} missing request id",
                path.display()
            );
            for m in &req.request.mods {
                assert!(m.is_valid(), "{}: empty mod", path.display());
            }
        }
    }
    assert!(
        parsed >= 8,
        "expected >= 8 meta requests with mods, got {parsed}"
    );
}

#[test]
fn spot_checks() {
    let Some(root) = corpus_dir() else {
        return;
    };

    // 553 SPEAR Unlocker: 1 mod, 2 propertyValue patches on DB_Aircraft.
    let raw = std::fs::read_to_string(root.join("553-spear-unlocker/spearunlock.dtp")).unwrap();
    let p = parse_preset_json(&raw).unwrap();
    assert_eq!(p.mods.len(), 1);
    let m = &p.mods[0];
    assert_eq!(m.label(), "SPEAR Unlock (by agc93 & JohnVicres)");
    let sets = m
        .asset_patches
        .get("ProjectWingman/Content/ProjectWingman/Blueprints/Data/AircraftData/DB_Aircraft.uexp")
        .expect("DB_Aircraft target");
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].patches.len(), 2);
    assert_eq!(sets[0].patches[0].patch_type, "propertyValue");
    assert!(sets[0].patches[0].template.contains("FixedLoadout"));
    assert!(sets[0].patches[1]
        .template
        .contains("HardpointCompatibilityList"));

    // 042 All Weapons: wildcard `[[*]]` + `_vars` preserved.
    let raw = std::fs::read_to_string(root.join("042-all-weapons/awfap-preset.dtp")).unwrap();
    let p = parse_preset_json(&raw).unwrap();
    let m = &p.mods[0];
    assert!(m.sicario.overwrites);
    assert!(m.variables.contains_key("allWeapsList"));
    let sets = m.asset_patches.values().next().unwrap();
    assert!(sets[0].patches[0].template.contains("[[*]]"));

    // 256 Improved Chimera: duplicateEntry + textProperty with templating.
    let raw = std::fs::read_to_string(
        root.join("256-improved-chimera/ImprovedChimera_P-sicario-meta.json"),
    )
    .unwrap();
    let req = parse_meta_request_json(&raw).unwrap();
    let m = &req.request.mods[0];
    assert_eq!(m.variables.get("aircraftName").unwrap(), "ACG-01X");
    let all: Vec<&modman_core::manifest::Patch> = m
        .asset_patches
        .values()
        .flat_map(|sets| sets.iter())
        .flat_map(|s| s.patches.iter())
        .collect();
    assert!(all
        .iter()
        .any(|p| p.patch_type == "duplicateEntry" && p.value.contains("ACG-01X")));
    assert!(all
        .iter()
        .any(|p| p.patch_type == "textProperty" && p.value.contains("{{ vars.aircraftName }}")));

    // 125 Payday: 2 inputs + enableSteps with Liquid expressions.
    let raw = std::fs::read_to_string(root.join("125-payday/Payday.dtp")).unwrap();
    let p = parse_preset_json(&raw).unwrap();
    assert_eq!(p.mod_parameters.get("payoutFactor").unwrap(), "5");
    let m = &p.mods[0];
    assert_eq!(m.inputs.len(), 2);
    assert_eq!(m.inputs[0].id, "payoutFactor");
    assert!(m.sicario.enable_steps.contains_key("ReplaceAny"));

    // 162 Spearifier: meta request with enableSteps referencing inputs.
    let raw = std::fs::read_to_string(
        root.join("162-spearifier/TheSpearifier (SP-34R)_P-sicario-meta.json"),
    )
    .unwrap();
    let req = parse_meta_request_json(&raw).unwrap();
    let m = &req.request.mods[0];
    assert!(m
        .sicario
        .enable_steps
        .values()
        .any(|v| v.contains("inputs.")));

    // 200 Prez Nowhere: hex filePatches parse (inPlace type).
    let raw =
        std::fs::read_to_string(root.join("200-prez-nowhere/DisablePrez_P-sicario-meta.json"))
            .unwrap();
    let req = parse_meta_request_json(&raw).unwrap();
    let m = &req.request.mods[0];
    assert!(m.is_valid());
}

#[test]
fn direct_mod_parse_matches_preset_embedded_mod() {
    let Some(root) = corpus_dir() else {
        return;
    };
    // A mod parsed standalone must equal the same mod embedded in its preset.
    let raw = std::fs::read_to_string(root.join("553-spear-unlocker/spearunlock.dtp")).unwrap();
    let p = parse_preset_json(&raw).unwrap();
    let mod_json = serde_json::to_string(&p.mods[0]).unwrap();
    let direct = parse_mod_json(&mod_json).unwrap();
    assert_eq!(direct.patch_count(), p.mods[0].patch_count());
    assert_eq!(direct.label(), p.mods[0].label());
}

#[test]
fn all_corpus_templates_parse() {
    let Some(root) = corpus_dir() else {
        return;
    };
    let mut mods = Vec::new();
    for ext in ["dtp", "json"] {
        for path in collect(&root, ext) {
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Ok(p) = parse_preset_json(&raw) {
                mods.extend(p.mods);
                continue;
            }
            if let Ok(r) = parse_meta_request_json(&raw) {
                mods.extend(r.request.mods);
            }
        }
    }
    assert!(!mods.is_empty());

    // Also include mods embedded in corpus paks (presets + build requests).
    let report = modman_core::discovery::scan_paks_dir(&root);
    for c in report.components {
        mods.extend(c.mods);
    }

    let mut count = 0usize;
    for m in &mods {
        for sets in m.asset_patches.values() {
            for set in sets {
                for patch in &set.patches {
                    let ctx = modman_core::fragment::parse_template(&patch.template)
                        .unwrap_or_else(|e| {
                            panic!("template failed to parse: `{}` ({e})", patch.template)
                        });
                    assert!(
                        !ctx.fragments.is_empty(),
                        "no fragments parsed: `{}`",
                        patch.template
                    );
                    count += 1;
                }
            }
        }
    }
    assert!(count >= 80, "expected many templates, got {count}");
}
