//! The Sicario build: gather components, order the mods, merge their patches
//! against the vanilla tables, and emit one mod pak.
//!
//! This is the same pipeline the CLI has always run; it lives here so the GUI
//! can run it too without a second copy.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A build failure with the exit code the CLI reports for it, so scripted
/// callers keep seeing the same numbers.
#[derive(Debug, Clone)]
pub struct BuildError {
    pub message: String,
    pub code: i32,
}

impl BuildError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code,
        }
    }
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BuildError {}

impl From<String> for BuildError {
    fn from(message: String) -> Self {
        Self::new(1, message)
    }
}

impl From<&str> for BuildError {
    fn from(message: &str) -> Self {
        Self::new(1, message)
    }
}

pub enum BuildStage<'a> {
    Scanning,
    Merging {
        target: &'a str,
        index: usize,
        total: usize,
    },
    Writing {
        files: usize,
    },
    Packing,
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Embed the skin files found in installed skin paks so the built pak works
    /// on its own. Off restores the strict C# behaviour.
    pub embed_skins: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self { embed_skins: true }
    }
}

#[derive(Debug, Clone)]
pub struct BuildOutcome {
    pub output: PathBuf,
    pub asset_targets: usize,
    pub files: usize,
    pub field_conflicts: Vec<String>,
    /// One line per mod, in merge order, with the reason it sits there.
    pub order: Vec<String>,
    /// Component descriptions, in provider order.
    pub components: Vec<String>,
    pub inputs: usize,
    pub warnings: Vec<String>,
}

/// Load a `.dtm` patch, a `.dtp` preset, or a pak-embedded build request.
pub fn load_mods_from_file(path: &Path) -> Result<Vec<crate::manifest::WingmanMod>, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut v: serde_json::Value =
        serde_json::from_str(&crate::manifest::sanitize_json(&raw)).map_err(|e| e.to_string())?;
    crate::manifest::normalize_json_keys(&mut v);
    let obj = v.as_object().ok_or("not a JSON object")?;
    if obj.contains_key("request") {
        Ok(crate::manifest::parse_meta_request_json(&raw)
            .map_err(|e| e.to_string())?
            .request
            .mods)
    } else if obj.contains_key("mods") {
        Ok(crate::manifest::parse_preset_json(&raw)
            .map_err(|e| e.to_string())?
            .mods)
    } else {
        Ok(vec![
            crate::manifest::parse_mod_json(&raw).map_err(|e| e.to_string())?
        ])
    }
}

/// Everything the build found before it merges anything: used by the writer and
/// by the CLI's dry-run preview, so both see the same order and the same mods.
pub struct Gather {
    pub components: Vec<crate::components::MergeComponent>,
    pub component_messages: Vec<String>,
    pub mods: Vec<crate::manifest::WingmanMod>,
    pub params: std::collections::HashMap<String, String>,
    pub warnings: Vec<String>,
}

/// Scan for components and order the mods, without merging anything.
pub fn gather(game_root: &Path, inputs: &[PathBuf]) -> Result<Gather, BuildError> {
    use crate::components as comps;

    let game_paks = game_root.join("ProjectWingman/Content/Paks");
    if !game_paks.is_dir() {
        return Err(BuildError::new(
            404,
            format!(
                "Install not found! {} does not contain ProjectWingman/Content/Paks",
                game_root.display()
            ),
        ));
    }

    // --- Loose presets: folders are searched, files load as-is. ---
    let mut search_dirs: Vec<PathBuf> = Vec::new();
    let mut direct_mods: Vec<crate::manifest::WingmanMod> = Vec::new();
    for path in inputs {
        if path.is_dir() {
            search_dirs.push(path.clone());
        } else {
            direct_mods.extend(load_mods_from_file(path)?);
        }
    }
    search_dirs.push(game_root.join("ProjectWingman/Content/Presets"));
    search_dirs.push(game_paks.join("~mods"));
    search_dirs.push(game_paks.join("~presets"));

    let mut warnings = Vec::new();
    let loose_files = comps::collect_dtp_files(&search_dirs);
    let (mut loose_comp, w) = comps::loose_component(&loose_files, comps::ENGINE_VERSION);
    warnings.extend(w);
    for m in direct_mods {
        loose_comp.mods.push(m);
    }

    // --- Embedded components from installed paks. ---
    let scan = crate::discovery::scan_paks_dir(&game_paks);
    let (mut embedded, w) = comps::embedded_components(&scan, comps::ENGINE_VERSION);
    warnings.extend(w);

    // Provider order: embeddedPresets, sicarioRequests, loosePresets, skins.
    let mut components: Vec<comps::MergeComponent> = Vec::new();
    components.append(&mut embedded);
    components.push(loose_comp);
    let (skin_comp, w) = comps::skin_component(&game_paks);
    warnings.extend(w);
    if let Some(sc) = skin_comp {
        components.push(sc);
    }

    let component_messages: Vec<String> = components.iter().map(|c| c.message.clone()).collect();
    let params = comps::merged_params(&components);

    let mut mods = comps::take_ordered_mods(&mut components);
    if mods.is_empty() {
        return Err(BuildError::new(1, "No mods or presets found to build!"));
    }
    for m in &mut mods {
        crate::templating::apply_variables_to_mod(m, &params);
    }

    Ok(Gather {
        components,
        component_messages,
        mods,
        params,
        warnings,
    })
}

/// Run the whole build for `game_root` from `inputs` (folders, `.dtp`, `.dtm`).
pub fn run_to_dir(
    game_root: &Path,
    inputs: &[PathBuf],
    out_dir: &Path,
    options: &BuildOptions,
    mut on_stage: impl FnMut(BuildStage<'_>),
) -> Result<BuildOutcome, BuildError> {
    use crate::components as comps;

    on_stage(BuildStage::Scanning);
    let gathered = gather(game_root, inputs)?;
    let game_paks = game_root.join("ProjectWingman/Content/Paks");

    let order: Vec<String> = gathered
        .mods
        .iter()
        .enumerate()
        .map(|(i, m)| {
            format!(
                "{}  [{}]",
                m.label(),
                if m.sicario.overwrites {
                    "overwrite".to_string()
                } else {
                    format!("priority {}", i + 1)
                }
            )
        })
        .collect();

    let extra_files: Vec<(String, Vec<u8>)> = if options.embed_skins {
        comps::collect_skin_files(&game_paks)
    } else {
        Vec::new()
    };

    let mod_refs: Vec<&crate::manifest::WingmanMod> = gathered.mods.iter().collect();
    let (asset_targets, files, field_conflicts) = build_pak(
        &game_paks,
        &mod_refs,
        &extra_files,
        out_dir,
        "SicarioMerge_P.pak",
        &mut on_stage,
    )?;

    Ok(BuildOutcome {
        output: out_dir.join("SicarioMerge_P.pak"),
        asset_targets,
        files,
        field_conflicts,
        order,
        components: gathered.component_messages,
        inputs: gathered.params.len(),
        warnings: gathered.warnings,
    })
}

/// Merge every mod's patches against the vanilla tables and write one pak.
pub fn build_pak(
    game_paks: &Path,
    mods: &[&crate::manifest::WingmanMod],
    extra_files: &[(String, Vec<u8>)],
    out_dir: &Path,
    pak_name: &str,
    on_stage: &mut impl FnMut(BuildStage<'_>),
) -> Result<(usize, usize, Vec<String>), BuildError> {
    let staging = out_dir.join("staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    let mut targets: BTreeSet<String> = BTreeSet::new();
    let mut asset_targets: BTreeSet<String> = BTreeSet::new();
    for m in mods {
        targets.extend(m.asset_patches.keys().cloned());
        targets.extend(m.file_patches.keys().cloned());
        asset_targets.extend(m.asset_patches.keys().cloned());
    }

    let main_pak_path = game_paks.join("pakchunk0-WindowsNoEditor.pak");
    let pak = modman_pak::PakArchive::open(&main_pak_path)
        .map_err(|e| BuildError::new(1, format!("Pak error: {e}")))?;
    let all_files = pak.files();
    let extract_dir = std::env::temp_dir().join("modman-build");
    let _ = std::fs::remove_dir_all(&extract_dir);
    std::fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;

    // Load every target (plus .uexp/.uasset sidecars) into a virtual file map.
    let mut files: crate::merge::FileMap = crate::merge::FileMap::new();
    let find_entry = |name: &str| -> Option<String> {
        all_files
            .iter()
            .find(|f| f.as_str() == name)
            .or_else(|| all_files.iter().find(|f| f.ends_with(name)))
            .cloned()
    };
    for target in &targets {
        let needle = target.trim_start_matches("../../../").replace('\\', "/");
        let mut wanted = vec![needle.clone()];
        if needle.ends_with(".uexp") {
            wanted.push(needle.replace(".uexp", ".uasset"));
        } else if needle.ends_with(".uasset") {
            wanted.push(needle.replace(".uasset", ".uexp"));
        }
        for w in wanted {
            let Some(entry) = find_entry(&w) else {
                continue;
            };
            if files.contains_key(&entry) {
                continue;
            }
            let stem = Path::new(&entry)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "asset".to_string());
            let ext = Path::new(&entry)
                .extension()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let local = extract_dir.join(format!("{stem}.{ext}"));
            if let Err(e) = pak.extract_entry(&entry, &local) {
                return Err(BuildError::new(1, format!("Extract error ({entry}): {e}")));
            }
            files.insert(entry, std::fs::read(&local).map_err(|e| e.to_string())?);
        }
    }

    // Phase 1: hex patches for all mods, including the .uexp length fix-up.
    crate::merge::apply_hex_phase(&mut files, mods).map_err(|e| {
        let code = match e {
            crate::apply::ApplyError::Asset(_) => 412,
            _ => 422,
        };
        BuildError::new(code, format!("Hex patch error: {e}"))
    })?;

    // Phase 2: DataTable asset patches.
    let mut merged_count = 0usize;
    let mut field_conflicts: Vec<String> = Vec::new();
    let total = asset_targets.len();
    for (index, target) in asset_targets.iter().enumerate() {
        on_stage(BuildStage::Merging {
            target,
            index: index + 1,
            total,
        });
        let needle = target.trim_start_matches("../../../").replace('\\', "/");
        let uexp_needle = if needle.ends_with(".uexp") {
            needle.clone()
        } else if needle.ends_with(".uasset") {
            needle.replace(".uasset", ".uexp")
        } else {
            format!("{needle}.uexp")
        };
        let uasset_needle = uexp_needle.replace(".uexp", ".uasset");
        let (Some(uexp_key), Some(uasset_key)) = (
            crate::merge::resolve_target(&files, &uexp_needle).cloned(),
            crate::merge::resolve_target(&files, &uasset_needle).cloned(),
        ) else {
            // C# parity: a patch targeting a file that isn't in the game aborts
            // with 412 (SourceFileNotFoundException).
            return Err(BuildError::new(
                412,
                format!("no pak entries for '{target}' (patch targets a missing file)"),
            ));
        };
        let uasset = files.get(&uasset_key).unwrap().clone();
        let uexp = files.get(&uexp_key).unwrap().clone();
        let merged = crate::merge::merge_mods(&uasset, &uexp, mods, target)
            .map_err(|e| BuildError::new(422, format!("Merge error on {target}: {e}")))?;
        let short = target.split('/').next_back().unwrap_or(target).to_string();
        field_conflicts.extend(merged.conflicts.iter().map(|c| format!("{short}: {c}")));
        files.insert(uasset_key, merged.uasset);
        files.insert(uexp_key, merged.uexp);
        merged_count += 1;
    }

    for (key, bytes) in extra_files {
        files.insert(key.clone(), bytes.clone());
    }

    on_stage(BuildStage::Writing { files: files.len() });
    for (key, bytes) in &files {
        let out_path = staging.join(key);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&out_path, bytes).map_err(|e| e.to_string())?;
    }
    if files.is_empty() {
        return Err(BuildError::new(1, "No targets merged."));
    }

    on_stage(BuildStage::Packing);
    let pak_out = out_dir.join(pak_name);
    modman_pak::pack(
        &staging,
        &pak_out,
        modman_pak::Version::V3,
        "../../../".to_string(),
        None,
    )
    .map_err(|e| BuildError::new(1, format!("Pack error: {e}")))?;
    // The staging dir is scratch: the C# transport left none behind, and in the
    // Vortex flow it would deploy to the game as junk files.
    let _ = std::fs::remove_dir_all(out_dir.join("staging"));
    Ok((merged_count, files.len(), field_conflicts))
}
