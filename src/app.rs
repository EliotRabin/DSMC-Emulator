//! Desktop controller. Presentation is split into screen, panels and theme modules.

mod capture;
mod panels;
mod screen;
mod settings;
mod theme;
mod title_bar;

use crate::{
    engine::{Config, Report},
    input::Input,
    worker::{Command, Worker},
};
use eframe::egui::{self, Color32, RichText, Vec2};
use settings::Settings;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Screen,
    Hardware,
    Capture,
    Console,
    Settings,
}

struct Preview {
    output: PathBuf,
    firmware: Option<PathBuf>,
    requested: bool,
    capture: bool,
}

pub struct App {
    settings: Settings,
    worker: Option<Worker>,
    page: Page,
    report: Report,
    running: bool,
    paused: bool,
    texture: Option<egui::TextureHandle>,
    frame_serial: u64,
    error: Option<String>,
    notice: Option<String>,
    mouse_down: bool,
    last_move: Instant,
    last_pos: Option<(i16, i16)>,
    preview: Option<Preview>,
    capture: crate::capture::Capture,
    capture_status: crate::capture::Status,
    capture_texture: Option<egui::TextureHandle>,
    capture_serial: u64,
}

impl App {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&context.egui_ctx);
        let arguments: Vec<String> = std::env::args().collect();
        let argument = |name| {
            arguments
                .iter()
                .position(|value| value == name)
                .and_then(|index| arguments.get(index + 1))
                .map(PathBuf::from)
        };
        let preview = argument("--ui-screenshot").map(|output| Preview {
            output,
            firmware: argument("--ui-firmware"),
            requested: false,
            capture: arguments.iter().any(|argument| argument == "--ui-capture"),
        });

        let mut app = Self {
            settings: if preview.is_some() {
                Settings::default()
            } else {
                Settings::load()
            },
            worker: None,
            page: Page::Screen,
            report: Report::default(),
            running: false,
            paused: false,
            texture: None,
            frame_serial: 0,
            error: None,
            notice: None,
            mouse_down: false,
            last_move: Instant::now(),
            last_pos: None,
            preview,
            capture: crate::capture::Capture::new(),
            capture_status: crate::capture::Status::default(),
            capture_texture: None,
            capture_serial: 0,
        };

        if app.preview.as_ref().is_some_and(|preview| preview.capture) {
            app.page = Page::Capture;
            app.capture.send(crate::capture::Command::Preview(true));
        }

        if let Some(path) = app
            .preview
            .as_ref()
            .and_then(|preview| preview.firmware.clone())
        {
            app.set_firmware(path);
            app.start();
        }
        app
    }

    fn set_firmware(&mut self, path: PathBuf) {
        self.settings.assets = path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_string_lossy()
            .into_owned();
        self.settings.firmware = path.to_string_lossy().into_owned();
    }

    fn choose_firmware(&mut self) {
        let selected = rfd::FileDialog::new()
            .set_title("Ouvrir le programme ARM extrait")
            .add_filter("Programme ARM", &["mem", "bin"])
            .pick_file();

        if let Some(path) = selected {
            self.set_firmware(path);
            self.save_settings();
        }
    }

    fn save_settings(&mut self) {
        if self.preview.is_some() {
            return;
        }
        match self.settings.save() {
            Ok(()) => self.notice = Some("Réglages enregistrés.".into()),
            Err(error) => self.error = Some(error),
        }
    }

    fn start(&mut self) {
        if self.settings.firmware.is_empty() {
            self.choose_firmware();
            if self.settings.firmware.is_empty() {
                return;
            }
        }
        self.stop();
        self.texture = None;
        self.frame_serial = 0;
        self.report = Report::default();
        self.error = None;
        self.notice = None;
        self.mouse_down = false;
        self.save_settings();
        self.worker = Some(Worker::start(
            Config {
                firmware: PathBuf::from(&self.settings.firmware),
                assets: PathBuf::from(&self.settings.assets),
                offline: self.settings.offline,
                virtual_storage: self.settings.offline,
                hardware: self.settings.hardware,
                allow_unknown: false,
            },
            Some(self.capture.view.clone()),
        ));
        self.running = true;
    }

    fn stop(&mut self) {
        if let Some(worker) = &mut self.worker {
            worker.stop();
        }
        self.running = false;
        self.paused = false;
    }

    fn toggle_pause(&mut self) {
        if let Some(worker) = &self.worker {
            if !worker.send(Command::Pause(!self.paused)) {
                self.error = Some("Impossible de transmettre la commande de pause.".into());
            }
        }
    }

    fn send_input(&mut self, input: Input) {
        if !self.running {
            return;
        }
        if let Some(worker) = &self.worker {
            if !worker.send(Command::Input(input)) {
                self.error = Some("La file d'entrée est pleine. Recommencez l'action.".into());
            }
        }
    }

    fn synchronize_worker(&mut self, context: &egui::Context) {
        let Some(worker) = &self.worker else {
            return;
        };
        // Release the worker lock before uploading pixels to the GPU.
        let (report, frame, running, paused) = {
            let view = worker
                .view
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (
                view.report.clone(),
                view.frame.clone(),
                view.running,
                view.paused,
            )
        };
        self.report = report;
        self.running = running;
        self.paused = paused;

        if let Some(frame) = frame {
            if self.texture.is_none() || self.frame_serial != frame.serial {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width, frame.height],
                    &frame.rgba,
                );
                if let Some(texture) = &mut self.texture {
                    texture.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.texture =
                        Some(context.load_texture("firmware", image, egui::TextureOptions::LINEAR));
                }
                self.frame_serial = frame.serial;
            }
        }
    }

    fn process_preview(&mut self, context: &egui::Context) {
        let Some(preview) = &mut self.preview else {
            return;
        };
        let screenshot = context.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });

        if let Some(image) = screenshot {
            let mut bytes = format!("P6\n{} {}\n255\n", image.width(), image.height()).into_bytes();
            for pixel in &image.pixels {
                bytes.extend(&pixel.to_array()[..3]);
            }
            let _ = std::fs::write(&preview.output, bytes);
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if !preview.requested {
            let ready = if preview.capture {
                self.capture_texture.is_some()
            } else {
                preview.firmware.is_none() || (self.texture.is_some() && self.report.ui_ready)
            };
            if ready {
                context.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                preview.requested = true;
            }
        }
    }

    fn render_notices(&mut self, ui: &mut egui::Ui) {
        let message = self.error.as_ref().or(self.report.error.as_ref());
        if let Some(message) = message {
            egui::Frame::new()
                .fill(Color32::from_rgb(49, 22, 32))
                .corner_radius(12)
                .inner_margin(14)
                .show(ui, |ui| {
                    ui.label(RichText::new(message).color(theme::PINK));
                });
            ui.add_space(14.0);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, context: &egui::Context, _: &mut eframe::Frame) {
        self.synchronize_worker(context);
        self.synchronize_capture(context);
        self.process_preview(context);

        title_bar::show(context);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BACKGROUND).inner_margin(28))
            .show(context, |ui| {
                let bounds = ui.max_rect();
                theme::draw_dots(ui.painter(), bounds);
                let width = bounds.width().min(1200.0);
                let content = egui::Rect::from_min_size(
                    egui::pos2(bounds.center().x - width / 2.0, bounds.top()),
                    Vec2::new(width, bounds.height()),
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(content)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                    |ui| {
                        ui.set_width(width);
                        self.navigation(ui);
                        ui.add_space(28.0);
                        self.render_notices(ui);
                        match self.page {
                            Page::Screen => self.screen(ui),
                            Page::Capture => self.capture_panel(ui),
                            Page::Hardware => self.hardware_panel(ui),
                            Page::Console => self.diagnostics_panel(ui),
                            Page::Settings => self.settings_panel(ui),
                        }
                    },
                );
            });

        title_bar::resize_borders(context);

        let refresh_ms = if self.running || self.capture_status.preview {
            33
        } else {
            200
        };
        context.request_repaint_after(Duration::from_millis(refresh_ms));
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if self.preview.is_none() {
            let _ = self.settings.save();
        }
        self.stop();
        self.capture.stop();
    }
}
