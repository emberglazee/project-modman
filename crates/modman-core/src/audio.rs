//! Audio extraction (`modman audio`): list the baked sound assets in a pak set
//! and write each SoundWave out as the Ogg Vorbis stream it already contains.
//!
//! Nothing is transcoded. Project Wingman stores Vorbis, so the output is the
//! game's own stream minus UE's wrapper; that is why `.ogg` is the only format
//! offered.

use crate::apply::ApplyError;
use crate::combine::PakSource;
use modman_uasset::soundwave;
use std::path::{Path, PathBuf};

/// One extracted sound.
pub struct AudioEntry {
    /// Record path inside the pak, with the mount prefix removed.
    pub asset: String,
    /// Where the `.ogg` was written, relative to the output directory.
    pub output: PathBuf,
    pub bytes: usize,
    pub streamed: bool,
}

pub struct AudioReport {
    pub written: Vec<AudioEntry>,
    /// `(asset, reason)` for entries that are not SoundWaves.
    pub skipped: Vec<(String, String)>,
    /// `(asset, reason)` for SoundWaves that could not be read or parsed. One
    /// bad asset must not abort a run over thousands.
    pub failed: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

/// Strip the leading mount-relative noise so paths read like game paths.
fn clean(path: &str) -> String {
    let mut p = path.replace('\\', "/");
    while let Some(rest) = p.strip_prefix("../") {
        p = rest.to_string();
    }
    p
}

/// The output path for an asset: its record path minus the always-present
/// `ProjectWingman/Content/` prefix, with the extension swapped to `.ogg`.
fn output_path(asset: &str) -> Option<PathBuf> {
    let cleaned = clean(asset);
    let stem = cleaned
        .strip_suffix(".uasset")
        .or_else(|| cleaned.strip_suffix(".uexp"))
        .unwrap_or(&cleaned);
    let trimmed = stem.strip_prefix("ProjectWingman/Content/").unwrap_or(stem);
    modman_pak::safe_rel(&format!("{trimmed}.ogg"))
}

fn matches_filter(asset: &str, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(f) => asset.to_ascii_lowercase().contains(&f.to_ascii_lowercase()),
    }
}

/// `(uasset, uexp, optional ubulk)` triples, in pak order. Paths stay exactly
/// as the pak index reports them, because they are also used to read entries;
/// cleaning happens only when building a display path or output path.
fn pairs(src: &dyn PakSource) -> Vec<(String, String, Option<String>)> {
    let files: Vec<String> = src.files();
    let present: std::collections::BTreeSet<&str> = files.iter().map(|f| f.as_str()).collect();

    let mut out = Vec::new();
    for file in &files {
        let Some(stem) = file.strip_suffix(".uasset") else {
            continue;
        };
        let uexp = format!("{stem}.uexp");
        if !present.contains(uexp.as_str()) {
            continue;
        }
        let ubulk = format!("{stem}.ubulk");
        let bulk = present.contains(ubulk.as_str()).then_some(ubulk);
        out.push((stem.to_string(), uexp, bulk));
    }
    out
}

/// Classify the assets a filter selects, without extracting anything.
/// Assets that fail to parse are reported as `unreadable` rather than aborting
/// the listing.
pub fn list(
    src: &dyn PakSource,
    filter: Option<&str>,
) -> Result<Vec<(String, String)>, ApplyError> {
    let mut out = Vec::new();
    for (stem, _, _) in pairs(src) {
        let asset = format!("{stem}.uasset");
        if !matches_filter(&asset, filter) {
            continue;
        }
        let class = match src.read(&asset) {
            Ok(uasset) => match soundwave::export_class(&uasset) {
                Ok(Some(c)) => c,
                Ok(None) => "unknown".to_string(),
                Err(e) => format!("unreadable ({e})"),
            },
            Err(e) => format!("unreadable ({e})"),
        };
        out.push((asset, class));
    }
    Ok(out)
}

/// Extract every SoundWave the filter selects into `out_dir`, mirroring the
/// asset tree. Non-sound assets are counted, not written.
pub fn extract(
    src: &dyn PakSource,
    out_dir: &Path,
    filter: Option<&str>,
    manifest: Option<&Path>,
) -> Result<AudioReport, ApplyError> {
    let mut report = AudioReport {
        written: Vec::new(),
        skipped: Vec::new(),
        failed: Vec::new(),
        warnings: Vec::new(),
    };

    for (stem, uexp_path, ubulk_path) in pairs(src) {
        let asset = format!("{stem}.uasset");
        if !matches_filter(&asset, filter) {
            continue;
        }

        let uasset = match src.read(&asset) {
            Ok(b) => b,
            Err(e) => {
                report.failed.push((asset, format!("read: {e}")));
                continue;
            }
        };
        let class = match soundwave::export_class(&uasset) {
            Ok(Some(c)) => c,
            Ok(None) => "unknown".to_string(),
            Err(e) => {
                // Not every asset in a pak is a sound, and some are stubs this
                // parser cannot classify. Skip with the reason instead of
                // calling it a failure.
                report.skipped.push((asset, format!("unreadable: {e}")));
                continue;
            }
        };
        if class != "SoundWave" {
            report.skipped.push((asset, class));
            continue;
        }

        let uexp = match src.read(&uexp_path) {
            Ok(b) => b,
            Err(e) => {
                report.failed.push((asset, format!("read uexp: {e}")));
                continue;
            }
        };
        let ubulk = match &ubulk_path {
            Some(p) => match src.read(p) {
                Ok(b) => Some(b),
                Err(e) => {
                    report.failed.push((asset, format!("read ubulk: {e}")));
                    continue;
                }
            },
            None => None,
        };
        let extracted = match soundwave::extract(&uexp, ubulk.as_deref()) {
            Ok(x) => x,
            Err(e) => {
                report.failed.push((asset, format!("extract: {e}")));
                continue;
            }
        };
        for w in &extracted.warnings {
            report.warnings.push(format!("{asset}: {w}"));
        }

        let Some(rel) = output_path(&asset) else {
            report
                .failed
                .push((asset, "unsafe output path".to_string()));
            continue;
        };
        let full = out_dir.join(&rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ApplyError::Asset(format!("{}: {e}", parent.display())))?;
        }
        std::fs::write(&full, &extracted.stream)
            .map_err(|e| ApplyError::Asset(format!("{}: {e}", full.display())))?;

        report.written.push(AudioEntry {
            asset,
            output: rel,
            bytes: extracted.stream.len(),
            streamed: extracted.streamed,
        });
    }

    if let Some(path) = manifest {
        write_manifest(&report, path)?;
    }
    Ok(report)
}

fn write_manifest(report: &AudioReport, path: &Path) -> Result<(), ApplyError> {
    fn quote(s: &str) -> String {
        if s.contains(',') || s.contains('"') || s.contains('\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    }
    let mut text = String::from("asset,output,bytes,streamed\n");
    for e in &report.written {
        text.push_str(&format!(
            "{},{},{},{}\n",
            quote(&e.asset),
            quote(&e.output.to_string_lossy().replace('\\', "/")),
            e.bytes,
            e.streamed
        ));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ApplyError::Asset(format!("{}: {e}", parent.display())))?;
        }
    }
    std::fs::write(path, text).map_err(|e| ApplyError::Asset(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_path_strips_the_content_prefix_and_swaps_the_extension() {
        assert_eq!(
            output_path("../../../ProjectWingman/Content/Assets/Audio/Dialog/C_M6/x.uasset")
                .unwrap()
                .as_path(),
            Path::new("Assets/Audio/Dialog/C_M6/x.ogg")
        );
        assert_eq!(
            output_path("ProjectWingman/Content/Assets/Audio/Music/08_-_Conquest.uasset")
                .unwrap()
                .as_path(),
            Path::new("Assets/Audio/Music/08_-_Conquest.ogg")
        );
        assert!(output_path("ProjectWingman/Content/../../../../evil.uasset").is_none());
    }

    #[test]
    fn filter_is_case_insensitive_and_substring_based() {
        let asset = "Assets/Audio/Dialog/C_M6/x.uasset";
        assert!(matches_filter(asset, None));
        assert!(matches_filter(asset, Some("dialog")));
        assert!(matches_filter(asset, Some("C_M6")));
        assert!(!matches_filter(asset, Some("music")));
    }
}
