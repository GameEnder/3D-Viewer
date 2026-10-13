// crates/viewer-app/src/paint.rs
use std::sync::{Arc, Mutex};

use eframe::egui;
use egui_wgpu::{CallbackResources, CallbackTrait, RenderState};
use viewer_render::Renderer;

/// Shared between app update() and the wgpu paint callbacks.
#[derive(Clone)]
pub struct SharedRenderer(pub Arc<Mutex<Renderer>>);

/// Insert the shared renderer into egui_wgpu's callback resources once at
/// startup (after `Renderer::new`).
pub fn install(state: &RenderState, renderer: Arc<Mutex<Renderer>>) {
    state
        .renderer
        .write()
        .callback_resources
        .insert(SharedRenderer(renderer));
}

pub struct ScenePaint;

impl CallbackTrait for ScenePaint {
    fn prepare(
        &self,
        device: &egui_wgpu::wgpu::Device,
        queue: &egui_wgpu::wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut egui_wgpu::wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<egui_wgpu::wgpu::CommandBuffer> {
        let shared = resources
            .get::<SharedRenderer>()
            .expect("SharedRenderer installed");
        let mut r = shared.0.lock().expect("renderer mutex");
        r.prepare_render(device, queue, egui_encoder);
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut egui_wgpu::wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let shared = resources
            .get::<SharedRenderer>()
            .expect("SharedRenderer installed");
        let r = shared.0.lock().expect("renderer mutex");
        let vp = info.viewport_in_pixels();
        r.blit(
            render_pass,
            [
                vp.left_px as f32,
                vp.top_px as f32,
                vp.width_px as f32,
                vp.height_px as f32,
            ],
        );
    }
}
