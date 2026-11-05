//! Shared colors, typography and small drawing primitives for the desktop UI.

use eframe::egui::{
    self, Color32, FontId, Painter, Pos2, Rect, Response, RichText, Stroke, Ui, Vec2,
};

pub const BACKGROUND: Color32 = Color32::from_rgb(9, 10, 12);
pub const PANEL: Color32 = Color32::from_rgb(16, 17, 20);
pub const PANEL_RAISED: Color32 = Color32::from_rgb(23, 24, 28);
pub const BORDER: Color32 = Color32::from_rgb(36, 38, 43);
pub const TEXT: Color32 = Color32::from_rgb(245, 246, 248);
pub const MUTED: Color32 = Color32::from_rgb(145, 149, 158);
pub const BLUE: Color32 = Color32::from_rgb(31, 100, 255);
pub const CYAN: Color32 = Color32::from_rgb(55, 190, 200);
pub const PINK: Color32 = Color32::from_rgb(242, 66, 121);
pub const AMBER: Color32 = Color32::from_rgb(255, 187, 42);

pub fn apply(context: &egui::Context) {
    context.set_theme(egui::Theme::Dark);

    let mut style = (*context.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.spacing.item_spacing = Vec2::new(12.0, 10.0);
    style.spacing.button_padding = Vec2::new(18.0, 10.0);
    style.spacing.interact_size.y = 38.0;
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BACKGROUND;
    style.visuals.faint_bg_color = PANEL_RAISED;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.selection.bg_fill = BLUE;
    style.visuals.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);

    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
    ] {
        widget.corner_radius = 12.into();
        widget.fg_stroke.color = TEXT;
    }

    style.visuals.widgets.inactive.bg_fill = PANEL_RAISED;
    style.visuals.widgets.inactive.weak_bg_fill = PANEL_RAISED;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(37, 39, 46);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(37, 39, 46);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, MUTED);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(47, 50, 58);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(47, 50, 58);

    for (text_style, size) in [
        (egui::TextStyle::Heading, 30.0),
        (egui::TextStyle::Body, 15.0),
        (egui::TextStyle::Button, 14.0),
        (egui::TextStyle::Small, 12.0),
    ] {
        style
            .text_styles
            .insert(text_style, FontId::proportional(size));
    }

    style
        .text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(13.0));
    context.set_style_of(egui::Theme::Dark, style);
}

pub fn panel() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(20)
        .inner_margin(22)
}

pub fn primary_button(ui: &mut Ui, label: &str, width: f32) -> Response {
    pill_button(ui, label, width, false)
}

pub fn action_button(ui: &mut Ui, label: &str, width: f32) -> Response {
    pill_button(ui, label, width, true)
}

fn pill_button(ui: &mut Ui, label: &str, width: f32, with_arrow: bool) -> Response {
    let caption = if with_arrow {
        format!("{label}   ")
    } else {
        label.to_owned()
    };
    let response = ui.add(
        egui::Button::new(RichText::new(caption).color(BACKGROUND).size(14.0))
            .fill(TEXT)
            .stroke(Stroke::NONE)
            .corner_radius(28)
            .min_size(Vec2::new(width, 42.0)),
    );

    if with_arrow {
        // Draw the arrow as geometry, so it works without a symbol-font fallback.
        let center = response.rect.right_center() - Vec2::new(19.0, 0.0);
        let stroke = Stroke::new(1.3_f32, BACKGROUND);
        let tip = center + Vec2::new(5.0, 0.0);
        ui.painter()
            .line_segment([center - Vec2::new(5.0, 0.0), tip], stroke);
        ui.painter()
            .line_segment([tip - Vec2::new(4.0, 4.0), tip], stroke);
        ui.painter()
            .line_segment([tip - Vec2::new(4.0, -4.0), tip], stroke);
    }
    response
}

pub fn secondary_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(13.0))
            .fill(PANEL_RAISED)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(24)
            .min_size(Vec2::new(94.0, 36.0)),
    )
}

pub fn badge(ui: &mut Ui, label: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .corner_radius(20)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(11.0).color(color));
        });
}

pub fn page_title(ui: &mut Ui, eyebrow: &str, title: &str, description: &str) {
    ui.label(RichText::new(eyebrow).size(11.0).color(BLUE).strong());
    ui.add_space(5.0);
    ui.label(RichText::new(title).size(31.0).color(TEXT));
    ui.add_space(4.0);
    ui.label(RichText::new(description).size(13.0).color(MUTED));
    ui.add_space(22.0);
}

pub fn draw_dots(painter: &Painter, bounds: Rect) {
    let spacing = 27.0;
    let color = Color32::from_rgb(29, 31, 36);
    let mut y = bounds.top() + 9.0;

    while y < bounds.bottom() {
        let mut x = bounds.left() + 9.0;
        while x < bounds.right() {
            painter.circle_filled(Pos2::new(x, y), 0.8, color);
            x += spacing;
        }
        y += spacing;
    }
}

pub fn draw_brand(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(30.0, 32.0), egui::Sense::hover());
    let center = rect.center();
    let painter = ui.painter();
    let stroke = Stroke::new(1.3_f32, TEXT);

    painter.circle_stroke(center, 11.0, stroke);
    painter.add(egui::epaint::EllipseShape::stroke(
        center,
        Vec2::new(6.0, 11.0),
        stroke,
    ));
    painter.line_segment(
        [center - Vec2::new(0.0, 11.0), center + Vec2::new(0.0, 11.0)],
        stroke,
    );
    ui.label(RichText::new("DSMC").size(17.0).strong());
    ui.label(RichText::new("Emulator").size(14.0).color(MUTED));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_keeps_text_readable_when_windows_uses_light_mode() {
        let context = egui::Context::default();
        apply(&context);
        let input = egui::RawInput {
            system_theme: Some(egui::Theme::Light),
            ..Default::default()
        };
        let _ = context.run(input, |context| {
            assert_eq!(context.theme(), egui::Theme::Dark);
            assert_eq!(context.style().visuals.override_text_color, Some(TEXT));
            assert!(context.style().visuals.dark_mode);
        });
    }
}
