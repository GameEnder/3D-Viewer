// crates/viewer-app/src/app.rs
use std::sync::{Arc, Mutex};

use eframe::egui;
use glam::Vec3;
use viewer_render::{DEFAULT_CLEAR_COLOR, FrameParams, LightInput, RenderSettings, Renderer};

use crate::camera::OrbitCamera;
use crate::paint;

pub struct ViewerApp {
    render_state: Option<egui_wgpu::RenderState>,
    renderer: Option<Arc<Mutex<Renderer>>>,
    camera: OrbitCamera,
    scene: Option<Arc<viewer_scene::Scene>>,
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
            scene: None,
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
                ground: false,
                grid: false,
                bloom: false,
                exposure: 0.5,
                contrast: 1.66,
                ground_opacity: 0.4,
            },
            anim: None,
        }
    }
}

impl eframe::App for ViewerApp {
    // eframe 0.36 replaced `App::update(ctx, frame)` with `App::ui(ui, frame)`;
    // CentralPanel::show now also takes `&mut Ui` instead of `&Context`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let Some(state) = self.render_state.as_ref() else {
            return;
        };

        // Renderer creation on first frame (needs the device + target format).
        if self.renderer.is_none() {
            let renderer = Arc::new(Mutex::new(Renderer::new(
                &state.device,
                state.target_format,
            )));
            if let Some(scene) = self.scene.take() {
                renderer.lock().expect("renderer mutex").set_scene(
                    &state.device,
                    &state.queue,
                    scene,
                );
            }
            paint::install(state, renderer.clone());
            self.renderer = Some(renderer);
        }
        let renderer = self.renderer.as_ref().expect("renderer created above");
        let mut r = renderer.lock().expect("renderer mutex");

        // Side effects collected inside the panel closure, applied after it
        // (the closure borrows disjoint fields; mutating camera/uniform state
        // through `self` there would conflict with the whole-self method call
        // in frame_params).
        let mut new_size: Option<(u32, u32)> = None;
        let mut orbit = (0.0f32, 0.0f32);
        let mut dolly_mult = 1.0f32;
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

                // Minimal camera input for the Task 7 smoke: drag orbits,
                // wheel dollies (pan stays unwired until a later task).
                let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
                if response.dragged() {
                    let d = response.drag_delta();
                    orbit = (d.x, -d.y);
                }
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    dolly_mult = (scroll * 0.0015).exp();
                }

                let cb = egui_wgpu::Callback::new_paint_callback(rect, paint::ScenePaint);
                ui.painter().add(cb);
            });

        if let Some((w, h)) = new_size {
            r.resize(&state.device, w, h);
        }
        if orbit.0 != 0.0 || orbit.1 != 0.0 {
            self.camera.orbit(orbit.0, orbit.1);
        }
        if dolly_mult != 1.0 {
            self.camera.dolly(dolly_mult);
        }

        // Queue the frame for the paint callback's prepare phase (runs after
        // ui() returns, once the mutex guard is released).
        r.queue_frame(self.frame_params(aspect));
        drop(r);

        // Continuous rendering while a scene is loaded (egui 0.36 is reactive;
        // a single repaint per frame is enough for Task 7).
        if self.scene.is_some() {
            ui.ctx().request_repaint();
        }
    }
}
