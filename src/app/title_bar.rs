//! Custom desktop chrome. Window actions still go through the native backend.

use super::theme;
use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, Vec2, ViewportCommand};

const HEIGHT: f32 = 38.0;
const BUTTON_WIDTH: f32 = 46.0;

#[derive(Clone, Copy)]
enum Control {
    Minimize,
    Maximize,
    Close,
}

pub fn show(context: &egui::Context) {
    let maximized = context.input(|input| input.viewport().maximized.unwrap_or(false));
    let focused = context.input(|input| input.viewport().focused.unwrap_or(true));

    egui::TopBottomPanel::top("window_title_bar")
        .exact_height(HEIGHT)
        .frame(egui::Frame::new().fill(theme::BACKGROUND))
        .show(context, |ui| {
            let bounds = ui.max_rect();
            let controls_left = bounds.right() - BUTTON_WIDTH * 3.0;
            let drag_rect =
                Rect::from_min_max(bounds.min, egui::pos2(controls_left, bounds.bottom()));
            let response = ui.interact(drag_rect, ui.id().with("drag"), Sense::click_and_drag());

            // Buttons have their own hit regions and never initiate a window drag.
            if response.double_clicked() {
                context.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            } else if response.drag_started_by(egui::PointerButton::Primary) {
                context.send_viewport_cmd(ViewportCommand::StartDrag);
            }

            let color = if focused { theme::TEXT } else { theme::MUTED };
            let center = egui::pos2(bounds.left() + 24.0, bounds.center().y);
            ui.painter().circle_filled(center, 5.0, theme::BLUE);
            ui.painter().text(
                center + Vec2::new(16.0, 0.0),
                Align2::LEFT_CENTER,
                "DSMC Emulator",
                FontId::proportional(12.0),
                color,
            );
            ui.painter().hline(
                bounds.x_range(),
                bounds.bottom() - 0.5,
                Stroke::new(1.0_f32, theme::BORDER),
            );

            for (index, control) in [Control::Minimize, Control::Maximize, Control::Close]
                .into_iter()
                .enumerate()
            {
                let rect = Rect::from_min_size(
                    egui::pos2(controls_left + index as f32 * BUTTON_WIDTH, bounds.top()),
                    Vec2::new(BUTTON_WIDTH, HEIGHT),
                );
                control_button(ui, rect, control, maximized, color);
            }
        });
}

fn control_button(
    ui: &mut egui::Ui,
    rect: Rect,
    control: Control,
    maximized: bool,
    color: Color32,
) {
    let (id, label, command) = match control {
        Control::Minimize => ("minimize", "R\u{e9}duire", ViewportCommand::Minimized(true)),
        Control::Maximize => (
            "maximize",
            if maximized { "Restaurer" } else { "Agrandir" },
            ViewportCommand::Maximized(!maximized),
        ),
        Control::Close => ("close", "Fermer", ViewportCommand::Close),
    };
    let response = ui
        .interact(rect, ui.id().with(id), Sense::click())
        .on_hover_text(label);
    if response.hovered() || response.has_focus() {
        let fill = if matches!(control, Control::Close) {
            theme::PINK
        } else {
            theme::PANEL_RAISED
        };
        ui.painter().rect_filled(rect.shrink(4.0), 8, fill);
    }
    let center = rect.center();
    let stroke = Stroke::new(1.2_f32, color);
    let square = Rect::from_center_size(center, Vec2::splat(10.0));
    match control {
        Control::Minimize => {
            ui.painter().line_segment(
                [center - Vec2::new(5.0, 0.0), center + Vec2::new(5.0, 0.0)],
                stroke,
            );
        }
        Control::Maximize => {
            if maximized {
                let back = square.translate(Vec2::new(2.0, -2.0));
                ui.painter()
                    .line_segment([back.left_top(), back.right_top()], stroke);
                ui.painter()
                    .line_segment([back.right_top(), back.right_bottom()], stroke);
            }
            ui.painter()
                .rect_stroke(square, 0, stroke, egui::StrokeKind::Inside);
        }
        Control::Close => {
            ui.painter()
                .line_segment([square.left_top(), square.right_bottom()], stroke);
            ui.painter()
                .line_segment([square.right_top(), square.left_bottom()], stroke);
        }
    }
    if response.clicked() {
        ui.ctx().send_viewport_cmd(command);
    }
}

/// Undecorated windows need explicit resize handles, including all four corners.
pub fn resize_borders(context: &egui::Context) {
    if context.input(|input| input.viewport().maximized.unwrap_or(false)) {
        return;
    }
    let bounds = context.input(|input| input.viewport_rect());
    let edge = 5.0;
    let corner = 10.0;
    use egui::{CursorIcon as Cursor, ResizeDirection as Direction};
    for (index, (direction, cursor, min, max)) in [
        (
            Direction::NorthWest,
            Cursor::ResizeNwSe,
            bounds.min,
            bounds.min + Vec2::splat(corner),
        ),
        (
            Direction::NorthEast,
            Cursor::ResizeNeSw,
            bounds.right_top() - Vec2::new(corner, 0.0),
            bounds.right_top() + Vec2::new(0.0, corner),
        ),
        (
            Direction::SouthWest,
            Cursor::ResizeNeSw,
            bounds.left_bottom() - Vec2::new(0.0, corner),
            bounds.left_bottom() + Vec2::new(corner, 0.0),
        ),
        (
            Direction::SouthEast,
            Cursor::ResizeNwSe,
            bounds.max - Vec2::splat(corner),
            bounds.max,
        ),
        (
            Direction::North,
            Cursor::ResizeVertical,
            bounds.min + Vec2::new(corner, 0.0),
            bounds.right_top() + Vec2::new(-corner, edge),
        ),
        (
            Direction::South,
            Cursor::ResizeVertical,
            bounds.left_bottom() + Vec2::new(corner, -edge),
            bounds.max - Vec2::new(corner, 0.0),
        ),
        (
            Direction::West,
            Cursor::ResizeHorizontal,
            bounds.min + Vec2::new(0.0, corner),
            bounds.left_bottom() + Vec2::new(edge, -corner),
        ),
        (
            Direction::East,
            Cursor::ResizeHorizontal,
            bounds.right_top() + Vec2::new(-edge, corner),
            bounds.max - Vec2::new(0.0, corner),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        egui::Area::new(egui::Id::new(("window_resize", index)))
            .order(egui::Order::Foreground)
            .fixed_pos(min)
            .constrain(false)
            .movable(false)
            .show(context, |ui| {
                ui.spacing_mut().interact_size = Vec2::ZERO;
                let (_, response) = ui.allocate_exact_size(max - min, Sense::drag());
                let response = response.on_hover_cursor(cursor);
                if response.drag_started_by(egui::PointerButton::Primary) {
                    context.send_viewport_cmd(ViewportCommand::BeginResize(direction));
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(x: f32, maximized: bool) -> Vec<ViewportCommand> {
        let context = egui::Context::default();
        theme::apply(&context);
        let frame = |events| {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1000.0, 740.0),
                )),
                events,
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .maximized = Some(maximized);
            context.run(input, show)
        };
        let _ = frame(vec![]);
        let position = egui::pos2(x, HEIGHT / 2.0);
        let _ = frame(vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]);
        let output = frame(vec![egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .clone()
    }

    #[test]
    fn window_buttons_send_native_commands_without_dragging() {
        for (x, maximized, expected) in [
            (885.0, false, ViewportCommand::Minimized(true)),
            (931.0, false, ViewportCommand::Maximized(true)),
            (931.0, true, ViewportCommand::Maximized(false)),
            (977.0, false, ViewportCommand::Close),
        ] {
            let commands = click(x, maximized);
            assert_eq!(commands, vec![expected]);
        }
    }
}
