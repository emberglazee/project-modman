//! Translation tab: extract a `.locres`, patch it from a CSV, diff two of them.

use eframe::egui;
use std::path::PathBuf;

use crate::job::Job;

pub struct LocresTab {
    pub pak: Option<PathBuf>,
    pub lang: String,
    pub csv_out: bool,
    pub out_dir: PathBuf,
    pub patch_locres: Option<PathBuf>,
    pub patch_csv: Option<PathBuf>,
    pub patch_out: PathBuf,
    pub diff_a: Option<PathBuf>,
    pub diff_b: Option<PathBuf>,
}

impl LocresTab {
    pub fn new(out_dir: PathBuf) -> Self {
        Self {
            pak: None,
            lang: String::new(),
            csv_out: false,
            patch_out: out_dir.join("ProjectWingman.locres"),
            out_dir,
            patch_locres: None,
            patch_csv: None,
            diff_a: None,
            diff_b: None,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game_pak: Option<&std::path::Path>) -> Option<Job> {
        let mut job = None;
        ui.label(
            egui::RichText::new(
                "Patch mode keeps the file's namespaces, hashes and order and swaps only the strings, which is what the game's lookups depend on.",
            )
            .small()
            .weak(),
        );

        ui.separator();
        ui.strong("Extract from a pak");
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
            ui.label("Language:");
            ui.add(egui::TextEdit::singleline(&mut self.lang).desired_width(80.0));
            ui.label(egui::RichText::new("empty = every language").small().weak());
            ui.checkbox(&mut self.csv_out, "as CSV");
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
        let pak = self
            .pak
            .clone()
            .or_else(|| game_pak.map(|p| p.to_path_buf()));
        if ui
            .add_enabled(pak.is_some(), egui::Button::new("Extract"))
            .clicked()
        {
            job = Some(Job::LocresExtract {
                pak: pak.unwrap(),
                lang: (!self.lang.trim().is_empty()).then(|| self.lang.trim().to_string()),
                out_dir: self.out_dir.clone(),
                csv: self.csv_out,
            });
        }

        ui.separator();
        ui.strong("Patch from a CSV");
        ui.horizontal(|ui| {
            ui.label(".locres:");
            if ui.button("Choose…").clicked() {
                if let Some(f) = rfd::FileDialog::new()
                    .add_filter("locres", &["locres"])
                    .pick_file()
                {
                    self.patch_locres = Some(f);
                }
            }
            ui.label(
                egui::RichText::new(
                    self.patch_locres
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "none".into()),
                )
                .small()
                .weak(),
            );
        });
        ui.horizontal(|ui| {
            ui.label("CSV:");
            if ui.button("Choose…").clicked() {
                if let Some(f) = rfd::FileDialog::new()
                    .add_filter("csv", &["csv"])
                    .pick_file()
                {
                    self.patch_csv = Some(f);
                }
            }
            ui.label(
                egui::RichText::new(
                    self.patch_csv
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "none".into()),
                )
                .small()
                .weak(),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Output file:");
            ui.label(
                egui::RichText::new(self.patch_out.display().to_string())
                    .small()
                    .weak(),
            );
            if ui.button("Change…").clicked() {
                if let Some(f) = rfd::FileDialog::new().save_file() {
                    self.patch_out = f;
                }
            }
        });
        let can_patch = self.patch_locres.is_some() && self.patch_csv.is_some();
        if ui
            .add_enabled(can_patch, egui::Button::new("Patch"))
            .clicked()
        {
            job = Some(Job::LocresPatch {
                locres: self.patch_locres.clone().unwrap(),
                csv: self.patch_csv.clone().unwrap(),
                out: self.patch_out.clone(),
            });
        }

        ui.separator();
        ui.strong("Compare two files");
        ui.horizontal(|ui| {
            for (label, slot) in [("A:", &mut self.diff_a), ("B:", &mut self.diff_b)] {
                ui.label(label);
                if ui.button("Choose…").clicked() {
                    if let Some(f) = rfd::FileDialog::new()
                        .add_filter("locres", &["locres"])
                        .pick_file()
                    {
                        *slot = Some(f);
                    }
                }
                ui.label(
                    egui::RichText::new(
                        slot.as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "none".into()),
                    )
                    .small()
                    .weak(),
                );
            }
        });
        if ui
            .add_enabled(
                self.diff_a.is_some() && self.diff_b.is_some(),
                egui::Button::new("Compare"),
            )
            .clicked()
        {
            job = Some(Job::LocresDiff {
                a: self.diff_a.clone().unwrap(),
                b: self.diff_b.clone().unwrap(),
            });
        }
        job
    }
}
