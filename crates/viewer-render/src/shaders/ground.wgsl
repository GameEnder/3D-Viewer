// crates/viewer-render/src/shaders/ground.wgsl
//
// Ground disc: soft radial-gradient disc (radius 2.6 model units, mesh built
// CPU-side in world space) drawn after opaque meshes and before blend meshes.
//
// Binding contract: same group 0 as pbr.wgsl (globals / lights / shadow maps /
// comparison sampler). Group 2 (model) is bound with a constant identity
// matrix per the brief but intentionally unused — the disc vertices are
// pre-built in world space.
//
// Globals extensions used here (written per frame in Renderer::prepare_render,
// no layout change — same 128-byte uniform):
//   flags.z = f32::to_bits(exposure), flags.w = f32::to_bits(contrast)
//   pad.x   = ground opacity (RenderSettings.ground_opacity)
//   pad.yzw = background color, LINEAR (RenderSettings.clear_color)
//
// Task 8 adaptations over the brief (mechanism matches Task 7's pbr.wgsl
// alpha_out fix, see task-7-report.md §Fix Round 1):
//
// 1. Marker alpha. post.wgsl classifies HDR alpha < 0.5 as background and
//    discards the fragment rgb entirely. The brief's `vec4(col, ground_alpha)`
//    (alpha ≤ 0.4) would make the disc invisible over the background. The
//    output alpha is therefore floored at 0.5 (marker) while the visual
//    opacity (pad.x * radial falloff) is preserved by pre-compensating rgb for
//    the SrcAlpha blend factor the marker forces: with ALPHA_BLENDING
//    (SrcAlpha, OneMinusSrcAlpha) the blended result still equals
//    va*col + (1-va)*background over the clear color, for any va.
//
// 2. Fade-band survival. post.wgsl tonemaps scene pixels with ACES and
//    (c-0.5)*contrast+0.5, which clamps anything dimmer than the background to
//    pure black — the brief's falloff-to-zero would end in a black ring at the
//    disc rim (HDR composite → bg ≈ 0.06 is far below the black-clamp
//    threshold). The fragment lifts the effective opacity so the composite
//    never drops below T_BG, the HDR value post.wgsl maps back to the
//    displayed background color; the rim therefore displays exactly the
//    background and the disc fades out seamlessly. T_BG is derived in-shader
//    from the exposure/contrast carried in flags.zw, so user settings stay
//    honored. (Known limitation: where the disc's own shaded color is dimmer
//    than T_BG — only reachable with non-default lighting/exposure — the
//    composite cannot reach the threshold and those pixels render dark.)

struct Globals {
  view_proj: mat4x4<f32>,
  camera_pos: vec4<f32>,
  env_color: vec4<f32>,   // rgb + intensity
  flags: vec4<u32>,       // x=wireframe, y=grid, z=exposure bits, w=contrast bits
  pad: vec4<f32>,         // x=ground opacity, yzw=background rgb (linear)
};
struct Light {
  pos: vec4<f32>,        // xyz + intensity
  color: vec4<f32>,      // rgb
  view_proj: mat4x4<f32>,
  shadow: vec4<f32>,     // x=enabled, y=texel
};
@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var<uniform> L: array<Light, 3>;
@group(0) @binding(2) var shadow0: texture_depth_2d;
@group(0) @binding(3) var shadow1: texture_depth_2d;
@group(0) @binding(4) var shadow2: texture_depth_2d;
@group(0) @binding(5) var shadow_cmp: sampler_comparison;

struct ModelU { model: mat4x4<f32>, normal_mat: mat4x4<f32> };
@group(2) @binding(0) var<uniform> MU: ModelU;

struct VIn { @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) bary: vec3<f32> };
struct VOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) world: vec3<f32>,
  @location(1) uv: vec2<f32>,
  @location(2) sp0: vec3<f32>,
  @location(3) sp1: vec3<f32>,
  @location(4) sp2: vec3<f32>,
};
@vertex
fn vs_main(v: VIn) -> VOut {
  var o: VOut;
  let world = vec4<f32>(v.pos, 1.0); // identity model
  o.world = world.xyz;
  o.clip = G.view_proj * world;
  o.uv = v.uv;
  let s0 = L[0].view_proj * world; o.sp0 = s0.xyz / max(s0.w, 1e-6);
  let s1 = L[1].view_proj * world; o.sp1 = s1.xyz / max(s1.w, 1e-6);
  let s2 = L[2].view_proj * world; o.sp2 = s2.xyz / max(s2.w, 1e-6);
  return o;
}

fn pcf_g(tex: texture_depth_2d, uv: vec2<f32>, depth: f32, texel: f32) -> f32 {
  var sum = 0.0;
  for (var dy: i32 = -1; dy <= 1; dy++) {
    for (var dx: i32 = -1; dx <= 1; dx++) {
      sum += textureSampleCompare(tex, shadow_cmp, uv + vec2<f32>(f32(dx), f32(dy)) * texel, depth - 0.0015);
    }
  }
  return sum / 9.0;
}

/// HDR input value that post.wgsl (aces after exp2(exposure), then
/// (c-0.5)*contrast+0.5) maps back to `bg` (linear). Anything dimmer clamps
/// to black, so the disc's composite must not sink below this while fading.
fn tonemap_bg_threshold(bg: vec3<f32>, exposure: f32, contrast: f32) -> vec3<f32> {
  let y = clamp((bg - vec3<f32>(0.5)) / max(contrast, 1e-4) + vec3<f32>(0.5), vec3<f32>(0.0), vec3<f32>(1.0));
  // aces_inv(y): solve (a - y*c)z^2 + (y*d - b)z - y*e = 0 (positive root)
  let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
  let qa = max(a - y * c, vec3<f32>(1e-4));
  let qb = y * d - b;
  let disc = max(qb * qb + 4.0 * qa * y * e, vec3<f32>(0.0));
  let z = (qb + sqrt(disc)) / (2.0 * qa);
  return max(z, vec3<f32>(0.0)) / max(exp2(exposure), 1e-4);
}

@fragment
fn fs_main(f: VOut) -> @location(0) vec4<f32> {
  let radial = f.uv.x;
  let ground_alpha = G.pad.x * (1.0 - smoothstep(0.75, 1.0, radial));
  let bg = G.pad.yzw;
  var col = vec3<f32>(0.55, 0.55, 0.55) * G.env_color.rgb * G.env_color.w * 0.35;
  // simple lambert per light with shadows
  let N = vec3<f32>(0.0, 1.0, 0.0);
  for (var i: u32 = 0u; i < 3u; i = i + 1u) {
    let l_vec = L[i].pos.xyz - f.world;
    let dist = max(length(l_vec), 1e-4);
    let Ldir = l_vec / dist;
    let n_dot_l = max(dot(N, Ldir), 0.0);
    var vis = 1.0;
    if (L[i].shadow.x > 0.5 && n_dot_l > 0.0) {
      var uv = vec2<f32>(0.0, 0.0);
      var sp = vec3<f32>(0.0, 0.0, 0.0);
      if (i == 0u) { sp = f.sp0; } else if (i == 1u) { sp = f.sp1; } else { sp = f.sp2; }
      uv = vec2<f32>(sp.x * 0.5 + 0.5, 1.0 - (sp.y * 0.5 + 0.5));
      if (uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0 && sp.z <= 1.0 && sp.z >= 0.0) {
        let texel = L[i].shadow.y;
        if (i == 0u) { vis = pcf_g(shadow0, uv, sp.z, texel); }
        else if (i == 1u) { vis = pcf_g(shadow1, uv, sp.z, texel); }
        else { vis = pcf_g(shadow2, uv, sp.z, texel); }
      }
    }
    col += vec3<f32>(0.55) * L[i].color.rgb * L[i].pos.w * 1.5 / (dist * dist + 1.0) * n_dot_l * vis / 3.14159265;
  }
  // Task 18b: radial pool falloff calibrated to the original app's ground
  // (encoded pool ~119/255 at the center darkening to ~37/255 at the disc
  // edge, ratio ~0.31). Applied before the grid so grid lines sit on the
  // darkened ground.
  col *= mix(1.0, 0.31, smoothstep(0.05, 0.9, radial));
  if (G.flags.y == 1u) { // grid overlay
    let g = abs(fract(f.world.xz * 2.0 - 0.5) - 0.5) / max(fwidth(f.world.xz * 2.0), vec2<f32>(1e-5));
    let line = 1.0 - min(min(g.x, g.y), 1.0);
    col = mix(col, vec3<f32>(0.3, 0.32, 0.36), line * 0.6);
  }

  // Lift the effective opacity where the falloff would sink the composite
  // below the tonemap-survivable threshold (per channel, worst case wins).
  let t_bg = tonemap_bg_threshold(bg, bitcast<f32>(G.flags.z), bitcast<f32>(G.flags.w));
  let t_min = clamp((t_bg - bg) / max(col - bg, vec3<f32>(1e-4)), vec3<f32>(0.0), vec3<f32>(1.0));
  let va = max(ground_alpha, max(t_min.r, max(t_min.g, t_min.b)));
  // Task 7 marker mechanism: floor the output alpha at 0.5 (post.wgsl
  // discards fragments under it) and pre-compensate rgb for the SrcAlpha
  // blend factor the marker forces.
  let a_out = max(va, 0.5);
  let rgb_out = (va * col + (a_out - va) * bg) / a_out;
  return vec4<f32>(rgb_out, a_out);
}
