// crates/viewer-render/src/shaders/post.wgsl
//
// ACES tonemap + exposure/contrast, HDR (rgba16f) -> final (target_format).
//
// Task 7 adaptations (documented in task-7-report.md):
// 1. The runtime framebuffer is Bgra8Unorm (non-Srgb), so this shader
//    sRGB-encodes its output itself (controller ruling from Task 2).
// 2. Background pixels (HDR alpha < 0.5, i.e. the cleared color) bypass tone
//    mapping and output the theme clear color directly. ACES + contrast would
//    otherwise crush the dark clear color to pure black, contradicting the
//    ported constant rgb(69,74,80). Clear color arrives in PostU as LINEAR rgb.
// 3. Bloom stays in the brief's position (added in HDR-linear space before
//    tonemapping); note it only shows on scene pixels until Task 10 revisits.

struct PostU {
  exposure: f32,
  contrast: f32,
  bloom_weight: f32,
  pad: f32,
  clear_color: vec4<f32>, // linear rgb of the theme background
};
@group(0) @binding(0) var scene_tex: texture_2d<f32>;   // resolved HDR rgba16f
@group(0) @binding(1) var bloom_tex: texture_2d<f32>;   // half-res chain result (dummy 1x1 black in Task 7)
@group(0) @binding(2) var lin_sampler: sampler;
@group(0) @binding(3) var<uniform> P: PostU;

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

fn aces(x: vec3<f32>) -> vec3<f32> {
  let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
  return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Exact sRGB piecewise transfer (the display target is non-Srgb Bgra8Unorm).
fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
  let lo = c * 12.92;
  let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
  return select(lo, hi, c > vec3<f32>(0.0031308));
}

@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
  let s = textureSample(scene_tex, lin_sampler, v.uv);
  var col = s.rgb;
  if (P.bloom_weight > 0.0) {
    col += textureSample(bloom_tex, lin_sampler, v.uv).rgb * P.bloom_weight;
  }
  col *= exp2(P.exposure);
  col = aces(col);
  col = (col - vec3<f32>(0.5)) * P.contrast + vec3<f32>(0.5);
  col = clamp(col, vec3<f32>(0.0), vec3<f32>(1.0));
  // background bypass: the PBR pass clears HDR alpha to 0 and scene draws
  // write alpha >= material alpha, so 0.5 separates background from scene.
  let is_bg = s.a < 0.5;
  let out = select(col, clamp(P.clear_color.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<bool>(is_bg));
  return vec4<f32>(srgb_encode(out), 1.0);
}
