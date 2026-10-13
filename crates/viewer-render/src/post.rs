// crates/viewer-render/src/post.rs
//! Bloom chain (Task 10): threshold + separable 9-tap gaussian blur at half
//! resolution, feeding post.wgsl's `bloom_tex` slot. Recorded after the MSAA
//! resolve (the pbr pass) and before the post pass, only when
//! `RenderSettings.bloom` is set:
//!
//! 1. threshold — samples the resolved HDR, keeps only contributions with
//!    luminance above `BLOOM_THRESHOLD` (1.0) → `bloom_a`
//! 2. blur-H — 9-tap gaussian along (1/w, 0) → `bloom_b`
//! 3. blur-V — 9-tap gaussian along (0, 1/h) → `bloom_a`
//!
//! The post pass then samples `bloom_a` (upscaled by its linear sampler) and
//! adds it pre-tonemap with weight `BLOOM_WEIGHT`.
//!
//! Why three uniform buffers: the passes need three DIFFERENT `BlurParams`
//! (threshold 1.0 / direction (1/w, 0) / direction (0, 1/h)) recorded into the
//! same command buffer. wgpu's `queue.write_buffer` is a submit-time operation
//! — three writes to one buffer before a single submit collapse to the last
//! write, so every pass would run with blur-V's params. Each pass therefore
//! owns a dedicated 16-byte uniform buffer + bind group (dynamic offsets would
//! force 256-byte alignment slots for the same effect).

use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

/// CPU mirror of bloom.wgsl's `BlurParams` (16 bytes, 16-aligned).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct BlurParams {
    /// Texel size of the half-res chain target along the blur axis (uv units).
    pub direction: [f32; 2],
    /// >= 0.0 selects the threshold branch; -1.0 selects the blur branch.
    pub threshold: f32,
    pub pad: f32,
}

/// HDR luminance cutoff for the threshold pass (the original app's default).
pub const BLOOM_THRESHOLD: f32 = 1.0;
/// Post-pass bloom mix weight — the original app's exact default (0.11).
pub const BLOOM_WEIGHT: f32 = 0.11;

/// Half-resolution ping-pong resources, recreated on resize together with the
/// frame targets (the threshold bind group pins the per-resize `hdr` view).
/// Only views are stored — a wgpu 30 view pins its parent texture via an Arc
/// (same pattern as `FrameTargets`).
struct Chain {
    bloom_a_view: wgpu::TextureView,
    bloom_b_view: wgpu::TextureView,
    bg_threshold: wgpu::BindGroup,
    bg_blur_h: wgpu::BindGroup,
    bg_blur_v: wgpu::BindGroup,
}

/// Stateless bloom pipeline + bind group layout + the per-size chain. Lives on
/// the `Renderer`; `resize` rebuilds the chain, `record` runs the three passes.
pub struct Bloom {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    chain: Option<Chain>,
}

impl Bloom {
    /// Builds the bloom pipeline once (Rgba16Float target, fullscreen triangle,
    /// sample_count 1 — matching the half-res chain targets).
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bloom.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/bloom.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    ..crate::TEX_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<BlurParams>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bloom_layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = crate::build_fullscreen_pipeline(
            device,
            &layout,
            &shader,
            "bloom_pipeline",
            wgpu::TextureFormat::Rgba16Float,
        );
        Self {
            pipeline,
            bgl,
            chain: None,
        }
    }

    /// (Re)creates the half-res chain. `width`/`height` are the chain's own
    /// size (half the final target, min 1) — the caller computes them.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        lin_sampler: &wgpu::Sampler,
        hdr_view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        // Creates one half-res rgba16float target and returns its view (the
        // `Texture` handle is pinned by the view — wgpu 30 Arc — and the bind
        // groups below; no separate handle is stored).
        let make_target = |label: &str| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            tex.create_view(&Default::default())
        };
        let bloom_a_view = make_target("bloom_a");
        let bloom_b_view = make_target("bloom_b");

        let make_ub = |label: &str, params: BlurParams| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        };
        // Threshold 1.0 never changes; the blur directions are the chain
        // target's texel sizes, so the blur uniforms are rebuilt with the
        // targets on every resize.
        let ub_threshold = make_ub(
            "bloom_ub_threshold",
            BlurParams {
                direction: [0.0, 0.0],
                threshold: BLOOM_THRESHOLD,
                pad: 0.0,
            },
        );
        let ub_blur_h = make_ub(
            "bloom_ub_blur_h",
            BlurParams {
                direction: [1.0 / width as f32, 0.0],
                threshold: -1.0,
                pad: 0.0,
            },
        );
        let ub_blur_v = make_ub(
            "bloom_ub_blur_v",
            BlurParams {
                direction: [0.0, 1.0 / height as f32],
                threshold: -1.0,
                pad: 0.0,
            },
        );

        let make_bg = |label: &str, view: &wgpu::TextureView, ub: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(lin_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: ub.as_entire_binding(),
                    },
                ],
            })
        };
        // Threshold reads the resolved HDR (recreated per resize, so the bind
        // group is rebuilt with it); blur-H consumes bloom_a, blur-V consumes
        // the result back into bloom_a.
        let bg_threshold = make_bg("bloom_bg_threshold", hdr_view, &ub_threshold);
        let bg_blur_h = make_bg("bloom_bg_blur_h", &bloom_a_view, &ub_blur_h);
        let bg_blur_v = make_bg("bloom_bg_blur_v", &bloom_b_view, &ub_blur_v);
        let chain = Chain {
            bloom_a_view,
            bloom_b_view,
            bg_threshold,
            bg_blur_h,
            bg_blur_v,
        };
        self.chain = Some(chain);
    }

    /// Drops the chain (zero-size resize).
    pub fn clear(&mut self) {
        self.chain = None;
    }

    /// The view the post pass must bind as `bloom_tex` when bloom is on.
    pub fn bloom_a_view(&self) -> Option<&wgpu::TextureView> {
        self.chain.as_ref().map(|c| &c.bloom_a_view)
    }

    /// Records threshold → blur-H → blur-V. No-op without a chain (zero-size
    /// window or before the first resize).
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(chain) = self.chain.as_ref() else {
            return;
        };
        // (label, attachment, bind group) per pass. Pass order matters:
        // threshold fills bloom_a, blur-H consumes it into bloom_b, blur-V
        // consumes that back into bloom_a (what post samples). wgpu inserts
        // the attachment→sampler barriers at the pass boundaries.
        for (label, view, bg) in [
            ("bloom_threshold", &chain.bloom_a_view, &chain.bg_threshold),
            ("bloom_blur_h", &chain.bloom_b_view, &chain.bg_blur_h),
            ("bloom_blur_v", &chain.bloom_a_view, &chain.bg_blur_v),
        ] {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    // The fullscreen triangle covers every pixel; the clear is
                    // defensive only.
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_params_match_wgsl_layout() {
        // bloom.wgsl: vec2 (align 8, off 0) + f32 @8 + f32 @12 -> 16 bytes.
        assert_eq!(size_of::<BlurParams>(), 16);
    }

    #[test]
    fn blur_direction_is_chain_texel_size() {
        // Half-res chain 400x300 -> H direction (1/400, 0), V direction
        // (0, 1/300); the threshold params carry no direction.
        let w = 400u32;
        let h = 300u32;
        let thr = BlurParams {
            direction: [0.0, 0.0],
            threshold: BLOOM_THRESHOLD,
            pad: 0.0,
        };
        let bh = BlurParams {
            direction: [1.0 / w as f32, 0.0],
            threshold: -1.0,
            pad: 0.0,
        };
        let bv = BlurParams {
            direction: [0.0, 1.0 / h as f32],
            threshold: -1.0,
            pad: 0.0,
        };
        assert_eq!(thr.threshold, 1.0);
        assert_eq!(bh.direction, [0.0025, 0.0]);
        assert_eq!(bv.direction, [0.0, 1.0 / 300.0]);
        assert!(bh.threshold < 0.0 && bv.threshold < 0.0);
    }
}
