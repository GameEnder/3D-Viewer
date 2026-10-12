// crates/viewer-app/src/app.rs
use eframe::egui;
use egui_wgpu::{Callback, CallbackResources, CallbackTrait, RenderState};

use crate::paint;

pub struct ViewerApp {
    #[allow(dead_code)] // used from Task 7 onward
    render_state: Option<RenderState>,
}

impl ViewerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // cc.wgpu_render_state is a FIELD in eframe 0.36 (Option<RenderState>;
        // RenderState itself is cloned, no outer Arc in 0.36).
        if let Some(state) = cc.wgpu_render_state.as_ref() {
            paint::install(state);
        }
        Self {
            render_state: cc.wgpu_render_state.clone(),
        }
    }
}

impl eframe::App for ViewerApp {
    // eframe 0.36 replaced `App::update(ctx, frame)` with `App::ui(ui, frame)`;
    // CentralPanel::show now also takes `&mut Ui` instead of `&Context`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default())
            .show(ui, |ui| {
                let rect = ui.available_rect_before_wrap();
                ui.painter()
                    .add(Callback::new_paint_callback(rect, SolidPainter));
                ui.label("3D Viewer (Rust clone)");
            });
    }
}

struct SolidPainter;

impl CallbackTrait for SolidPainter {
    fn paint(
        &self,
        info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut egui_wgpu::wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let pipeline = resources
            .get::<paint::SolidPipeline>()
            .expect("SolidPipeline installed at startup");
        let vp = info.viewport_in_pixels();
        render_pass.set_pipeline(&pipeline.0);
        render_pass.set_viewport(
            vp.left_px as f32,
            vp.top_px as f32,
            vp.width_px as f32,
            vp.height_px as f32,
            0.0,
            1.0,
        );
        render_pass.draw(0..3, 0..1);
    }
}
