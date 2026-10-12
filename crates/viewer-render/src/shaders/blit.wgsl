// crates/viewer-render/src/shaders/blit.wgsl
//
// Final texture -> egui render pass. Passes pixel values through unchanged:
// the source (`final`) and the egui framebuffer are both non-Srgb
// (Bgra8Unorm at runtime), and post.wgsl already encoded sRGB.
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
  var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(3.0, 1.0), vec2<f32>(-1.0, 1.0));
  let xy = p[vi];
  var o: VOut;
  o.pos = vec4<f32>(xy, 0.0, 1.0);
  o.uv = vec2<f32>((xy.x + 1.0) * 0.5, 1.0 - (xy.y + 1.0) * 0.5);
  return o;
}
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var lin_sampler: sampler;
@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
  return textureSample(src, lin_sampler, v.uv);
}
