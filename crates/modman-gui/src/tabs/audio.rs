//! Audio tab: list the game's sound assets, or extract them as .ogg.

use eframe::egui;
use std::path::PathBuf;

use crate::job::Job;

pub struct AudioTab {
    pub pak: Option<PathBuf>,
    pub filter: String,
    pub out_dir: PathBuf,
    pub manifest: bool,
}

impl AudioTab {
    pub fn new(out_dir: PathBuf) -> Self {
        Self {
            pak: None,
            filter: String::new(),
            out_dir,
            manifest: true,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game_pak: Option<&std::path::Path>) -> Option<Job> {
        let mut job = None;
        ui.label(
            egui::RichText::new(
                "The game stores its audio as Ogg Vorbis, so extraction copies the stream out without re-encoding.",
            )
            .small()
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.label("Pak:");
            let name = self
                .pak
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| match game_pak {
                    Some(_) => "the game's own pak (default)".to_string(),
                    None => "not found".to_string(),
                });
            ui.label(egui::RichText::new(name).small().weak());
            if ui.button("Choose…").clicked() {
                if let Some(f) = rfd::FileDialog::new()
                    .add_filter("pak", &["pak"])
                    .pick_file()
                {
                    self.pak = Some(f);
                }
            }
            if self.pak.is_some() && ui.button("Use game pak").clicked() {
                self.pak = None;
            }
        });

        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.filter);
            ui.label(
                egui::RichText::new("substring of the asset path, e.g. Dialog/C_M6/")
                    .small()
                    .weak(),
            );
        });
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
        ui.checkbox(&mut self.manifest, "Write a manifest CSV");

        let pak = self
            .pak
            .clone()
            .or_else(|| game_pak.map(|p| p.to_path_buf()));
        let filter = (!self.filter.trim().is_empty()).then(|| self.filter.trim().to_string());

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(pak.is_some(), egui::Button::new("List sounds"))
                .clicked()
            {
                job = Some(Job::AudioList {
                    pak: pak.clone().unwrap(),
                    filter: filter.clone(),
                });
            }
            if ui
                .add_enabled(pak.is_some(), egui::Button::new("Extract to .ogg"))
                .clicked()
            {
                job = Some(Job::AudioExtract {
                    pak: pak.clone().unwrap(),
                    out_dir: self.out_dir.clone(),
                    filter,
                    manifest: self.manifest.then(|| self.out_dir.join("manifest.csv")),
                });
            }
        });
        job
    }
}
