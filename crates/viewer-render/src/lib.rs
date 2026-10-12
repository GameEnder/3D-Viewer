// crates/viewer-render/src/lib.rs
//! wgpu renderer: PBR forward pass into an MSAA-resolved HDR target, ACES post
//! with exposure/contrast, and a final `target_format` texture that the app
//! blits into the egui render pass.
//!
//! Binding contract (must match the WGSL in shaders/):
//! - globals group 0: bindings 0-1 uniforms, 2-4 shadow depth textures, 5 comparison sampler
//! - material group 1: bindings 0-4 uniform + textures, 5 sampler
//! - model group 2: binding 0 uniform with dynamic offset (256-byte slots)
//!
//! Task 7 adaptations over the plan (full list in task-7-report.md):
//! - post.wgsl sRGB-encodes its output (runtime target is non-Srgb Bgra8Unorm)
//!   and bypasses tonemapping for background pixels (HDR alpha < 0.5), which
//!   requires `PostU.clear_color` (linear) — see uniforms.rs.
//! - `prepare_render` gained `&Queue` per the Step 6 integration shape.
//! - Screenshot readback swaps BGRA->RGBA (the `final` texture is Bgra8Unorm at
//!   runtime); Task 13 consumes `collect_screenshot`.

pub mod gpu_scene;
pub mod uniforms;

use std::cmp::Ordering;
use std::sync::Arc;

use bytemuck::Zeroable;
use egui_wgpu::wgpu;
use glam::{Mat3, Mat4, Vec3};
use uniforms::{GlobalsU, LIGHT_COUNT, LightU, MaterialU, ModelU, PostU};

/// Light intensity multiplier — the theme's normalized intensity (100 = 1.0)
/// is multiplied by this before reaching the shader. Keep in sync with
/// shaders/pbr.wgsl (`* 8.0` in `shade_light`). Adjust during smoke if the
/// model reads brighter/dimmer than the original app.
pub const LIGHT_SCALE: f32 = 8.0;
/// Environment (ambient) scale: contribution = albedo * env_color *
/// env_intensity * ENV_SCALE. Keep in sync with shaders/pbr.wgsl (`* 0.35`
/// in `fs_main`).
pub const ENV_SCALE: f32 = 0.35;
/// Theme background color rgb(69,74,80) in LINEAR space. The PBR pass clears
/// the HDR buffer with it; post.wgsl sRGB-encodes it on output.
pub const DEFAULT_CLEAR_COLOR: [f32; 3] = [0.0592, 0.0677, 0.0797];

/// One slot in the fixed 3-light rig.
#[derive(Debug)]
pub struct LightInput {
    pub pos: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub shadows: bool,
}

/// Per-frame render toggles/values (toolbar-driven from Task 12 on).
pub struct RenderSettings {
    /// Linear-space background color (post.wgsl encodes sRGB on output).
    pub clear_color: [f32; 3],
    pub wireframe: bool,
    pub ground: bool,
    pub grid: bool,
    pub bloom: bool,
    pub exposure: f32,
    pub contrast: f32,
    pub ground_opacity: f32,
}

/// Everything the renderer needs to draw one frame.
pub struct FrameParams {
    pub view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 3],
    pub turntable_deg: f32,
    pub lights: [LightInput; 3],
    pub env_color: [f32; 3],
    pub env_intensity: f32,
    pub settings: RenderSettings,
    /// (animation_index, elapsed_seconds) — Task 17 evaluates and feeds this.
    pub anim: Option<(usize, f32)>,
}

struct PbrPipelines {
    opaque: wgpu::RenderPipeline,
    double_sided: wgpu::RenderPipeline,
    blend: wgpu::RenderPipeline,
}

/// Offscreen render targets recreated on resize. Only the views of the
/// MSAA/depth targets are stored (a wgpu view pins its parent texture via an
/// Arc); `final_tex` is kept as a handle for the screenshot copy.
struct FrameTargets {
    hdr_msaa_view: wgpu::TextureView,
    hdr_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    final_tex: wgpu::Texture,
    final_view: wgpu::TextureView,
}

/// 1x1 stand-ins for shadow maps / bloom / empty-scene sampling (Task 9/10
/// swap in real resources without touching the shaders).
struct Dummies {
    shadow_views: Vec<wgpu::TextureView>,
    black_tex: wgpu::Texture,
    black_view: wgpu::TextureView,
    /// (bloom_a, bloom_b) dummies; bloom_a's view fills the post bloom slot
    /// until Task 10 builds the real half-res chain.
    bloom_chain: Vec<(wgpu::Texture, wgpu::TextureView)>,
}

struct Fallbacks {
    base_tex: wgpu::Texture,
    base: wgpu::TextureView,
    mr_tex: wgpu::Texture,
    mr: wgpu::TextureView,
    normal_tex: wgpu::Texture,
    normal: wgpu::TextureView,
    emissive_tex: wgpu::Texture,
    emissive: wgpu::TextureView,
}

struct ScreenshotJob {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    bytes_per_row: u32,
}

pub struct Renderer {
    target_format: wgpu::TextureFormat,
    pbr_pipelines: PbrPipelines,
    post_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    material_bgl: wgpu::BindGroupLayout,
    model_bgl: wgpu::BindGroupLayout,
    post_bgl: wgpu::BindGroupLayout,
    blit_bgl: wgpu::BindGroupLayout,
    globals_ub: wgpu::Buffer,
    lights_ub: wgpu::Buffer,
    model_ub: wgpu::Buffer,
    post_ub: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    model_bg: wgpu::BindGroup,
    post_bg: Option<wgpu::BindGroup>,
    post_bg_empty: wgpu::BindGroup,
    blit_bg: Option<wgpu::BindGroup>,
    lin_sampler: wgpu::Sampler,
    targets: Option<FrameTargets>,
    size: (u32, u32),
    gpu_scene: Option<gpu_scene::GpuScene>,
    queued: Option<FrameParams>,
    pending_screenshot: bool,
    screenshot_job: Option<ScreenshotJob>,
    dummies: Dummies,
    fallbacks: Fallbacks,
    dummies_cleared: bool,
}

fn dummy_texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

impl Renderer {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let pbr_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pbr.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/pbr.wgsl").into()),
        });
        let post_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/post.wgsl").into()),
        });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blit.wgsl").into()),
        });

        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<GlobalsU>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            (size_of::<LightU>() * LIGHT_COUNT) as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    ..SHADOW_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    ..SHADOW_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    ..SHADOW_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let material_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<MaterialU>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    ..TEX_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    ..TEX_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    ..TEX_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    ..TEX_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let model_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(size_of::<ModelU>() as u64),
                },
                count: None,
            }],
        });
        let post_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    ..TEX_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    ..TEX_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<PostU>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    ..TEX_ENTRY
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pbr_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pbr_layout"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&material_bgl), Some(&model_bgl)],
            immediate_size: 0,
        });
        let post_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post_layout"),
            bind_group_layouts: &[Some(&post_bgl)],
            immediate_size: 0,
        });
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit_layout"),
            bind_group_layouts: &[Some(&blit_bgl)],
            immediate_size: 0,
        });

        let pbr_pipelines = PbrPipelines {
            opaque: build_pbr_pipeline(
                device,
                &pbr_layout,
                &pbr_shader,
                "pbr_opaque",
                Some(wgpu::Face::Back),
                true,
                None,
            ),
            double_sided: build_pbr_pipeline(
                device,
                &pbr_layout,
                &pbr_shader,
                "pbr_double_sided",
                None,
                true,
                None,
            ),
            blend: build_pbr_pipeline(
                device,
                &pbr_layout,
                &pbr_shader,
                "pbr_blend",
                None,
                false,
                Some(wgpu::BlendState::ALPHA_BLENDING),
            ),
        };
        let post_pipeline = build_fullscreen_pipeline(
            device,
            &post_layout,
            &post_shader,
            "post_pipeline",
            target_format,
        );
        let blit_pipeline = build_fullscreen_pipeline(
            device,
            &blit_layout,
            &blit_shader,
            "blit_pipeline",
            target_format,
        );

        // Uniform buffers.
        let globals_ub = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals_ub"),
            size: size_of::<GlobalsU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lights_ub = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lights_ub"),
            size: (size_of::<LightU>() * LIGHT_COUNT) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_ub = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model_ub"),
            size: 256, // one slot minimum; resized in set_scene
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let post_ub = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post_ub"),
            size: size_of::<PostU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Dummy shadow maps (depth, 1x1, cleared to 1.0 on first prepare) and
        // black 1x1 float textures for the empty-scene post pass / bloom slot.
        let mut shadow_views = Vec::new();
        for i in 0..3 {
            let tex = dummy_texture(
                device,
                &format!("dummy_shadow_{i}"),
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            shadow_views.push(tex.create_view(&Default::default()));
        }
        let black_tex = dummy_texture(
            device,
            "dummy_black",
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let black_view = black_tex.create_view(&Default::default());
        let mut bloom_chain = Vec::new();
        for i in 0..2 {
            let tex = dummy_texture(
                device,
                &format!("dummy_bloom_{i}"),
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            let view = tex.create_view(&Default::default());
            bloom_chain.push((tex, view));
        }
        let dummies = Dummies {
            shadow_views,
            black_tex,
            black_view,
            bloom_chain,
        };

        // 1x1 material fallbacks (white base/mr identity, neutral normal, black
        // emissive). Pixels are written via the queue on first prepare.
        let make_fallback = |label: &str, format: wgpu::TextureFormat| {
            let tex = dummy_texture(
                device,
                label,
                format,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            let view = tex.create_view(&Default::default());
            (tex, view)
        };
        let (base_tex, base) = make_fallback("fallback_base", wgpu::TextureFormat::Rgba8UnormSrgb);
        let (mr_tex, mr) = make_fallback("fallback_mr", wgpu::TextureFormat::Rgba8Unorm);
        let (normal_tex, normal) =
            make_fallback("fallback_normal", wgpu::TextureFormat::Rgba8Unorm);
        let (emissive_tex, emissive) =
            make_fallback("fallback_emissive", wgpu::TextureFormat::Rgba8UnormSrgb);
        let fallbacks = Fallbacks {
            base_tex,
            base,
            mr_tex,
            mr,
            normal_tex,
            normal,
            emissive_tex,
            emissive,
        };

        let lin_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lin_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let shadow_cmp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow_cmp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals_bg"),
            layout: &globals_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals_ub.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: lights_ub.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&dummies.shadow_views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&dummies.shadow_views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&dummies.shadow_views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&shadow_cmp),
                },
            ],
        });
        let model_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model_bg"),
            layout: &model_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &model_ub,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<ModelU>() as u64),
                }),
            }],
        });
        let post_bg_empty = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post_bg_empty"),
            layout: &post_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&dummies.black_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&dummies.bloom_chain[0].1),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&lin_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: post_ub.as_entire_binding(),
                },
            ],
        });

        Self {
            target_format,
            pbr_pipelines,
            post_pipeline,
            blit_pipeline,
            material_bgl,
            model_bgl,
            post_bgl,
            blit_bgl,
            globals_ub,
            lights_ub,
            model_ub,
            post_ub,
            globals_bg,
            model_bg,
            post_bg: None,
            post_bg_empty,
            blit_bg: None,
            lin_sampler,
            targets: None,
            size: (0, 0),
            gpu_scene: None,
            queued: None,
            pending_screenshot: false,
            screenshot_job: None,
            dummies,
            fallbacks,
            dummies_cleared: false,
        }
    }

    pub fn set_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: Arc<viewer_scene::Scene>,
    ) {
        let mut gs = gpu_scene::upload(device, queue, scene);
        let res = gpu_scene::MaterialResources {
            bgl: &self.material_bgl,
            lin_sampler: &self.lin_sampler,
            fallback_base: &self.fallbacks.base,
            fallback_mr: &self.fallbacks.mr,
            fallback_normal: &self.fallbacks.normal,
            fallback_emissive: &self.fallbacks.emissive,
        };
        gs.materials = gpu_scene::upload_materials(device, queue, &gs.scene, &res);
        let slots = (gs.draws.len() * 256).max(256) as u64;
        self.model_ub = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model_ub"),
            size: slots,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.model_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model_bg"),
            layout: &self.model_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.model_ub,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<ModelU>() as u64),
                }),
            }],
        });
        self.gpu_scene = Some(gs);
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            self.targets = None;
            self.size = (width, height);
            self.post_bg = None;
            self.blit_bg = None;
            return;
        }
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let hdr_msaa_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hdr_msaa"),
            size,
            mip_level_count: 1,
            sample_count: 4,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let hdr_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hdr"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth_msaa"),
            size,
            mip_level_count: 1,
            sample_count: 4,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24Plus,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let final_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("final"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let hdr_msaa_view = hdr_msaa_tex.create_view(&Default::default());
        let hdr_view = hdr_tex.create_view(&Default::default());
        let depth_view = depth_tex.create_view(&Default::default());
        let final_view = final_tex.create_view(&Default::default());

        // Rebuild the bind groups that reference the resized textures.
        self.post_bg = Some(self.build_post_bg(device, &hdr_view));
        self.blit_bg = Some(self.build_blit_bg(device, &final_view));

        self.targets = Some(FrameTargets {
            hdr_msaa_view,
            hdr_view,
            depth_view,
            final_tex,
            final_view,
        });
        self.size = (width, height);
        // Any in-flight screenshot job referred to the old final texture.
        self.screenshot_job = None;
    }

    pub fn queue_frame(&mut self, params: FrameParams) {
        self.queued = Some(params);
    }

    pub fn request_screenshot(&mut self) {
        self.pending_screenshot = true;
    }

    /// Records every pass (PBR + post, or a clear-color-only post when empty)
    /// into the `final` texture. Called from egui-wgpu's `prepare` phase.
    pub fn prepare_render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if self.size.0 == 0 || self.size.1 == 0 {
            return;
        }
        self.ensure_dummies_initialized(queue, encoder);
        let Some(params) = self.queued.take() else {
            return;
        };

        // Post uniforms (bloom weight stays 0.0 until Task 10 feeds the chain).
        let post = PostU {
            exposure: params.settings.exposure,
            contrast: params.settings.contrast,
            bloom_weight: 0.0,
            pad: 0.0,
            clear_color: [
                params.settings.clear_color[0],
                params.settings.clear_color[1],
                params.settings.clear_color[2],
                0.0,
            ],
        };
        queue.write_buffer(&self.post_ub, 0, bytemuck::bytes_of(&post));

        // Light uniforms: dummy shadow state until Task 9 (view_proj identity,
        // shadow.x flag = 0 disables sampling in pbr.wgsl).
        let mut lights = [LightU::zeroed(); LIGHT_COUNT];
        for (lu, li) in lights.iter_mut().zip(params.lights.iter()) {
            *lu = LightU {
                pos: [li.pos[0], li.pos[1], li.pos[2], li.intensity],
                color: [li.color[0], li.color[1], li.color[2], 0.0],
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                shadow: [if li.shadows { 1.0 } else { 0.0 }, 1.0, 0.0, 0.0],
            };
        }
        queue.write_buffer(&self.lights_ub, 0, bytemuck::cast_slice(&lights));

        let globals = GlobalsU {
            view_proj: params.view_proj,
            camera_pos: [
                params.camera_pos[0],
                params.camera_pos[1],
                params.camera_pos[2],
                0.0,
            ],
            env_color: [
                params.env_color[0],
                params.env_color[1],
                params.env_color[2],
                params.env_intensity,
            ],
            flags: [
                u32::from(params.settings.wireframe),
                u32::from(params.settings.grid),
                0,
                0,
            ],
            pad: [0.0; 4],
        };
        queue.write_buffer(&self.globals_ub, 0, bytemuck::bytes_of(&globals));

        // HDR clear color: linear background + alpha 0 background marker for
        // post.wgsl.
        let clear = wgpu::Color {
            r: params.settings.clear_color[0] as f64,
            g: params.settings.clear_color[1] as f64,
            b: params.settings.clear_color[2] as f64,
            a: 0.0,
        };

        let Some(targets) = self.targets.as_ref() else {
            return;
        };

        if let Some(gs) = self.gpu_scene.as_ref() {
            // Per-draw model uniforms: world transform * turntable spin around
            // the bounds center. Animation overrides arrive in Task 17.
            let world = viewer_scene::world_transforms(&gs.scene, &[]);
            let center = gs.scene.bounds.center();
            let spin = if params.turntable_deg != 0.0 {
                Mat4::from_translation(center)
                    * Mat4::from_rotation_y(params.turntable_deg.to_radians())
                    * Mat4::from_translation(-center)
            } else {
                Mat4::IDENTITY
            };
            for (slot, (_mesh_idx, node_idx)) in gs.draws.iter().enumerate() {
                let model = spin * world[*node_idx];
                let mu = ModelU {
                    model: model.to_cols_array_2d(),
                    normal_mat: normal_matrix(model).to_cols_array_2d(),
                };
                queue.write_buffer(&self.model_ub, (slot * 256) as u64, bytemuck::bytes_of(&mu));
            }

            // Draw order: opaque first (any order, depth-tested), then blend
            // far->near by squared distance from the camera.
            let cam = Vec3::from_array(params.camera_pos);
            let world_center =
                |i: usize| (spin * world[gs.draws[i].1]).transform_point3(Vec3::ZERO);
            let mut order: Vec<usize> = (0..gs.draws.len()).collect();
            order.sort_by(|&a, &b| {
                match (
                    gs.meshes[gs.draws[a].0].blend,
                    gs.meshes[gs.draws[b].0].blend,
                ) {
                    (false, true) => Ordering::Less,
                    (true, false) => Ordering::Greater,
                    (true, true) => {
                        let da = (world_center(a) - cam).length_squared();
                        let db = (world_center(b) - cam).length_squared();
                        db.total_cmp(&da) // far first
                    }
                    (false, false) => Ordering::Equal,
                }
            });

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pbr_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.hdr_msaa_view,
                    resolve_target: Some(&targets.hdr_view),
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            let mut current: Option<&wgpu::RenderPipeline> = None;
            for &d in &order {
                let (mesh_idx, _node_idx) = gs.draws[d];
                let mesh = &gs.meshes[mesh_idx];
                let pipe = if mesh.blend {
                    &self.pbr_pipelines.blend
                } else if mesh.double_sided {
                    &self.pbr_pipelines.double_sided
                } else {
                    &self.pbr_pipelines.opaque
                };
                if current != Some(pipe) {
                    pass.set_pipeline(pipe);
                    current = Some(pipe);
                }
                pass.set_bind_group(0, &self.globals_bg, &[]);
                let mat = gs
                    .materials
                    .get(mesh.material)
                    .expect("every referenced material uploaded");
                pass.set_bind_group(1, &mat.bind_group, &[]);
                pass.set_bind_group(2, &self.model_bg, &[(d * 256) as u32]);
                pass.set_vertex_buffer(0, mesh.vertex_buf.slice(..));
                pass.set_index_buffer(mesh.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }

        // Post pass: HDR -> final (full-screen triangle). With no scene the
        // black 1x1 source has alpha 0, so every pixel takes the background
        // branch and `final` ends up holding the encoded clear color.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.final_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.post_pipeline);
            let bg = match self.gpu_scene.as_ref() {
                Some(_) => self.post_bg.as_ref().or(Some(&self.post_bg_empty)),
                None => Some(&self.post_bg_empty),
            };
            if let Some(bg) = bg {
                pass.set_bind_group(0, bg, &[]);
            }
            pass.draw(0..3, 0..1);
        }

        // Screenshot staging copy (Task 13 collects via collect_screenshot).
        let new_job = if self.pending_screenshot {
            let (w, h) = self.size;
            self.targets.as_ref().map(|t| {
                let bytes_per_row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("screenshot_staging"),
                    size: (bytes_per_row * h) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &t.final_tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(bytes_per_row),
                            rows_per_image: Some(h),
                        },
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
                ScreenshotJob {
                    buffer,
                    width: w,
                    height: h,
                    bytes_per_row,
                }
            })
        } else {
            None
        };
        self.pending_screenshot = false;
        if new_job.is_some() {
            self.screenshot_job = new_job;
        }
    }

    /// Blits the `final` texture into the egui render pass at the callback
    /// viewport. Called from the ScenePaint callback's `paint`.
    pub fn blit(&self, render_pass: &mut wgpu::RenderPass<'static>, viewport: [f32; 4]) {
        let Some(bg) = self.blit_bg.as_ref() else {
            return;
        };
        render_pass.set_pipeline(&self.blit_pipeline);
        render_pass.set_bind_group(0, bg, &[]);
        render_pass.set_viewport(viewport[0], viewport[1], viewport[2], viewport[3], 0.0, 1.0);
        render_pass.draw(0..3, 0..1);
    }

    /// Maps the staging buffer of the last requested screenshot and converts
    /// BGRA (Bgra8Unorm runtime target) to RGBA bytes.
    pub fn collect_screenshot(&mut self, device: &wgpu::Device) -> Option<image::RgbaImage> {
        let job = self.screenshot_job.take()?;
        let len = job.bytes_per_row * job.height;
        let (tx, rx) = std::sync::mpsc::channel();
        job.buffer
            .map_async(wgpu::MapMode::Read, 0..len as u64, move |res| {
                let _ = tx.send(res.is_ok());
            });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let mapped = rx.recv().unwrap_or(false);
        if !mapped {
            return None;
        }
        let view = job.buffer.get_mapped_range(0..len as u64).ok()?;
        let mut img = image::RgbaImage::new(job.width, job.height);
        for y in 0..job.height {
            let row_start = (y * job.bytes_per_row) as usize;
            for x in 0..job.width {
                let o = row_start + (x * 4) as usize;
                let (b, g, r, a) = (view[o], view[o + 1], view[o + 2], view[o + 3]);
                img.put_pixel(x, y, image::Rgba([r, g, b, a]));
            }
        }
        drop(view);
        job.buffer.unmap();
        Some(img)
    }

    /// One-time initialization of the 1x1 dummy resources: float dummies get
    /// their pixels via the queue, depth dummies are cleared to 1.0 with tiny
    /// render passes in the current encoder (before any sampling pass).
    fn ensure_dummies_initialized(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if self.dummies_cleared {
            return;
        }
        let write_pixel = |tex: &wgpu::Texture, rgba: [u8; 4]| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        };
        // float16 black dummies (empty-scene source + bloom chain)
        let zero16f = [0u8; 8]; // one rgba16float pixel
        for tex in [
            &self.dummies.black_tex,
            &self.dummies.bloom_chain[0].0,
            &self.dummies.bloom_chain[1].0,
        ] {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &zero16f,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(8),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        // 1x1 material fallbacks: white base, white mr, neutral normal, black emissive
        write_pixel(&self.fallbacks.base_tex, [255, 255, 255, 255]);
        write_pixel(&self.fallbacks.mr_tex, [255, 255, 255, 255]);
        write_pixel(&self.fallbacks.normal_tex, [128, 128, 255, 255]);
        write_pixel(&self.fallbacks.emissive_tex, [0, 0, 0, 255]);
        for view in &self.dummies.shadow_views {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear_dummy_shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.dummies_cleared = true;
    }

    fn build_post_bg(
        &self,
        device: &wgpu::Device,
        hdr_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post_bg"),
            layout: &self.post_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.dummies.bloom_chain[0].1),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.lin_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.post_ub.as_entire_binding(),
                },
            ],
        })
    }

    fn build_blit_bg(
        &self,
        device: &wgpu::Device,
        final_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit_bg"),
            layout: &self.blit_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(final_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.lin_sampler),
                },
            ],
        })
    }
}

const SHADOW_ENTRY: wgpu::BindGroupLayoutEntry = wgpu::BindGroupLayoutEntry {
    binding: 0,
    visibility: wgpu::ShaderStages::FRAGMENT,
    ty: wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Depth,
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    },
    count: None,
};

const TEX_ENTRY: wgpu::BindGroupLayoutEntry = wgpu::BindGroupLayoutEntry {
    binding: 0,
    visibility: wgpu::ShaderStages::FRAGMENT,
    ty: wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: true },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    },
    count: None,
};

#[allow(clippy::too_many_arguments)]
fn build_pbr_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    cull_mode: Option<wgpu::Face>,
    depth_write: bool,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(gpu_scene::vertex_layout())],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth24Plus,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: 4,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba16Float,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn build_fullscreen_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    target_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Normal matrix = inverse-transpose of the model's upper-left 3x3, padded
/// back to 4x4. Falls back to identity for degenerate (zero-scale) transforms.
fn normal_matrix(model: Mat4) -> Mat4 {
    let m3 = Mat3::from_mat4(model);
    if m3.determinant().abs() < 1e-12 {
        Mat4::IDENTITY
    } else {
        Mat4::from_mat3(m3.inverse().transpose())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_matrix_handles_uniform_scale() {
        let m = Mat4::from_scale(Vec3::splat(2.0));
        let n = normal_matrix(m);
        // inverse-transpose of 2*s = 0.5*s; padded 4x4 scales normals by 0.5
        assert!((n.x_axis.x - 0.5).abs() < 1e-6);
        assert!((n.w_axis.w - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normal_matrix_identity_for_rigid() {
        let m = Mat4::from_rotation_y(1.23);
        let n = normal_matrix(m);
        assert!(n.abs_diff_eq(m, 1e-5));
    }
}
