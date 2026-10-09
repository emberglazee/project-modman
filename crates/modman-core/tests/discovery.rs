//! Discovery gates: scan fixture paks + (when present) the real corpus.

use modman_core::discovery::{scan_paks_dir, ComponentKind};
use std::path::{Path, PathBuf};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/paks")
}

#[test]
fn scans_fixture_paks() {
    let report = scan_paks_dir(&fixture_dir());
    assert_eq!(report.paks_scanned, 2, "~sicario dir must be excluded");
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
    assert_eq!(report.components.len(), 1);
    let c = &report.components[0];
    assert_eq!(c.kind, ComponentKind::BuildRequest);
    assert!(c.record_path.to_ascii_lowercase().contains("_meta/sicario"));
    assert_eq!(c.mods.len(), 1);
    assert_eq!(c.mods[0].label(), "New and Improved Chimera (by agc93)");
    assert_eq!(c.mods[0].patch_count(), 15);
}

#[test]
fn merge_output_pak_has_no_components() {
    let tmp = std::env::temp_dir().join("modman-discovery-test");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::copy(
        fixture_dir().join("SicarioMerge_P.pak"),
        tmp.join("SicarioMerge_P.pak"),
    )
    .unwrap();
    let report = scan_paks_dir(&tmp);
    assert_eq!(report.paks_scanned, 1);
    assert!(report.errors.is_empty());
    assert_eq!(report.components.len(), 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn scans_real_corpus() {
    let Some(home) = std::env::var("HOME").ok() else {
        return;
    };
    let corpus = Path::new(&home).join("modding/project-wingman/sicario-corpus");
    if !corpus.is_dir() {
        return;
    }
    let report = scan_paks_dir(&corpus);
    assert!(
        report.paks_scanned >= 15,
        "scanned {} paks",
        report.paks_scanned
    );
    assert!(
        report.mod_count() >= 12,
        "found {} mods: {:?}",
        report.mod_count(),
        report
            .components
            .iter()
            .map(|c| (c.pak_path.file_name(), c.mods.len()))
            .collect::<Vec<_>>()
    );
    assert!(report
        .components
        .iter()
        .any(|c| c.kind == ComponentKind::Preset));
    assert!(report
        .components
        .iter()
        .any(|c| c.kind == ComponentKind::BuildRequest));
}
