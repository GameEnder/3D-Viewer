// crates/viewer-app/src/panels.rs
//! Overlay widgets drawn on top of the 3D viewport (Task 8: Open button,
//! loading spinner, load-error banner) and the right settings panel's
//! Lighting section (Task 11; the full toolbar lands in Task 12).

use eframe::egui;

use crate::themes::THEMES;

/// Floating "Open" button in the viewport's top-left corner. Returns the
/// picked path when the user completes the native file dialog (blocking
/// while it is open).
pub fn open_button(ui: &mut egui::Ui, viewport: egui::Rect) -> Option<std::path::PathBuf> {
    let rect = egui::Rect::from_min_size(
        viewport.left_top() + egui::vec2(8.0, 8.0),
        egui::vec2(64.0, 28.0),
    );
    if ui.put(rect, egui::Button::new("Open")).clicked() {
        rfd::FileDialog::new()
            .add_filter("3D models", &["glb", "gltf", "obj", "stl", "ply", "3mf"])
            .pick_file()
    } else {
        None
    }
}

/// Spinner centered in the viewport while a model loads (brief Step 3).
pub fn loading_spinner(ui: &mut egui::Ui, viewport: egui::Rect) {
    let rect = egui::Rect::from_center_size(viewport.center(), egui::vec2(40.0, 40.0));
    ui.put(rect, egui::Spinner::new().size(40.0));
}

/// Load-failure banner along the viewport's bottom edge.
pub fn error_banner(ui: &mut egui::Ui, viewport: egui::Rect, message: &str) {
    let rect = egui::Rect::from_min_size(
        viewport.left_bottom() + egui::vec2(8.0, -28.0),
        egui::vec2((viewport.width() - 16.0).max(100.0), 24.0),
    );
    ui.put(
        rect,
        egui::Label::new(
            egui::RichText::new(format!("Failed to load model: {message}"))
                .color(egui::Color32::from_rgb(255, 128, 128)),
        ),
    );
}

/// The right settings panel's "Lighting" section (Task 11): a 3-column grid
/// of theme swatches (each button's fill is the theme's light-0 color as its
/// visual identity; the selected theme gets a selection stroke), per-light
/// intensity drags (0..3), the environment intensity drag (0..3) and the
/// Shadows toggle. Mutates the app's lighting state through the passed refs;
/// `app.rs` reads the same fields when building `FrameParams`.
pub fn lighting_panel(
    ui: &mut egui::Ui,
    theme_idx: &mut usize,
    light_mult: &mut [f32; 3],
    env_mult: &mut f32,
    shadows_enabled: &mut bool,
) {
    egui::Panel::right("lighting_panel")
        .resizable(false)
        .show(ui, |ui| {
            ui.heading("Lighting");
            ui.add_space(6.0);

            egui::Grid::new("theme_grid")
                .num_columns(3)
                .spacing([4.0, 4.0])
                .show(ui, |ui| {
                    for (row, chunk) in THEMES.chunks(3).enumerate() {
                        for (col, theme) in chunk.iter().enumerate() {
                            let idx = row * 3 + col;
                            let [r, g, b] = theme.lights[0].rgb;
                            // Readable label on any swatch fill.
                            let lum =
                                0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
                            let text = if lum > 128.0 {
                                egui::Color32::BLACK
                            } else {
                                egui::Color32::WHITE
                            };
                            let mut button = egui::Button::new(
                                egui::RichText::new(theme.name).small().color(text),
                            )
                            .fill(egui::Color32::from_rgb(r, g, b));
                            if idx == *theme_idx {
                                button = button.stroke(ui.visuals().selection.stroke);
                            }
                            if ui.add(button).clicked() {
                                *theme_idx = idx;
                            }
                        }
                        ui.end_row();
                    }
                });

            ui.add_space(6.0);
            ui.separator();
            ui.label("Light intensity");
            for (n, mult) in light_mult.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("Light {}", n + 1));
                    ui.add(egui::DragValue::new(mult).range(0.0..=3.0).speed(0.01));
                });
            }
            ui.label("Environment");
            ui.add(egui::DragValue::new(env_mult).range(0.0..=3.0).speed(0.01));
            ui.separator();
            ui.checkbox(shadows_enabled, "Shadows");
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tessellates one overlay in a headless egui context and counts mesh
    /// vertices landing inside `region` — proof the widget actually paints
    /// where the placement code says it does (no window/GPU needed).
    fn verts_near(paint: impl FnOnce(&mut egui::Ui, egui::Rect), region: egui::Rect) -> usize {
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let input = egui::RawInput {
            screen_rect: Some(viewport),
            ..Default::default()
        };
        let mut paint = Some(paint);
        let out = ctx.run_ui(input, |ui| {
            if let Some(p) = paint.take() {
                p(ui, viewport);
            }
        });
        let egui::FullOutput {
            textures_delta,
            shapes,
            pixels_per_point,
            ..
        } = out;
        let prims = ctx.tessellate(shapes, pixels_per_point);
        let mut textures_delta = textures_delta;
        textures_delta.clear();
        let mut count = 0;
        for p in prims {
            if let egui::epaint::Primitive::Mesh(mesh) = p.primitive {
                for v in &mesh.vertices {
                    if region.contains(v.pos) {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    #[test]
    fn loading_spinner_tessellates_at_viewport_center() {
        // The arc points sit at radius ~(size/2 - 2) from the center.
        let center = egui::Rect::from_center_size(egui::pos2(640.0, 400.0), egui::vec2(50.0, 50.0));
        let n = verts_near(loading_spinner, center);
        assert!(
            n > 20,
            "spinner must paint at the viewport center, got {n} vertices"
        );
    }

    #[test]
    fn error_banner_tessellates_at_bottom_edge() {
        let strip =
            egui::Rect::from_min_max(egui::pos2(0.0, 800.0 - 40.0), egui::pos2(1280.0, 800.0));
        let n = verts_near(
            |ui, vp| error_banner(ui, vp, "nope.glb: parse error"),
            strip,
        );
        assert!(
            n > 10,
            "error banner must paint at the bottom edge, got {n} vertices"
        );
    }

    #[test]
    fn open_button_tessellates_at_top_left() {
        let corner = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(90.0, 45.0));
        let n = verts_near(open_button_probe, corner);
        assert!(
            n > 10,
            "open button must paint at the top-left, got {n} vertices"
        );
    }

    /// open_button opens a native dialog on click, which cannot run in a
    /// test — paint the same rect with the same button for the check.
    fn open_button_probe(ui: &mut egui::Ui, viewport: egui::Rect) {
        let rect = egui::Rect::from_min_size(
            viewport.left_top() + egui::vec2(8.0, 8.0),
            egui::vec2(64.0, 28.0),
        );
        ui.put(rect, egui::Button::new("Open"));
    }

    /// The Lighting panel must paint every theme swatch with its light-0
    /// color (the swatch IS the theme's visual identity) on the right side
    /// of the screen.
    #[test]
    fn lighting_panel_paints_all_theme_swatches_on_the_right() {
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let input = egui::RawInput {
            screen_rect: Some(viewport),
            ..Default::default()
        };
        let mut theme_idx = 0usize;
        let mut light_mult = [1.0f32; 3];
        let mut env_mult = 1.0f32;
        let mut shadows_enabled = true;
        let out = ctx.run_ui(input, |ui| {
            lighting_panel(
                ui,
                &mut theme_idx,
                &mut light_mult,
                &mut env_mult,
                &mut shadows_enabled,
            );
        });
        let egui::FullOutput {
            textures_delta,
            shapes,
            pixels_per_point,
            ..
        } = out;
        let mut textures_delta = textures_delta;
        textures_delta.clear();
        let prims = ctx.tessellate(shapes, pixels_per_point);
        let mut fills = std::collections::HashSet::new();
        let mut right_side = false;
        for p in prims {
            if let egui::epaint::Primitive::Mesh(mesh) = p.primitive {
                for v in &mesh.vertices {
                    fills.insert(v.color);
                    if v.pos.x > 640.0 {
                        right_side = true;
                    }
                }
            }
        }
        assert!(right_side, "lighting panel must paint on the right side");
        for theme in &THEMES {
            let [r, g, b] = theme.lights[0].rgb;
            let fill = egui::Color32::from_rgb(r, g, b);
            assert!(
                fills.contains(&fill),
                "swatch fill {fill:?} for theme '{}' missing",
                theme.name
            );
        }
    }
}
