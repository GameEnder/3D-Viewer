// crates/viewer-render/src/shaders/shadow.wgsl
//
// Depth-only vertex shader for the per-light shadow maps (Task 9). The
// renderer binds one ShadowVp uniform per light (group 0) and reuses the
// shared model uniform + dynamic-offset slots (group 1) so the shadow pass
// draws exactly the instances the pbr pass draws. The resulting depth map is
// sampled by pbr.wgsl/ground.wgsl with `LightU.view_proj` — the renderer
// writes the SAME matrix to both, so reprojected receivers line up with the
// rasterized depths.
struct ShadowVp { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> SVP: ShadowVp;
struct ModelU { model: mat4x4<f32>, normal_mat: mat4x4<f32> };
@group(1) @binding(0) var<uniform> MU: ModelU;

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
  return SVP.view_proj * MU.model * vec4<f32>(pos, 1.0);
}
