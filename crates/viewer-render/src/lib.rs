// crates/viewer-render/src/lib.rs
//! wgpu renderer: PBR forward pass into an MSAA-resolved HDR target, ACES post
//! with exposure/contrast, and a final `target_format` texture that the app
//! blits into the egui render pass.
//!
//! Bloom (Task 10, see post.rs): when `RenderSettings.bloom` is set, a
//! threshold + separable gaussian chain runs at half resolution between the
//! MSAA resolve and the post pass; post samples its `bloom_a` result with
//! weight 0.11. When off, the post bind group keeps the 1x1 dummy and weight
//! 0.0 (bit-identical to the pre-bloom path).
//!
//! Binding contract (must match the WGSL in shaders/):
//! - globals group 0: bindings 0-1 uniforms, 2-4 shadow depth textures, 5 comparison sampler
//! - material group 1: bindings 0-4 uniform + textures, 5 sampler
//! - model group 2: binding 0 uniform with dynamic offset (256-byte slots)
//!
//! Globals extensions for ground.wgsl (Task 8; same 128-byte layout):
//! - `flags.z`/`flags.w` carry `exposure`/`contrast` as f32 bit patterns
//! - `pad.x` = ground opacity, `pad.yzw` = linear background color
//!
//! Background (Task 18b): a fullscreen gradient quad (`shaders/background.wgsl`)
//! is drawn first in the scene pass - and alone when no scene is loaded - so
//! the HDR always holds the calibrated radial gradient with the alpha-0
//! background marker; post.wgsl's bypass branch re-encodes it and ground/blend
//! pixels blend against the true gradient rather than a flat clear color.
//!
//! Task 7 adaptations over the plan (full list in task-7-report.md):
//! - post.wgsl sRGB-encodes its output (runtime target is non-Srgb Bgra8Unorm)
//!   and bypasses tonemapping for background pixels (HDR alpha < 0.5); since
//!   Task 18b the bypass re-encodes the HDR's gradient directly (no bg
//!   uniform in PostU).
//! - `prepare_render` gained `&Queue` per the Step 6 integration shape.
//! - Screenshot readback swaps BGRA->RGBA (the `final` texture is Bgra8Unorm at
//!   runtime); Task 13 consumes `collect_screenshot`.

pub mod gpu_scene;
pub mod post;
pub mod uniforms;

use std::cmp::Ordering;
use std::sync::Arc;

use bytemuck::Zeroable;
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;
use glam::{Mat3, Mat4, Vec3};
use uniforms::{BgU, GlobalsU, LIGHT_COUNT, LightU, MaterialU, ModelU, PostU, ShadowVpU};

/// Light intensity multiplier — the theme's normalized intensity (100 = 1.0)
/// is multiplied by this before reaching the shader. Keep in sync with
/// shaders/pbr.wgsl (* 1.85 in shade_light) and ground.wgsl. Calibrated in
/// Task 18b against the original app's runtime ground-pool brightness
/// (~119/255 encoded) via tests/calibration.rs; was 8.0 pre-calibration; retuned 1.5 -> 1.85 after the Task 8 lift was replaced by the rgb_out floor.
pub const LIGHT_SCALE: f32 = 1.85;
/// Environment (ambient) scale: contribution = albedo * env_color *
/// env_intensity * ENV_SCALE. Keep in sync with shaders/pbr.wgsl (`* 0.35`
/// in `fs_main`).
pub const ENV_SCALE: f32 = 0.35;
/// The PBR pass clears the HDR buffer with it (alpha 0 marker); ground.wgsl
/// also uses it as the blend reference for its rim fade. The VISIBLE
/// background is the radial gradient below (Task 18b) - this flat color only
/// feeds blend math.
pub const DEFAULT_CLEAR_COLOR: [f32; 3] = [0.0592, 0.0677, 0.0797];

/// Background gradient calibration (Task 18b, from Task 12's live A/B against
/// the original app): neutral (R=G=B) radial gradient, encoded sRGB ~110/255
/// at the viewport center fading to ~97/255 at the corners. Stored as ENCODED
/// targets and converted to linear for the HDR-drawn gradient, so the post
/// pass's re-encode round-trips to exactly these values.
pub const BG_CENTER_ENCODED: f32 = 110.0 / 255.0;
pub const BG_EDGE_ENCODED: f32 = 97.0 / 255.0;

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear-space background gradient values for `BgU`.
pub fn bg_gradient_linear() -> ([f32; 3], [f32; 3]) {
    let to_lin = |v: f32| [srgb_to_linear(v); 3];
    (to_lin(BG_CENTER_ENCODED), to_lin(BG_EDGE_ENCODED))
}

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
    /// Master shadow toggle (Task 9): a light casts only when BOTH this and
    /// its `LightInput.shadows` are set. Toolbar toggle lands in Task 12.
    pub shadows: bool,
    pub exposure: f32,
    pub contrast: f32,
    pub ground_opacity: f32,
}

/// Everything the renderer needs to draw one frame.
pub struct FrameParams {
    pub view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 3],
    pub turntable_deg: f32,
    /// Uniform model scale (Task 13), folded CPU-side into the per-draw model
    /// matrices about the bounds center (no uniform change; the ground disc
    /// is NOT part of the model and stays unscaled).
    pub model_scale: f32,
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

/// Ground disc geometry: radius 2.6 model units, 64 segments (brief Step 2).
const GROUND_SEGMENTS: u32 = 64;
const GROUND_RADIUS: f32 = 2.6;

/// Per-light shadow map edge length in texels (Task 9). `LightU.shadow.y`
/// (the PCF texel size) must stay 1/SHADOW_MAP_SIZE.
const SHADOW_MAP_SIZE: u32 = 512;
/// Texel size fed to the PCF samplers (`LightU.shadow.y`).
const SHADOW_TEXEL: f32 = 1.0 / SHADOW_MAP_SIZE as f32;
/// Depth bias for the shadow pass (constant, in depth-format units) — tuned
/// at smoke: kills self-shadow acne on the cube's lit faces without visible
/// peter-panning.
const SHADOW_DEPTH_BIAS: i32 = 2;
/// Slope-scaled depth bias for the shadow pass. Tuned at smoke: 1.0 left one
/// failing PCF tap (1/255 darkening) on grazing faces — the downhill texel's
/// stored depth landed within the receiver bias. 2.0 covers the steepest
/// rig-angle face slopes without visible peter-panning.
const SHADOW_SLOPE_BIAS: f32 = 2.0;

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

/// Per-light shadow resources (Task 9): one 512² depth32float map plus the
/// tiny view-projection uniform the depth-only pipeline renders with. The
/// view pins its texture (wgpu 30 Arc), so no separate `Texture` handle is
/// stored.
struct ShadowMap {
    view: wgpu::TextureView,
    ub: wgpu::Buffer,
    bg: wgpu::BindGroup,
}

/// 1x1 stand-ins for the bloom-off post bind group (Task 10: the bloom-on
/// group binds the real half-res chain's `bloom_a`; the 1x1 dummies stay bound
/// whenever bloom is off - weight 0.0, never sampled). The empty-scene post
/// pass was removed in Task 18b: the background quad now fills the HDR there
/// too, so the real `post_bg` serves every case. The shadow maps are real
/// since Task 9 (512 per light, bound directly by `globals_bg`).
struct Dummies {
    /// (bloom_a, bloom_b) 1x1 dummies; bloom_a's stays in the post bind group
    /// while bloom is off (never sampled there - weight 0.0).
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
    ground_pipeline: wgpu::RenderPipeline,
    post_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    /// Depth-only pipeline for the per-light shadow maps (Task 9).
    shadow_pipeline: wgpu::RenderPipeline,
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
    /// Identity model bind group for the ground disc (the scene's
    /// dynamic-offset slot 0 holds draw 0's matrix, so the disc gets its own
    /// uniform buffer; wgpu 30 bind groups pin their buffers via Arc, so no
    /// separate handle is stored).
    ground_model_bg: wgpu::BindGroup,
    /// Dummy material bind group for the ground pass (Task 13 review): the
    /// ground pipeline shares `pbr_layout`, whose group 1 is the material
    /// layout that ground.wgsl never samples — wgpu still requires the group
    /// bound at draw time, and with an all-blend scene the ground draw is the
    /// FIRST draw of the pbr pass (no mesh's group 1 is live yet), which used
    /// to fail validation. Binds the 1x1 fallbacks + a zeroed uniform, so the
    /// draw is valid regardless of the scene's materials.
    ground_material_bg: wgpu::BindGroup,
    ground_vb: wgpu::Buffer,
    ground_ib: wgpu::Buffer,
    ground_index_count: u32,
    /// Background gradient quad (Task 18b): drawn first in the scene pass and
    /// alone in the empty-scene pass, so the HDR always holds the gradient and
    /// post.wgsl's background branch just re-encodes it.
    bg_pipeline: wgpu::RenderPipeline,
    bg_ub: wgpu::Buffer,
    bg_bg: wgpu::BindGroup,
    post_bg: Option<wgpu::BindGroup>,
    /// Post bind group with the real `bloom_a` view (bloom ON).
    post_bg_bloom: Option<wgpu::BindGroup>,
    /// Bloom chain (Task 10): stateless pipeline + half-res targets/binds.
    bloom: post::Bloom,
    blit_bg: Option<wgpu::BindGroup>,
    lin_sampler: wgpu::Sampler,
    /// ClampToEdge sampler for the bloom chain bind groups and the post pass
    /// (blur taps reach beyond [0,1]; glTF material sampling keeps the shared
    /// Repeat `lin_sampler`).
    bloom_sampler: wgpu::Sampler,
    targets: Option<FrameTargets>,
    size: (u32, u32),
    gpu_scene: Option<gpu_scene::GpuScene>,
    queued: Option<FrameParams>,
    pending_screenshot: bool,
    screenshot_job: Option<ScreenshotJob>,
    /// One 512² depth map per rig light, indexed like `FrameParams.lights`
    /// (Task 9).
    shadow_maps: Vec<ShadowMap>,
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
        let ground_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ground.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/ground.wgsl").into()),
        });
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shadow.wgsl").into()),
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
        // Shadow pass uniforms (Task 9): one ShadowVp per light at group 0;
        // the model slot reuses model_bgl at group 1 (shadow.wgsl declares it
        // there — see the binding contract in shadow.wgsl).
        let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<ShadowVpU>() as u64),
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
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow_layout"),
            bind_group_layouts: &[Some(&shadow_bgl), Some(&model_bgl)],
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
        // Ground disc: same vertex layout and pass targets as the pbr
        // pipelines, but depth-write off and alpha blended (Task 8 Step 2).
        // Shares pbr_layout (its group 1 gets the dedicated dummy
        // ground_material_bg — ground.wgsl never samples the material group
        // but the layout declares it, see the Task 13 review note there).
        // Reuses build_pbr_pipeline, whose fixed Rgba16Float/Depth24Plus/4xMSAA
        // match the pbr pass.
        let ground_pipeline = build_pbr_pipeline(
            device,
            &pbr_layout,
            &ground_shader,
            "ground_pipeline",
            None,
            false,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        // Shadow pass (Task 9): depth-only (no fragment stage), 512²
        // Depth32Float attachment, sample_count 1, pipeline depth bias to
        // suppress acne (constants tuned at smoke, see SHADOW_* consts).
        let shadow_pipeline = build_shadow_pipeline(device, &shadow_layout, &shadow_shader);

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

        // Real shadow maps (Task 9): one 512² depth32float texture per rig
        // light + its view-proj uniform and bind group. Cleared to 1.0 each
        // frame by the shadow pass for lights with shadows enabled; disabled
        // lights skip the pass and their stale maps are never sampled
        // (LightU.shadow.x = 0 gates the shader side).
        let mut shadow_maps = Vec::with_capacity(LIGHT_COUNT);
        for i in 0..LIGHT_COUNT {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("shadow_map_{i}")),
                size: wgpu::Extent3d {
                    width: SHADOW_MAP_SIZE,
                    height: SHADOW_MAP_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            let ub = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!("shadow_vp_ub_{i}")),
                size: size_of::<ShadowVpU>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("shadow_vp_bg_{i}")),
                layout: &shadow_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ub.as_entire_binding(),
                }],
            });
            shadow_maps.push(ShadowMap { view, ub, bg });
        }

        // Bloom chain (Task 10): pipeline + bind group layout built once; the
        // half-res targets and per-pass bind groups follow the frame targets
        // in resize().
        let bloom = post::Bloom::new(device);

        // 1x1 float dummies for the bloom-off post bind group's bloom slot
        // (never sampled there - weight 0.0).
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
        let dummies = Dummies { bloom_chain };

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
        // Screen-space sampler for the bloom chain + post pass (fix round 1):
        // the blur taps sample uv ± i·texel, up to 4 texels outside [0,1] —
        // with the shared Repeat sampler, bright content at a viewport edge
        // would wrap its glow tail to the OPPOSITE edge (mirrored fringe) and
        // read the wrong texels beyond the edge. ClampToEdge keeps the tail
        // clamped at the border. glTF material sampling (pbr/ground) keeps the
        // shared Repeat sampler per glTF wrap semantics.
        let bloom_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bloom_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
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

        // Dedicated dummy material bind group for the ground pass (Task 13
        // review): the ground pipeline shares pbr_layout whose group 1 is the
        // material layout, but ground.wgsl never samples it — wgpu still
        // requires the group bound at draw time, and with an all-blend scene
        // the ground draw is the FIRST draw of the pbr pass (no mesh's group
        // 1 is live yet), which failed validation. The dummy (zeroed uniform
        // + 1x1 fallbacks) keeps the draw valid regardless of scene materials.
        let ground_material_ub = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ground_material_ub"),
            contents: bytemuck::bytes_of(&MaterialU::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // Pinned by ground_material_bg (wgpu 30); no separate handle stored.
        let ground_material_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ground_material_bg"),
            layout: &material_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ground_material_ub.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&fallbacks.base),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&fallbacks.mr),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&fallbacks.normal),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&fallbacks.emissive),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&lin_sampler),
                },
            ],
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
                    resource: wgpu::BindingResource::TextureView(&shadow_maps[0].view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&shadow_maps[1].view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&shadow_maps[2].view),
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

        // Ground disc mesh + constant identity model bind (brief Step 2).
        let (ground_vb, ground_ib, ground_index_count) = build_disc_buffers(device);
        let ground_model_ub = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ground_model_ub"),
            contents: bytemuck::bytes_of(&ModelU {
                model: Mat4::IDENTITY.to_cols_array_2d(),
                normal_mat: Mat4::IDENTITY.to_cols_array_2d(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // Pinned by ground_model_bg (wgpu 30).
        let ground_model_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ground_model_bg"),
            layout: &model_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &ground_model_ub,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<ModelU>() as u64),
                }),
            }],
        });
        // Background gradient quad (Task 18b): drawn first in the scene pass
        // and alone when no scene is loaded, so the HDR always holds the
        // calibrated gradient (post.wgsl's background branch re-encodes it).
        let bg_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("background.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/background.wgsl").into()),
        });
        let bg_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bg_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<BgU>() as u64),
                },
                count: None,
            }],
        });
        let bg_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bg_layout"),
            bind_group_layouts: &[Some(&bg_bgl)],
            immediate_size: 0,
        });
        let bg_pipeline = build_background_pipeline(device, &bg_layout, &bg_shader);
        let bg_ub = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg_ub"),
            size: size_of::<BgU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg_bg"),
            layout: &bg_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: bg_ub.as_entire_binding(),
            }],
        });

        Self {
            target_format,
            pbr_pipelines,
            ground_pipeline,
            post_pipeline,
            blit_pipeline,
            shadow_pipeline,
            material_bgl,
            model_bgl,
            post_bgl,
            blit_bgl,
            bg_pipeline,
            bg_ub,
            bg_bg,
            globals_ub,
            lights_ub,
            model_ub,
            post_ub,
            globals_bg,
            model_bg,
            ground_model_bg,
            ground_material_bg,
            ground_vb,
            ground_ib,
            ground_index_count,
            post_bg: None,
            post_bg_bloom: None,
            blit_bg: None,
            lin_sampler,
            bloom_sampler,
            targets: None,
            size: (0, 0),
            gpu_scene: None,
            queued: None,
            pending_screenshot: false,
            screenshot_job: None,
            shadow_maps,
            dummies,
            fallbacks,
            dummies_cleared: false,
            bloom,
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
            self.post_bg_bloom = None;
            self.blit_bg = None;
            self.bloom.clear();
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

        // Half-res bloom chain (Task 10): ping-pong targets at half the final
        // size (min 1), recreated with the frame targets. The threshold pass's
        // bind group pins this resize's hdr view.
        let bw = (width / 2).max(1);
        let bh = (height / 2).max(1);
        self.bloom
            .resize(device, &self.bloom_sampler, &hdr_view, bw, bh);

        // Rebuild the bind groups that reference the resized textures. The
        // bloom-off group keeps the 1x1 dummy in the bloom slot (never
        // sampled at weight 0.0); the bloom-on group binds the chain result.
        // Labels (ruling, Task 9/10 review): the two scene post groups are
        // named by bloom state so captures distinguish them.
        self.post_bg_bloom = self
            .bloom
            .bloom_a_view()
            .map(|v| self.build_post_bg(device, "post_bg_bloom_on", &hdr_view, v));
        self.post_bg = Some(self.build_post_bg(
            device,
            "post_bg_bloom_off",
            &hdr_view,
            &self.dummies.bloom_chain[0].1,
        ));
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
        self.ensure_dummies_initialized(queue);
        let Some(params) = self.queued.take() else {
            return;
        };

        // Post uniforms. Bloom weight = the original app's exact default when
        // the chain runs, 0.0 otherwise. The background gradient constants go
        // to their own uniform below (Task 18b) - PostU no longer carries a
        // clear color (post.wgsl re-encodes the HDR's background quad).
        let post = PostU {
            exposure: params.settings.exposure,
            contrast: params.settings.contrast,
            bloom_weight: if params.settings.bloom {
                post::BLOOM_WEIGHT
            } else {
                0.0
            },
            pad: 0.0,
        };
        queue.write_buffer(&self.post_ub, 0, bytemuck::bytes_of(&post));
        let (bg_center, bg_edge) = bg_gradient_linear();
        queue.write_buffer(
            &self.bg_ub,
            0,
            bytemuck::bytes_of(&BgU {
                center: [bg_center[0], bg_center[1], bg_center[2], 0.0],
                edge: [bg_edge[0], bg_edge[1], bg_edge[2], 0.0],
            }),
        );

        // Per-light shadow state (Task 9): fit an ortho projection around the
        // scene bounds for every light that casts. The SAME matrix goes into
        // LightU.view_proj (pbr.wgsl/ground.wgsl transform world→shadow space
        // with it) and the per-light SVP uniform the shadow pass renders with,
        // so reprojected receivers line up with the rasterized depths.
        let mut shadow_vp = [Mat4::IDENTITY; LIGHT_COUNT];
        let mut shadow_on = [false; LIGHT_COUNT];
        if let Some(gs) = self.gpu_scene.as_ref() {
            let center = gs.scene.bounds.center();
            let radius = gs.scene.bounds.bounding_sphere_radius();
            for (i, li) in params.lights.iter().enumerate() {
                if li.shadows && params.settings.shadows {
                    shadow_vp[i] = shadow_view_proj(Vec3::from_array(li.pos), center, radius);
                    shadow_on[i] = true;
                }
            }
        }

        let mut lights = [LightU::zeroed(); LIGHT_COUNT];
        for (i, lu) in lights.iter_mut().enumerate() {
            let li = &params.lights[i];
            *lu = LightU {
                pos: [li.pos[0], li.pos[1], li.pos[2], li.intensity],
                color: [li.color[0], li.color[1], li.color[2], 0.0],
                view_proj: shadow_vp[i].to_cols_array_2d(),
                shadow: [if shadow_on[i] { 1.0 } else { 0.0 }, SHADOW_TEXEL, 0.0, 0.0],
            };
        }
        queue.write_buffer(&self.lights_ub, 0, bytemuck::cast_slice(&lights));
        for (i, on) in shadow_on.iter().enumerate() {
            if *on {
                queue.write_buffer(
                    &self.shadow_maps[i].ub,
                    0,
                    bytemuck::bytes_of(&ShadowVpU {
                        view_proj: shadow_vp[i].to_cols_array_2d(),
                    }),
                );
            }
        }

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
                // ground.wgsl derives the tonemap-survival threshold from
                // exposure/contrast; bitcast through the unused u32 slots.
                params.settings.exposure.to_bits(),
                params.settings.contrast.to_bits(),
            ],
            pad: [
                params.settings.ground_opacity,
                params.settings.clear_color[0],
                params.settings.clear_color[1],
                params.settings.clear_color[2],
            ],
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
            // Task 17: animation overrides — evaluate the requested clip at
            // the app's playback time; `anim = None` or an out-of-range clip
            // renders the static pose (empty overrides).
            let overrides = match params.anim {
                Some((clip, time)) if clip < gs.scene.animations.len() => {
                    viewer_scene::evaluate(&gs.scene, clip, time)
                }
                _ => Vec::new(),
            };
            // Per-draw model uniforms: world transform * turntable spin and
            // uniform scale, both around the bounds center (the two compose
            // either way — uniform scaling commutes with rotation). Scale is
            // folded CPU-side per the Task 13 controller ruling (no uniform
            // change); the turntable stays the root post-rotation on top of
            // the animated node transforms.
            let world = viewer_scene::world_transforms(&gs.scene, &overrides);
            let center = gs.scene.bounds.center();
            let spin = if params.turntable_deg != 0.0 {
                Mat4::from_translation(center)
                    * Mat4::from_rotation_y(params.turntable_deg.to_radians())
                    * Mat4::from_translation(-center)
            } else {
                Mat4::IDENTITY
            };
            let scale = if params.model_scale != 1.0 {
                Mat4::from_translation(center)
                    * Mat4::from_scale(Vec3::splat(params.model_scale))
                    * Mat4::from_translation(-center)
            } else {
                Mat4::IDENTITY
            };
            let xform = scale * spin;
            for (slot, (_mesh_idx, node_idx)) in gs.draws.iter().enumerate() {
                let model = xform * world[*node_idx];
                let mu = ModelU {
                    model: model.to_cols_array_2d(),
                    normal_mat: normal_matrix(model).to_cols_array_2d(),
                };
                queue.write_buffer(&self.model_ub, (slot * 256) as u64, bytemuck::bytes_of(&mu));
            }

            // Draw order: opaque first (any order, depth-tested), then the
            // ground disc (Task 8: after opaques, before blend meshes), then
            // blend meshes far->near by squared distance from the camera.
            let cam = Vec3::from_array(params.camera_pos);
            let world_center =
                |i: usize| (xform * world[gs.draws[i].1]).transform_point3(Vec3::ZERO);
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
            // The sort places every non-blend draw before every blend draw,
            // so the ground slots in at the first blend draw.
            let blend_start = order
                .iter()
                .position(|&d| gs.meshes[gs.draws[d].0].blend)
                .unwrap_or(order.len());

            enum DrawStep {
                Background,
                Mesh(usize),
                Ground,
            }
            let ground_draw = params.settings.ground && params.settings.ground_opacity > 0.0;
            let steps = std::iter::once(DrawStep::Background)
                .chain(order[..blend_start].iter().copied().map(DrawStep::Mesh))
                .chain(ground_draw.then_some(DrawStep::Ground))
                .chain(order[blend_start..].iter().copied().map(DrawStep::Mesh));

            // Shadow passes (Task 9): one depth-only pass per shadow-casting
            // light — clear its 512² map to 1.0 and draw the non-blend draws
            // with the same model dynamic-offset slots the pbr pass uses.
            // Blend casters are a known simplification (the original app does
            // similar). Disabled lights skip the pass entirely: their map
            // holds stale depth that is never sampled (LightU.shadow.x = 0).
            for (i, on) in shadow_on.iter().enumerate() {
                if !on {
                    continue;
                }
                let map = &self.shadow_maps[i];
                let mut spass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    // Per-light label (ruling, Task 9/10 review): makes each
                    // light's shadow pass identifiable in captures and error
                    // messages.
                    label: Some(&format!("shadow_pass_l{i}")),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &map.view,
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
                spass.set_pipeline(&self.shadow_pipeline);
                spass.set_bind_group(0, &map.bg, &[]);
                for &d in &order[..blend_start] {
                    let (mesh_idx, _node_idx) = gs.draws[d];
                    let mesh = &gs.meshes[mesh_idx];
                    spass.set_bind_group(1, &self.model_bg, &[(d * 256) as u32]);
                    spass.set_vertex_buffer(0, mesh.vertex_buf.slice(..));
                    spass.set_index_buffer(mesh.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                    spass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }

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
            for step in steps {
                match step {
                    DrawStep::Background => {
                        // Task 18b: the gradient goes in FIRST (alpha 0
                        // marker, no depth), so ground and blend pixels blend
                        // against the true gradient and post.wgsl's background
                        // branch re-encodes exactly what was rendered.
                        pass.set_pipeline(&self.bg_pipeline);
                        pass.set_bind_group(0, &self.bg_bg, &[]);
                        pass.draw(0..3, 0..1);
                    }
                    DrawStep::Ground => {
                        pass.set_pipeline(&self.ground_pipeline);
                        pass.set_bind_group(0, &self.globals_bg, &[]);
                        // Group 1: ground.wgsl never samples the material
                        // group, but the shared pbr_layout declares it and
                        // wgpu requires it bound at draw time — with an
                        // all-blend scene the ground draw is the pass's FIRST
                        // draw (no mesh's group 1 is live yet), which failed
                        // validation (Task 13 review). The dedicated dummy
                        // keeps the draw valid regardless of scene materials.
                        pass.set_bind_group(1, &self.ground_material_bg, &[]);
                        pass.set_bind_group(2, &self.ground_model_bg, &[0]);
                        pass.set_vertex_buffer(0, self.ground_vb.slice(..));
                        pass.set_index_buffer(self.ground_ib.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..self.ground_index_count, 0, 0..1);
                    }
                    DrawStep::Mesh(d) => {
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
                        // Ruling (Task 7 review): a malformed file can
                        // reference a material index that was never uploaded
                        // — fall back to material 0 instead of panicking. A
                        // scene without ANY materials cannot bind anything,
                        // so the draw is skipped (loaders always upload at
                        // least one material).
                        let mat = match gs
                            .materials
                            .get(mesh.material)
                            .or_else(|| gs.materials.first())
                        {
                            Some(mat) => mat,
                            None => continue,
                        };
                        pass.set_bind_group(1, &mat.bind_group, &[]);
                        pass.set_bind_group(2, &self.model_bg, &[(d * 256) as u32]);
                        pass.set_vertex_buffer(0, mesh.vertex_buf.slice(..));
                        pass.set_index_buffer(mesh.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    }
                }
            }
        }

        // Bloom chain (Task 10): threshold + separable blur at half res, run
        // after the MSAA resolve (the pbr pass above) and before the post
        // pass, only when bloom is enabled and a scene is present. The
        // threshold reads the resolved HDR; background pixels hold the
        // gradient (luminance ~0.12-0.16 << 1.0) and never pass the
        // threshold, so no halo forms over the background.
        if params.settings.bloom && self.gpu_scene.is_some() {
            self.bloom.record(encoder);
        }

        // No scene: still fill the HDR with the background gradient (alpha 0
        // marker), so the post's background branch re-encodes the same
        // calibrated look a loaded scene shows (Task 18b; replaces the old
        // post_bg_empty 1x1-black path).
        if self.gpu_scene.is_none() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("background_only_pass"),
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
                    // The background pipeline declares Depth24Plus (the scene
                    // pass carries depth), so this pass must attach the same
                    // target even though the quad ignores depth.
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
            pass.set_pipeline(&self.bg_pipeline);
            pass.set_bind_group(0, &self.bg_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        // Post pass: HDR -> final (full-screen triangle). Background pixels
        // (alpha 0) re-encode the HDR's gradient; scene pixels take the
        // tonemap chain.
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
            // Bloom on -> the real chain result; otherwise the HDR directly
            // (the 1x1 bloom-slot dummy stays bound but is never sampled at
            // weight 0.0).
            let bg = match (self.gpu_scene.as_ref(), params.settings.bloom) {
                (Some(_), true) => self.post_bg_bloom.as_ref().or(self.post_bg.as_ref()),
                _ => self.post_bg.as_ref(),
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
    ///
    /// Orientation (verified empirically in the Task 13 smoke — the harness
    /// compared a `collect_screenshot` capture against a readback of the same
    /// frame blitted to a screen-sized texture): the staging copy's row 0 IS
    /// the screen's top row, so the PNG needs NO row reversal. The suspected
    /// flip (Task 7 review) does not exist: post.wgsl and blit.wgsl both map
    /// uv.y = 1 - (ndc.y+1)/2, and with wgpu's NDC-to-framebuffer mapping
    /// that composes to orientation-preserving copies — the screen is upright
    /// and `final` is stored upright in memory.
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
        let img = readback_to_image(&view, job.width, job.height, job.bytes_per_row);
        drop(view);
        job.buffer.unmap();
        Some(img)
    }

    /// One-time initialization of the 1x1 dummy resources: float dummies get
    /// their pixels via the queue. (The shadow maps are real since Task 9 and
    /// are cleared by their own render passes each frame.)
    fn ensure_dummies_initialized(&mut self, queue: &wgpu::Queue) {
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
        // float16 black dummies (bloom chain; never sampled at weight 0.0)
        let zero16f = [0u8; 8]; // one rgba16float pixel
        for tex in [
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
        self.dummies_cleared = true;
    }

    fn build_post_bg(
        &self,
        device: &wgpu::Device,
        label: &str,
        hdr_view: &wgpu::TextureView,
        bloom_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.post_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(bloom_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    // post.wgsl uses one sampler for scene_tex AND bloom_tex;
                    // both are screen-space (fullscreen-triangle uv never
                    // crosses the border in practice), so the ClampToEdge
                    // sampler is strictly more correct here. glTF Repeat
                    // semantics live in material_bgl, untouched.
                    resource: wgpu::BindingResource::Sampler(&self.bloom_sampler),
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

/// Background gradient quad (Task 18b): fullscreen triangle into the HDR
/// (Rgba16Float, 4x MSAA to match the scene pass), no depth, no blend. It
/// must be drawn FIRST so every later draw overdraws it, and it writes the
/// alpha-0 background marker post.wgsl keys on.
fn build_background_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("background_pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        // The scene pass carries a Depth24Plus attachment, so the pipeline
        // must declare it; write OFF + Always means the quad neither tests
        // nor writes depth (drawn first, everything overdraws it).
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth24Plus,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
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
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Depth-only pipeline for the per-light shadow maps (Task 9): vertex stage
/// only, full scene vertex layout (only position is consumed by shadow.wgsl),
/// Depth32Float attachment with sample_count 1 (the maps are not MSAA) and
/// pipeline depth bias against self-shadow acne.
fn build_shadow_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shadow_pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(gpu_scene::vertex_layout())],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState {
                constant: SHADOW_DEPTH_BIAS,
                slope_scale: SHADOW_SLOPE_BIAS,
                clamp: 0.0,
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: None,
        multiview_mask: None,
        cache: None,
    })
}

/// Ortho view-projection fitting one shadow-casting light around the scene
/// bounds (brief Step 2): half-extent = radius * 1.25, near 0.05, far
/// dist + 2.5 * half-extent, view = look_at_rh(light → center). Up is +Y
/// except for near-vertical lights, where look_at with up = +Y degenerates —
/// the brief's guard falls back to +Z there.
fn shadow_view_proj(light_pos: Vec3, center: Vec3, bounds_radius: f32) -> Mat4 {
    let r = bounds_radius * 1.25;
    let to_light = light_pos - center;
    let dist = to_light.length().max(1e-4);
    let up = if to_light.y.abs() > 0.99 * dist {
        Vec3::Z
    } else {
        Vec3::Y
    };
    Mat4::orthographic_rh(-r, r, -r, r, 0.05, dist + 2.5 * r)
        * Mat4::look_at_rh(light_pos, center, up)
}

/// Converts one BGRA readback buffer into an RGBA `RgbaImage`, rows in order:
/// the staging copy's row 0 is the screen's top row (orientation verified
/// against a same-frame blit readback in the Task 13 smoke — see
/// collect_screenshot), so no flip is applied; only BGRA->RGBA is swapped.
fn readback_to_image(view: &[u8], width: u32, height: u32, bytes_per_row: u32) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(width, height);
    for y in 0..height {
        let row_start = (y * bytes_per_row) as usize;
        for x in 0..width {
            let o = row_start + (x * 4) as usize;
            let (b, g, r, a) = (view[o], view[o + 1], view[o + 2], view[o + 3]);
            img.put_pixel(x, y, image::Rgba([r, g, b, a]));
        }
    }
    img
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

/// Ground disc mesh (Task 8 Step 2): fan at `GROUND_RADIUS` in the XZ plane
/// with `GROUND_SEGMENTS` segments — 65 vertices (center + rim) plus a fan
/// index buffer wound CCW when viewed from +Y. `uv.x` carries the radial
/// coordinate 0..1 for ground.wgsl's falloff. The barycentric attribute is
/// part of the fixed 44-byte layout but unused by ground.wgsl (values are
/// don't-care; indexed fan vertices cannot carry per-corner barycentrics).
fn build_disc_buffers(device: &wgpu::Device) -> (wgpu::Buffer, wgpu::Buffer, u32) {
    let mut verts = Vec::with_capacity((GROUND_SEGMENTS + 1) as usize);
    verts.push(uniforms::Vertex {
        pos: [0.0, 0.0, 0.0],
        normal: [0.0, 1.0, 0.0],
        uv: [0.0, 0.0],
        bary: uniforms::BARY[0],
    });
    for i in 0..GROUND_SEGMENTS {
        let a = (i as f32) / (GROUND_SEGMENTS as f32) * std::f32::consts::TAU;
        verts.push(uniforms::Vertex {
            pos: [GROUND_RADIUS * a.cos(), 0.0, GROUND_RADIUS * a.sin()],
            normal: [0.0, 1.0, 0.0],
            uv: [1.0, 0.0],
            bary: uniforms::BARY[1],
        });
    }
    let mut indices = Vec::with_capacity((GROUND_SEGMENTS * 3) as usize);
    for i in 0..GROUND_SEGMENTS {
        let next = (i + 1) % GROUND_SEGMENTS;
        // (center, rim_next, rim_cur) faces +Y so the disc is front-facing
        // from above.
        indices.extend_from_slice(&[0, 1 + next, 1 + i]);
    }
    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ground_vertices"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ground_indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    (vb, ib, indices.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    /// The screenshot readback conversion: BGRA->RGBA byte swap with rows in
    /// order (the staging copy's row 0 is the screen's top row — orientation
    /// verified against a same-frame blit readback in the Task 13 smoke).
    #[test]
    fn readback_swaps_to_rgba_preserving_row_order() {
        // 2x3 image, padded rows (bytes_per_row 16); distinct values per row
        // and channel prove the byte swap and that no flip is applied.
        let px = |r: u8, g: u8, b: u8, a: u8| [b, g, r, a]; // BGRA order
        let mut buf = vec![0u8; 16 * 3];
        buf[0..4].copy_from_slice(&px(1, 2, 3, 4));
        buf[4..8].copy_from_slice(&px(5, 6, 7, 8));
        buf[16..20].copy_from_slice(&px(9, 10, 11, 12));
        buf[20..24].copy_from_slice(&px(13, 14, 15, 16));
        buf[32..36].copy_from_slice(&px(17, 18, 19, 20));
        buf[36..40].copy_from_slice(&px(21, 22, 23, 24));
        let img = readback_to_image(&buf, 2, 3, 16);
        // Screen row 0 comes from buffer row 0, RGBA-swapped.
        assert_eq!(img.get_pixel(0, 0).0, [1, 2, 3, 4]);
        assert_eq!(img.get_pixel(1, 0).0, [5, 6, 7, 8]);
        assert_eq!(img.get_pixel(0, 1).0, [9, 10, 11, 12]);
        assert_eq!(img.get_pixel(1, 1).0, [13, 14, 15, 16]);
        assert_eq!(img.get_pixel(0, 2).0, [17, 18, 19, 20]);
        assert_eq!(img.get_pixel(1, 2).0, [21, 22, 23, 24]);
    }

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

    #[test]
    fn shadow_fit_centers_scene_and_covers_catchment() {
        // A rig light (40° elevation, 2.2 out) around a normalized scene
        // (radius 1, base y=0): the bounds center must project to the map
        // center with depth inside (0,1), and the ground point where the
        // model-top shadow lands (~1.4 units from center, away from the
        // light) must still reproject into the map's uv/depth range —
        // otherwise the shadow tail would be culled by shadow_factor's uv
        // guard.
        let center = Vec3::new(0.0, 0.5, 0.0);
        let el = 40.0f32.to_radians();
        let eye = center + Vec3::new(el.cos(), el.sin(), 0.0) * 2.2;
        let vp = shadow_view_proj(eye, center, 1.0);
        let c = vp * center.extend(1.0);
        assert!((c.x / c.w).abs() < 1e-5 && (c.y / c.w).abs() < 1e-5);
        assert!(c.z / c.w > 0.0 && c.z / c.w < 1.0);
        // catchment: away-from-light horizontal direction, ground level
        let away = -Vec3::new(el.cos(), 0.0, 0.0);
        let p = Vec3::new(center.x, 0.0, center.z) + away * 1.4;
        let s = vp * p.extend(1.0);
        let uv = Vec2::new(s.x / s.w * 0.5 + 0.5, 1.0 - (s.y / s.w * 0.5 + 0.5));
        assert!(uv.x > 0.0 && uv.x < 1.0 && uv.y > 0.0 && uv.y < 1.0);
        assert!(s.z / s.w > 0.0 && s.z / s.w < 1.0);
    }

    #[test]
    fn shadow_fit_survives_top_light() {
        // A light directly above the center degenerates look_at(up = +Y);
        // the guard must keep every projected coordinate finite.
        let center = Vec3::new(0.0, 0.5, 0.0);
        let vp = shadow_view_proj(Vec3::new(0.0, 4.0, 0.0), center, 1.0);
        let c = vp * center.extend(1.0);
        assert!(c.x.is_finite() && c.y.is_finite() && c.z.is_finite() && c.w.is_finite());
        assert!((c.x / c.w).abs() < 1e-5 && (c.y / c.w).abs() < 1e-5);
        assert!(c.z / c.w > 0.0 && c.z / c.w < 1.0);
    }
}
