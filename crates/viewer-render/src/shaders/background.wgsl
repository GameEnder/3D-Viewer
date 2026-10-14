// crates/viewer-render/src/shaders/background.wgsl
//
// Task 18b: the viewport background. A fullscreen triangle drawn FIRST in the
// scene pass (HDR, alpha 0 = the background marker post.wgsl keys on), so
// ground and blend pixels blend against the true gradient rather than a flat
// clear color, and the post pass re-encodes exactly what was rendered.
//
// Calibration targets (Task 12 live A/B against the original app): neutral
// radial gradient, encoded sRGB ~110/255 at the viewport center fading to
// ~97/255 at the corners, R=G=B. Linear values arrive via BgU.

struct BgU {
  center: vec4<f32>, // linear rgb + pad
  edge: vec4<f32>,   // linear rgb + pad
};
@group(0) @binding(0) var<uniform> B: BgU;

struct VOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
  var p = array<vec2<f32>, 3>(
      vec2<f32>(-1.0, -3.0), vec2<f32>(3.0, 1.0), vec2<f32>(-1.0, 1.0));
  let xy = p[vi];
  var o: VOut;
  o.pos = vec4<f32>(xy, 0.0, 1.0);
  o.uv = vec2<f32>((xy.x + 1.0) * 0.5, 1.0 - (xy.y + 1.0) * 0.5);
  return o;
}

@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
  // 0 at the viewport center, 1 at the corners (uv-space diagonal).
  let t = clamp(distance(v.uv, vec2<f32>(0.5, 0.5)) * 1.41421354, 0.0, 1.0);
  let col = mix(B.center.rgb, B.edge.rgb, vec3<f32>(t));
  // alpha 0 marks background for post.wgsl's bypass branch; the depth test is
  // disabled and depth write is off, so every later draw simply overdraws it.
  return vec4<f32>(col, 0.0);
}
