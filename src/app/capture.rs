//! Virtual sensor preview and PC recording controls. Firmware surfaces stay separate.

use super::{App, Page, theme};
use crate::capture::{Command, FPS, HEIGHT, WIDTH};
use eframe::egui::{self, RichText, Vec2};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

impl App {
    pub(super) fn synchronize_capture(&mut self, context: &egui::Context) {
        let (status, frame) = {
            let view = self
                .capture
                .view
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (view.status.clone(), view.frame.clone())
        };
        self.capture_status = status;
        if let Some(frame) = frame {
            if self.page == Page::Capture && frame.serial != self.capture_serial {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width, frame.height],
                    &frame.rgba,
                );
                if let Some(texture) = &mut self.capture_texture {
                    texture.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.capture_texture = Some(context.load_texture(
                        "virtual-sensor",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                self.capture_serial = frame.serial;
            }
        }
    }

    fn capture_command(&mut self, command: Command) {
        if !self.capture.send(command) {
            self.error = Some("La capture ne peut pas recevoir la commande.".into());
        }
    }

    fn capture_destination(&self, extension: &str) -> Option<PathBuf> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut dialog = rfd::FileDialog::new()
            .set_title("Enregistrer une capture sur le PC")
            .set_file_name(format!("DSMC-{stamp}.{extension}"))
            .add_filter(
                if extension == "avi" {
                    "Vid\u{e9}o AVI / MJPEG"
                } else {
                    "Image JPEG"
                },
                &[extension],
            );
        if let Some(parent) = self
            .capture_status
            .output
            .as_ref()
            .and_then(|path| path.parent())
        {
            dialog = dialog.set_directory(parent);
        }
        dialog.save_file()
    }

    pub(super) fn capture_panel(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "CAPTEUR VIRTUEL",
            "De l'image au fichier.",
            "Mire anim\u{e9}e, pr\u{e9}visualisation et enregistrement sur votre PC.",
        );
        egui::ScrollArea::vertical().show(ui, |ui| {
            theme::panel().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let status = &self.capture_status;
                    let (label, color) = if status.recording { ("REC / AVI", theme::PINK) }
                        else if status.preview { ("SOURCE ACTIVE", theme::CYAN) }
                        else { ("SOURCE EN VEILLE", theme::MUTED) };
                    theme::badge(ui, label, color);
                    ui.label(RichText::new(format!("{WIDTH} \u{d7} {HEIGHT}  /  {FPS} images/s  /  mire synth\u{e9}tique"))
                        .size(12.0).color(theme::MUTED));
                });
                ui.add_space(12.0);
                let width = ui.available_width();
                let size = Vec2::new(width, (width * HEIGHT as f32 / WIDTH as f32).min(390.0));
                let (bounds, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(bounds, 12, egui::Color32::BLACK);
                if let Some(texture) = &self.capture_texture {
                    let ratio = (bounds.width() / WIDTH as f32).min(bounds.height() / HEIGHT as f32);
                    let image = egui::Rect::from_center_size(bounds.center(), Vec2::new(WIDTH as f32, HEIGHT as f32) * ratio);
                    ui.painter().image(texture.id(), image,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                } else {
                    ui.painter().text(bounds.center(), egui::Align2::CENTER_CENTER, "Activez la source pour afficher la mire.",
                        egui::FontId::proportional(16.0), theme::MUTED);
                }
                ui.add_space(14.0);
                ui.horizontal_wrapped(|ui| {
                    let preview = self.capture_status.preview;
                    if theme::secondary_button(ui, if preview { "Couper la source" } else { "Activer la source" }).clicked() {
                        self.capture_command(Command::Preview(!preview));
                    }
                    if self.capture_status.recording {
                        if theme::primary_button(ui, "Terminer le clip", 150.0).clicked() {
                            self.capture_command(Command::StopRecord);
                        }
                    } else if theme::primary_button(ui, "Enregistrer AVI", 150.0).clicked() {
                        if let Some(path) = self.capture_destination("avi") { self.capture_command(Command::Record(path)); }
                    }
                    ui.add_enabled_ui(self.capture_texture.is_some(), |ui| {
                        if theme::secondary_button(ui, "Photo JPEG").clicked() {
                            if let Some(path) = self.capture_destination("jpg") { self.capture_command(Command::Snapshot(path)); }
                        }
                    });
                });
            });
            ui.add_space(14.0);
            theme::panel().show(ui, |ui| {
                let status = &self.capture_status;
                ui.label(RichText::new(format!("{} images enregistr\u{e9}es  /  {:.1} s  /  {:.1} Mo",
                    status.recorded, status.recorded as f64 / f64::from(FPS), status.bytes as f64 / 1_000_000.0)).size(17.0));
                ui.label(RichText::new("AVI MJPEG sans audio. La capture continue pendant une pause ARM. Limite : 2 Go par clip.")
                    .size(12.0).color(theme::MUTED));
                if let Some(path) = &status.output { ui.label(format!("Vid\u{e9}o : {}", path.display())); }
                if let Some(path) = &status.snapshot { ui.label(format!("Photo : {}", path.display())); }
                if let Some(error) = &status.error { ui.label(RichText::new(error).color(theme::PINK)); }
            });
            ui.add_space(12.0);
            if let Some(hardware) = &self.report.hardware {
                let capture = &hardware.capture;
                ui.label(format!("CCDC : {} trames DMA / {} octets / {} trames non acquitt\u{e9}es / {} rejets",
                    capture.frames, capture.bytes, capture.overruns, capture.rejected));
                if let Some(error) = &capture.last_error { ui.label(RichText::new(error).color(theme::AMBER)); }
            } else {
                ui.label(RichText::new("Activez les composants mat\u{e9}riels virtuels pour tester les transferts CCDC vers la RAM ARM.")
                    .size(12.0).color(theme::MUTED));
            }
            ui.label(RichText::new("L'AVI est produit par le PC. Le bouton REC du firmware, REDCODE/R3D et le SSD de la cam\u{e9}ra ne sont pas encore reli\u{e9}s \u{e0} ce module.")
                .size(12.0).color(theme::MUTED));
        });
    }
}
