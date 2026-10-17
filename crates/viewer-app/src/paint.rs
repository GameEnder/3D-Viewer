// crates/viewer-app/src/paint.rs
//! The bridge between egui's paint callback system and our wgpu renderer.
//!
//! egui-wgpu renders the UI itself, but our 3D scene lives in its own
//! offscreen `final` texture. An `egui::PaintCallback` (`ScenePaint`) gives
//! us the two hooks we need at exactly the right moments in the frame:
//! `prepare` (where wgpu requires all offscreen rendering to be encoded —
//! we run the full PBR/post chain here) and `paint` (inside egui's own
//! render pass, where we blit the finished texture under whatever egui
//! drew on top).

use std::sync::{Arc, Mutex};

use eframe::egui;
use egui_wgpu::{CallbackResources, CallbackTrait, RenderState};
use viewer_render::Renderer;

/// The renderer shared between the app thread (which queues state) and
/// egui's paint callback (which encodes/blits). Arc<Mutex> because the
/// callback runs on the same thread but through egui's borrow boundaries.
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

/// The paint callback the app paints into the 3D viewport rect each frame.
pub struct ScenePaint;

impl CallbackTrait for ScenePaint {
    /// Encode the whole offscreen frame (shadow passes, scene pass, bloom,
    /// post) into egui's encoder. Runs BEFORE egui's own pass.
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
