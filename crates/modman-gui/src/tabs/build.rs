//! Build tab: gather the installed Sicario components, order them the way the
//! game's loader does, and emit one `SicarioMerge_P.pak`.

use eframe::egui;
use std::path::PathBuf;

use crate::job::Job;

pub struct BuildTab {
    pub inputs: Vec<PathBuf>,
    pub out_dir: PathBuf,
    pub embed_skins: bool,
}

impl BuildTab {
    pub fn new(out_dir: PathBuf) -> Self {
        Self {
            inputs: Vec::new(),
            out_dir,
            embed_skins: true,
        }
    }

    fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for p in paths {
            let ok = p.is_dir()
                || p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| {
                        let e = e.to_ascii_lowercase();
                        matches!(e.as_str(), "dtp" | "dtm" | "json")
                    })
                    .unwrap_or(false);
            if ok && !self.inputs.contains(&p) {
                self.inputs.push(p);
            }
        }
        self.inputs.sort();
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game_root: Option<&std::path::Path>) -> Option<Job> {
        let mut job = None;
        ui.label(
            egui::RichText::new(
                "Builds a mod pak from the presets you list plus everything installed in ~mods and ~presets, in the same order the game's loader uses.",
            )
            .small()
            .weak(),
        );

        ui.horizontal(|ui| {
            if ui.button("Add presets…").clicked() {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("presets", &["dtp", "dtm", "json"])
                    .pick_files()
                {
                    self.add_paths(files);
                }
            }
            if ui.button("Add a folder…").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.add_paths([dir]);
                }
            }
            if ui.button("Clear").clicked() {
                self.inputs.clear();
            }
        });

        egui::ScrollArea::vertical()
            .max_height(180.0)
            .show(ui, |ui| {
                for p in &self.inputs {
                    let name = p
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.display().to_string());
                    let kind = if p.is_dir() { "folder" } else { "preset" };
                    ui.label(format!("{name}  [{kind}]"));
                }
                if self.inputs.is_empty() {
                    ui.weak(
                        "Nothing listed. The build still picks up ~mods, ~presets and the \
                         install's Presets folder.",
                    );
                }
            });

        ui.add_space(6.0);
        ui.checkbox(
            &mut self.embed_skins,
            "Embed skin files from installed skin paks",
        );
        ui.label(
            egui::RichText::new(
                "On by default so the built pak works without the original skin paks installed.",
            )
            .small()
            .weak(),
        );

        ui.horizontal(|ui| {
            ui.label("Output folder:");
            ui.label(
                egui::RichText::new(self.out_dir.display().to_string())
                    .small()
                    .weak(),
            );
            if ui.button("Change…").clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    self.out_dir = d;
                }
            }
        });

        ui.add_space(8.0);
        if ui
            .add_enabled(game_root.is_some(), egui::Button::new("Build"))
            .clicked()
        {
            if let Some(root) = game_root {
                job = Some(Job::Build {
                    game_root: root.to_path_buf(),
                    inputs: self.inputs.clone(),
                    out_dir: self.out_dir.clone(),
                    embed_skins: self.embed_skins,
                });
            }
        }
        job
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_paths_accepts_presets_and_rejects_paks() {
        let mut tab = BuildTab::new(PathBuf::new());
        tab.add_paths([
            PathBuf::from("b.dtm"),
            PathBuf::from("a.dtp"),
            PathBuf::from("c.json"),
            PathBuf::from("d.pak"),
            PathBuf::from("e.zip"),
        ]);
        assert_eq!(
            tab.inputs,
            vec![
                PathBuf::from("a.dtp"),
                PathBuf::from("b.dtm"),
                PathBuf::from("c.json"),
            ]
        );
    }
}
