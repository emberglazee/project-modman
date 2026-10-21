//! modman-gui — the native desktop front end.
//!
//! The merge itself is `modman_core::combine::run_to_dir`, the same function
//! the CLI runs, so this window adds no merge behaviour of its own. What it
//! adds is the part a terminal cannot give a non-technical user: native file
//! dialogs, archive extraction (zip / 7z / rar, all pure Rust), a progress
//! bar, and an install step that backs up what it replaces.

use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use modman_core::combine::{CombineOptions, CombineRun, CombineStage};

/// Paths may be passed on the command line ("open with"), and `--merge-now`
/// starts the merge immediately so CI can smoke-test the real code path.
struct Args {
    paths: Vec<PathBuf>,
    merge_now: bool,
    exit_when_done: bool,
    out: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args {
        paths: Vec::new(),
        merge_now: false,
        exit_when_done: false,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--merge-now" => args.merge_now = true,
            "--exit-when-done" => args.exit_when_done = true,
            "--out" => args.out = it.next().map(PathBuf::from),
            other if !other.starts_with('-') => args.paths.push(PathBuf::from(other)),
            _ => {}
        }
    }
    args
}

fn main() -> eframe::Result<()> {
    let args = parse_args();
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([940.0, 680.0])
            .with_min_inner_size([720.0, 520.0])
            .with_title("Project Modman"),
        ..Default::default()
    };
    eframe::run_native(
        "Project Modman",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, args)))),
    )
}

enum Msg {
    Stage(CombineStage),
    Extracting {
        label: String,
        done: usize,
        total: usize,
    },
    Done(Result<CombineRun, String>),
}

struct App {
    game_root: Option<PathBuf>,
    paks_dir: Option<PathBuf>,
    inputs: Vec<PathBuf>,
    out_dir: PathBuf,
    status: String,
    running: bool,
    rx: Option<Receiver<Msg>>,
    result: Option<CombineRun>,
    error: Option<String>,
    installed: Option<String>,
    auto_merge: bool,
    exit_when_done: bool,
    ctx: egui::Context,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        let detected = modman_core::game::detect_game();
        let (game_root, paks_dir) = match &detected {
            Some(g) => (Some(g.path.clone()), Some(g.paks_path.clone())),
            None => (None, None),
        };
        let out_dir = args
            .out
            .unwrap_or_else(|| std::env::temp_dir().join("modman-gui-out"));
        App {
            game_root,
            paks_dir,
            inputs: args.paths,
            out_dir,
            status: match detected {
                Some(g) => format!("Game found: {}", g.path.display()),
                None => "Game not found. Choose the Project Wingman folder.".to_string(),
            },
            running: false,
            rx: None,
            result: None,
            error: None,
            installed: None,
            auto_merge: args.merge_now,
            exit_when_done: args.exit_when_done,
            ctx: cc.egui_ctx.clone(),
        }
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            let is_mod = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    matches!(e.as_str(), "pak" | "zip" | "7z" | "rar")
                })
                .unwrap_or(false);
            if is_mod && !self.inputs.contains(&p) {
                self.inputs.push(p);
            }
        }
        self.inputs.sort();
    }

    fn start_merge(&mut self) {
        let Some(paks_dir) = self.paks_dir.clone() else {
            self.error = Some("No game install selected.".to_string());
            return;
        };
        if self.inputs.is_empty() {
            return;
        }

        self.running = true;
        self.error = None;
        self.result = None;
        self.installed = None;
        self.status = "Preparing…".to_string();

        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
        self.rx = Some(rx);

        let inputs = self.inputs.clone();
        let out_dir = self.out_dir.clone();
        let ctx = self.ctx.clone();
        let base = paks_dir.join("pakchunk0-WindowsNoEditor.pak");

        std::thread::spawn(move || {
            let send = |m: Msg| {
                let _ = tx.send(m);
                ctx.request_repaint();
            };

            // 1. Resolve inputs to paks, extracting archives on the way.
            let mut paks: Vec<PathBuf> = Vec::new();
            let total = inputs.len();
            for (i, path) in inputs.iter().enumerate() {
                let label = path
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                send(Msg::Extracting {
                    label: label.clone(),
                    done: i,
                    total,
                });

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
                let dest = std::env::temp_dir().join("modman-gui-extract").join(&stem);
                let _ = std::fs::remove_dir_all(&dest);

                let extracted = match extract_archive(path, &dest) {
                    Ok(n) => n,
                    Err(e) => {
                        send(Msg::Done(Err(format!("{label}: {e}"))));
                        return;
                    }
                };
                if extracted == 0 {
                    send(Msg::Done(Err(format!(
                        "{label}: no .pak files inside the archive"
                    ))));
                    return;
                }
                paks.extend(collect_paks(&dest));
            }
            paks.sort();
            if paks.is_empty() {
                send(Msg::Done(Err("no mod paks to merge".to_string())));
                return;
            }

            // 2. The shared merge, reporting each stage back to the UI.
            let tx2 = tx.clone();
            let ctx2 = ctx.clone();
            let run = modman_core::combine::run_to_dir(
                &base,
                &paks,
                &out_dir,
                &CombineOptions {
                    datatables_only: false,
                    ..Default::default()
                },
                |stage| {
                    let _ = tx2.send(Msg::Stage(stage.clone()));
                    ctx2.request_repaint();
                },
            );
            send(Msg::Done(run));
        });
    }

    fn poll(&mut self) {
        // Take the receiver out so handling a message can mutate self.
        let Some(rx) = self.rx.take() else { return };
        let mut finished = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Extracting { label, done, total } => {
                    self.status = format!("Extracting {label} ({}/{total})…", done + 1);
                }
                Msg::Stage(stage) => {
                    eprintln!("stage: {stage:?}");
                    self.status = match stage {
                        CombineStage::Opening {
                            index,
                            total,
                            label,
                        } => {
                            format!("Reading {label} ({index}/{total})…")
                        }
                        CombineStage::Merging => "Merging datatables…".to_string(),
                        CombineStage::Writing { files } => {
                            format!("Writing {files} file(s) to disk…")
                        }
                        CombineStage::Packing => "Packing the merged pak…".to_string(),
                    };
                }
                Msg::Done(res) => {
                    finished = true;
                    self.running = false;
                    match res {
                        Ok(run) => {
                            self.status = format!(
                                "Merged {} datatable(s), {} file(s) total.",
                                run.merged, run.files
                            );
                            eprintln!(
                                "MERGED {} datatable(s), {} file(s) -> {}",
                                run.merged,
                                run.files,
                                run.output.display()
                            );
                            self.result = Some(run);
                        }
                        Err(e) => {
                            eprintln!("FAILED {e}");
                            self.error = Some(e);
                            self.status = "Failed.".to_string();
                        }
                    }
                }
            }
        }
        if !finished {
            self.rx = Some(rx);
        } else if self.exit_when_done {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn install(&mut self) {
        let (Some(run), Some(paks_dir)) = (&self.result, &self.paks_dir) else {
            return;
        };
        match install_to_mods(&run.output, paks_dir) {
            Ok(path) => self.installed = Some(path.display().to_string()),
            Err(e) => self.error = Some(format!("install: {e}")),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.auto_merge && !self.running && self.result.is_none() {
            self.auto_merge = false;
            if !self.inputs.is_empty() && self.paks_dir.is_some() {
                self.start_merge();
            }
        }
        self.poll();

        // Drag and drop anywhere on the window.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.add_paths(dropped);
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.heading("Project Modman");
            ui.label("Merge Project Wingman mods that fight over the same files.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Choose game folder…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        let paks = dir.join("ProjectWingman/Content/Paks");
                        if paks.is_dir() {
                            self.paks_dir = Some(paks);
                            self.game_root = Some(dir);
                            self.status = "Game folder set.".to_string();
                        } else {
                            self.error = Some(format!(
                                "{} does not contain ProjectWingman/Content/Paks",
                                dir.display()
                            ));
                        }
                    }
                }
                match &self.game_root {
                    Some(p) => ui.label(format!("Game: {}", p.display())),
                    None => {
                        ui.colored_label(egui::Color32::from_rgb(255, 150, 90), "Game: not found")
                    }
                };
            });
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if self.running {
                    ui.spinner();
                }
                ui.label(&self.status);
            });
            if let Some(inst) = &self.installed {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 220, 140),
                    format!("Installed: {inst} (restart the game to load it)"),
                );
            }
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Add archives or paks…").clicked() {
                    if let Some(files) = rfd::FileDialog::new()
                        .add_filter("mods", &["pak", "zip", "7z", "rar"])
                        .pick_files()
                    {
                        self.add_paths(files);
                    }
                }
                if ui.button("Add a folder…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.add_paths(collect_paks(&dir));
                    }
                }
                if ui.button("Clear").clicked() {
                    self.inputs.clear();
                }
            });
            ui.label(
                egui::RichText::new("Drop .rar, .7z, .zip or .pak files anywhere on this window.")
                    .small()
                    .weak(),
            );
            ui.add_space(6.0);

            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for p in &self.inputs {
                        let name = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                        ui.label(format!("{name}  ({:.1} MB)", size as f64 / 1_048_576.0));
                    }
                    if self.inputs.is_empty() {
                        ui.weak("Nothing added yet.");
                    }
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Output folder:");
                ui.label(
                    egui::RichText::new(self.out_dir.display().to_string())
                        .small()
                        .weak(),
                );
                if ui.button("Change…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.out_dir = dir;
                    }
                }
            });

            ui.add_space(10.0);
            let can_merge = !self.running && !self.inputs.is_empty() && self.paks_dir.is_some();
            if ui
                .add_enabled(can_merge, egui::Button::new("Merge"))
                .clicked()
            {
                self.start_merge();
            }

            if let Some(err) = &self.error {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::from_rgb(255, 120, 120), err);
            }

            let result_snapshot = self.result.clone();
            let mut install_clicked = false;
            if let Some(run) = &result_snapshot {
                ui.separator();
                ui.heading("Report");
                ui.label(format!(
                    "Merged {} datatable(s), {} file(s) total",
                    run.merged, run.files
                ));
                ui.label(
                    egui::RichText::new(format!("Written to {}", run.output.display()))
                        .small()
                        .weak(),
                );

                if ui.button("Install into ~mods").clicked() {
                    install_clicked = true;
                }
                ui.label(
                    egui::RichText::new(
                        "Install backs up an existing SicarioCombine_P.pak first. Move the mod paks you \
                         merged out of ~mods afterwards, or the game loads the same edits twice.",
                    )
                    .small()
                    .weak(),
                );

                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    if !run.field_conflicts.is_empty() {
                        ui.add_space(6.0);
                        ui.strong("Field conflicts (later mod won)");
                        for c in &run.field_conflicts {
                            ui.label(egui::RichText::new(c).small());
                        }
                    }
                    if !run.pass_through_conflicts.is_empty() {
                        ui.add_space(6.0);
                        ui.strong("Files that cannot be combined (last kept)");
                        for c in &run.pass_through_conflicts {
                            ui.label(egui::RichText::new(c).small());
                        }
                    }
                    if !run.warnings.is_empty() {
                        ui.add_space(6.0);
                        ui.strong("Warnings");
                        for w in &run.warnings {
                            ui.label(egui::RichText::new(w).small());
                        }
                    }
                });
            }
            if install_clicked {
                self.install();
            }
        });
    }
}

// ── Archives ──────────────────────────────────────────────────────────────

/// Reject absolute paths and `..` so a hostile archive cannot write outside
/// the extraction directory.
fn safe_join(dest: &Path, name: &[u8]) -> Option<PathBuf> {
    let name = String::from_utf8_lossy(name).replace('\\', "/");
    if name.starts_with('/') || name.contains(':') {
        return None;
    }
    let mut out = dest.to_path_buf();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p => out.push(p),
        }
    }
    Some(out)
}

/// Extract one archive into `dest`, returning how many files landed.
fn extract_archive(path: &Path, dest: &Path) -> Result<usize, String> {
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
            sevenz_rust::decompress_file(path, dest).map_err(|e| e.to_string())?;
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

fn walk_files(dir: &Path) -> Vec<PathBuf> {
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

fn collect_paks(dir: &Path) -> Vec<PathBuf> {
    walk_files(dir)
        .into_iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("pak"))
        })
        .collect()
}

// ── Install ───────────────────────────────────────────────────────────────

/// Copy the merged pak into `~mods`, keeping a timestamped copy of whatever it
/// replaces. Overwriting silently is how a user loses the setup that worked.
fn install_to_mods(merged: &Path, paks_dir: &Path) -> Result<PathBuf, String> {
    let mods = paks_dir.join("~mods");
    std::fs::create_dir_all(&mods).map_err(|e| format!("{}: {e}", mods.display()))?;
    let target = mods.join("SicarioCombine_P.pak");
    if target.exists() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backup = mods.join(format!("SicarioCombine_P.pak.{stamp}.bak"));
        std::fs::rename(&target, &backup).map_err(|e| format!("backup: {e}"))?;
    }
    std::fs::copy(merged, &target).map_err(|e| format!("copy: {e}"))?;
    Ok(target)
}
