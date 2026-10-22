//! Merge tab: the original GUI, now one tab among several.

use eframe::egui;
use std::path::PathBuf;

use crate::archive;
use crate::job::Job;

pub struct MergeTab {
    pub inputs: Vec<PathBuf>,
    pub out_dir: PathBuf,
}

impl MergeTab {
    pub fn new(out_dir: PathBuf) -> Self {
        Self {
            inputs: Vec::new(),
            out_dir,
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
        self.inputs.sort();
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
                    self.add_paths(archive::collect_paks(&dir));
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
        let can = !self.inputs.is_empty() && game_paks.is_some();
        if ui.add_enabled(can, egui::Button::new("Merge")).clicked() {
            if let Some(paks) = game_paks {
                job = Some(Job::Merge {
                    base_pak: paks.join("pakchunk0-WindowsNoEditor.pak"),
                    inputs: self.inputs.clone(),
                    out_dir: self.out_dir.clone(),
                });
            }
        }
        job
    }
}
