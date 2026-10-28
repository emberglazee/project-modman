//! Merge tab: the original GUI, now one tab among several.

use eframe::egui;
use std::path::PathBuf;

use crate::archive;
use crate::job::Job;

/// Which end of the display list is applied last and wins a conflict. The list
/// itself never moves when this changes: `application_inputs` flips the order
/// fed to the merger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PriorityDirection {
    #[default]
    LaterWins,
    EarlierWins,
}

pub struct MergeTab {
    pub inputs: Vec<PathBuf>,
    pub out_dir: PathBuf,
    pub direction: PriorityDirection,
}

impl MergeTab {
    pub fn new(out_dir: PathBuf) -> Self {
        Self {
            inputs: Vec::new(),
            out_dir,
            direction: PriorityDirection::default(),
        }
    }

    pub fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
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
            }
        }
    }

    pub fn move_up(&mut self, index: usize) {
        if index > 0 && index < self.inputs.len() {
            self.inputs.swap(index - 1, index);
        }
    }

    pub fn move_down(&mut self, index: usize) {
        if index + 1 < self.inputs.len() {
            self.inputs.swap(index, index + 1);
        }
    }

    /// The order handed to the merger: the display list, or its reverse when
    /// the earlier entry should win.
    pub fn application_inputs(&self) -> Vec<PathBuf> {
        match self.direction {
            PriorityDirection::LaterWins => self.inputs.clone(),
            PriorityDirection::EarlierWins => self.inputs.iter().rev().cloned().collect(),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game_paks: Option<&std::path::Path>) -> Option<Job> {
        let mut job = None;
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
            }
        });

        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                for p in &self.inputs {
                    let name = p
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                    ui.label(format!("{name}  ({:.1} MB)", size as f64 / 1_048_576.0));
                }
                if self.inputs.is_empty() {
                    ui.weak("Nothing added yet.");
                }
            });

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

        ui.add_space(8.0);
        let base_pak = game_paks.and_then(modman_core::game::find_base_pak);
        let can = !self.inputs.is_empty() && base_pak.is_some();
        if ui.add_enabled(can, egui::Button::new("Merge")).clicked() {
            if let Some(base_pak) = base_pak {
                job = Some(Job::Merge {
                    base_pak,
                    inputs: self.application_inputs(),
                    out_dir: self.out_dir.clone(),
                });
            }
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
}
