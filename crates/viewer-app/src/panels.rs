// crates/viewer-app/src/panels.rs
//! Overlay widgets drawn on top of the 3D viewport (Task 8: Open button,
//! loading spinner, load-error banner), the left vertical toolbar and the
//! right settings panel (Task 12: the Lighting section from Task 11 plus the
//! new Effects section).

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

/// Side effects the toolbar collected for the app to apply (Task 12). The
/// Open dialog runs inside the panel (native, blocking while open), and the
/// Screenshot click only raises a flag: the app flips the renderer's pending
/// screenshot flag after the panel returns (Task 13 collects the PNG), so no
/// toolbar button can panic without a scene or renderer.
pub struct ToolbarOutcome {
    pub open: Option<std::path::PathBuf>,
    pub screenshot: bool,
}

/// The left vertical toolbar (~56px): Open, Turntable (toggle), Screenshot,
/// Settings (toggles the right settings panel). Turntable/Screenshot are
/// inert until Task 13 — they set state only.
pub fn toolbar(
    ui: &mut egui::Ui,
    turntable: &mut bool,
    settings_open: &mut bool,
) -> ToolbarOutcome {
    let mut out = ToolbarOutcome {
        open: None,
        screenshot: false,
    };
    egui::Panel::left("toolbar")
        .resizable(false)
        .exact_size(56.0)
        .show(ui, |ui| {
            let w = ui.available_width();
            if ui
                .add_sized(
                    [w, 34.0],
                    egui::Button::new(egui::RichText::new("Open").small()),
                )
                .on_hover_text("Open a model (GLB, GLTF, OBJ, STL, PLY, 3MF)")
                .clicked()
            {
                out.open = rfd::FileDialog::new()
                    .add_filter("3D models", &["glb", "gltf", "obj", "stl", "ply", "3mf"])
                    .pick_file();
            }
            *turntable = toggle_button(ui, "Turntable", *turntable, w);
            if ui
                .add_sized(
                    [w, 34.0],
                    egui::Button::new(egui::RichText::new("Screenshot").small()),
                )
                .on_hover_text("Save a screenshot of the 3D view")
                .clicked()
            {
                out.screenshot = true;
            }
            *settings_open = toggle_button(ui, "Settings", *settings_open, w);
        });
    out
}

/// egui 0.36 has no builtin toggle button (brief note): a plain button whose
/// fill marks the selected state. Returns the new state (flipped on click).
fn toggle_button(ui: &mut egui::Ui, text: &str, on: bool, width: f32) -> bool {
    let fill = if on {
        ui.visuals().selection.bg_fill
    } else {
        ui.visuals().widgets.inactive.bg_fill
    };
    let clicked = ui
        .add_sized(
            [width, 34.0],
            egui::Button::new(egui::RichText::new(text).small()).fill(fill),
        )
        .clicked();
    clicked != on
}

/// The right settings panel: the Lighting section (Task 11) and the Effects
/// section (Task 12) share ONE panel — two stacked right panels would place
/// the second to the left of the first, and the original app exposes both as
/// sections of a single panel. Mutates the app state through the passed
/// refs; `app.rs` reads the same fields when building `FrameParams`.
#[allow(clippy::too_many_arguments)]
pub fn settings_panel(
    ui: &mut egui::Ui,
    theme_idx: &mut usize,
    light_mult: &mut [f32; 3],
    env_mult: &mut f32,
    shadows_enabled: &mut bool,
    wireframe: &mut bool,
    ground: &mut bool,
    grid: &mut bool,
    bloom: &mut bool,
) {
    egui::Panel::right("settings_panel")
        .resizable(false)
        .show(ui, |ui| {
            lighting_section(ui, theme_idx, light_mult, env_mult, shadows_enabled);
            ui.separator();
            effects_section(ui, wireframe, ground, grid, bloom);
        });
}

/// The Lighting section's content (Task 11's lighting_panel body, unchanged —
/// now a section of settings_panel): a 3-column grid of theme swatches (each
/// button's fill is the theme's light-0 color as its visual identity; the
/// selected theme gets a selection stroke), per-light intensity drags (0..3),
/// the environment intensity drag (0..3) and the Shadows toggle.
fn lighting_section(
    ui: &mut egui::Ui,
    theme_idx: &mut usize,
    light_mult: &mut [f32; 3],
    env_mult: &mut f32,
    shadows_enabled: &mut bool,
) {
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
                    let lum = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
                    let text = if lum > 128.0 {
                        egui::Color32::BLACK
                    } else {
                        egui::Color32::WHITE
                    };
                    let mut button =
                        egui::Button::new(egui::RichText::new(theme.name).small().color(text))
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
}

/// The Effects section (Task 12): each checkbox writes straight into the
/// per-frame `RenderSettings` (wireframe → `G.flags.x`, grid → `G.flags.y`,
/// ground disc / bloom → the render pass gates). Defaults live in app.rs.
fn effects_section(
    ui: &mut egui::Ui,
    wireframe: &mut bool,
    ground: &mut bool,
    grid: &mut bool,
    bloom: &mut bool,
) {
    ui.heading("Effects");
    ui.add_space(4.0);
    ui.checkbox(wireframe, "Wireframe");
    ui.checkbox(ground, "Ground plane");
    ui.checkbox(grid, "Grid");
    ui.checkbox(bloom, "Bloom");
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

    /// The settings panel must paint every theme swatch with its light-0
    /// color (the swatch IS the theme's visual identity) on the right side
    /// of the screen.
    #[test]
    fn settings_panel_paints_all_theme_swatches_on_the_right() {
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
        let mut wireframe = false;
        let mut ground = true;
        let mut grid = true;
        let mut bloom = true;
        let out = ctx.run_ui(input, |ui| {
            settings_panel(
                ui,
                &mut theme_idx,
                &mut light_mult,
                &mut env_mult,
                &mut shadows_enabled,
                &mut wireframe,
                &mut ground,
                &mut grid,
                &mut bloom,
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
        assert!(right_side, "settings panel must paint on the right side");
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

    /// Headless run of the toolbar with state capture (click tests reuse the
    /// geometry; `run` returns the shapes plus the toolbar outcome).
    struct ToolbarRun {
        shapes: Vec<egui::epaint::ClippedShape>,
        visuals: egui::Visuals,
        outcome: ToolbarOutcome,
    }

    fn run_toolbar(
        ctx: &egui::Context,
        turntable: &mut bool,
        settings_open: &mut bool,
        extra_events: Vec<egui::Event>,
    ) -> ToolbarRun {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let input = egui::RawInput {
            screen_rect: Some(viewport),
            events: extra_events,
            ..Default::default()
        };
        let mut visuals = None;
        let mut outcome = None;
        let out = ctx.run_ui(input, |ui| {
            visuals = Some(ui.visuals().clone());
            outcome = Some(toolbar(ui, turntable, settings_open));
        });
        let mut textures_delta = out.textures_delta;
        textures_delta.clear();
        ToolbarRun {
            shapes: out.shapes,
            visuals: visuals.expect("run_ui closure executed"),
            outcome: outcome.expect("run_ui closure executed"),
        }
    }

    /// The pre-tessellation rect shapes with their fills. Widget buttons
    /// paint their background as a rect shape each; the tessellated mesh
    /// stream is coalesced, so shape level is where buttons stay separate.
    fn button_rects(shapes: &[egui::epaint::ClippedShape]) -> Vec<(egui::Rect, egui::Color32)> {
        shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) => Some((r.rect, r.fill)),
                _ => None,
            })
            .collect()
    }

    /// The toolbar must sit on the LEFT edge with its four buttons stacked
    /// vertically (Open, Turntable, Screenshot, Settings), and the Settings
    /// toggle (on here) must paint with the selection fill while the others
    /// use the inactive fill.
    #[test]
    fn toolbar_paints_four_buttons_on_the_left_edge() {
        let ctx = egui::Context::default();
        let mut turntable = false;
        let mut settings_open = true;
        let run = run_toolbar(&ctx, &mut turntable, &mut settings_open, vec![]);
        let inactive = run.visuals.widgets.inactive.bg_fill;
        let selected = run.visuals.selection.bg_fill;

        let rects = button_rects(&run.shapes);
        let sel_count = rects.iter().filter(|(_, c)| *c == selected).count();
        let inactive_rects: Vec<egui::Rect> = rects
            .iter()
            .filter(|(_, c)| *c == inactive)
            .map(|(r, _)| *r)
            .collect();
        assert_eq!(sel_count, 1, "the on-state Settings toggle is selected");
        assert_eq!(
            inactive_rects.len(),
            3,
            "Open/Turntable/Screenshot use the inactive fill: {rects:?}"
        );

        let mut buttons: Vec<egui::Rect> = rects
            .iter()
            .filter(|(_, c)| *c == selected || *c == run.visuals.widgets.inactive.bg_fill)
            .map(|(r, _)| *r)
            .collect();
        buttons.sort_by(|a, b| a.top().total_cmp(&b.top()));
        for r in &buttons {
            assert!(
                r.right() <= 57.0 && r.left() >= -0.5,
                "button must stay inside the ~56px left strip: {r:?}"
            );
        }
        for w in buttons.windows(2) {
            assert!(
                w[1].top() > w[0].top() + 20.0,
                "the four buttons must stack vertically: {buttons:?}"
            );
        }
        assert!(
            buttons[3].top() > 100.0,
            "Settings is the bottom-most button: {buttons:?}"
        );
        assert!(!turntable && settings_open, "states must be unchanged");
        assert!(!run.outcome.screenshot && run.outcome.open.is_none());
    }

    /// Clicking a toolbar toggle flips its state (the egui hit-test runs on
    /// real painted geometry — the click point comes from the previous
    /// frame's Settings button rect, the bottom-most button, so the native
    /// Open dialog can never be triggered by this test).
    #[test]
    fn toolbar_toggle_click_flips_settings_state() {
        let ctx = egui::Context::default();
        let mut turntable = false;
        let mut settings_open = true;

        // Frame 1: locate the Settings button (bottom-most button, whatever
        // its current fill — it starts selected since the panel is open; its
        // top is well below the Open button at the very top).
        let run = run_toolbar(&ctx, &mut turntable, &mut settings_open, vec![]);
        let inactive = run.visuals.widgets.inactive.bg_fill;
        let selected = run.visuals.selection.bg_fill;
        let rect = button_rects(&run.shapes)
            .iter()
            .filter(|(_, c)| *c == inactive || *c == selected)
            .map(|(r, _)| *r)
            .max_by(|a, b| a.top().total_cmp(&b.top()))
            .expect("Settings button must paint");
        assert!(
            rect.top() > 100.0,
            "safety: the click target must not be the Open button (top {})",
            rect.top()
        );
        let center = rect.center();

        // Frame 2: press. Frame 3: release -> the toggle flips.
        let press = vec![
            egui::Event::PointerMoved(center),
            egui::Event::PointerButton {
                pos: center,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ];
        let release = vec![
            egui::Event::PointerMoved(center),
            egui::Event::PointerButton {
                pos: center,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ];
        run_toolbar(&ctx, &mut turntable, &mut settings_open, press.clone());
        run_toolbar(&ctx, &mut turntable, &mut settings_open, release.clone());
        assert!(
            !settings_open && !turntable,
            "clicking the on-state Settings toggle must hide the panel"
        );

        // Click again: the off-state toggle turns back on.
        run_toolbar(&ctx, &mut turntable, &mut settings_open, press.clone());
        run_toolbar(&ctx, &mut turntable, &mut settings_open, release.clone());
        assert!(settings_open && !turntable, "second click re-opens");
    }

    /// The toggle helper: fill follows the selected state; a click on real
    /// geometry flips the value.
    #[test]
    fn toggle_button_click_flips_and_fill_follows_state() {
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let run = |events: Vec<egui::Event>, on: &mut bool| -> (Vec<egui::Rect>, Vec<egui::Rect>) {
            let input = egui::RawInput {
                screen_rect: Some(viewport),
                events,
                ..Default::default()
            };
            let mut visuals = None;
            let out = ctx.run_ui(input, |ui| {
                visuals = Some(ui.visuals().clone());
                *on = toggle_button(ui, "Turntable", *on, ui.available_width());
            });
            let mut textures_delta = out.textures_delta;
            textures_delta.clear();
            let v = visuals.expect("run_ui closure executed");
            let rects = button_rects(&out.shapes);
            (
                rects
                    .iter()
                    .filter(|(_, c)| *c == v.widgets.inactive.bg_fill)
                    .map(|(r, _)| *r)
                    .collect(),
                rects
                    .iter()
                    .filter(|(_, c)| *c == v.selection.bg_fill)
                    .map(|(r, _)| *r)
                    .collect(),
            )
        };

        // Off state: plain inactive fill.
        let mut on = false;
        let (inactive_rects, selected_rects) = run(vec![], &mut on);
        assert!(!on, "no click yet");
        assert_eq!(inactive_rects.len(), 1, "off-state uses the inactive fill");
        assert!(
            selected_rects.is_empty(),
            "off-state must not use the selection fill"
        );

        // Click it (full-width button at the top of the window).
        let click = |pressed: bool| {
            vec![egui::Event::PointerButton {
                pos: egui::pos2(400.0, 10.0),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            }]
        };
        run(click(true), &mut on);
        let (inactive_rects, selected_rects) = run(click(false), &mut on);
        assert!(on, "click must flip the toggle on");
        // The click frame still paints the OLD fill (immediate-mode paint
        // lags the state flip by one frame); the next frame shows the new one.
        assert_eq!(
            inactive_rects.len(),
            1,
            "click frame paints the pre-click (inactive) fill"
        );
        assert!(selected_rects.is_empty(), "selection fill lags one frame");
        let (inactive_rects, selected_rects) = run(vec![], &mut on);
        assert!(
            inactive_rects.is_empty(),
            "on-state must not use the inactive fill"
        );
        assert_eq!(selected_rects.len(), 1, "on-state uses the selection fill");
    }

    /// The Effects section's four checkboxes reflect their state: checked
    /// boxes paint the check glyph, so the all-on frame must tessellate more
    /// vertices than the all-off frame (same labels/fonts otherwise).
    #[test]
    fn effects_checkboxes_paint_their_state_on_the_right() {
        let paint = |all_on: bool| {
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
            let (mut wf, mut g, mut gr, mut b) = (all_on, all_on, all_on, all_on);
            let out = ctx.run_ui(input, |ui| {
                settings_panel(
                    ui,
                    &mut theme_idx,
                    &mut light_mult,
                    &mut env_mult,
                    &mut shadows_enabled,
                    &mut wf,
                    &mut g,
                    &mut gr,
                    &mut b,
                );
            });
            let mut textures_delta = out.textures_delta;
            textures_delta.clear();
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            let mut verts_right = 0;
            for p in &prims {
                if let egui::epaint::Primitive::Mesh(mesh) = &p.primitive {
                    for v in &mesh.vertices {
                        if v.pos.x > 640.0 {
                            verts_right += 1;
                        }
                    }
                }
            }
            verts_right
        };
        let off = paint(false);
        let on = paint(true);
        assert!(
            on > off,
            "checked boxes must paint the check glyph ({on} vs {off})"
        );
    }
}
