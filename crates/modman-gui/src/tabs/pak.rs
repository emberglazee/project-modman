//! Pak tab: inspect, unpack and repack `.pak` files.

use eframe::egui;
use std::path::PathBuf;

use crate::job::Job;

pub struct PakTab {
    pub pak: Option<PathBuf>,
    pub out_dir: PathBuf,
    pub pack_dir: Option<PathBuf>,
    pub pack_out: PathBuf,
}

impl PakTab {
    pub fn new(out_dir: PathBuf) -> Self {
        Self {
            pak: None,
            pack_out: out_dir.join("packed_P.pak"),
            out_dir,
            pack_dir: None,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Job> {
        let mut job = None;

        ui.separator();
        ui.strong("Inspect or unpack a pak");
        ui.horizontal(|ui| {
            ui.label("Pak:");
            if ui.button("Choose…").clicked() {
                if let Some(f) = rfd::FileDialog::new()
                    .add_filter("pak", &["pak"])
                    .pick_file()
                {
                    self.pak = Some(f);
                }
            }
            ui.label(
                egui::RichText::new(
                    self.pak
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "none".into()),
                )
                .small()
                .weak(),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Unpack into:");
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
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.pak.is_some(), egui::Button::new("Show info"))
                .clicked()
            {
                job = Some(Job::PakInfo {
                    pak: self.pak.clone().unwrap(),
                });
            }
            if ui
                .add_enabled(self.pak.is_some(), egui::Button::new("Unpack"))
                .clicked()
            {
                job = Some(Job::PakUnpack {
                    pak: self.pak.clone().unwrap(),
                    out_dir: self.out_dir.clone(),
                });
            }
        });

        ui.separator();
        ui.strong("Pack a folder into a mod pak");
        ui.horizontal(|ui| {
            ui.label("Folder:");
            if ui.button("Choose…").clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    self.pack_dir = Some(d);
                }
            }
            ui.label(
                egui::RichText::new(
                    self.pack_dir
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "none".into()),
                )
                .small()
                .weak(),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Output:");
            ui.label(
                egui::RichText::new(self.pack_out.display().to_string())
                    .small()
                    .weak(),
            );
            if ui.button("Change…").clicked() {
                if let Some(f) = rfd::FileDialog::new().save_file() {
                    self.pack_out = f;
                }
            }
        });
        if ui
            .add_enabled(self.pack_dir.is_some(), egui::Button::new("Pack"))
            .clicked()
        {
            job = Some(Job::PakPack {
                dir: self.pack_dir.clone().unwrap(),
                out: self.pack_out.clone(),
            });
        }
        job
    }
}
