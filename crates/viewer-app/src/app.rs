// crates/viewer-app/src/app.rs
use std::sync::{Arc, Mutex};

use eframe::egui;
use glam::Vec3;
use viewer_render::{DEFAULT_CLEAR_COLOR, FrameParams, LightInput, RenderSettings, Renderer};

use crate::camera::OrbitCamera;
use crate::paint;
use crate::panels;

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
        }
    }

    /// Default light rig per the design spec: 3 white lights, equal 120deg
    /// spacing, elevation ~40deg, radius ~2.2 model units, rig rotation 0.
    fn default_lights() -> [LightInput; 3] {
        let (elev, radius) = (40.0f32.to_radians(), 2.2f32);
        let rig: Vec<LightInput> = (0..3)
            .map(|i| {
                let az = (i as f32) * (120.0f32).to_radians();
                LightInput {
                    pos: [
                        az.cos() * elev.cos() * radius,
                        elev.sin() * radius,
                        az.sin() * elev.cos() * radius,
                    ],
                    color: [1.0, 1.0, 1.0],
                    intensity: 1.0,
                    shadows: false,
                }
            })
            .collect();
        rig.try_into().expect("3 lights")
    }

    fn frame_params(&self, aspect: f32) -> FrameParams {
        FrameParams {
            view_proj: self.camera.view_proj(aspect),
            camera_pos: self.camera.eye().to_array(),
            turntable_deg: 0.0,
            lights: Self::default_lights(),
            env_color: [0.4, 0.4, 0.4],
            env_intensity: 1.0,
            settings: RenderSettings {
                clear_color: DEFAULT_CLEAR_COLOR,
                wireframe: false,
                ground: true,
                grid: false,
                bloom: false,
                exposure: 0.5,
                contrast: 1.66,
                ground_opacity: 0.4,
            },
            anim: None,
        }
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
                if let Some(path) = panels::open_button(ui, rect) {
                    self.open_path(ui.ctx(), path);
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
        if reset {
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
