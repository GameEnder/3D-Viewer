// crates/viewer-app/src/paint.rs
use egui_wgpu::RenderState;
use egui_wgpu::wgpu;

/// Stored in egui_wgpu callback_resources; retrieved by paint callbacks.
pub struct SolidPipeline(pub wgpu::RenderPipeline);

/// Insert shared GPU resources once at app creation.
pub fn install(state: &RenderState) {
    let device = &state.device;
    let format = state.target_format;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("solid.wgsl"),
        source: wgpu::ShaderSource::Wgsl(SOLID_WGSL.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("solid_pipeline"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        multiview_mask: None,
        cache: None,
    });
    state
        .renderer
        .write()
        .callback_resources
        .insert(SolidPipeline(pipeline));
}

const SOLID_WGSL: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0), vec2<f32>(3.0, 1.0), vec2<f32>(-1.0, 1.0));
    return vec4<f32>(pos[vi], 0.0, 1.0);
}
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // linear values for rgb(69,74,80). Runtime check: the surface format is
    // Bgra8Unorm (egui_wgpu::preferred_framebuffer_format prefers non-Srgb
    // formats), so the target does NOT sRGB-encode — we must encode here.
    // pow(c, 1/2.2) ≈ proper sRGB encode within ±2/255 of the reference color.
    return vec4<f32>(
        pow(0.0592, 1.0 / 2.2),
        pow(0.0677, 1.0 / 2.2),
        pow(0.0797, 1.0 / 2.2),
        1.0,
    );
}
"#;
