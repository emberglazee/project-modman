//! modman-gui — the native desktop front end.
//!
//! Every operation calls into `modman-core`, the same code the CLI runs, so the
//! GUI cannot grow behaviour the command line does not have. The window is a
//! shell: a tab bar, one job at a time on a worker thread, and a log.

mod archive;
mod job;
mod tabs;

use eframe::egui;
use job::{Job, Msg};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use tabs::Tab;

/// Paths may be passed on the command line ("open with"), and `--merge-now`
/// starts a merge immediately so CI can smoke-test the real code path.
struct Args {
    paths: Vec<PathBuf>,
    merge_now: bool,
    probe_now: bool,
    build_now: bool,
    first_wins: bool,
    exit_when_done: bool,
    version: bool,
    out: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args {
        paths: Vec::new(),
        merge_now: false,
        probe_now: false,
        build_now: false,
        first_wins: false,
        exit_when_done: false,
        version: false,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--merge-now" => args.merge_now = true,
            "--probe-now" => args.probe_now = true,
            "--build-now" => args.build_now = true,
            "--first-wins" => args.first_wins = true,
            "--exit-when-done" => args.exit_when_done = true,
            "--version" | "-V" => args.version = true,
            "--out" => args.out = it.next().map(PathBuf::from),
            other if !other.starts_with('-') => args.paths.push(PathBuf::from(other)),
            _ => {}
        }
    }
    args
}

/// Headless conflict probe for CI. Prints one line and exits 0 on a report,
/// 1 on any error, without creating a window.
fn run_probe(args: &Args) -> i32 {
    if args.paths.is_empty() {
        eprintln!("no mod paks given");
        return 1;
    }
    let Some(game) = modman_core::game::detect_game() else {
        eprintln!("No game pak found");
        return 1;
    };
    let Some(base_pak) = modman_core::game::find_base_pak(&game.paks_path) else {
        eprintln!("No game pak found in {}", game.paks_path.display());
        return 1;
    };
    let mut inputs = args.paths.clone();
    if args.first_wins {
        inputs.reverse();
    }
    let (paks, _temps) = match job::prepare_inputs(&inputs, |_| {}) {
        Ok(found) => found,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    match modman_core::combine::probe(&base_pak, &paks, &Default::default()) {
        Ok(report) => {
            println!(
                "PROBE OK merged={} files={} order={} conflicts={} passThrough={} warnings={} skipped={}",
                report.merged,
                report.files,
                report.order.join(","),
                report.field_conflicts.len(),
                report.pass_through_conflicts.len(),
                report.warnings.len(),
                report.skipped.len()
            );
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn main() -> eframe::Result<()> {
    let args = parse_args();
    // Answers the packaging smoke test without needing a display.
    if args.version {
        println!("modman-gui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.probe_now {
        std::process::exit(run_probe(&args));
    }
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 720.0])
            .with_min_inner_size([780.0, 560.0])
            .with_title("Project Modman"),
        ..Default::default()
    };
    eframe::run_native(
        "Project Modman",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, args)))),
    )
}

struct App {
    game_root: Option<PathBuf>,
    paks_dir: Option<PathBuf>,
    tab: Tab,
    merge: tabs::merge::MergeTab,
    build: tabs::build::BuildTab,
    audio: tabs::audio::AudioTab,
    locres: tabs::locres::LocresTab,
    pak: tabs::pak::PakTab,
    tools: tabs::tools::ToolsTab,
    rx: Option<Receiver<Msg>>,
    running: bool,
    status: String,
    log: String,
    /// The pak the finished job produced, ready to install into `~mods`.
    install_path: Option<PathBuf>,
    installed: Option<String>,
    auto_merge: bool,
    auto_build: bool,
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
        let out_for_build = out_dir.clone();
        let mut merge = tabs::merge::MergeTab::new(out_dir);
        merge.add_paths(args.paths.clone());
        if args.first_wins {
            merge.direction = tabs::merge::PriorityDirection::EarlierWins;
        }

        App {
            game_root,
            paks_dir,
            tab: Tab::Merge,
            merge,
            build: {
                let mut b = tabs::build::BuildTab::new(out_for_build);
                b.inputs = args.paths;
                b
            },
            audio: tabs::audio::AudioTab::new(std::env::temp_dir().join("modman-gui-audio")),
            locres: tabs::locres::LocresTab::new(std::env::temp_dir().join("modman-gui-locres")),
            pak: tabs::pak::PakTab::new(std::env::temp_dir().join("modman-gui-pak")),
            tools: tabs::tools::ToolsTab::new(),
            rx: None,
            running: false,
            status: match detected {
                Some(g) => format!("Game found: {}", g.path.display()),
                None => "Game not found. Choose the Project Wingman folder.".to_string(),
            },
            log: String::new(),
            install_path: None,
            installed: None,
            auto_merge: args.merge_now,
            auto_build: args.build_now,
            exit_when_done: args.exit_when_done,
            ctx: cc.egui_ctx.clone(),
        }
    }

    fn start(&mut self, job: Job, is_merge: bool) {
        self.running = true;
        self.status = "Working…".to_string();
        self.log.clear();
        if is_merge {
            self.installed = None;
        }
        self.install_path = None;
        self.rx = Some(job::spawn(job, self.ctx.clone()));
    }

    fn poll(&mut self) {
        // Take the receiver out so handling a message can mutate self.
        let Some(rx) = self.rx.take() else { return };
        let mut finished = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Stage(text) => self.status = text,
                Msg::Done(res) => {
                    finished = true;
                    self.running = false;
                    match res {
                        Ok(out) => {
                            self.status = "Done.".to_string();
                            self.log.push_str(&out.report);
                            self.log.push('\n');
                            eprintln!("DONE {}", out.report);
                            self.install_path = out.install;
                        }
                        Err(e) => {
                            self.status = "Failed.".to_string();
                            self.log.push_str(&format!("Error: {e}\n"));
                            eprintln!("FAILED {e}");
                            self.install_path = None;
                        }
                    }
                }
            }
        }
        if !finished
            && matches!(
                rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            )
        {
            // The worker panicked or exited without sending Done; without this
            // the spinner would run forever.
            self.running = false;
            self.status = "Failed.".to_string();
            self.log
                .push_str("Error: the worker stopped without a result\n");
            eprintln!("FAILED worker disconnected");
            finished = true;
        }
        if !finished {
            self.rx = Some(rx);
        } else if self.exit_when_done {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn install(&mut self) {
        let (Some(built), Some(paks)) = (self.install_path.clone(), self.paks_dir.clone()) else {
            return;
        };
        let name = built
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "mod_P.pak".to_string());
        match archive::install_to_mods(&built, &paks, &name) {
            Ok(p) => self.installed = Some(p.display().to_string()),
            Err(e) => self.log.push_str(&format!("install: {e}\n")),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();

        if self.auto_merge && !self.running {
            self.auto_merge = false;
            if !self.merge.inputs.is_empty() {
                if let Some(paks) = self.paks_dir.clone() {
                    match modman_core::game::find_base_pak(&paks) {
                        Some(base_pak) => {
                            let job = Job::Merge {
                                base_pak,
                                inputs: self.merge.application_inputs(),
                                out_dir: self.merge.out_dir.clone(),
                            };
                            self.start(job, true);
                        }
                        None => self
                            .log
                            .push_str(&format!("No game pak found in {}\n", paks.display())),
                    }
                }
            }
        }

        if self.auto_build && !self.running {
            self.auto_build = false;
            if let Some(root) = self.game_root.clone() {
                let job = Job::Build {
                    game_root: root,
                    inputs: self.build.inputs.clone(),
                    out_dir: self.build.out_dir.clone(),
                    embed_skins: self.build.embed_skins,
                };
                self.start(job, false);
            }
        }

        // Drag and drop anywhere on the window feeds the merge tab.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.merge.add_paths(dropped);
            self.tab = Tab::Merge;
        }

        self.poll();

        egui::Panel::top("top").show(root, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Project Modman");
                ui.label(egui::RichText::new("for Project Wingman").small().weak());
            });
            ui.horizontal(|ui| {
                if ui.button("Choose game folder…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        let paks = dir.join("ProjectWingman/Content/Paks");
                        if paks.is_dir() {
                            self.paks_dir = Some(paks);
                            self.game_root = Some(dir);
                            self.status = "Game folder set.".to_string();
                        } else {
                            self.log = format!(
                                "{} does not contain ProjectWingman/Content/Paks\n",
                                dir.display()
                            );
                        }
                    }
                }
                match &self.game_root {
                    Some(p) => ui.label(
                        egui::RichText::new(format!("Game: {}", p.display()))
                            .small()
                            .weak(),
                    ),
                    None => {
                        ui.colored_label(egui::Color32::from_rgb(255, 150, 90), "Game: not found")
                    }
                };
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for tab in [
                    Tab::Merge,
                    Tab::Build,
                    Tab::Audio,
                    Tab::Locres,
                    Tab::Pak,
                    Tab::Tools,
                ] {
                    if ui.selectable_label(self.tab == tab, tab.label()).clicked() {
                        self.tab = tab;
                    }
                }
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("status").show(root, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if self.running {
                    ui.spinner();
                }
                ui.label(&self.status);
                if !self.running
                    && self.install_path.is_some()
                    && ui.button("Install into ~mods").clicked()
                {
                    self.install();
                }
            });
            if let Some(inst) = &self.installed {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 220, 140),
                    format!("Installed: {inst} (restart the game to load it)"),
                );
            }
            if !self.log.is_empty() {
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .show(ui, |ui| {
                        let mut text = self.log.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .desired_width(f32::INFINITY)
                                .font(egui::TextStyle::Monospace),
                        );
                    });
            }
            ui.add_space(4.0);
        });

        let game_paks = self.paks_dir.clone();
        let game_pak = game_paks
            .as_ref()
            .and_then(|p| modman_core::game::find_base_pak(p));
        let mut job: Option<(Job, bool)> = None;

        let busy = self.running;
        egui::CentralPanel::default().show(root, |ui| {
            ui.add_enabled_ui(!busy, |ui| match self.tab {
                Tab::Merge => {
                    if let Some(j) = self.merge.ui(ui, game_paks.as_deref()) {
                        job = Some((j, true));
                    }
                }
                Tab::Build => {
                    if let Some(j) = self.build.ui(ui, self.game_root.as_deref()) {
                        job = Some((j, false));
                    }
                }
                Tab::Audio => {
                    if let Some(j) = self.audio.ui(ui, game_pak.as_deref()) {
                        job = Some((j, false));
                    }
                }
                Tab::Locres => {
                    if let Some(j) = self.locres.ui(ui, game_pak.as_deref()) {
                        job = Some((j, false));
                    }
                }
                Tab::Pak => {
                    if let Some(j) = self.pak.ui(ui) {
                        job = Some((j, false));
                    }
                }
                Tab::Tools => {
                    if let Some(j) = self.tools.ui(ui, game_paks.as_deref()) {
                        job = Some((j, false));
                    }
                }
            });
        });

        if let Some((job, is_merge)) = job {
            if !self.running {
                self.start(job, is_merge);
            }
        }
    }
}
