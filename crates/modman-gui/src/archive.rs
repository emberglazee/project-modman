//! Archive extraction for the GUI: zip, 7z and rar, all pure Rust so nothing
//! has to be installed on the user's machine.

use std::path::{Path, PathBuf};

/// Reject absolute paths and `..` so a hostile archive cannot write outside the
/// extraction directory. One implementation, shared with pak entries.
pub fn safe_join(dest: &Path, name: &[u8]) -> Option<PathBuf> {
    modman_pak::safe_rel(&String::from_utf8_lossy(name)).map(|rel| dest.join(rel))
}

/// Extract one archive into `dest`, returning how many files landed.
pub fn extract_archive(path: &Path, dest: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match ext.as_str() {
        "zip" => {
            let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
            archive.extract(dest).map_err(|e| e.to_string())?;
        }
        "7z" => {
            sevenz_rust2::decompress_file(path, dest).map_err(|e| e.to_string())?;
        }
        "rar" => {
            let archive = rars::ArchiveReader::read_path(path).map_err(|e| e.to_string())?;
            let mut failed: Option<String> = None;
            archive
                .extract_to(None, |meta| {
                    let Some(out) = safe_join(dest, &meta.name) else {
                        failed = Some(format!(
                            "refused unsafe path: {}",
                            String::from_utf8_lossy(&meta.name)
                        ));
                        return Ok(Box::new(std::io::sink()));
                    };
                    if meta.is_directory {
                        let _ = std::fs::create_dir_all(&out);
                        return Ok(Box::new(std::io::sink()));
                    }
                    if let Some(parent) = out.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::File::create(&out) {
                        Ok(f) => Ok(Box::new(f) as Box<dyn std::io::Write>),
                        Err(e) => {
                            failed = Some(format!("{}: {e}", out.display()));
                            Ok(Box::new(std::io::sink()))
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            if let Some(e) = failed {
                return Err(e);
            }
        }
        other => return Err(format!("unsupported archive type: .{other}")),
    }

    Ok(walk_files(dest).len())
}

pub fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_files(&p));
        } else {
            out.push(p);
        }
    }
    out
}

pub fn collect_paks(dir: &Path) -> Vec<PathBuf> {
    walk_files(dir)
        .into_iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("pak"))
        })
        .collect()
}

/// Copy the merged pak into `~mods`, keeping a timestamped copy of whatever it
/// replaces.
pub fn install_to_mods(merged: &Path, paks_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let mods = paks_dir.join("~mods");
    std::fs::create_dir_all(&mods).map_err(|e| format!("{}: {e}", mods.display()))?;
    let target = mods.join(name);
    if target.exists() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backup = mods.join(format!("{name}.{stamp}.bak"));
        std::fs::rename(&target, &backup).map_err(|e| format!("backup: {e}"))?;
    }
    std::fs::copy(merged, &target).map_err(|e| format!("copy: {e}"))?;
    Ok(target)
}
