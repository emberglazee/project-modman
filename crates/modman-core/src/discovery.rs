//! Mod discovery from installed paks — mirrors the C# `MergeLoader` scan.
//!
//! Walks a Paks directory recursively for `*.pak` files (excluding the base
//! game paks and the `~sicario` output dir), reads each archive's index, and
//! collects the embedded Sicario components:
//!   - presets: a record whose path contains `sicario` and ends with `.dtp`
//!   - build requests: a record under `_meta/sicario/` ending with `.json`
//!
//! Each component parses (leniently) into one or more `WingmanMod`s.

use crate::manifest::{parse_meta_request_json, parse_preset_json, WingmanMod};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    /// Embedded preset (`Content/sicario/*.dtp`).
    Preset,
    /// Embedded build request (`_meta/sicario/*.json`).
    BuildRequest,
}

#[derive(Debug)]
pub struct DiscoveredComponent {
    pub pak_path: PathBuf,
    pub record_path: String,
    pub kind: ComponentKind,
    pub mods: Vec<WingmanMod>,
}

#[derive(Debug, Default)]
pub struct ScanReport {
    pub paks_scanned: usize,
    pub components: Vec<DiscoveredComponent>,
    pub errors: Vec<(PathBuf, String)>,
}

impl ScanReport {
    pub fn mod_count(&self) -> usize {
        self.components.iter().map(|c| c.mods.len()).sum()
    }
}

/// True for the base game paks, which are skipped. The C# loader only skips
/// `ProjectWingman-WindowsNoEditor` by name; we additionally skip
/// `pakchunk0-WindowsNoEditor` since 2.x ships it as the base pak.
fn is_base_pak(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "projectwingman-windowsnoeditor.pak" || n == "pakchunk0-windowsnoeditor.pak"
}

/// Scan a Paks directory (recursively) for Sicario merge components.
pub fn scan_paks_dir(paks_dir: &Path) -> ScanReport {
    let mut report = ScanReport::default();
    let mut paks: Vec<PathBuf> = Vec::new();
    collect_paks(paks_dir, &mut paks);
    paks.sort();
    for pak in paks {
        report.paks_scanned += 1;
        if let Err(e) = scan_single_pak(&pak, &mut report) {
            report.errors.push((pak, e));
        }
    }
    report
}

fn collect_paks(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // C# parity: the ~sicario output dir is excluded from mod scanning.
            if path.file_name().and_then(|n| n.to_str()) == Some("~sicario") {
                continue;
            }
            collect_paks(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pak"))
        {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !is_base_pak(&name) {
                out.push(path);
            }
        }
    }
}

fn scan_single_pak(pak_path: &Path, report: &mut ScanReport) -> Result<(), String> {
    let archive = modman_pak::PakArchive::open(pak_path).map_err(|e| e.to_string())?;
    for record in archive.files() {
        let norm = record.replace('\\', "/").to_ascii_lowercase();
        let is_request = norm.contains("_meta/sicario") && norm.ends_with(".json");
        let is_preset = !is_request && norm.contains("sicario") && norm.ends_with(".dtp");
        if !is_request && !is_preset {
            continue;
        }
        let bytes = archive
            .read_entry(&record)
            .map_err(|e| format!("{record}: {e}"))?;
        let text = String::from_utf8_lossy(&bytes);
        let component = if is_request {
            let req = parse_meta_request_json(&text).map_err(|e| format!("{record}: {e}"))?;
            DiscoveredComponent {
                pak_path: pak_path.to_path_buf(),
                record_path: record,
                kind: ComponentKind::BuildRequest,
                mods: req.request.mods,
            }
        } else {
            let preset = parse_preset_json(&text).map_err(|e| format!("{record}: {e}"))?;
            DiscoveredComponent {
                pak_path: pak_path.to_path_buf(),
                record_path: record,
                kind: ComponentKind::Preset,
                mods: preset.mods,
            }
        };
        if !component.mods.is_empty() {
            report.components.push(component);
        }
    }
    Ok(())
}
