//! Merge tab: the original GUI, now one tab among several.

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crate::archive;
use crate::job::{prepare_inputs, Job};
use modman_core::combine::{CombineOptions, CombineReport};

/// Which end of the display list is applied last and wins a conflict. The list
/// itself never moves when this changes: `application_inputs` flips the order
/// fed to the merger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PriorityDirection {
    #[default]
    LaterWins,
    EarlierWins,
}

/// A probe runs on its own thread so it never takes the single job slot and
/// never disables Merge. `generation` is the latest list revision; a result
/// from an older revision is dropped.
#[derive(Default)]
struct ProbeState {
    generation: u64,
    inflight: Option<u64>,
    due: Option<Instant>,
    receiver: Option<Receiver<(u64, Result<CombineReport, String>)>>,
    result: Option<CombineReport>,
    error: Option<String>,
}

pub struct MergeTab {
    pub inputs: Vec<PathBuf>,
    pub out_dir: PathBuf,
    pub direction: PriorityDirection,
    probe: ProbeState,
}

impl MergeTab {
    pub fn new(out_dir: PathBuf) -> Self {
        Self {
            inputs: Vec::new(),
            out_dir,
            direction: PriorityDirection::default(),
            probe: ProbeState::default(),
        }
    }

    pub fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let mut added = false;
        for p in paths {
            let ok = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    matches!(e.as_str(), "pak" | "zip" | "7z" | "rar")
                })
                .unwrap_or(false);
            if ok && !self.inputs.contains(&p) {
                self.inputs.push(p);
                added = true;
            }
        }
        if added {
            self.schedule_probe();
        }
    }

    pub fn move_up(&mut self, index: usize) {
        if index > 0 && index < self.inputs.len() {
            self.inputs.swap(index - 1, index);
        }
        self.schedule_probe();
    }

    pub fn move_down(&mut self, index: usize) {
        if index + 1 < self.inputs.len() {
            self.inputs.swap(index, index + 1);
        }
        self.schedule_probe();
    }

    fn move_to(&mut self, from: usize, to: usize) {
        if from < self.inputs.len() && to < self.inputs.len() && from != to {
            let item = self.inputs.remove(from);
            self.inputs.insert(to, item);
        }
        self.schedule_probe();
    }

    /// The order handed to the merger: the display list, or its reverse when
    /// the earlier entry should win.
    pub fn application_inputs(&self) -> Vec<PathBuf> {
        match self.direction {
            PriorityDirection::LaterWins => self.inputs.clone(),
            PriorityDirection::EarlierWins => self.inputs.iter().rev().cloned().collect(),
        }
    }

    fn schedule_probe(&mut self) {
        self.probe.generation = self.probe.generation.wrapping_add(1);
        self.probe.due = Some(Instant::now() + Duration::from_millis(400));
    }

    fn check_now(&mut self) {
        self.probe.generation = self.probe.generation.wrapping_add(1);
        self.probe.due = Some(Instant::now());
    }

    fn poll_probe(&mut self) {
        let Some(receiver) = self.probe.receiver.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok((generation, result)) => {
                self.probe.inflight = None;
                if generation == self.probe.generation {
                    match result {
                        Ok(report) => {
                            self.probe.result = Some(report);
                            self.probe.error = None;
                        }
                        Err(error) => self.probe.error = Some(error),
                    }
                }
            }
            Err(TryRecvError::Empty) => self.probe.receiver = Some(receiver),
            Err(TryRecvError::Disconnected) => {
                self.probe.inflight = None;
                self.probe.error = Some("probe worker stopped".to_string());
            }
        }
    }

    fn maybe_start_probe(&mut self, base_pak: &std::path::Path, ctx: egui::Context) {
        if self.probe.inflight.is_some() || self.inputs.is_empty() {
            return;
        }
        let Some(due) = self.probe.due else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        self.probe.due = None;
        let generation = self.probe.generation;
        let inputs = self.application_inputs();
        let base = base_pak.to_path_buf();
        let (sender, receiver) = channel();
        self.probe.inflight = Some(generation);
        self.probe.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = (|| -> Result<CombineReport, String> {
                let (paks, _temps) = prepare_inputs(&inputs, |_| {})?;
                if paks.is_empty() {
                    return Err("no mod paks to merge".to_string());
                }
                // `_temps` lives until this closure returns, so archive
                // extraction directories outlive the probe's readers.
                modman_core::combine::probe(&base, &paks, &CombineOptions::default())
            })();
            let _ = sender.send((generation, result));
            // The UI polls the receiver in `ui`; without a repaint the result
            // would sit unread until the next input event.
            ctx.request_repaint();
        });
    }

    fn probe_panel(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Conflicts").strong());
            if self.probe.inflight.is_some() {
                ui.spinner();
                ui.weak("Checking…");
            }
        });
        if let Some(error) = &self.probe.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        let Some(report) = &self.probe.result else {
            return;
        };
        ui.label(format!(
            "{} field conflict(s), {} file(s) that can't be combined, {} warning(s)",
            report.field_conflicts.len(),
            report.pass_through_conflicts.len(),
            report.warnings.len()
        ));
        for (title, list) in [
            ("Field conflicts", &report.field_conflicts),
            (
                "Files that can't be combined",
                &report.pass_through_conflicts,
            ),
            ("Warnings", &report.warnings),
        ] {
            if list.is_empty() {
                continue;
            }
            egui::CollapsingHeader::new(format!("{title} ({})", list.len())).show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for line in list {
                            ui.label(line);
                        }
                    });
            });
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game_paks: Option<&std::path::Path>) -> Option<Job> {
        let mut job = None;
        let base_pak = game_paks.and_then(modman_core::game::find_base_pak);
        self.poll_probe();

        ui.label(
            egui::RichText::new(
                "Drop .rar, .7z, .zip or .pak files on this window. Archives are extracted for you.",
            )
            .small()
            .weak(),
        );
        ui.horizontal(|ui| {
            if ui.button("Add files…").clicked() {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("mods", &["pak", "zip", "7z", "rar"])
                    .pick_files()
                {
                    self.add_paths(files);
                }
            }
            if ui.button("Add a folder…").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    // A bulk folder import has no user order and read_dir
                    // order is not stable; sort it before appending. Individual
                    // files and drops keep the order they arrived in.
                    let mut paks = archive::collect_paks(&dir);
                    paks.sort();
                    self.add_paths(paks);
                }
            }
            if ui.button("Clear").clicked() {
                self.inputs.clear();
                self.probe.result = None;
                self.probe.error = None;
                self.schedule_probe();
            }
        });

        ui.add_space(4.0);
        let before = self.direction;
        ui.horizontal(|ui| {
            ui.label("Priority:");
            ui.selectable_value(
                &mut self.direction,
                PriorityDirection::LaterWins,
                "Later in the list wins",
            );
            ui.selectable_value(
                &mut self.direction,
                PriorityDirection::EarlierWins,
                "Earlier in the list wins",
            );
        });
        if self.direction != before {
            self.schedule_probe();
        }
        ui.label(
            egui::RichText::new(match self.direction {
                PriorityDirection::LaterWins => "Applied top to bottom. Later entries win.",
                PriorityDirection::EarlierWins => "Applied bottom to top. Earlier entries win.",
            })
            .small()
            .weak(),
        );

        enum RowAction {
            Up(usize),
            Down(usize),
            Remove(usize),
            Move(usize, usize),
        }
        let mut action: Option<RowAction> = None;
        let len = self.inputs.len();
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .show(ui, |ui| {
                for i in 0..len {
                    let path = self.inputs[i].clone();
                    let name = path
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let wins = len > 1
                        && match self.direction {
                            PriorityDirection::LaterWins => i + 1 == len,
                            PriorityDirection::EarlierWins => i == 0,
                        };
                    let (_, dropped) = ui.dnd_drop_zone::<usize, _>(egui::Frame::NONE, |ui| {
                        ui.horizontal(|ui| {
                            ui.dnd_drag_source(
                                ui.scope_id().with(("modman-merge-drag", i)),
                                i,
                                |ui| {
                                    ui.label(egui::RichText::new("::").monospace())
                                        .on_hover_text("Drag to reorder");
                                },
                            );
                            ui.label(format!("{}.", i + 1));
                            ui.label(name);
                            ui.label(format!("({:.1} MB)", size as f64 / 1_048_576.0));
                            if wins {
                                ui.label(
                                    egui::RichText::new("wins")
                                        .small()
                                        .strong()
                                        .color(egui::Color32::from_rgb(120, 200, 120)),
                                );
                            }
                            if ui
                                .add_enabled(i > 0, egui::Button::new("Up"))
                                .on_hover_text("Move up")
                                .clicked()
                            {
                                action = Some(RowAction::Up(i));
                            }
                            if ui
                                .add_enabled(i + 1 < len, egui::Button::new("Down"))
                                .on_hover_text("Move down")
                                .clicked()
                            {
                                action = Some(RowAction::Down(i));
                            }
                            if ui.button("x").on_hover_text("Remove").clicked() {
                                action = Some(RowAction::Remove(i));
                            }
                        });
                    });
                    if let Some(from) = dropped {
                        if *from != i {
                            action = Some(RowAction::Move(*from, i));
                        }
                    }
                }
                if len == 0 {
                    ui.weak("Nothing added yet.");
                }
            });

        match action {
            Some(RowAction::Up(i)) => self.move_up(i),
            Some(RowAction::Down(i)) => self.move_down(i),
            Some(RowAction::Remove(i)) => {
                if i < self.inputs.len() {
                    self.inputs.remove(i);
                }
                self.schedule_probe();
            }
            Some(RowAction::Move(from, to)) => self.move_to(from, to),
            None => {}
        }

        ui.add_space(6.0);
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

        ui.add_space(6.0);
        self.probe_panel(ui);
        if ui
            .add_enabled(
                self.probe.inflight.is_none() && !self.inputs.is_empty(),
                egui::Button::new("Check now"),
            )
            .clicked()
        {
            self.check_now();
        }

        ui.add_space(8.0);
        let can = !self.inputs.is_empty() && base_pak.is_some();
        if ui.add_enabled(can, egui::Button::new("Merge")).clicked() {
            if let Some(base_pak) = &base_pak {
                job = Some(Job::Merge {
                    base_pak: base_pak.clone(),
                    inputs: self.application_inputs(),
                    out_dir: self.out_dir.clone(),
                });
            }
        }
        if let Some(base) = &base_pak {
            self.maybe_start_probe(base, ui.ctx().clone());
        }
        // egui sleeps between input events. Wake it for the debounce deadline
        // and while a probe is running; the thread wakes it when it finishes.
        if let Some(due) = self.probe.due {
            ui.ctx()
                .request_repaint_after(due.saturating_duration_since(Instant::now()));
        }
        if self.probe.inflight.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        job
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(names: &[&str]) -> MergeTab {
        let mut tab = MergeTab::new(PathBuf::from("/tmp"));
        tab.inputs = names.iter().map(PathBuf::from).collect();
        tab
    }

    #[test]
    fn move_up_and_down_stop_at_the_ends() {
        let mut tab = tab(&["a", "b", "c"]);
        tab.move_up(0);
        tab.move_down(2);
        assert_eq!(
            tab.inputs,
            vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]
        );
        tab.move_down(0);
        assert_eq!(
            tab.inputs,
            vec![PathBuf::from("b"), PathBuf::from("a"), PathBuf::from("c")]
        );
        tab.move_up(2);
        assert_eq!(
            tab.inputs,
            vec![PathBuf::from("b"), PathBuf::from("c"), PathBuf::from("a")]
        );
    }

    #[test]
    fn application_order_follows_the_direction() {
        let mut tab = tab(&["a", "b", "c"]);
        assert_eq!(
            tab.application_inputs(),
            vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]
        );
        tab.direction = PriorityDirection::EarlierWins;
        assert_eq!(
            tab.application_inputs(),
            vec![PathBuf::from("c"), PathBuf::from("b"), PathBuf::from("a")]
        );
    }

    #[test]
    fn move_to_reorders_and_keeps_every_entry() {
        let mut tab = tab(&["a", "b", "c"]);
        tab.move_to(0, 2);
        assert_eq!(
            tab.inputs,
            vec![PathBuf::from("b"), PathBuf::from("c"), PathBuf::from("a")]
        );
        tab.move_to(2, 0);
        assert_eq!(
            tab.inputs,
            vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]
        );
    }
}
