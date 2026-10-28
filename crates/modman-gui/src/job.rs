//! Background jobs: everything the GUI does runs here, off the UI thread.
//!
//! Each job calls into `modman-core` — the same functions the CLI calls — so a
//! tab can never grow behaviour the command line does not have.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};

use modman_core::combine::{CombineOptions, FilePakSource};
use modman_pak::PakArchive;

/// Messages from a worker back to the UI.
pub enum Msg {
    Stage(String),
    Done(Result<JobOutput, String>),
}

/// What a finished job leaves behind: a report for the log, and optionally a
/// pak the user can install into `~mods` with one click.
pub struct JobOutput {
    pub report: String,
    pub install: Option<PathBuf>,
}

impl JobOutput {
    fn text(report: impl Into<String>) -> Self {
        Self {
            report: report.into(),
            install: None,
        }
    }

    fn installable(report: impl Into<String>, path: PathBuf) -> Self {
        Self {
            report: report.into(),
            install: Some(path),
        }
    }
}

pub enum Job {
    Merge {
        base_pak: PathBuf,
        inputs: Vec<PathBuf>,
        out_dir: PathBuf,
    },
    AudioList {
        pak: PathBuf,
        filter: Option<String>,
    },
    AudioExtract {
        pak: PathBuf,
        out_dir: PathBuf,
        filter: Option<String>,
        manifest: Option<PathBuf>,
    },
    LocresExtract {
        pak: PathBuf,
        lang: Option<String>,
        out_dir: PathBuf,
        csv: bool,
    },
    LocresPatch {
        locres: PathBuf,
        csv: PathBuf,
        out: PathBuf,
    },
    LocresDiff {
        a: PathBuf,
        b: PathBuf,
    },
    PakInfo {
        pak: PathBuf,
    },
    PakUnpack {
        pak: PathBuf,
        out_dir: PathBuf,
    },
    PakPack {
        dir: PathBuf,
        out: PathBuf,
    },
    Scan {
        paks_dir: PathBuf,
    },
    PatchInspect {
        file: PathBuf,
    },
    Build {
        game_root: PathBuf,
        inputs: Vec<PathBuf>,
        out_dir: PathBuf,
        embed_skins: bool,
    },
}

/// Run `job` on a worker thread; returns the channel the UI polls.
pub fn spawn(job: Job, ctx: eframe::egui::Context) -> Receiver<Msg> {
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
    std::thread::spawn(move || {
        let send = |m: Msg| {
            let _ = tx.send(m);
            ctx.request_repaint();
        };
        let result = run(job, &send);
        send(Msg::Done(result));
    });
    rx
}

fn run(job: Job, send: &dyn Fn(Msg)) -> Result<JobOutput, String> {
    match job {
        Job::Merge {
            base_pak,
            inputs,
            out_dir,
        } => merge(&base_pak, &inputs, &out_dir, send),

        Job::AudioList { pak, filter } => {
            let archive = PakArchive::open(&pak).map_err(|e| format!("{}: {e}", pak.display()))?;
            let src = FilePakSource(&archive);
            let rows =
                modman_core::audio::list(&src, filter.as_deref()).map_err(|e| e.to_string())?;
            let sounds: Vec<&String> = rows
                .iter()
                .filter(|(_, c)| c == "SoundWave")
                .map(|(p, _)| p)
                .collect();
            let mut out = String::new();
            for p in sounds.iter().take(500) {
                out.push_str(p);
                out.push('\n');
            }
            if sounds.len() > 500 {
                out.push_str(&format!("... {} more\n", sounds.len() - 500));
            }
            out.push_str(&format!(
                "\n{} sound asset(s), {} other asset(s)\n",
                sounds.len(),
                rows.len() - sounds.len()
            ));
            Ok(JobOutput::text(out))
        }

        Job::AudioExtract {
            pak,
            out_dir,
            filter,
            manifest,
        } => {
            let archive = PakArchive::open(&pak).map_err(|e| format!("{}: {e}", pak.display()))?;
            let src = FilePakSource(&archive);
            let report =
                modman_core::audio::extract(&src, &out_dir, filter.as_deref(), manifest.as_deref())
                    .map_err(|e| e.to_string())?;
            let mut out = String::new();
            for w in &report.warnings {
                out.push_str(&format!("warning: {w}\n"));
            }
            for (asset, why) in &report.failed {
                out.push_str(&format!("failed: {asset}: {why}\n"));
            }
            out.push_str(&format!(
                "Wrote {} .ogg file(s) to {} ({} non-sound asset(s) skipped, {} failed)",
                report.written.len(),
                out_dir.display(),
                report.skipped.len(),
                report.failed.len()
            ));
            Ok(JobOutput::text(out))
        }

        Job::LocresExtract {
            pak,
            lang,
            out_dir,
            csv,
        } => {
            let archive = PakArchive::open(&pak).map_err(|e| format!("{}: {e}", pak.display()))?;
            std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
            let mut wanted: Vec<String> = archive
                .files()
                .into_iter()
                .filter(|f| f.ends_with(".locres") && f.contains("Localization/ProjectWingman/"))
                .collect();
            wanted.sort();
            if let Some(l) = &lang {
                let needle = format!("/{l}/");
                wanted.retain(|f| f.replace('\\', "/").contains(&needle));
            }
            if wanted.is_empty() {
                return Err(match lang {
                    Some(l) => format!("no .locres for language '{l}' in this pak"),
                    None => "no .locres files in this pak".to_string(),
                });
            }
            let mut written = Vec::new();
            for entry in &wanted {
                send(Msg::Stage(format!("Reading {entry}…")));
                let bytes = archive.read_entry(entry).map_err(|e| e.to_string())?;
                let parsed =
                    modman_core::locres::LocresFile::parse(&bytes).map_err(|e| e.to_string())?;
                let lang_name = entry
                    .trim_end_matches(".locres")
                    .rsplit('/')
                    .next()
                    .unwrap_or("locres")
                    .to_string();
                let path = if csv {
                    let p = out_dir.join(format!("{lang_name}.csv"));
                    std::fs::write(&p, parsed.to_csv()).map_err(|e| e.to_string())?;
                    p
                } else {
                    let p = out_dir.join(format!("{lang_name}.locres"));
                    std::fs::write(&p, parsed.write()).map_err(|e| e.to_string())?;
                    p
                };
                written.push((path, parsed.entry_count()));
            }
            let mut out = String::new();
            for (p, n) in &written {
                out.push_str(&format!("{} ({} entries)\n", p.display(), n));
            }
            out.push_str(&format!("Extracted {} file(s).", written.len()));
            Ok(JobOutput::text(out))
        }

        Job::LocresPatch { locres, csv, out } => {
            let bytes = std::fs::read(&locres).map_err(|e| e.to_string())?;
            let mut parsed =
                modman_core::locres::LocresFile::parse(&bytes).map_err(|e| e.to_string())?;
            let text = std::fs::read_to_string(&csv).map_err(|e| e.to_string())?;
            let patched = parsed.patch_from_csv(&text);
            std::fs::write(&out, parsed.write()).map_err(|e| e.to_string())?;
            Ok(JobOutput::text(format!(
                "Patched {patched} string(s); wrote {}",
                out.display()
            )))
        }

        Job::LocresDiff { a, b } => {
            let da = std::fs::read(&a).map_err(|e| e.to_string())?;
            let db = std::fs::read(&b).map_err(|e| e.to_string())?;
            let fa = modman_core::locres::LocresFile::parse(&da).map_err(|e| e.to_string())?;
            let fb = modman_core::locres::LocresFile::parse(&db).map_err(|e| e.to_string())?;
            let diff = fa.diff(&fb);
            Ok(JobOutput::text(format!(
                "{} changed, {} added, {} removed",
                diff.changed.len(),
                diff.added.len(),
                diff.removed.len()
            )))
        }

        Job::PakInfo { pak } => {
            let archive = PakArchive::open(&pak).map_err(|e| format!("{}: {e}", pak.display()))?;
            let info = archive.info();
            let files = archive.files();
            let mut out = format!(
                "{}\nVersion: {:?}\nMount point: {}\nFiles: {}\nEncrypted: {}\nCompression: {:?}\n",
                pak.display(),
                info.version_major,
                info.mount_point,
                files.len(),
                info.encrypted_index,
                info.compression
            );
            for f in files.iter().take(40) {
                out.push_str(&format!("  {f}\n"));
            }
            if files.len() > 40 {
                out.push_str(&format!("  ... {} more\n", files.len() - 40));
            }
            Ok(JobOutput::text(out))
        }

        Job::PakUnpack { pak, out_dir } => {
            let archive = PakArchive::open(&pak).map_err(|e| format!("{}: {e}", pak.display()))?;
            std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
            let mut n = 0usize;
            for entry in archive.files() {
                send(Msg::Stage(format!("Extracting {entry}…")));
                let Some(path) = modman_pak::safe_join(&out_dir, &entry) else {
                    return Err(format!("unsafe entry path escapes destination: {entry}"));
                };
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                archive
                    .extract_entry(&entry, &path)
                    .map_err(|e| format!("{entry}: {e}"))?;
                n += 1;
            }
            Ok(JobOutput::text(format!(
                "Unpacked {n} file(s) to {}",
                out_dir.display()
            )))
        }

        Job::PakPack { dir, out } => {
            modman_pak::pack(&dir, &out, modman_pak::Version::V3, "../../../".to_string())
                .map_err(|e| e.to_string())?;
            Ok(JobOutput::text(format!(
                "Packed {} into {}",
                dir.display(),
                out.display()
            )))
        }

        Job::Scan { paks_dir } => {
            let scan = modman_core::discovery::scan_paks_dir(&paks_dir);
            let (components, warnings) = modman_core::components::embedded_components(
                &scan,
                modman_core::components::ENGINE_VERSION,
            );
            let mut out = format!(
                "{}\n{} component(s), {} mod(s)\n",
                paks_dir.display(),
                components.len(),
                scan.mod_count()
            );
            for c in &components {
                out.push_str(&format!("  {}\n", c.message));
            }
            for w in &warnings {
                out.push_str(&format!("  warning: {w}\n"));
            }
            Ok(JobOutput::text(out))
        }

        Job::Build {
            game_root,
            inputs,
            out_dir,
            embed_skins,
        } => {
            let outcome = modman_core::build::run_to_dir(
                &game_root,
                &inputs,
                &out_dir,
                &modman_core::build::BuildOptions { embed_skins },
                |stage| {
                    use modman_core::build::BuildStage::*;
                    let text = match stage {
                        Scanning => "Scanning for mods and presets…".to_string(),
                        Merging {
                            target,
                            index,
                            total,
                        } => format!("Merging {target} ({index}/{total})…"),
                        Writing { files } => format!("Writing {files} file(s)…"),
                        Packing => "Packing SicarioMerge_P.pak…".to_string(),
                    };
                    send(Msg::Stage(text));
                },
            )
            .map_err(|e| e.message)?;
            let mut out = String::new();
            for w in &outcome.warnings {
                out.push_str(&format!("warning: {w}\n"));
            }
            for c in &outcome.components {
                out.push_str(&format!("  {c}\n"));
            }
            out.push_str(&format!(
                "Built {} ({} asset target(s), {} file(s))\n{} parameter(s), {} mod(s)\n",
                outcome.output.display(),
                outcome.asset_targets,
                outcome.files,
                outcome.inputs,
                outcome.order.len()
            ));
            if outcome.field_conflicts.is_empty() {
                out.push_str("Conflicts: none detected");
            } else {
                out.push_str(&format!(
                    "Conflicts (later mod won): {}",
                    outcome.field_conflicts.join("; ")
                ));
            }
            Ok(JobOutput::installable(out, outcome.output.clone()))
        }

        Job::PatchInspect { file } => {
            let raw = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
            if let Ok(m) = modman_core::manifest::parse_mod_json(&raw) {
                return Ok(JobOutput::text(format!(
                    "mod: {} by {} — {} asset target(s), {} file target(s)",
                    m.label(),
                    m.meta
                        .as_ref()
                        .map(|x| x.author.clone())
                        .unwrap_or_default(),
                    m.asset_patches.len(),
                    m.file_patches.len()
                )));
            }
            if let Ok(p) = modman_core::manifest::parse_preset_json(&raw) {
                return Ok(JobOutput::text(format!(
                    "preset (format v{}): {} included mod(s), {} parameter(s)",
                    p.version,
                    p.mods.len(),
                    p.mod_parameters.len()
                )));
            }
            if let Ok(r) = modman_core::manifest::parse_meta_request_json(&raw) {
                return Ok(JobOutput::text(format!(
                    "build request: {} mod(s)",
                    r.request.mods.len()
                )));
            }
            Err("unrecognised file: not a .dtm, .dtp or embedded build request".to_string())
        }
    }
}

/// Removes per-job extraction directories when the job ends, success or error.
#[derive(Default)]
pub(crate) struct TempDirs(Vec<PathBuf>);

impl TempDirs {
    fn push(&mut self, dir: PathBuf) {
        self.0.push(dir);
    }
}

impl Drop for TempDirs {
    fn drop(&mut self) {
        for d in &self.0 {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

static EXTRACT_SEQ: AtomicU64 = AtomicU64::new(0);

/// Turn display inputs into the pak list the merger applies. Paks are pushed in
/// the order given; an archive is extracted and its paks are sorted by path
/// before being appended, so one archive is deterministic while archive blocks
/// keep the user's order. The `TempDirs` must outlive every read of the result.
pub(crate) fn prepare_inputs(
    inputs: &[PathBuf],
    mut on_stage: impl FnMut(&str),
) -> Result<(Vec<PathBuf>, TempDirs), String> {
    let mut paks: Vec<PathBuf> = Vec::new();
    let mut temps = TempDirs::default();
    let total = inputs.len();
    for (i, path) in inputs.iter().enumerate() {
        let label = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        on_stage(&format!("Preparing {label} ({}/{total})…", i + 1));
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pak"))
        {
            paks.push(path.clone());
            continue;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("mod{i}"));
        let seq = EXTRACT_SEQ.fetch_add(1, Ordering::Relaxed);
        let dest = std::env::temp_dir()
            .join("modman-gui-extract")
            .join(format!("{}-{seq}-{stem}", std::process::id()));
        temps.push(dest.clone());
        match crate::archive::extract_archive(path, &dest) {
            Ok(_) => {
                let mut found = crate::archive::collect_paks(&dest);
                if found.is_empty() {
                    return Err(format!("{label}: no .pak files inside the archive"));
                }
                found.sort();
                paks.extend(found);
            }
            Err(e) => return Err(format!("{label}: {e}")),
        }
    }
    Ok((paks, temps))
}

/// The merge job: extract archives, then run the shared combine.
fn merge(
    base_pak: &std::path::Path,
    inputs: &[PathBuf],
    out_dir: &std::path::Path,
    send: &dyn Fn(Msg),
) -> Result<JobOutput, String> {
    let (paks, _temps) = prepare_inputs(inputs, |text| send(Msg::Stage(text.to_string())))?;
    if paks.is_empty() {
        return Err("no mod paks to merge".to_string());
    }

    let run = modman_core::combine::run_to_dir(
        base_pak,
        &paks,
        out_dir,
        &CombineOptions::default(),
        |stage| {
            use modman_core::combine::CombineStage::*;
            let text = match stage {
                Opening {
                    index,
                    total,
                    label,
                } => format!("Reading {label} ({index}/{total})…"),
                Merging => "Merging datatables…".to_string(),
                Writing { files } => format!("Writing {files} file(s) to disk…"),
                Packing => "Packing the merged pak…".to_string(),
            };
            send(Msg::Stage(text));
        },
    )?;

    Ok(JobOutput::installable(
        format!(
            "Merged {} datatable(s), {} file(s) total\n{}\n{} field conflict(s), {} warning(s)",
            run.merged,
            run.files,
            run.output.display(),
            run.field_conflicts.len(),
            run.warnings.len()
        ),
        run.output.clone(),
    ))
}

#[cfg(test)]
mod prepare_tests {
    use super::*;

    #[test]
    fn pak_inputs_keep_their_order() {
        let dir = std::env::temp_dir().join(format!("modman-gui-order-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.pak");
        let b = dir.join("b.pak");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let (paks, temps) = prepare_inputs(&[b.clone(), a.clone()], |_| {}).unwrap();
        assert_eq!(paks, vec![b, a]);
        drop(temps);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
