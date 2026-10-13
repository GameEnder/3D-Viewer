// crates/viewer-app/src/app.rs
use std::sync::{Arc, Mutex};

use eframe::egui;
use glam::Vec3;
use viewer_render::{DEFAULT_CLEAR_COLOR, FrameParams, LightInput, RenderSettings, Renderer};

use crate::camera::OrbitCamera;
use crate::paint;
use crate::panels;
use crate::themes::{THEMES, frame_lights};

/// File-loading state machine (Task 8 brief Step 3).
pub enum AppState {
    Idle,
    Loading,
    Ready(Arc<viewer_scene::Scene>),
    Failed(String),
}

type LoadResult = Result<viewer_scene::Scene, viewer_scene::LoadError>;

pub struct ViewerApp {
    render_state: Option<egui_wgpu::RenderState>,
    renderer: Option<Arc<Mutex<Renderer>>>,
    camera: OrbitCamera,
    state: AppState,
    /// Result channel of the in-flight worker thread (Some while Loading).
    load_rx: Option<std::sync::mpsc::Receiver<LoadResult>>,
    last_size: [f32; 2],
    /// Active theme index into `themes::THEMES` (0 = Default).
    theme_idx: usize,
    /// User per-light intensity multipliers (panel drags, 0..3).
    light_mult: [f32; 3],
    /// User environment intensity multiplier (panel drag, 0..3).
    env_mult: f32,
    /// App-level shadow override: when off, every light's `shadows` is
    /// forced false regardless of theme intensity.
    shadows_enabled: bool,
    /// Effects toggles (Task 12) — written by the Effects panel, read every
    /// frame into `RenderSettings`. Defaults match the original app: ground
    /// disc + grid visible, bloom on (its `ImageProcessing.BloomEnabled`).
    wireframe: bool,
    ground: bool,
    grid: bool,
    bloom: bool,
    /// Turntable rotation toggle (toolbar): advances `turntable_deg` each
    /// frame while on (Task 13).
    turntable: bool,
    /// Accumulated turntable rotation in degrees (mod 360), advanced by
    /// `turntable_speed` while the toggle is on (Task 13).
    turntable_deg: f32,
    /// Turntable rate in deg/s (Model panel slider 0..90). 30 deg/s ≈ 12 s
    /// per revolution (the original's exact rate was never extracted).
    turntable_speed: f32,
    /// Static rotation offset in degrees (Model panel slider 0..360); added
    /// to `turntable_deg` before reaching the renderer.
    rot_offset_deg: f32,
    /// Uniform model scale (Model panel slider 0.1..10); the renderer folds
    /// it into the per-draw model matrices about the bounds center.
    model_scale: f32,
    /// Post-chain exposure (Camera panel slider −3..3 → RenderSettings).
    exposure: f32,
    /// Post-chain contrast (Camera panel slider 0.2..3 → RenderSettings).
    contrast: f32,
    /// Whether the right settings panel is shown (toolbar toggle).
    settings_open: bool,
    /// Animation playback state (Task 17): the bar writes it, `step_animation`
    /// advances it, `frame_params` feeds `(clip, time)` to the renderer.
    anim: panels::AnimationState,
}

impl ViewerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // cc.wgpu_render_state is a FIELD in eframe 0.36 (Option<RenderState>;
        // RenderState itself is cloned, no outer Arc in 0.36).
        let render_state = cc.wgpu_render_state.clone();
        debug_assert!(
            render_state.is_some(),
            "eframe must be built with the wgpu renderer for the 3D viewport"
        );

        Self {
            render_state,
            renderer: None,
            camera: OrbitCamera::default_for(Vec3::ZERO, 1.0),
            state: AppState::Idle,
            load_rx: None,
            last_size: [0.0, 0.0],
            theme_idx: 0,
            light_mult: [1.0; 3],
            env_mult: 1.0,
            shadows_enabled: true,
            wireframe: false,
            ground: true,
            grid: true,
            bloom: true,
            turntable: false,
            turntable_deg: 0.0,
            turntable_speed: 30.0,
            rot_offset_deg: 0.0,
            model_scale: 1.0,
            exposure: 0.5,
            contrast: 1.66,
            settings_open: true,
            anim: panels::AnimationState::default(),
        }
    }

    /// Builds the per-frame lighting from the active theme (Task 11): the
    /// theme's 3-light rig (colors sRGB→linear, intensities percent-scaled),
    /// rotated per `rig_rot_deg`; env ambient from the theme's env light.
    /// The panel's Shadows toggle is an app-level override forcing
    /// `shadows = false` on every light when off.
    fn themed_lights(&self) -> ([LightInput; 3], [f32; 3], f32) {
        let (mut lights, (env_color, env_intensity)) =
            frame_lights(&THEMES[self.theme_idx], self.light_mult, self.env_mult);
        if !self.shadows_enabled {
            for light in &mut lights {
                light.shadows = false;
            }
        }
        (lights, env_color, env_intensity)
    }

    fn frame_params(&self, aspect: f32) -> FrameParams {
        let (lights, env_color, env_intensity) = self.themed_lights();
        // Task 17: feed the selected clip + playback time to the renderer
        // whenever a scene with animations is Ready and the clip is in range
        // (a stale/out-of-range clip renders the static pose instead).
        let anim = match &self.state {
            AppState::Ready(scene) if self.anim.clip < scene.animations.len() => {
                Some((self.anim.clip, self.anim.time))
            }
            _ => None,
        };
        FrameParams {
            view_proj: self.camera.view_proj(aspect),
            camera_pos: self.camera.eye().to_array(),
            turntable_deg: (self.turntable_deg + self.rot_offset_deg) % 360.0,
            model_scale: self.model_scale,
            lights,
            env_color,
            env_intensity,
            settings: RenderSettings {
                clear_color: DEFAULT_CLEAR_COLOR,
                wireframe: self.wireframe,
                ground: self.ground,
                grid: self.grid,
                bloom: self.bloom,
                shadows: true,
                exposure: self.exposure,
                contrast: self.contrast,
                ground_opacity: 0.4,
            },
            anim,
        }
    }

    /// Advances the turntable rotation while the toolbar toggle is on
    /// (Task 13): `turntable_speed` deg/s accumulated mod 360. `dt` is
    /// clamped to 0.1 s so a stalled frame (first frame, window drag,
    /// debugger halt) cannot jump the rotation.
    fn step_turntable(&mut self, dt_seconds: f32) {
        if !self.turntable {
            return;
        }
        let dt = dt_seconds.min(0.1);
        self.turntable_deg = (self.turntable_deg + self.turntable_speed * dt) % 360.0;
    }

    /// Per-frame turntable update: reads dt from the egui input state
    /// (`unstable_dt`, present in egui 0.36) and keeps repainting while the
    /// toggle is on.
    fn advance_turntable(&mut self, ctx: &egui::Context) {
        if !self.turntable {
            return;
        }
        let dt = ctx.input(|i| i.unstable_dt);
        self.step_turntable(dt);
        ctx.request_repaint();
    }

    /// Duration of the selected animation clip (Task 17). `None` without a
    /// Ready scene or with an out-of-range clip.
    fn animation_duration(&self) -> Option<f32> {
        match &self.state {
            AppState::Ready(scene) => scene.animations.get(self.anim.clip).map(|a| a.duration),
            _ => None,
        }
    }

    /// Advances animation playback (Task 17): `time += speed * dt` while
    /// playing, wrapping at the clip duration so the scrub slider stays in
    /// range (the renderer wraps again via `evaluate`'s `rem_euclid`). dt is
    /// clamped to 0.1 s like the turntable so a stalled frame cannot jump
    /// the pose. Paused or clipless: no-op.
    fn step_animation(&mut self, dt_seconds: f32) {
        if !self.anim.playing {
            return;
        }
        let Some(duration) = self.animation_duration() else {
            return;
        };
        if duration <= 0.0 {
            return;
        }
        let dt = dt_seconds.min(0.1);
        self.anim.time = (self.anim.time + self.anim.speed * dt).rem_euclid(duration);
    }

    /// Per-frame animation update (Task 17): reads dt from the egui input
    /// state (`unstable_dt`, present in egui 0.36) and keeps repainting while
    /// playing (the app repaints continuously while Ready anyway; the
    /// explicit request keeps playback alive if that ever changes).
    fn advance_animation(&mut self, ctx: &egui::Context) {
        if !self.anim.playing {
            return;
        }
        let dt = ctx.input(|i| i.unstable_dt);
        self.step_animation(dt);
        ctx.request_repaint();
    }

    /// Spawns the worker thread that loads `path` and switches to Loading.
    /// The scene is normalized (unit bounding sphere, base at y=0), so the
    /// ground disc (radius 2.6 at y=0) always fits around any model.
    fn open_path(&mut self, ctx: &egui::Context, path: std::path::PathBuf) {
        self.state = AppState::Loading;
        let (tx, rx) = std::sync::mpsc::channel::<LoadResult>();
        std::thread::spawn(move || {
            let _ = tx.send(viewer_scene::load(&path));
        });
        self.load_rx = Some(rx);
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    /// Collects the worker result, if any (single lock — the SharedRenderer
    /// Arc IS self.renderer, so this must run before ui() takes the guard).
    fn poll_loading(&mut self, _ctx: &egui::Context) {
        let result = self.load_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if result.is_some() {
            self.load_rx = None;
        }
        let Some(result) = result else { return };
        match result {
            Ok(scene) => {
                self.camera
                    .fit(scene.bounds.center(), scene.bounds.bounding_sphere_radius());
                // Task 17: every load restarts animation playback from clip
                // 0, paused (a stale clip index would point nowhere).
                self.anim = panels::AnimationState::default();
                let arc = Arc::new(scene);
                if let Some(rs) = self.render_state.as_ref() {
                    // parking_lot guard: read() cannot fail, so no expect.
                    let shared = rs
                        .renderer
                        .read()
                        .callback_resources
                        .get::<paint::SharedRenderer>()
                        .cloned();
                    if let Some(shared) = shared {
                        shared.0.lock().expect("renderer mutex").set_scene(
                            &rs.device,
                            &rs.queue,
                            Arc::clone(&arc),
                        );
                    }
                }
                self.state = AppState::Ready(arc);
            }
            Err(e) => self.state = AppState::Failed(e.to_string()),
        }
    }
}

impl eframe::App for ViewerApp {
    // eframe 0.36 replaced `App::update(ctx, frame)` with `App::ui(ui, frame)`;
    // CentralPanel::show now also takes `&mut Ui` instead of `&Context`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Cloned (RenderState is Copy-like to clone in 0.36) rather than
        // borrowed: the panel closure below calls self.open_path(), which
        // needs unique access to all of self for the closure's lifetime.
        let Some(state) = self.render_state.clone() else {
            return;
        };

        // Renderer creation on first frame (needs the device + target format).
        if self.renderer.is_none() {
            let renderer = Arc::new(Mutex::new(Renderer::new(
                &state.device,
                state.target_format,
            )));
            if let AppState::Ready(scene) = &self.state {
                renderer.lock().expect("renderer mutex").set_scene(
                    &state.device,
                    &state.queue,
                    Arc::clone(scene),
                );
            }
            paint::install(&state, renderer.clone());
            self.renderer = Some(renderer);
        }

        // Loading -> Ready/Failed transition; locks the shared renderer Arc,
        // so it must run before the frame's own lock below.
        self.poll_loading(ui.ctx());

        // Owned Arc clone: the mutex guard below must not borrow self while
        // the panel closure holds unique access.
        let renderer = self
            .renderer
            .as_ref()
            .expect("renderer created above")
            .clone();

        // Task 13 screenshot completion: the previous frame's prepare phase
        // turned a pending request into a staging copy (the paint callback
        // runs after ui() returns), so collect it here and, when present,
        // save it as PNG through the native save dialog. The device.poll
        // wait + dialog only block the UI thread right after the user asked
        // for a screenshot.
        if let Some(img) = renderer
            .lock()
            .expect("renderer mutex")
            .collect_screenshot(&state.device)
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name("screenshot.png")
                .add_filter("PNG", &["png"])
                .save_file()
        {
            let _ = img.save(&path);
        }

        // Turntable spin (Task 13): advance + keep repainting while on.
        self.advance_turntable(ui.ctx());

        // Animation playback (Task 17): advance + keep repainting while on.
        self.advance_animation(ui.ctx());

        // Left toolbar (Task 12): Open / Turntable / Screenshot / Settings.
        // The Settings toggle gates the right settings panel; both panels run
        // before the CentralPanel so the viewport shrinks this same frame.
        let toolbar = panels::toolbar(ui, &mut self.turntable, &mut self.settings_open);
        let mut reset_view = false;
        if self.settings_open {
            let (source_name, tri_count, vert_count) = match &self.state {
                AppState::Ready(scene) => (
                    Some(scene.source_name.as_str()),
                    scene.tri_count,
                    scene.vert_count,
                ),
                _ => (None, 0, 0),
            };
            panels::settings_panel(
                ui,
                &mut self.theme_idx,
                &mut self.light_mult,
                &mut self.env_mult,
                &mut self.shadows_enabled,
                &mut self.wireframe,
                &mut self.ground,
                &mut self.grid,
                &mut self.bloom,
                panels::ModelSection {
                    source_name,
                    tri_count,
                    vert_count,
                    turntable_speed: &mut self.turntable_speed,
                    rot_offset_deg: &mut self.rot_offset_deg,
                    model_scale: &mut self.model_scale,
                },
                panels::CameraSection {
                    fov_y_deg: &mut self.camera.fov_y_deg,
                    exposure: &mut self.exposure,
                    contrast: &mut self.contrast,
                    reset_view: &mut reset_view,
                },
            );
        }

        // Bottom animation bar (Task 17): only for a scene WITH animations.
        // Shown before the CentralPanel so the viewport shrinks this frame.
        let animations: &[viewer_scene::Animation] = match &self.state {
            AppState::Ready(scene) => &scene.animations,
            _ => &[],
        };
        if !animations.is_empty() {
            panels::animation_bar(ui, &mut self.anim, animations);
        }

        // Toolbar side effects: the Open request reuses open_path (worker
        // thread + Loading state); Screenshot flips the renderer's pending
        // flag with a brief lock (the frame's staging copy is created by the
        // paint callback's prepare phase and collected at the top of the
        // next ui() call). Without a renderer neither can run.
        if let Some(path) = toolbar.open {
            self.open_path(ui.ctx(), path);
        }
        if toolbar.screenshot {
            renderer
                .lock()
                .expect("renderer mutex")
                .request_screenshot();
            // The staging copy is made by THIS frame's paint callback; the
            // next frame must run promptly to collect it and open the save
            // dialog (an idle egui sleeps until the next input event).
            ui.ctx().request_repaint();
        }

        let mut r = renderer.lock().expect("renderer mutex");

        // Side effects collected inside the panel closure, applied after it
        // (the closure borrows disjoint fields; mutating camera/uniform state
        // through `self` there would conflict with the whole-self method call
        // in frame_params).
        let mut new_size: Option<(u32, u32)> = None;
        let mut orbit = (0.0f32, 0.0f32);
        let mut pan = (0.0f32, 0.0f32);
        let mut dolly_mult = 1.0f32;
        let mut reset = false;
        let mut aspect = 1.0f32;

        egui::CentralPanel::default()
            .frame(egui::Frame::default())
            .show(ui, |ui| {
                let rect = ui.available_rect_before_wrap();
                let ppp = ui.ctx().pixels_per_point();
                aspect = rect.width() / rect.height().max(1.0);

                // Resize the offscreen targets when the panel size changes.
                let px = [(rect.width() * ppp).round(), (rect.height() * ppp).round()];
                if px != self.last_size {
                    self.last_size = px;
                    new_size = Some((px[0] as u32, px[1] as u32));
                }

                // Drag-and-drop open (the drop event fires on one frame only).
                // egui 0.36: dropped files are trait objects exposing path().
                let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
                if let Some(file) = dropped.first() {
                    self.open_path(ui.ctx(), file.path().to_path_buf());
                }

                // LMB orbit, RMB pan, double-click reset, wheel dolly.
                let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
                if response.dragged_by(egui::PointerButton::Primary) {
                    let d = response.drag_delta();
                    orbit = (d.x, -d.y);
                }
                if response.dragged_by(egui::PointerButton::Secondary) {
                    let d = response.drag_delta();
                    pan = (d.x, d.y);
                }
                reset = response.double_clicked();
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    dolly_mult = (scroll * 0.0015).exp();
                }

                let cb = egui_wgpu::Callback::new_paint_callback(rect, paint::ScenePaint);
                ui.painter().add(cb);

                // Overlays on top of the 3D view.
                if matches!(self.state, AppState::Loading) {
                    panels::loading_spinner(ui, rect);
                }
                if let AppState::Failed(err) = &self.state {
                    panels::error_banner(ui, rect, err);
                }
            });

        if let Some((w, h)) = new_size {
            r.resize(&state.device, w, h);
        }
        if orbit.0 != 0.0 || orbit.1 != 0.0 {
            self.camera.orbit(orbit.0, orbit.1);
        }
        if pan.0 != 0.0 || pan.1 != 0.0 {
            self.camera.pan(pan.0, pan.1, aspect);
        }
        if dolly_mult != 1.0 {
            self.camera.dolly(dolly_mult);
        }
        // Double-click on the viewport AND the Camera panel's "Reset view"
        // button share the same fit-reset (Task 13).
        if reset || reset_view {
            match &self.state {
                AppState::Ready(scene) => self
                    .camera
                    .fit(scene.bounds.center(), scene.bounds.bounding_sphere_radius()),
                _ => self.camera.fit(Vec3::ZERO, 1.0),
            }
        }

        // Queue the frame for the paint callback's prepare phase (runs after
        // ui() returns, once the mutex guard is released).
        r.queue_frame(self.frame_params(aspect));
        drop(r);

        // egui 0.36 is reactive: Loading polls the worker + animates the
        // spinner on a timer; a loaded scene repaints continuously (Task 7
        // behavior).
        match self.state {
            AppState::Loading => {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(33));
            }
            AppState::Ready(_) => ui.ctx().request_repaint(),
            AppState::Idle | AppState::Failed(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ViewerApp without GPU state: enough for the pure state →
    /// FrameParams flow (no wgpu device in unit tests).
    fn test_app() -> ViewerApp {
        ViewerApp {
            render_state: None,
            renderer: None,
            camera: OrbitCamera::default_for(Vec3::ZERO, 1.0),
            state: AppState::Idle,
            load_rx: None,
            last_size: [0.0, 0.0],
            theme_idx: 0,
            light_mult: [1.0; 3],
            env_mult: 1.0,
            shadows_enabled: true,
            wireframe: false,
            ground: true,
            grid: true,
            bloom: true,
            turntable: false,
            turntable_deg: 0.0,
            turntable_speed: 30.0,
            rot_offset_deg: 0.0,
            model_scale: 1.0,
            exposure: 0.5,
            contrast: 1.66,
            settings_open: true,
            anim: panels::AnimationState::default(),
        }
    }

    /// A scene with one 2 s animation, as Ready app state (for the
    /// animation-flow tests).
    fn test_app_with_animation() -> ViewerApp {
        let mut app = test_app();
        let mut scene = viewer_scene::Scene::new("t".into());
        scene.animations.push(viewer_scene::Animation {
            name: "spin".into(),
            duration: 2.0,
            channels: vec![],
        });
        app.state = AppState::Ready(Arc::new(scene));
        app
    }

    /// Task 17 defaults: clip 0, paused, t = 0, speed 1.0.
    #[test]
    fn animation_state_defaults_match_the_brief() {
        let app = test_app();
        assert_eq!(app.anim.clip, 0);
        assert!(!app.anim.playing);
        assert_eq!(app.anim.time, 0.0);
        assert_eq!(app.anim.speed, 1.0);
    }

    /// Playback: `time += speed * dt` while playing (wrapping at the clip
    /// duration), dt clamped to 0.1 s like the turntable, frozen while
    /// paused, inert without a Ready scene.
    #[test]
    fn step_animation_advances_wraps_clamps_and_freezes() {
        let mut app = test_app_with_animation();
        app.anim.playing = true;
        app.anim.time = 1.95;
        app.step_animation(0.1);
        assert!((app.anim.time - 0.05).abs() < 1e-5, "wraps at duration 2.0");
        app.anim.speed = 2.0;
        app.anim.time = 1.0;
        app.step_animation(0.1);
        assert!((app.anim.time - 1.2).abs() < 1e-5, "speed multiplies dt");
        app.step_animation(5.0); // clamped: still only +0.2 s
        assert!((app.anim.time - 1.4).abs() < 1e-5);
        app.anim.playing = false;
        app.step_animation(0.1);
        assert!((app.anim.time - 1.4).abs() < 1e-5, "paused = frozen");
        let mut idle = test_app();
        idle.anim.playing = true;
        idle.step_animation(0.1);
        assert_eq!(idle.anim.time, 0.0, "no scene = inert");
    }

    /// The animation state reaches FrameParams: `Some((clip, time))` when a
    /// scene with a valid clip is Ready, `None` otherwise (no scene, no
    /// animations, or an out-of-range clip — the renderer then keeps the
    /// static pose).
    #[test]
    fn animation_flows_into_frame_params() {
        let mut app = test_app_with_animation();
        app.anim.clip = 0;
        app.anim.time = 0.75;
        let p = app.frame_params(1.6);
        assert_eq!(p.anim, Some((0, 0.75)));

        app.anim.clip = 1; // out of range
        assert_eq!(app.frame_params(1.6).anim, None);

        assert_eq!(test_app().frame_params(1.6).anim, None, "no scene");

        let mut empty = test_app();
        empty.state = AppState::Ready(Arc::new(viewer_scene::Scene::new("e".into())));
        assert_eq!(
            empty.frame_params(1.6).anim,
            None,
            "scene without animations"
        );
    }

    /// Defaults match the original app: ground disc + grid visible, bloom on
    /// (the original's ImageProcessing.BloomEnabled), wireframe off, settings
    /// panel open, turntable off.
    #[test]
    fn effect_defaults_match_the_original_app() {
        let app = test_app();
        assert!(!app.wireframe);
        assert!(app.ground);
        assert!(app.grid);
        assert!(app.bloom);
        assert!(!app.turntable);
        assert!(app.settings_open);

        let s = app.frame_params(1.0).settings;
        assert!(!s.wireframe);
        assert!(s.ground);
        assert!(s.grid);
        assert!(s.bloom);
        assert_eq!(s.exposure, 0.5);
        assert_eq!(s.contrast, 1.66);
        assert_eq!(s.ground_opacity, 0.4);
    }

    /// Every Effects toggle must reach the per-frame RenderSettings (the
    /// panel writes the fields; frame_params reads them the same frame).
    #[test]
    fn effect_toggles_flow_into_frame_settings() {
        let mut app = test_app();
        app.wireframe = true;
        app.ground = false;
        app.grid = false;
        app.bloom = false;
        let s = app.frame_params(1.6).settings;
        assert!(s.wireframe);
        assert!(!s.ground);
        assert!(!s.grid);
        assert!(!s.bloom);
    }

    /// Task 13 panel defaults: 30 deg/s turntable (12 s/rev), no rotation
    /// offset, scale 1, FOV 45, exposure 0.5, contrast 1.66.
    #[test]
    fn turntable_and_camera_defaults_match_the_brief() {
        let app = test_app();
        assert_eq!(app.turntable_speed, 30.0);
        assert_eq!(app.rot_offset_deg, 0.0);
        assert_eq!(app.model_scale, 1.0);
        assert_eq!(app.camera.fov_y_deg, 45.0);
        assert_eq!(app.exposure, 0.5);
        assert_eq!(app.contrast, 1.66);
    }

    /// Turntable advance: 30 deg/s, dt clamped to 0.1 s (a stalled frame
    /// must not jump the rotation), wraps mod 360, frozen while off.
    #[test]
    fn turntable_advances_clamps_and_wraps() {
        let mut app = test_app();
        app.turntable = true;
        app.step_turntable(0.1);
        assert!((app.turntable_deg - 3.0).abs() < 1e-5);
        app.step_turntable(1.0); // clamped: still only +3 deg
        assert!((app.turntable_deg - 6.0).abs() < 1e-5);
        app.turntable_deg = 359.0;
        app.step_turntable(0.1);
        assert!((app.turntable_deg - 2.0).abs() < 1e-5, "wraps mod 360");
        app.turntable = false;
        app.step_turntable(0.1);
        assert!((app.turntable_deg - 2.0).abs() < 1e-5, "off = frozen");
    }

    /// The Model/Camera panel state must reach FrameParams: the rotation
    /// offset adds to the accumulated turntable angle; scale and the post
    /// tuning flow into settings; FOV lives on the camera itself.
    #[test]
    fn model_and_camera_state_flows_into_frame_params() {
        let mut app = test_app();
        app.turntable_deg = 100.0;
        app.rot_offset_deg = 50.0;
        app.model_scale = 2.5;
        app.exposure = 1.0;
        app.contrast = 2.0;
        app.camera.fov_y_deg = 70.0;
        let p = app.frame_params(1.6);
        assert!((p.turntable_deg - 150.0).abs() < 1e-4);
        assert!((p.model_scale - 2.5).abs() < 1e-6);
        assert!((p.settings.exposure - 1.0).abs() < 1e-6);
        assert!((p.settings.contrast - 2.0).abs() < 1e-6);
        assert!((app.camera.fov_y_deg - 70.0).abs() < 1e-6);
    }
}
