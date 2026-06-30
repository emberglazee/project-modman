//! Game detection for Project Wingman
//!
//! Finds the Project Wingman installation directory across platforms.

use std::path::PathBuf;

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
    // Check common Steam library locations
    let candidates = [
        // Linux Steam
        PathBuf::from(
            std::env::var("HOME").unwrap_or_default()
                + "/.steam/steam/steamapps/common/Project Wingman",
        ),
        // Windows Steam (WSL/Linux cross-mount)
        PathBuf::from("/mnt/c/Program Files (x86)/Steam/steamapps/common/Project Wingman"),
        // User-specified via env var
        std::env::var("PW_INSTALL")
            .map(PathBuf::from)
            .ok()
            .unwrap_or_default(),
    ];

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

fn detect_engine_version(paks_path: &std::path::Path) -> String {
    // Check main pak for engine version info
    let main_pak = paks_path.join("pakchunk0-WindowsNoEditor.pak");
    if main_pak.exists() {
        if let Ok(archive) = modman_pak::PakArchive::open(&main_pak) {
            let info = archive.info();
            return format!("UE4.27 (PAK V{:?})", info.version_major);
        }
    }
    "unknown".into()
}
