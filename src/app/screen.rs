//! Firmware viewport, native input routing and session counters.

use super::{App, Page, theme};
use crate::input::Input;
use eframe::egui::{self, Color32, Rect, RichText, Vec2};
use std::time::{Duration, Instant};

impl App {
    pub(super) fn screen(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("ESPACE DE TRAVAIL")
                .size(10.0)
                .color(theme::BLUE),
        );
        ui.label(RichText::new("Votre studio local.").size(30.0));
        ui.label(
            RichText::new("Le programme original. Son interface native. Votre ordinateur.")
                .size(13.0)
                .color(theme::MUTED),
        );
        ui.add_space(12.0);

        let panel_height = (ui.available_height() - 166.0).max(310.0);
        theme::panel().inner_margin(20).show(ui, |ui| {
            self.viewport_toolbar(ui);
            ui.add_space(14.0);
            let size = Vec2::new(ui.available_width(), panel_height - 92.0);
            self.firmware_viewport(ui, size);
        });

        ui.add_space(12.0);
        self.session_counters(ui);
        ui.add_space(8.0);
        self.viewport_footer(ui);
    }

    fn viewport_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (label, color) = match (self.running, self.paused) {
                (true, true) => ("EN PAUSE", theme::AMBER),
                (true, false) => ("SESSION ACTIVE", theme::CYAN),
                _ => ("ECRAN NATIF", theme::MUTED),
            };
            theme::badge(ui, label, color);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(self.running, |ui| {
                    if theme::secondary_button(ui, "Menu  F1").clicked() {
                        self.send_input(Input::Menu);
                    }
                    if theme::secondary_button(ui, "Retour  Esc").clicked() {
                        self.send_input(Input::KeyDown(0x295));
                        self.send_input(Input::KeyUp(0x295));
                    }
                });
            });
        });
    }

    fn firmware_viewport(&mut self, ui: &mut egui::Ui, size: Vec2) {
        let (viewport, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter().rect_filled(viewport, 12, Color32::BLACK);

        let Some(texture) = &self.texture else {
            self.empty_viewport(ui, viewport);
            return;
        };

        let native_size = texture.size_vec2();
        let scale = (viewport.width() / native_size.x).min(viewport.height() / native_size.y);
        let image_rect = Rect::from_center_size(viewport.center(), native_size * scale);
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter()
            .image(texture.id(), image_rect, uv, Color32::WHITE);

        let response = ui.interact(
            image_rect,
            ui.id().with("firmware-surface"),
            egui::Sense::click_and_drag(),
        );
        self.pointer_input(ui, &response, image_rect, native_size, scale);
        self.keyboard_input(ui, &response);
    }

    fn empty_viewport(&mut self, ui: &mut egui::Ui, viewport: Rect) {
        let painter = ui.painter().with_clip_rect(viewport);
        theme::draw_dots(&painter, viewport);
        let text_rect = Rect::from_center_size(
            viewport.center(),
            Vec2::new((viewport.width() - 72.0).min(720.0), 220.0),
        );
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(text_rect)
                .layout(egui::Layout::top_down(egui::Align::Center)),
            |ui| {
                if self.running {
                    self.boot_message(ui);
                } else {
                    ui.label(RichText::new("Votre firmware,\nen grand.").size(33.0));
                    ui.add_space(5.0);
                    ui.label(
                        RichText::new("Retrouvez l'interface de votre caméra en chargeant son programme extrait.")
                            .size(14.0).color(theme::MUTED),
                    );
                    ui.add_space(15.0);
                    if theme::action_button(ui, "Choisir le firmware", 205.0).clicked() {
                        self.choose_firmware();
                    }
                }
            },
        );
    }

    fn boot_message(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(18.0).color(theme::CYAN));
            ui.label(RichText::new("DEMARRAGE").size(11.0).color(theme::CYAN));
        });
        ui.label(RichText::new("Le firmware\nprend sa place.").size(30.0));
        ui.add_space(4.0);
        let stage = self
            .report
            .stages
            .last()
            .map(String::as_str)
            .unwrap_or("Initialisation ARM");
        ui.label(RichText::new(stage).color(theme::MUTED));
        ui.label(
            RichText::new("La surface native apparaîtra ici dès qu'elle sera disponible.")
                .size(12.0)
                .color(theme::MUTED),
        );
    }

    fn pointer_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        image_rect: Rect,
        native_size: Vec2,
        scale: f32,
    ) {
        let (position, pressed, released) = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.pointer.button_pressed(egui::PointerButton::Primary),
                input.pointer.button_released(egui::PointerButton::Primary),
            )
        });
        let Some(position) = position else {
            return;
        };
        let x = ((position.x - image_rect.left()) / scale).clamp(0.0, native_size.x - 1.0) as i16;
        let y = ((position.y - image_rect.top()) / scale).clamp(0.0, native_size.y - 1.0) as i16;

        if image_rect.contains(position) && pressed {
            self.mouse_down = true;
            response.request_focus();
            self.send_input(Input::Down(x, y));
        }
        if self.mouse_down && released {
            self.mouse_down = false;
            self.send_input(Input::Up(x, y));
        } else if self.mouse_down
            && self.last_pos != Some((x, y))
            && self.last_move.elapsed() > Duration::from_millis(40)
        {
            self.send_input(Input::Move(x, y));
            self.last_move = Instant::now();
        }
        self.last_pos = Some((x, y));
    }

    fn keyboard_input(&mut self, ui: &egui::Ui, response: &egui::Response) {
        if !response.has_focus() {
            return;
        }
        let events = ui.input(|input| input.events.clone());
        for event in events {
            let egui::Event::Key {
                key,
                pressed,
                repeat,
                ..
            } = event
            else {
                continue;
            };
            if repeat {
                continue;
            }
            if key == egui::Key::F1 {
                if pressed {
                    self.send_input(Input::Menu);
                }
                continue;
            }
            let code = match key {
                egui::Key::Escape => 0x295,
                egui::Key::Enter => 13,
                egui::Key::Tab => 9,
                egui::Key::Space => 32,
                egui::Key::PageUp => 0x25c,
                egui::Key::PageDown => 0x25d,
                _ => continue,
            };
            let message = if pressed {
                Input::KeyDown(code)
            } else {
                Input::KeyUp(code)
            };
            self.send_input(message);
        }
    }

    fn session_counters(&self, ui: &mut egui::Ui) {
        let elapsed = self.report.elapsed_seconds as u64;
        let counters = [
            (
                "DEBIT ARM ESTIME",
                format!("{:.1} M / s", self.report.mips),
                theme::BLUE,
            ),
            (
                "INSTRUCTIONS",
                format!("{:.1} M", self.report.instructions as f64 / 1e6),
                theme::CYAN,
            ),
            (
                "TEMPS DE SESSION",
                format!("{:02}:{:02}", elapsed / 60, elapsed % 60),
                theme::AMBER,
            ),
        ];
        ui.columns(3, |columns| {
            for (column, (title, value, color)) in columns.iter_mut().zip(counters) {
                theme::panel().inner_margin(16).show(column, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().interact_size.y = 14.0;
                    ui.horizontal(|ui| {
                        let (dot, _) =
                            ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.0, color);
                        ui.label(RichText::new(title).size(10.0).color(theme::MUTED));
                    });
                    ui.label(RichText::new(value).size(23.0));
                });
            }
        });
    }

    fn viewport_footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let profile = if self.settings.hardware {
                "Matériel virtuel"
            } else {
                "Profil léger"
            };
            ui.label(
                RichText::new(format!("RED 6.3.106  /  {profile}"))
                    .size(11.0)
                    .color(theme::MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.link("Configurer la session").clicked() {
                    self.page = Page::Settings;
                }
            });
        });
    }
}
