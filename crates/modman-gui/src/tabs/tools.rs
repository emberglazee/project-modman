//! Tools tab: scan a Paks folder for Sicario components, inspect a patch file.

use eframe::egui;
use std::path::PathBuf;

use crate::job::Job;

pub struct ToolsTab {
    pub paks_dir: Option<PathBuf>,
    pub patch_file: Option<PathBuf>,
}

impl ToolsTab {
    pub fn new() -> Self {
        Self {
            paks_dir: None,
            patch_file: None,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game_paks: Option<&std::path::Path>) -> Option<Job> {
        let mut job = None;

        ui.separator();
        ui.strong("Scan for installed Sicario mods");
        ui.label(
            egui::RichText::new(
                "Reads the paks folder the way the merger does: embedded presets, loose presets, build requests.",
            )
            .small()
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.label("Paks folder:");
            ui.label(
                egui::RichText::new(match (&self.paks_dir, game_paks) {
                    (Some(p), _) => p.display().to_string(),
                    (None, Some(p)) => format!("{} (game default)", p.display()),
                    (None, None) => "not found".to_string(),
                })
                .small()
                .weak(),
            );
            if ui.button("Choose…").clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    self.paks_dir = Some(d);
                }
            }
        });
        let dir = self
            .paks_dir
            .clone()
            .or_else(|| game_paks.map(|p| p.to_path_buf()));
        if ui
            .add_enabled(dir.is_some(), egui::Button::new("Scan"))
            .clicked()
        {
            job = Some(Job::Scan {
                paks_dir: dir.unwrap(),
            });
        }

        ui.separator();
        ui.strong("Inspect a patch file");
        ui.label(
            egui::RichText::new("A .dtm patch, a .dtp preset, or a pak-embedded build request.")
                .small()
                .weak(),
        );
        ui.horizontal(|ui| {
            if ui.button("Choose…").clicked() {
                if let Some(f) = rfd::FileDialog::new()
                    .add_filter("patch", &["dtm", "dtp", "json"])
                    .pick_file()
                {
                    self.patch_file = Some(f);
                }
            }
            ui.label(
                egui::RichText::new(
                    self.patch_file
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "none".into()),
                )
                .small()
                .weak(),
            );
        });
        if ui
            .add_enabled(self.patch_file.is_some(), egui::Button::new("Inspect"))
            .clicked()
        {
            job = Some(Job::PatchInspect {
                file: self.patch_file.clone().unwrap(),
            });
        }
        job
    }
}
