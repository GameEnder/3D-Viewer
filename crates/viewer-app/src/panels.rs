// crates/viewer-app/src/panels.rs
//! The application chrome, redesigned to match the original 3D Viewer.
//!
//! Contains: a slim top strip (Open / Screenshot), the always-visible
//! bottom playback bar (turntable + animation transport), the modal
//! load-error dialog, the loading spinner, the status line, and the
//! docked right settings panel with an icon tab strip.

use eframe::egui;

use crate::themes::THEMES;

/// Spinner centered in the viewport while a model loads (brief Step 3).
pub fn loading_spinner(ui: &mut egui::Ui, viewport: egui::Rect) {
    let rect = egui::Rect::from_center_size(viewport.center(), egui::vec2(40.0, 40.0));
    ui.put(rect, egui::Spinner::new().size(40.0));
}

/// Modal load-error dialog (Task 18, replaces the Task 8 banner): a centered
/// fixed-size window with the failure message and an OK button. The dialog is
/// re-shown every frame while the app stays in `Failed`; `ack` is raised when
/// OK is clicked and the app returns to Idle.
pub fn error_dialog(ctx: &egui::Context, message: &str, ack: &mut bool) {
    egui::Window::new("Could not open model")
        .collapsible(false)
        .resizable(false)
        // An error modal must appear immediately, not fade in (also keeps
        // its widgets opaque from the first painted frame).
        .fade_in(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(message);
            if ui.button("OK").clicked() {
                *ack = true;
            }
        });
}

/// Status-line text for a loaded scene (Task 18): "name — N tris, M verts",
/// with a "(N warnings)" suffix when the loader reported any (kept pure so
/// the app can unit-test the wording).
pub fn status_text(scene: &viewer_scene::Scene) -> String {
    let warn = if scene.warnings.is_empty() {
        String::new()
    } else {
        format!(" ({} warnings)", scene.warnings.len())
    };
    format!(
        "{} — {} tris, {} verts{}",
        scene.source_name, scene.tri_count, scene.vert_count, warn
    )
}

/// Status line along the viewport's bottom-left edge (Task 18): filename,
/// triangle/vertex counts and the warning count; hovering lists the warnings
/// one per line.
pub fn status_line(ui: &mut egui::Ui, viewport: egui::Rect, scene: &viewer_scene::Scene) {
    let rect = egui::Rect::from_min_size(
        viewport.left_bottom() + egui::vec2(8.0, -24.0),
        egui::vec2((viewport.width() - 16.0).max(100.0), 20.0),
    );
    let resp = ui.put(
        rect,
        egui::Label::new(egui::RichText::new(status_text(scene)).small().weak()),
    );
    if !scene.warnings.is_empty() {
        resp.on_hover_text(scene.warnings.join("\n"));
    }
}

/// Transient status note (Task 18): e.g. a screenshot save failure, shown at
/// the viewport's bottom-left in a warning tint until the app expires it.
/// `above_line` lifts it one row so it does not cover the Ready status line.
pub fn status_note(ui: &mut egui::Ui, viewport: egui::Rect, text: &str, above_line: bool) {
    let dy = if above_line { -48.0 } else { -24.0 };
    let rect = egui::Rect::from_min_size(
        viewport.left_bottom() + egui::vec2(8.0, dy),
        egui::vec2((viewport.width() - 16.0).max(100.0), 20.0),
    );
    ui.put(
        rect,
        egui::Label::new(
            egui::RichText::new(text)
                .small()
                .color(egui::Color32::from_rgb(255, 170, 120)),
        ),
    );
}

/// Side effects the menu bar collected for the app to apply.
#[derive(Default)]
pub struct MenuBarOutcome {
    pub open: Option<std::path::PathBuf>,
    pub screenshot: bool,
    pub exit: bool,
    /// Reset ALL render/lighting/camera settings to their defaults.
    pub reset_settings: bool,
    /// Viewpoint preset (elevation, rotation) from the View menu.
    pub viewpoint: Option<(f32, f32)>,
    /// Reset camera framing (fit to model).
    pub reset_view: bool,
    /// Show the About dialog.
    pub about: bool,
    /// Open the Preferences (file associations) dialog.
    pub preferences: bool,
}

/// The traditional top menu bar (original-style: File / Edit / Tools /
/// View / Help), shown in a top panel. Actions are collected into
/// `MenuBarOutcome` for the app to apply after the bar returns.
pub fn menu_bar(ui: &mut egui::Ui) -> MenuBarOutcome {
    let mut out = MenuBarOutcome::default();
    egui::Panel::top("menu_bar")
        .resizable(false)
        .show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                // ── File ──
                ui.menu_button("File", |ui| {
                    if ui.button("Open…").clicked() {
                        out.open = rfd::FileDialog::new()
                            .add_filter(
                                "3D models",
                                &["glb", "gltf", "obj", "stl", "ply", "3mf", "fbx", "nif"],
                            )
                            .pick_file();
                        ui.close();
                    }
                    if ui.button("Save Screenshot…").clicked() {
                        out.screenshot = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Exit").clicked() {
                        out.exit = true;
                        ui.close();
                    }
                });
                // ── Edit ──
                ui.menu_button("Edit", |ui| {
                    if ui.button("Reset All Settings").clicked() {
                        out.reset_settings = true;
                        ui.close();
                    }
                    if ui.button("Preferences…").clicked() {
                        out.preferences = true;
                        ui.close();
                    }
                });
                // ── Tools ──
                ui.menu_button("Tools", |ui| {
                    if ui.button("Take Screenshot…").clicked() {
                        out.screenshot = true;
                        ui.close();
                    }
                });
                // ── View ──
                ui.menu_button("View", |ui| {
                    if ui.button("Reset View").clicked() {
                        out.reset_view = true;
                        ui.close();
                    }
                    ui.separator();
                    for &(el, ro, label) in VIEWPOINTS.iter() {
                        if ui.button(label).clicked() {
                            out.viewpoint = Some((el, ro));
                            ui.close();
                        }
                    }
                });
                // ── Help ──
                ui.menu_button("Help", |ui| {
                    if ui.button("About 3D Viewer").clicked() {
                        out.about = true;
                        ui.close();
                    }
                });
            });
        });
    out
}

/// Modal About dialog (original has a deprecation notice; ours shows the
/// clone's info). Dismissed with OK.
pub fn about_dialog(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }
    egui::Window::new("About 3D Viewer")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.heading("3D Viewer");
            ui.label("A Rust clone of Microsoft's 3D Viewer");
            ui.add_space(4.0);
            ui.label("Supports: GLB, GLTF, OBJ, STL, PLY, 3MF, FBX, NIF");
            ui.add_space(8.0);
            if ui.button("OK").clicked() {
                *open = false;
            }
        });
}

/// Preferences dialog (Edit → Preferences): Windows file association
/// management. Shows the current registration status and offers
/// register / unregister buttons. On non-Windows, the dialog shows a note.
#[derive(Default)]
pub struct PreferencesState {
    pub open: bool,
    /// Result message from the last register/unregister action.
    pub message: Option<Result<String, String>>,
}

pub fn preferences_dialog(ctx: &egui::Context, s: &mut PreferencesState) {
    if !s.open {
        return;
    }
    egui::Window::new("Preferences")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.heading("File Associations");
            ui.add_space(4.0);

            #[cfg(windows)]
            {
                ui.label("Make 3D Viewer the default app for these file types:");
                ui.add_space(6.0);

                // Show the extension list with current handler info.
                let registered = crate::file_assoc::is_registered();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(if registered {
                            "Status: Registered ✓"
                        } else {
                            "Status: Not registered"
                        })
                        .color(if registered {
                            egui::Color32::from_rgb(100, 200, 100)
                        } else {
                            ui.style().visuals.warn_fg_color
                        }),
                    );
                });
                ui.add_space(4.0);

                for ext in crate::file_assoc::EXTENSIONS {
                    let prog_id = crate::file_assoc::PROG_ID;
                    let handler = crate::file_assoc::effective_handler(ext);
                    let protected =
                        crate::file_assoc::user_choice(ext).is_some_and(|h| h != prog_id);
                    ui.horizontal(|ui| {
                        ui.label(format!(".{ext}"));
                        match handler {
                            Some(h) if h == prog_id => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(100, 200, 100),
                                    "→ 3D Viewer",
                                );
                            }
                            Some(h) if protected => {
                                ui.colored_label(
                                    ui.style().visuals.warn_fg_color,
                                    format!("{h} — Windows-protected, Register all can retake it"),
                                );
                            }
                            Some(h) => {
                                ui.weak(format!("({h})"));
                            }
                            None => {
                                ui.weak("(no default)");
                            }
                        }
                    });
                }

                ui.add_space(8.0);

                // Action result message.
                if let Some(msg) = &s.message {
                    match msg {
                        Ok(text) => {
                            ui.colored_label(egui::Color32::from_rgb(100, 200, 100), text);
                        }
                        Err(e) => {
                            ui.colored_label(
                                ui.style().visuals.error_fg_color,
                                format!("Error: {e}"),
                            );
                        }
                    }
                    ui.add_space(4.0);
                }

                ui.horizontal(|ui| {
                    if ui.button("Register all").clicked() {
                        s.message = Some(crate::file_assoc::register().map_err(|e| e.to_string()));
                    }
                    if ui.button("Unregister all").clicked() {
                        s.message =
                            Some(crate::file_assoc::unregister().map_err(|e| e.to_string()));
                    }
                });
                ui.add_space(4.0);
                ui.weak(
                    "Registering writes Windows-validated defaults, including for \
                     types normally protected by Windows. No admin rights required.",
                );
            }

            #[cfg(not(windows))]
            {
                ui.label("File associations are only supported on Windows.");
            }

            ui.add_space(8.0);
            if ui.button("Close").clicked() {
                s.open = false;
            }
        });
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

/// All editable state the right panel touches (the app owns the fields;
/// `app.rs` reads the same fields when building `FrameParams`).
pub struct PanelState<'a> {
    pub theme_idx: &'a mut usize,
    pub light_mult: &'a mut [f32; 3],
    pub env_mult: &'a mut f32,
    pub shadows_enabled: &'a mut bool,
    pub wireframe: &'a mut bool,
    pub ground: &'a mut bool,
    pub grid: &'a mut bool,
    pub bloom: &'a mut bool,
    pub source_name: Option<&'a str>,
    /// Computed mesh + texture statistics (original-style).
    pub stats: ModelStats,
    pub rot_offset_deg: &'a mut f32,
    pub model_scale: &'a mut f32,
    pub fov_y_deg: &'a mut f32,
    pub exposure: &'a mut f32,
    pub contrast: &'a mut f32,
    pub reset_view: &'a mut bool,
    pub active_tab: &'a mut usize,
    /// Rig rotation offset in degrees, added to the theme's rotation.
    pub rig_rot_deg: &'a mut f32,
    /// Per-light color overrides (None = use the theme's color).
    pub light_color_override: &'a mut [Option<egui::Color32>; 3],
    /// Environment (ambient) color override (None = the theme's).
    pub env_color_override: &'a mut Option<egui::Color32>,
    /// Requested camera viewpoint (elevation_deg, rotation_deg) — set by the
    /// preset buttons in the View tab, applied by the app after the panel.
    pub viewpoint_request: &'a mut Option<(f32, f32)>,
    /// Viewpoint preset thumbnails (one per preset, model-dependent).
    /// None = no model loaded yet → text-button fallback.
    pub view_thumbs: Option<&'a [egui::TextureHandle]>,
}

/// Theme thumbnails for the Lighting tab's 3×3 grid. `Textures` holds the
/// GPU-rendered previews (one per theme, original-style); `None` falls back
/// to the colored name swatches (headless tests, GPU-less startup).
pub enum ThemeThumbs<'a> {
    Textures(&'a [egui::TextureHandle]),
    None,
}

/// The docked right settings panel (original-style): an icon tab strip
/// (Lighting / Model / View) over large section headings. Mutates the app
/// state through `s`.
pub fn right_panel(ui: &mut egui::Ui, mut s: PanelState<'_>, thumbs: ThemeThumbs<'_>) {
    egui::Panel::right("settings_panel")
        .resizable(false)
        // .exact_size probe-removed
        .show(ui, |ui| {
            const TABS: [(&str, &str); 3] = [("☀", "Lighting"), ("📊", "Model"), ("👁", "View")];
            // Icon tab strip, right-aligned + TOP-aligned like the original's.
            // (Align::Center here poisons the panel's cursor flow — the body
            // below the strip then paints off-screen; user-visible as an
            // empty panel.)
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                ui.add_space(8.0);
                for (i, (icon, name)) in TABS.iter().enumerate() {
                    let selected = *s.active_tab == i;
                    let resp = ui
                        .add(
                            egui::Button::new(egui::RichText::new(*icon).size(17.0)).fill(
                                if selected {
                                    ui.visuals().selection.bg_fill
                                } else {
                                    egui::Color32::TRANSPARENT
                                },
                            ),
                        )
                        .on_hover_text(*name);
                    if resp.clicked() {
                        *s.active_tab = i;
                    }
                }
            });
            ui.add_space(2.0);
            ui.separator();
            ui.add_space(4.0);
            ui.heading(TABS[*s.active_tab].1);
            ui.add_space(2.0);
            let tab = *s.active_tab;
            match tab {
                0 => lighting_tab(ui, &mut s, thumbs),
                1 => model_tab(ui, &mut s),
                _ => view_tab(ui, &mut s),
            }
        });
}

fn section_header(ui: &mut egui::Ui, name: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(name).heading());
    ui.add_space(2.0);
}
/// The Lighting tab: the theme grid (thumbnails or swatch fallback), the
/// per-light color/intensity controls (original-style sliders), the
/// environment controls and the rig rotation.
fn lighting_tab(ui: &mut egui::Ui, s: &mut PanelState<'_>, thumbs: ThemeThumbs<'_>) {
    section_header(ui, "Themes");
    match thumbs {
        ThemeThumbs::Textures(textures) => theme_grid_thumbnails(ui, s.theme_idx, textures),
        ThemeThumbs::None => theme_grid_swatches(ui, s.theme_idx),
    }
    ui.add_space(6.0);
    ui.separator();
    section_header(ui, "Lights & Environment");
    let theme = &THEMES[*s.theme_idx];
    for (n, mult) in s.light_mult.iter_mut().enumerate() {
        // Color + intensity per light (original-style: swatch + slider).
        ui.horizontal(|ui| {
            ui.label(format!("Light {}", n + 1));
            // Color picker: shows the theme's color (or the user override);
            // picking a color sets the override; the ↺ clears it.
            let current = s.light_color_override[n].unwrap_or_else(|| {
                let [r, g, b] = theme.lights[n].rgb;
                egui::Color32::from_rgb(r, g, b)
            });
            let mut color = current;
            if ui.color_edit_button_srgba(&mut color).changed() {
                s.light_color_override[n] = Some(color);
            }
            if s.light_color_override[n].is_some() && ui.small_button("↺").clicked() {
                s.light_color_override[n] = None;
            }
        });
        ui.add(egui::Slider::new(mult, 0.0..=3.0).show_value(true));
    }
    ui.horizontal(|ui| {
        ui.label("Environment");
        let current = s.env_color_override.unwrap_or_else(|| {
            let [r, g, b] = theme.env.rgb;
            egui::Color32::from_rgb(r, g, b)
        });
        let mut color = current;
        if ui.color_edit_button_srgba(&mut color).changed() {
            *s.env_color_override = Some(color);
        }
        if s.env_color_override.is_some() && ui.small_button("↺").clicked() {
            *s.env_color_override = None;
        }
    });
    ui.add(egui::Slider::new(s.env_mult, 0.0..=3.0).show_value(true));
    ui.separator();
    ui.label("Light Rotation");
    ui.add(
        egui::Slider::new(s.rig_rot_deg, 0.0..=360.0)
            .suffix("°")
            .show_value(true),
    );
    ui.add_space(4.0);
    ui.separator();
    ui.checkbox(s.shadows_enabled, "Shadows");
}

/// Original-style theme swatches: one GPU-rendered preview per theme in a
/// 3×3 grid, selection stroke on the active theme.
fn theme_grid_thumbnails(
    ui: &mut egui::Ui,
    theme_idx: &mut usize,
    textures: &[egui::TextureHandle],
) {
    let cell = 88.0f32;
    egui::Grid::new("theme_grid")
        .num_columns(3)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            for (row, chunk) in textures.chunks(3).enumerate() {
                for (col, tex) in chunk.iter().enumerate() {
                    let idx = row * 3 + col;
                    let mut button =
                        egui::Button::image(egui::Image::new((tex.id(), egui::vec2(cell, cell))));
                    if idx == *theme_idx {
                        button = button.stroke(ui.visuals().selection.stroke);
                    }
                    if ui.add(button).on_hover_text(THEMES[idx].name).clicked() {
                        *theme_idx = idx;
                    }
                }
                ui.end_row();
            }
        });
}

/// Fallback theme swatches (no GPU thumbnails available): each button's fill
/// is the theme's light-0 color as its visual identity.
fn theme_grid_swatches(ui: &mut egui::Ui, theme_idx: &mut usize) {
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
}

/// The Model tab: mesh stats + the wireframe toggle (the original keeps its
/// triangle display under "Mesh Data" in its Stats & Shading tab), then the
/// transform controls.
/// Computed model statistics (original-style Mesh Data + Texture Data).
/// The app fills this from the loaded scene; the panel displays it read-only.
#[derive(Clone, Copy, Debug, Default)]
pub struct ModelStats {
    pub tri_count: u32,
    pub vert_count: u32,
    pub has_uv_set_0: bool,
    pub has_uv_set_1: bool,
    /// How many materials carry each texture channel.
    pub albedo_count: u32,
    pub normal_count: u32,
    pub gloss_count: u32,
    pub occlusion_count: u32,
    pub emissive_count: u32,
    pub material_count: u32,
}

fn model_tab(ui: &mut egui::Ui, s: &mut PanelState<'_>) {
    section_header(ui, "Mesh Data");
    match s.source_name {
        Some(name) => {
            ui.horizontal(|ui| {
                ui.label("File:");
                ui.label(name);
            });
            ui.separator();
            let m = s.stats;
            ui.label(format!("Triangles: {}", m.tri_count));
            ui.label(format!("Vertices: {}", m.vert_count));
            ui.label(format!(
                "UV Set 0: {}",
                if m.has_uv_set_0 { "Yes" } else { "No" }
            ));
            ui.label(format!(
                "UV Set 1: {}",
                if m.has_uv_set_1 { "Yes" } else { "No" }
            ));
        }
        None => {
            ui.label("No model loaded");
        }
    }
    ui.add_space(4.0);
    ui.checkbox(s.wireframe, "Wireframe");
    ui.add_space(6.0);
    ui.separator();
    section_header(ui, "Texture Data");
    let m = s.stats;
    ui.label(format!("Albedo: {} / {}", m.albedo_count, m.material_count));
    ui.label(format!("Normal: {} / {}", m.normal_count, m.material_count));
    ui.label(format!("Gloss: {} / {}", m.gloss_count, m.material_count));
    ui.label(format!(
        "Occlusion: {} / {}",
        m.occlusion_count, m.material_count
    ));
    ui.label(format!(
        "Emissive: {} / {}",
        m.emissive_count, m.material_count
    ));
    ui.add_space(6.0);
    ui.separator();
    section_header(ui, "Transform");
    ui.add(egui::Slider::new(s.rot_offset_deg, 0.0..=360.0).text("Rotation offset"));
    ui.add(egui::Slider::new(s.model_scale, 0.1..=10.0).text("Scale"));
}

/// The View tab: camera + shading controls and the viewport toggles.
/// Camera viewpoint presets matching the original app's Viewpoints section
/// (elevation_deg, rotation_deg, label).
pub const VIEWPOINTS: [(f32, f32, &str); 8] = [
    (0.0, 0.0, "Front"),
    (0.0, 180.0, "Back"),
    (0.0, -90.0, "Left"),
    (0.0, 90.0, "Right"),
    (90.0, 0.0, "Top"),
    (-90.0, 0.0, "Bottom"),
    (35.264, 45.0, "Iso"),
    (35.264, -135.0, "Iso 2"),
];

/// Angle-only view of VIEWPOINTS (for the thumbnail renderer in app.rs).
pub const VIEWPOINT_ANGLES: [(f32, f32); 8] = [
    (VIEWPOINTS[0].0, VIEWPOINTS[0].1),
    (VIEWPOINTS[1].0, VIEWPOINTS[1].1),
    (VIEWPOINTS[2].0, VIEWPOINTS[2].1),
    (VIEWPOINTS[3].0, VIEWPOINTS[3].1),
    (VIEWPOINTS[4].0, VIEWPOINTS[4].1),
    (VIEWPOINTS[5].0, VIEWPOINTS[5].1),
    (VIEWPOINTS[6].0, VIEWPOINTS[6].1),
    (VIEWPOINTS[7].0, VIEWPOINTS[7].1),
];

fn view_tab(ui: &mut egui::Ui, s: &mut PanelState<'_>) {
    section_header(ui, "Viewpoints");
    let thumbs = s.view_thumbs;
    // 4×2 grid of preset camera angle buttons (original-style rendered
    // thumbnails when a model is loaded, text buttons otherwise).
    let cell = 66.0f32;
    egui::Grid::new("viewpoints")
        .num_columns(4)
        .spacing([4.0, 4.0])
        .show(ui, |ui| {
            for (row, chunk) in VIEWPOINTS.chunks(4).enumerate() {
                for (col, &(el, ro, label)) in chunk.iter().enumerate() {
                    let idx = row * 4 + col;
                    let clicked = match thumbs.and_then(|t| t.get(idx)) {
                        Some(tex) => ui
                            .add(egui::Button::image(egui::Image::new((
                                tex.id(),
                                egui::vec2(cell, cell),
                            ))))
                            .on_hover_text(label)
                            .clicked(),
                        None => ui
                            .add_sized(
                                [cell, 28.0],
                                egui::Button::new(egui::RichText::new(label).small()),
                            )
                            .clicked(),
                    };
                    if clicked {
                        *s.viewpoint_request = Some((el, ro));
                    }
                }
                ui.end_row();
            }
        });
    if ui.button("Reset View").clicked() {
        *s.reset_view = true;
    }
    ui.add_space(6.0);
    ui.separator();
    section_header(ui, "Camera & Shading");
    ui.add(egui::Slider::new(s.fov_y_deg, 20.0..=90.0).text("FOV"));
    ui.add(egui::Slider::new(s.exposure, -3.0..=3.0).text("Exposure"));
    ui.add(egui::Slider::new(s.contrast, 0.2..=3.0).text("Contrast"));
    ui.add_space(6.0);
    ui.separator();
    section_header(ui, "Grid");
    ui.checkbox(s.grid, "Grid");
    ui.checkbox(s.ground, "Ground plane");
    ui.checkbox(s.bloom, "Bloom");
}

/// The bottom playback bar — ALWAYS visible (like the original's transport
/// strip): turntable play/pause + reset on the left, the animation transport
/// (clip, scrub, speed) when the loaded model has clips, and the turntable
/// speed on the right otherwise.
pub struct BottomBarState<'a> {
    pub turntable: &'a mut bool,
    pub turntable_speed: &'a mut f32,
    pub turntable_reset: &'a mut bool,
    pub anim: &'a mut AnimationState,
    pub animations: &'a [viewer_scene::Animation],
}

pub fn bottom_bar(ui: &mut egui::Ui, s: BottomBarState<'_>) {
    egui::Panel::bottom("playback_bar")
        .resizable(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Left: turntable transport (always).
                let play_label = if *s.turntable { "⏸" } else { "▶" };
                if ui
                    .button(play_label)
                    .on_hover_text(if *s.turntable {
                        "Pause turntable"
                    } else {
                        "Spin the model (turntable)"
                    })
                    .clicked()
                {
                    *s.turntable = !*s.turntable;
                }
                if ui
                    .button("↺")
                    .on_hover_text("Reset turntable rotation")
                    .clicked()
                {
                    *s.turntable_reset = true;
                }
                // Center: animation scrubber (when clips exist) — the original
                // keeps the timeline here, with clip selection on the right.
                if !s.animations.is_empty() {
                    ui.separator();
                    let clip = s.anim.clip.min(s.animations.len() - 1);
                    let label = if s.anim.playing { "Pause" } else { "Play" };
                    if ui
                        .add_sized(
                            [56.0, 22.0],
                            egui::Button::new(egui::RichText::new(label).small()),
                        )
                        .clicked()
                    {
                        s.anim.playing = !s.anim.playing;
                    }
                    let dur = s.animations[clip].duration.max(0.0);
                    let range = if dur > 0.0 { 0.0..=dur } else { 0.0..=1.0 };
                    ui.add(egui::Slider::new(&mut s.anim.time, range).show_value(true));
                }
                // Right: Quick Animations + speed (original-style).
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if s.animations.is_empty() {
                        // No clips: turntable speed on the right.
                        ui.add(
                            egui::DragValue::new(s.turntable_speed)
                                .range(0.0..=90.0)
                                .suffix(" °/s"),
                        )
                        .on_hover_text("Turntable speed");
                    } else {
                        // Animation speed drag (× multiplier).
                        ui.add(
                            egui::DragValue::new(&mut s.anim.speed)
                                .range(0.1..=4.0)
                                .speed(0.01)
                                .prefix("×"),
                        );
                        // "Normal animation speed" reset button.
                        if ui
                            .small_button("Normal")
                            .on_hover_text("Reset animation speed to ×1.0")
                            .clicked()
                        {
                            s.anim.speed = 1.0;
                        }
                        // Quick Animations dropdown (clip selector).
                        let clip = s.anim.clip.min(s.animations.len() - 1);
                        let clip_label = |i: usize| {
                            let name = &s.animations[i].name;
                            if name.is_empty() {
                                format!("Clip {}", i + 1)
                            } else {
                                name.clone()
                            }
                        };
                        let prev = s.anim.clip;
                        egui::ComboBox::from_id_salt("quick_animations")
                            .selected_text(format!("🎬 {}", clip_label(clip)))
                            .show_ui(ui, |ui| {
                                for i in 0..s.animations.len() {
                                    ui.selectable_value(&mut s.anim.clip, i, clip_label(i));
                                }
                            });
                        if s.anim.clip != prev {
                            s.anim.time = 0.0;
                        }
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tessellates one overlay in a headless egui context and counts mesh
    /// vertices landing inside `region` - proof the widget actually paints
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
        let center = egui::Rect::from_center_size(egui::pos2(640.0, 400.0), egui::vec2(50.0, 50.0));
        let n = verts_near(loading_spinner, center);
        assert!(
            n > 20,
            "spinner must paint at the viewport center, got {n} vertices"
        );
    }

    #[test]
    fn status_line_and_note_tessellate_at_bottom_left() {
        let strip =
            egui::Rect::from_min_max(egui::pos2(0.0, 800.0 - 40.0), egui::pos2(640.0, 800.0));
        let mut scene = viewer_scene::Scene::new("teapot.glb".into());
        scene.tri_count = 12;
        scene.vert_count = 8;
        let n = verts_near(|ui, vp| status_line(ui, vp, &scene), strip);
        assert!(
            n > 10,
            "status line must paint at the bottom-left, got {n} vertices"
        );
        let n = verts_near(
            |ui, vp| status_note(ui, vp, "Screenshot save failed: disk full", false),
            strip,
        );
        assert!(
            n > 10,
            "status note must paint at the bottom-left, got {n} vertices"
        );
    }

    /// The status line counts warnings in its text ("(N warnings)" suffix);
    /// without warnings the raw name/counts text is shown.
    #[test]
    fn status_text_counts_warnings() {
        let mut scene = viewer_scene::Scene::new("m.glb".into());
        scene.warnings = vec![
            "baseColorTexture: bad".into(),
            "metallicRoughnessTexture: missing".into(),
        ];
        assert_eq!(status_text(&scene), "m.glb — 0 tris, 0 verts (2 warnings)");
        let clean = viewer_scene::Scene::new("m.glb".into());
        assert_eq!(status_text(&clean), "m.glb — 0 tris, 0 verts");
    }

    /// The modal error dialog paints an OK button (its only button-fill rect)
    /// and clicking that real geometry raises the ack flag. The dialog paints
    /// into its own egui area layer, whose shapes arrive nested in
    /// `Shape::Vec` - hence the recursive collector.
    #[test]
    fn error_dialog_paints_message_and_ok_click_acknowledges() {
        let ctx = egui::Context::default();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let run = |events: Vec<egui::Event>, ack: &mut bool| -> (Vec<egui::Rect>, egui::Visuals) {
            let input = egui::RawInput {
                screen_rect: Some(viewport),
                events,
                ..Default::default()
            };
            let mut visuals = None;
            let out = ctx.run_ui(input, |ui| {
                visuals = Some(ui.visuals().clone());
                error_dialog(ui.ctx(), "FBX files are not supported", ack);
            });
            let mut textures_delta = out.textures_delta;
            textures_delta.clear();
            let v = visuals.expect("run_ui closure executed");
            let mut rects = Vec::new();
            fn walk(shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Color32)>) {
                match shape {
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                    egui::Shape::Rect(r) => out.push((r.rect, r.fill)),
                    _ => {}
                }
            }
            for cs in &out.shapes {
                walk(&cs.shape, &mut rects);
            }
            let rects = rects
                .into_iter()
                .filter(|(_, c)| *c == v.widgets.inactive.bg_fill)
                .map(|(r, _)| r)
                .collect();
            (rects, v)
        };

        // egui 0.36 renders windows one frame late (frame 0's shapes are
        // Noop placeholders), so warm up with a throwaway frame before
        // inspecting geometry.
        let mut ack = false;
        run(vec![], &mut ack);

        let (rects, _) = run(vec![], &mut ack);
        assert_eq!(rects.len(), 1, "the OK button is the dialog's only button");
        assert!(!ack, "no click yet");
        let center = rects[0].center();

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
        run(press, &mut ack);
        run(release, &mut ack);
        assert!(ack, "clicking OK must raise the ack flag");
    }

    /// Helper: run the right panel headlessly with the given tab and wireframe
    /// state; returns the tessellated primitives' vertex count on the right
    /// half plus every rect fill on the right half.
    fn run_right_panel(
        tab: usize,
        wireframe: bool,
        with_scene: bool,
    ) -> (usize, Vec<(egui::Rect, egui::Color32)>) {
        let mut wireframe = wireframe;
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
        let mut ground = true;
        let mut grid = true;
        let mut bloom = true;
        let (mut offset, mut scale) = (0.0f32, 1.0f32);
        let (mut fov, mut exposure, mut contrast, mut reset) = (45.0f32, 0.5f32, 1.66f32, false);
        let mut rig_rot = 0.0f32;
        let mut light_override = [None; 3];
        let mut env_override = None;
        let mut viewpoint_req = None;
        let mut active_tab = tab;
        let out = ctx.run_ui(input, |ui| {
            right_panel(
                ui,
                PanelState {
                    theme_idx: &mut theme_idx,
                    light_mult: &mut light_mult,
                    env_mult: &mut env_mult,
                    shadows_enabled: &mut shadows_enabled,
                    wireframe: &mut wireframe,
                    ground: &mut ground,
                    grid: &mut grid,
                    bloom: &mut bloom,
                    source_name: if with_scene {
                        Some("spinning_teapot_with_lid.glb")
                    } else {
                        None
                    },
                    stats: ModelStats {
                        tri_count: 123_456,
                        vert_count: 65_432,
                        ..ModelStats::default()
                    },
                    rot_offset_deg: &mut offset,
                    model_scale: &mut scale,
                    fov_y_deg: &mut fov,
                    exposure: &mut exposure,
                    contrast: &mut contrast,
                    reset_view: &mut reset,
                    rig_rot_deg: &mut rig_rot,
                    light_color_override: &mut light_override,
                    env_color_override: &mut env_override,
                    viewpoint_request: &mut viewpoint_req,
                    view_thumbs: None,
                    active_tab: &mut active_tab,
                },
                ThemeThumbs::None,
            );
        });
        let mut textures_delta = out.textures_delta;
        textures_delta.clear();
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        let mut verts = 0usize;
        let mut fills = Vec::new();
        for p in &prims {
            if let egui::epaint::Primitive::Mesh(mesh) = &p.primitive {
                for v in &mesh.vertices {
                    if v.pos.x > 640.0 {
                        verts += 1;
                        fills.push((v.pos, v.color));
                    }
                }
            }
        }
        // Rect-level fills (shape pass) restricted to the right half.
        let rect_fills = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.rect.right() > 640.0 => Some((r.rect, r.fill)),
                _ => None,
            })
            .collect();
        (verts, rect_fills)
    }

    /// The panel docks on the right and paints a different tab body per
    /// active tab (Model tab with scene info > without).
    #[test]
    fn right_panel_paints_on_the_right_and_tabs_switch_bodies() {
        let (model_with_scene, _) = run_right_panel(1, false, true);
        let (model_no_scene, _) = run_right_panel(1, false, false);
        assert!(model_with_scene > 0, "panel must paint on the right half");
        assert!(
            model_with_scene > model_no_scene,
            "scene info (name + counts) must paint in the Model tab \
             ({} vs {})",
            model_with_scene,
            model_no_scene
        );
    }

    /// The wireframe checkbox lives in the MODEL tab now (the original keeps
    /// its triangle display under Mesh Data): on-state paints the check glyph.
    #[test]
    fn wireframe_checkbox_paints_state_in_model_tab() {
        let (off, _) = run_right_panel(1, false, false);
        let (on, _) = run_right_panel(1, true, false);
        assert!(
            on > off,
            "checked wireframe box must paint the check glyph ({on} vs {off})"
        );
    }

    /// The Lighting tab (fallback swatches) paints every theme's light-0 fill
    /// - the swatch IS the theme's visual identity.
    #[test]
    fn lighting_tab_paints_all_theme_swatch_fills() {
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
        let (mut offset, mut scale) = (0.0f32, 1.0f32);
        let (mut fov, mut exposure, mut contrast, mut reset) = (45.0f32, 0.5f32, 1.66f32, false);
        let mut rig_rot = 0.0f32;
        let mut light_override = [None; 3];
        let mut env_override = None;
        let mut viewpoint_req = None;
        let mut active_tab = 0usize;
        let out = ctx.run_ui(input, |ui| {
            right_panel(
                ui,
                PanelState {
                    theme_idx: &mut theme_idx,
                    light_mult: &mut light_mult,
                    env_mult: &mut env_mult,
                    shadows_enabled: &mut shadows_enabled,
                    wireframe: &mut wireframe,
                    ground: &mut ground,
                    grid: &mut grid,
                    bloom: &mut bloom,
                    source_name: None,
                    stats: ModelStats::default(),
                    rot_offset_deg: &mut offset,
                    model_scale: &mut scale,
                    fov_y_deg: &mut fov,
                    exposure: &mut exposure,
                    contrast: &mut contrast,
                    reset_view: &mut reset,
                    rig_rot_deg: &mut rig_rot,
                    light_color_override: &mut light_override,
                    env_color_override: &mut env_override,
                    viewpoint_request: &mut viewpoint_req,
                    view_thumbs: None,
                    active_tab: &mut active_tab,
                },
                ThemeThumbs::None,
            );
        });
        let mut textures_delta = out.textures_delta;
        textures_delta.clear();
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        let mut fills = std::collections::HashSet::new();
        for p in prims {
            if let egui::epaint::Primitive::Mesh(mesh) = p.primitive {
                for v in &mesh.vertices {
                    fills.insert(v.color);
                }
            }
        }
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

    /// The bottom bar is ALWAYS visible (original's transport strip): the
    /// turntable play button paints with or without animations, and its label
    /// follows the playing state.
    #[test]
    fn bottom_bar_always_paints_turntable_transport() {
        let animations = vec![viewer_scene::Animation {
            name: "spin".into(),
            duration: 1.0,
            channels: vec![],
        }];
        let paint = |with_anims: bool, turntable: bool| -> usize {
            let mut turntable = turntable;
            let ctx = egui::Context::default();
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
            let input = egui::RawInput {
                screen_rect: Some(viewport),
                ..Default::default()
            };
            let mut anim = AnimationState::default();
            let mut speed = 30.0f32;
            let mut reset = false;
            let empty: Vec<viewer_scene::Animation> = vec![];
            let anims = if with_anims { &animations } else { &empty };
            let out = ctx.run_ui(input, |ui| {
                bottom_bar(
                    ui,
                    BottomBarState {
                        turntable: &mut turntable,
                        turntable_speed: &mut speed,
                        turntable_reset: &mut reset,
                        anim: &mut anim,
                        animations: anims,
                    },
                );
            });
            let mut textures_delta = out.textures_delta;
            textures_delta.clear();
            let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
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
        let no_anims = paint(false, false);
        let with_anims = paint(true, false);
        assert!(
            no_anims > 10,
            "bottom bar must paint even without animations, got {no_anims}"
        );
        assert!(
            with_anims > no_anims,
            "animation transport adds widgets ({with_anims} vs {no_anims})"
        );
    }
}
