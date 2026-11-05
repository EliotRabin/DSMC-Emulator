//! Navigation and secondary views, independent of the emulation backend.

use super::{App, Page, theme};
use eframe::egui::{self, RichText, Vec2};
use std::path::PathBuf;

impl App {
    pub(super) fn navigation(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
            .corner_radius(40)
            .inner_margin(egui::Margin::symmetric(20, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::draw_brand(ui);
                    ui.add_space(24.0);
                    self.navigation_tabs(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.session_actions(ui);
                    });
                });
            });
    }

    fn navigation_tabs(&mut self, ui: &mut egui::Ui) {
        for (page, label) in [
            (Page::Screen, "Écran"),
            (Page::Hardware, "Matériel"),
            (Page::Console, "Diagnostics"),
            (Page::Settings, "Réglages"),
        ] {
            let selected = self.page == page;
            let button = egui::Button::new(RichText::new(label).size(13.0).color(if selected {
                theme::TEXT
            } else {
                theme::MUTED
            }))
            .fill(if selected {
                theme::PANEL_RAISED
            } else {
                egui::Color32::TRANSPARENT
            })
            .stroke(egui::Stroke::NONE)
            .corner_radius(24)
            .min_size(Vec2::new(74.0, 38.0));
            if ui.add(button).clicked() {
                self.page = page;
            }
        }
    }

    fn session_actions(&mut self, ui: &mut egui::Ui) {
        if self.running {
            if theme::secondary_button(ui, "Arrêter").clicked() {
                self.stop();
            }
            let label = if self.paused { "Reprendre" } else { "Pause" };
            if theme::primary_button(ui, label, 100.0).clicked() {
                self.toggle_pause();
            }
        } else if theme::action_button(ui, "Démarrer", 137.0).clicked() {
            self.start();
        }
    }

    pub(super) fn hardware_panel(&self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "LABORATOIRE",
            "Le matériel, en détail.",
            "Des composants virtuels, avec un périmètre explicite pour chacun.",
        );
        egui::ScrollArea::vertical().show(ui, |ui| {
            let components = [
                (
                    "ARM & interruptions",
                    "Exécution du programme et vecteurs natifs.",
                    "ACTIF",
                    theme::BLUE,
                ),
                (
                    "Interface native",
                    "Surfaces du firmware, souris et clavier PEG.",
                    "DISPONIBLE",
                    theme::CYAN,
                ),
                (
                    "Stockage de session",
                    "Assets en lecture seule. Écritures conservées en RAM.",
                    "VIRTUEL",
                    theme::BLUE,
                ),
                (
                    "Contrôleurs OMAP",
                    "SDMA, SPI, I2C, GPIO, TPS65950 et RTC.",
                    "PARTIEL",
                    theme::AMBER,
                ),
                (
                    "Cartes & mémoire",
                    "NAND vierge et neuf identités EEPROM virtuelles.",
                    "PARTIEL",
                    theme::AMBER,
                ),
                (
                    "Capture & enregistrement",
                    "FPGA, DSP, capteur, flux audio et SSD restent absents.",
                    "NON EMULE",
                    theme::PINK,
                ),
            ];
            for pair in components.chunks(2) {
                ui.columns(2, |columns| {
                    for (column, component) in columns.iter_mut().zip(pair) {
                        let (title, detail, status, color) = component;
                        theme::panel().show(column, |ui| {
                            ui.set_width(ui.available_width());
                            theme::badge(ui, status, *color);
                            ui.add_space(8.0);
                            ui.label(RichText::new(*title).size(19.0));
                            ui.label(RichText::new(*detail).size(13.0).color(theme::MUTED));
                        });
                    }
                });
                ui.add_space(14.0);
            }
            self.hardware_counters(ui);
        });
    }

    fn hardware_counters(&self, ui: &mut egui::Ui) {
        if let Some(hardware) = &self.report.hardware {
            theme::panel().show(ui, |ui| {
                ui.label(RichText::new("Activité des contrôleurs").size(18.0));
                ui.label(format!(
                    "DMA : {} transferts / {} octets",
                    hardware.dma_completed, hardware.dma_bytes,
                ));
                ui.label(format!(
                    "I2C : {} transactions / SPI : {} mots / 1-Wire : {} resets",
                    hardware.i2c_transactions, hardware.spi_words, hardware.onewire_resets,
                ));
                for error in &hardware.dma_errors {
                    ui.label(RichText::new(error).size(12.0).color(theme::MUTED));
                }
            });
        }
        ui.add_space(10.0);
        ui.label(
            RichText::new(format!(
                "{} pages MMIO  /  {} accès  /  {} interruptions",
                self.report.mmio_pages, self.report.mmio_accesses, self.report.irq_deliveries,
            ))
            .size(12.0)
            .color(theme::MUTED),
        );
    }

    pub(super) fn settings_panel(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "VOTRE SESSION",
            "Un espace à votre mesure.",
            "Choisissez vos fichiers et le profil utilisé au prochain démarrage.",
        );
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.firmware_settings(ui);
            ui.add_space(16.0);
            self.profile_settings(ui);
            ui.add_space(20.0);
            ui.horizontal(|ui| {
                if theme::action_button(ui, "Enregistrer les réglages", 220.0).clicked() {
                    self.save_settings();
                }
                if let Some(notice) = &self.notice {
                    ui.label(RichText::new(notice).size(13.0).color(theme::CYAN));
                }
            });
        });
    }

    fn firmware_settings(&mut self, ui: &mut egui::Ui) {
        theme::panel().show(ui, |ui| {
            ui.label(RichText::new("Vos fichiers").size(21.0));
            ui.label(
                RichText::new("Ils restent sur votre ordinateur.")
                    .size(13.0)
                    .color(theme::MUTED),
            );
            ui.add_space(16.0);
            ui.label("Programme ARM extrait");
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 140.0).max(100.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings.firmware).desired_width(width),
                );
                if theme::secondary_button(ui, "Parcourir").clicked() {
                    self.choose_firmware();
                }
            });
            ui.add_space(12.0);
            ui.label("Dossier des assets");
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 140.0).max(100.0);
                ui.add(egui::TextEdit::singleline(&mut self.settings.assets).desired_width(width));
                if theme::secondary_button(ui, "Choisir").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.settings.assets = path.to_string_lossy().into_owned();
                    }
                }
            });
        });
    }

    fn profile_settings(&mut self, ui: &mut egui::Ui) {
        theme::panel().show(ui, |ui| {
            ui.label(RichText::new("Profil d'exécution").size(21.0));
            ui.add_space(12.0);
            ui.checkbox(&mut self.settings.offline, "Explorer les menus natifs");
            ui.label(
                RichText::new("Profil léger pour RED 6.3.106. La capture reste indisponible.")
                    .size(13.0)
                    .color(theme::MUTED),
            );
            ui.add_space(12.0);
            ui.checkbox(
                &mut self.settings.hardware,
                "Activer les composants matériels virtuels",
            );
            ui.label(
                RichText::new(
                    "Les services encore absents peuvent provoquer des diagnostics natifs.",
                )
                .size(13.0)
                .color(theme::MUTED),
            );
        });
    }

    pub(super) fn diagnostics_panel(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "SOUS LE CAPOT",
            "Suivez ce qui se passe.",
            "Les étapes, registres et messages de la session en cours.",
        );
        ui.horizontal(|ui| {
            if theme::action_button(ui, "Exporter le rapport", 185.0).clicked() {
                self.export_report();
            }
            if theme::secondary_button(ui, "Copier la console").clicked() {
                ui.ctx().copy_text(self.report.console.clone());
            }
            theme::badge(ui, &format!("PC 0x{:08x}", self.report.pc), theme::CYAN);
        });
        ui.add_space(16.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            theme::panel().show(ui, |ui| {
                ui.label(RichText::new("Démarrage").size(19.0));
                ui.label(RichText::new(&self.report.status).color(theme::MUTED));
                for stage in &self.report.stages {
                    ui.label(stage);
                }
            });
            ui.add_space(16.0);
            theme::panel().show(ui, |ui| {
                ui.label(RichText::new("Console UART").size(19.0));
                if self.report.console.is_empty() {
                    ui.label(RichText::new("Aucun message pour le moment.").color(theme::MUTED));
                } else {
                    ui.monospace(&self.report.console);
                }
            });
            ui.add_space(16.0);
            self.registers_panel(ui);
        });
    }

    fn registers_panel(&self, ui: &mut egui::Ui) {
        theme::panel().show(ui, |ui| {
            ui.label(RichText::new("Registres ARM").size(19.0));
            egui::Grid::new("registers")
                .num_columns(4)
                .spacing([24.0, 10.0])
                .show(ui, |ui| {
                    for (index, (name, value)) in self.report.registers.iter().enumerate() {
                        ui.label(RichText::new(name).color(theme::MUTED));
                        ui.monospace(format!("0x{value:08x}"));
                        if index % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
        });
    }

    fn export_report(&mut self) {
        let destination = rfd::FileDialog::new()
            .set_file_name("dsmc-report.json")
            .add_filter("Rapport JSON", &["json"])
            .save_file();
        let Some(path) = destination else {
            return;
        };
        let firmware = PathBuf::from(&self.settings.firmware);
        let result = crate::validate_output(&path, &firmware).and_then(|_| {
            let bytes =
                serde_json::to_vec_pretty(&self.report).map_err(|error| error.to_string())?;
            std::fs::write(path, bytes).map_err(|error| error.to_string())
        });
        if let Err(error) = result {
            self.error = Some(error);
        }
    }
}
