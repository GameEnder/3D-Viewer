// crates/viewer-app/src/panels.rs
//! Overlay widgets drawn on top of the 3D viewport (Task 8: loading spinner,
//! load-error banner), the left vertical toolbar and the right settings panel
//! (Task 12: Lighting + Effects sections; Task 13 adds Model + Camera).

use eframe::egui;

use crate::themes::THEMES;

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

/// App-side animation playback state (Task 17). The bar writes it; the app
/// advances `time` each frame and feeds `(clip, time)` into
/// `FrameParams.anim`. Defaults: clip 0, paused, t = 0, speed 1.0.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationState {
    pub clip: usize,
    pub playing: bool,
    pub time: f32,
    /// Playback rate multiplier (bar drag, 0.1..4, default 1.0).
    pub speed: f32,
}

impl Default for AnimationState {
    fn default() -> Self {
        Self {
            clip: 0,
            playing: false,
            time: 0.0,
            speed: 1.0,
        }
    }
}

/// The bottom animation bar (Task 17), shown only while a scene WITH
/// animations is loaded: clip picker, Play/Pause, a scrub slider that writes
/// `time` directly (playing state untouched) and the speed drag. Switching
/// clips restarts playback at t = 0.
pub fn animation_bar(
    ui: &mut egui::Ui,
    anim: &mut AnimationState,
    animations: &[viewer_scene::Animation],
) {
    if animations.is_empty() {
        return;
    }
    egui::Panel::bottom("animation_bar")
        .resizable(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Defensive clip clamp (the app resets state per load, so the
                // clip is in range; a stale index must not panic here).
                let clip = anim.clip.min(animations.len() - 1);
                let clip_label = |i: usize| {
                    let name = &animations[i].name;
                    if name.is_empty() {
                        format!("Clip {}", i + 1)
                    } else {
                        name.clone()
                    }
                };
                let prev = anim.clip;
                egui::ComboBox::from_id_salt("animation_clip")
                    .selected_text(clip_label(clip))
                    .show_ui(ui, |ui| {
                        for i in 0..animations.len() {
                            ui.selectable_value(&mut anim.clip, i, clip_label(i));
                        }
                    });
                if anim.clip != prev {
                    anim.time = 0.0;
                }
                let label = if anim.playing { "Pause" } else { "Play" };
                if ui
                    .add_sized(
                        [56.0, 22.0],
                        egui::Button::new(egui::RichText::new(label).small()),
                    )
                    .clicked()
                {
                    anim.playing = !anim.playing;
                }
                let dur = animations[clip].duration.max(0.0);
                let range = if dur > 0.0 { 0.0..=dur } else { 0.0..=1.0 };
                ui.add(
                    egui::Slider::new(&mut anim.time, range)
                        .text("t")
                        .show_value(true),
                );
                ui.add(
                    egui::DragValue::new(&mut anim.speed)
                        .range(0.1..=4.0)
                        .speed(0.01)
                        .prefix("x"),
                );
            });
        });
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

/// The Camera section's editable refs (Task 13). `fov_y_deg` points straight
/// into `OrbitCamera` (single source of truth — `camera.fit` restores the
/// 45° default on reset); `reset_view` is raised for the app to apply, since
/// the fit needs the scene bounds.
pub struct CameraSection<'a> {
    pub fov_y_deg: &'a mut f32,
    pub exposure: &'a mut f32,
    pub contrast: &'a mut f32,
    pub reset_view: &'a mut bool,
}

/// The Model section's data + editable refs (Task 13). `source_name`/counts
/// are read-only scene info (None/0 without a loaded scene — the section
/// then shows a placeholder instead of panicking).
pub struct ModelSection<'a> {
    pub source_name: Option<&'a str>,
    pub tri_count: u32,
    pub vert_count: u32,
    pub turntable_speed: &'a mut f32,
    pub rot_offset_deg: &'a mut f32,
    pub model_scale: &'a mut f32,
}

/// The right settings panel: Lighting (Task 11), Effects (Task 12), Model +
/// Camera (Task 13) share ONE panel — two stacked right panels would place
/// the second to the left of the first, and the original app exposes all of
/// these as sections of a single panel. A vertical ScrollArea keeps every
/// section reachable on short windows. Mutates the app state through the
/// passed refs; `app.rs` reads the same fields when building `FrameParams`.
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
    model: ModelSection<'_>,
    camera: CameraSection<'_>,
) {
    egui::Panel::right("settings_panel")
        .resizable(false)
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                lighting_section(ui, theme_idx, light_mult, env_mult, shadows_enabled);
                ui.separator();
                effects_section(ui, wireframe, ground, grid, bloom);
                ui.separator();
                model_section(ui, model);
                ui.separator();
                camera_section(ui, camera);
            });
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

/// The Model section (Task 13): source file name + triangle/vertex counts
/// (read-only scene info), then the turntable speed (0..90 deg/s, default
/// 30 ≈ 12 s/rev — the original's exact rate was never extracted), the
/// static rotation offset (0..360, added to the accumulated turntable
/// angle) and the uniform model scale (0.1..10, applied by the renderer
/// around the bounds center).
fn model_section(ui: &mut egui::Ui, m: ModelSection<'_>) {
    ui.heading("Model");
    ui.add_space(4.0);
    match m.source_name {
        Some(name) => {
            ui.horizontal(|ui| {
                ui.label("File:");
                ui.label(name);
            });
            ui.label(format!("Triangles: {}", m.tri_count));
            ui.label(format!("Vertices: {}", m.vert_count));
        }
        None => {
            ui.label("No model loaded");
        }
    }
    ui.separator();
    ui.add(egui::Slider::new(m.turntable_speed, 0.0..=90.0).text("Turntable speed"));
    ui.add(egui::Slider::new(m.rot_offset_deg, 0.0..=360.0).text("Rotation offset"));
    ui.add(egui::Slider::new(m.model_scale, 0.1..=10.0).text("Scale"));
}

/// The Camera section (Task 13): FOV (20..90, straight into the orbit
/// camera), exposure (−3..3) and contrast (0.2..3) into `RenderSettings`,
/// and a "Reset view" button that re-fits the camera to the scene bounds
/// (applied by the app, which owns the bounds).
fn camera_section(ui: &mut egui::Ui, c: CameraSection<'_>) {
    ui.heading("Camera");
    ui.add_space(4.0);
    ui.add(egui::Slider::new(c.fov_y_deg, 20.0..=90.0).text("FOV"));
    ui.add(egui::Slider::new(c.exposure, -3.0..=3.0).text("Exposure"));
    ui.add(egui::Slider::new(c.contrast, 0.2..=3.0).text("Contrast"));
    if ui.button("Reset view").clicked() {
        *c.reset_view = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bottom animation bar paints widgets in the bottom strip only when
    /// it is called (the app shows it only for scenes WITH animations), and
    /// the Play/Pause label follows the state (different glyph counts).
    #[test]
    fn animation_bar_paints_widgets_in_the_bottom_strip() {
        let animations = vec![viewer_scene::Animation {
            name: "spin".into(),
            duration: 1.0,
            channels: vec![],
        }];
        let paint = |show: bool, playing: bool| -> usize {
            let ctx = egui::Context::default();
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
            let input = egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            };
            let mut anim = AnimationState {
                playing,
                ..AnimationState::default()
            };
            let out = ctx.run_ui(input, |ui| {
                if show {
                    animation_bar(ui, &mut anim, &animations);
                }
            });
            let mut textures_delta = out.textures_delta;
            textures_delta.clear();
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            let mut count = 0;
            for p in &prims {
                if let egui::epaint::Primitive::Mesh(mesh) = &p.primitive {
                    for v in &mesh.vertices {
                        if v.pos.y > 700.0 {
                            count += 1;
                        }
                    }
                }
            }
            count
        };
        let baseline = paint(false, false);
        let shown_paused = paint(true, false);
        let shown_playing = paint(true, true);
        assert_eq!(baseline, 0, "without the bar the bottom strip stays empty");
        assert!(
            shown_paused > 20,
            "the bar must paint its widgets at the bottom, got {shown_paused}"
        );
        assert_ne!(
            shown_paused, shown_playing,
            "Play/Pause labels must follow the playing state"
        );
    }

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
        let (mut speed, mut offset, mut scale) = (30.0f32, 0.0f32, 1.0f32);
        let (mut fov, mut exposure, mut contrast, mut reset) = (45.0f32, 0.5f32, 1.66f32, false);
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
                ModelSection {
                    source_name: Some("Bee.glb"),
                    tri_count: 12,
                    vert_count: 8,
                    turntable_speed: &mut speed,
                    rot_offset_deg: &mut offset,
                    model_scale: &mut scale,
                },
                CameraSection {
                    fov_y_deg: &mut fov,
                    exposure: &mut exposure,
                    contrast: &mut contrast,
                    reset_view: &mut reset,
                },
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
            let (mut speed, mut offset, mut scale) = (30.0f32, 0.0f32, 1.0f32);
            let (mut fov, mut exposure, mut contrast, mut reset) =
                (45.0f32, 0.5f32, 1.66f32, false);
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
                    ModelSection {
                        source_name: None,
                        tri_count: 0,
                        vert_count: 0,
                        turntable_speed: &mut speed,
                        rot_offset_deg: &mut offset,
                        model_scale: &mut scale,
                    },
                    CameraSection {
                        fov_y_deg: &mut fov,
                        exposure: &mut exposure,
                        contrast: &mut contrast,
                        reset_view: &mut reset,
                    },
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

    /// The Model section paints the scene's file name and tri/vert counts:
    /// a panel run with scene info tessellates more right-side text than one
    /// without (the placeholder "No model loaded" is shorter).
    #[test]
    fn model_section_paints_scene_info() {
        let paint = |with_scene: bool| {
            let ctx = egui::Context::default();
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
            let input = egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            };
            let (mut speed, mut offset, mut scale) = (30.0f32, 0.0f32, 1.0f32);
            let (mut fov, mut exposure, mut contrast, mut reset) =
                (45.0f32, 0.5f32, 1.66f32, false);
            let out = ctx.run_ui(input, |ui| {
                settings_panel(
                    ui,
                    &mut 0usize,
                    &mut [1.0f32; 3],
                    &mut 1.0f32,
                    &mut true,
                    &mut false,
                    &mut true,
                    &mut true,
                    &mut true,
                    ModelSection {
                        source_name: with_scene.then_some("spinning_teapot_with_lid.glb"),
                        tri_count: 123_456,
                        vert_count: 65_432,
                        turntable_speed: &mut speed,
                        rot_offset_deg: &mut offset,
                        model_scale: &mut scale,
                    },
                    CameraSection {
                        fov_y_deg: &mut fov,
                        exposure: &mut exposure,
                        contrast: &mut contrast,
                        reset_view: &mut reset,
                    },
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
        let with = paint(true);
        let without = paint(false);
        assert!(
            with > without,
            "scene info (name + counts) must paint in the Model section ({with} vs {without})"
        );
    }

    /// Clicking the Camera section's "Reset view" button (real geometry from
    /// the previous frame, on the right side) raises the reset flag.
    #[test]
    fn reset_view_button_click_raises_flag() {
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let run = |events: Vec<egui::Event>, reset: &mut bool| -> Vec<egui::Rect> {
            let input = egui::RawInput {
                screen_rect: Some(viewport),
                events,
                ..Default::default()
            };
            let mut visuals = None;
            let out = ctx.run_ui(input, |ui| {
                visuals = Some(ui.visuals().clone());
                settings_panel(
                    ui,
                    &mut 0usize,
                    &mut [1.0f32; 3],
                    &mut 1.0f32,
                    &mut true,
                    &mut false,
                    &mut true,
                    &mut true,
                    &mut true,
                    ModelSection {
                        source_name: None,
                        tri_count: 0,
                        vert_count: 0,
                        turntable_speed: &mut 30.0,
                        rot_offset_deg: &mut 0.0,
                        model_scale: &mut 1.0,
                    },
                    CameraSection {
                        fov_y_deg: &mut 45.0,
                        exposure: &mut 0.5,
                        contrast: &mut 1.66,
                        reset_view: reset,
                    },
                );
            });
            let mut textures_delta = out.textures_delta;
            textures_delta.clear();
            let inactive = visuals
                .expect("run_ui closure executed")
                .widgets
                .inactive
                .bg_fill;
            button_rects(&out.shapes)
                .into_iter()
                .filter(|(r, c)| *c == inactive && r.left() > 640.0)
                .map(|(r, _)| r)
                .collect()
        };

        // Frame 1: locate the Reset view button — the bottom-most button-fill
        // rect in the right-side panel (the sliders use different fills).
        let rects = run(vec![], &mut false);
        let target = rects
            .iter()
            .copied()
            .max_by(|a, b| a.top().total_cmp(&b.top()))
            .expect("Reset view button must paint");
        let center = target.center();

        // Frames 2/3: press + release -> the flag flips.
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
        run(press, &mut false);
        let mut reset = false;
        run(release, &mut reset);
        assert!(reset, "clicking Reset view must raise the reset flag");
    }
}
