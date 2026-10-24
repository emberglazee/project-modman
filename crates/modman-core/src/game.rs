//! Game detection for Project Wingman
//!
//! Finds the Project Wingman installation directory across platforms.

use std::path::{Path, PathBuf};

/// Information about a detected PW installation
#[derive(Debug, Clone)]
pub struct GameInstall {
    pub path: PathBuf,
    #[allow(dead_code)]
    pub paks_path: PathBuf,
    #[allow(dead_code)]
    pub engine_version: String,
}

/// Detect Project Wingman installation
pub fn detect_game() -> Option<GameInstall> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates: Vec<PathBuf> = Vec::new();

    // User-specified first: an explicit choice always wins.
    if let Ok(pw) = std::env::var("PW_INSTALL") {
        candidates.push(PathBuf::from(pw));
    }
    // Linux Steam: modern default, legacy symlink, Flatpak.
    candidates.push(PathBuf::from(format!(
        "{home}/.local/share/Steam/steamapps/common/Project Wingman"
    )));
    candidates.push(PathBuf::from(format!(
        "{home}/.steam/steam/steamapps/common/Project Wingman"
    )));
    candidates.push(PathBuf::from(format!(
        "{home}/.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common/Project Wingman"
    )));
    // Windows Steam (native, and WSL cross-mount).
    candidates.push(PathBuf::from(
        "C:/Program Files (x86)/Steam/steamapps/common/Project Wingman",
    ));
    candidates.push(PathBuf::from(
        "/mnt/c/Program Files (x86)/Steam/steamapps/common/Project Wingman",
    ));

    for candidate in &candidates {
        let paks = candidate.join("ProjectWingman/Content/Paks");
        if paks.exists() {
            let version = detect_engine_version(&paks);
            return Some(GameInstall {
                path: candidate.clone(),
                paks_path: paks,
                engine_version: version,
            });
        }
    }

    None
}

/// Base-game pak names in preference order. PW 2.x renamed the archive from
/// `ProjectWingman-WindowsNoEditor.pak` to `pakchunk0-WindowsNoEditor.pak`, so
/// every caller must resolve the name instead of joining a fixed one.
const BASE_PAK_NAMES: [&str; 2] = [
    "pakchunk0-WindowsNoEditor.pak",
    "ProjectWingman-WindowsNoEditor.pak",
];

/// True when a file name looks like the game's own base archive.
///
/// The game ships `pakchunk<N>-Windows[NoEditor].pak` (2.x) or
/// `ProjectWingman-WindowsNoEditor.pak` (1.x). Mod paks carry a `_P.pak`
/// suffix and are never the base archive, even when they start with
/// `pakchunk`.
pub fn is_base_pak_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.ends_with("_p.pak") {
        return false;
    }
    n == "projectwingman-windowsnoeditor.pak"
        || (n.starts_with("pakchunk") && n.contains("-windows"))
}

/// Find the game's base pak inside a `Content/Paks` directory.
///
/// Order: the 2.x name, the 1.x name, then the lowest-numbered top-level
/// `pakchunk*.pak` the build ships. Returns `None` when no base archive is
/// present, so callers can name the directory they looked in instead of
/// reporting a path that never existed.
pub fn find_base_pak(paks_dir: &Path) -> Option<PathBuf> {
    let entries: Vec<(String, PathBuf)> = std::fs::read_dir(paks_dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().to_string(),
                entry.path(),
            )
        })
        .collect();

    for wanted in BASE_PAK_NAMES {
        if let Some((_, path)) = entries
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
        {
            return Some(path.clone());
        }
    }

    let mut chunks: Vec<(u64, String, PathBuf)> = entries
        .into_iter()
        .filter(|(name, _)| is_base_pak_name(name))
        .map(|(name, path)| {
            let number = name
                .to_ascii_lowercase()
                .strip_prefix("pakchunk")
                .unwrap_or_default()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(u64::MAX);
            (number, name.to_ascii_lowercase(), path)
        })
        .collect();
    chunks.sort();
    chunks.into_iter().next().map(|(_, _, path)| path)
}

fn detect_engine_version(paks_path: &std::path::Path) -> String {
    if let Some(main_pak) = find_base_pak(paks_path) {
        if let Ok(archive) = modman_pak::PakArchive::open(&main_pak) {
            let info = archive.info();
            return format!("UE4.27 (PAK V{:?})", info.version_major);
        }
    }
    "unknown".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("modman-game-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"pak").unwrap();
    }

    #[test]
    fn prefers_the_2x_name() {
        let dir = scratch("prefers-2x");
        touch(&dir, "ProjectWingman-WindowsNoEditor.pak");
        touch(&dir, "pakchunk0-WindowsNoEditor.pak");
        touch(&dir, "pakchunk7-Windows.pak");
        let found = find_base_pak(&dir).unwrap();
        assert_eq!(found.file_name().unwrap(), "pakchunk0-WindowsNoEditor.pak");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn falls_back_to_the_legacy_name() {
        let dir = scratch("legacy");
        touch(&dir, "ProjectWingman-WindowsNoEditor.pak");
        let found = find_base_pak(&dir).unwrap();
        assert_eq!(
            found.file_name().unwrap(),
            "ProjectWingman-WindowsNoEditor.pak"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn falls_back_to_any_chunk_pak() {
        let dir = scratch("any-chunk");
        touch(&dir, "pakchunk12-Windows.pak");
        touch(&dir, "pakchunk4-WindowsNoEditor.pak");
        let found = find_base_pak(&dir).unwrap();
        assert_eq!(found.file_name().unwrap(), "pakchunk4-WindowsNoEditor.pak");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignores_mod_paks() {
        let dir = scratch("mods-only");
        touch(&dir, "MyMod_P.pak");
        touch(&dir, "pakchunk9-WindowsNoEditor_P.pak");
        assert!(find_base_pak(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recognises_base_names() {
        assert!(is_base_pak_name("pakchunk0-WindowsNoEditor.pak"));
        assert!(is_base_pak_name("pakchunk3-Windows.pak"));
        assert!(is_base_pak_name("ProjectWingman-WindowsNoEditor.pak"));
        assert!(!is_base_pak_name("MyMod_P.pak"));
        assert!(!is_base_pak_name("pakchunk9-WindowsNoEditor_P.pak"));
        assert!(!is_base_pak_name("SicarioCombine_P.pak"));
    }
}
