// crates/viewer-render/src/shaders/pbr.wgsl
//
// Binding contract (must match lib.rs bind group layouts):
//   group 0: 0 globals uniform, 1 lights array, 2-4 shadow depth textures, 5 comparison sampler
//   group 1: 0 material uniform, 1-4 float textures, 5 filtering sampler
//   group 2: 0 model uniform (dynamic offset)
//
// Tuning constants hardcoded here — keep in sync with the named consts in
// crates/viewer-render/src/lib.rs: LIGHT_SCALE = 1.85 (shade_light radiance),
// ENV_SCALE = 0.35 (fs_main env term).

struct Globals {
  view_proj: mat4x4<f32>,
  camera_pos: vec4<f32>,
  env_color: vec4<f32>,   // rgb + intensity
  flags: vec4<u32>,       // x=wireframe
  pad: vec4<f32>,
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

struct MaterialU {
  base_color: vec4<f32>,
  params: vec4<f32>,    // metallic, roughness, alpha_cutoff
  emissive: vec4<f32>,
  flags: vec4<u32>,     // has_base, has_mr, has_normal, has_emissive
  mode: vec4<u32>,      // x=alpha_mode
};
@group(1) @binding(0) var<uniform> M: MaterialU;
@group(1) @binding(1) var base_tex: texture_2d<f32>;
@group(1) @binding(2) var mr_tex: texture_2d<f32>;
@group(1) @binding(3) var normal_tex: texture_2d<f32>;
@group(1) @binding(4) var emissive_tex: texture_2d<f32>;
@group(1) @binding(5) var lin_sampler: sampler;

struct VIn {
  @location(0) pos: vec3<f32>,
  @location(1) normal: vec3<f32>,
  @location(2) uv: vec2<f32>,
  @location(3) bary: vec3<f32>,
};
struct VOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) world: vec3<f32>,
  @location(1) normal: vec3<f32>,
  @location(2) uv: vec2<f32>,
  @location(3) bary: vec3<f32>,
  @location(4) sp0: vec3<f32>,
  @location(5) sp1: vec3<f32>,
  @location(6) sp2: vec3<f32>,
};

@vertex
fn vs_main(v: VIn) -> VOut {
  var o: VOut;
  let world = MU.model * vec4(v.pos, 1.0);
  o.world = world.xyz;
  o.clip = G.view_proj * world;
  o.normal = normalize((MU.normal_mat * vec4(v.normal, 0.0)).xyz);
  o.uv = v.uv;
  o.bary = v.bary;
  let s0 = L[0].view_proj * world;
  let s1 = L[1].view_proj * world;
  let s2 = L[2].view_proj * world;
  o.sp0 = s0.xyz / max(s0.w, 1e-6);
  o.sp1 = s1.xyz / max(s1.w, 1e-6);
  o.sp2 = s2.xyz / max(s2.w, 1e-6);
  return o;
}

fn pcf(tex: texture_depth_2d, uv: vec2<f32>, depth: f32, texel: f32) -> f32 {
  var sum = 0.0;
  for (var dy: i32 = -1; dy <= 1; dy++) {
    for (var dx: i32 = -1; dx <= 1; dx++) {
      sum += textureSampleCompare(tex, shadow_cmp, uv + vec2<f32>(f32(dx), f32(dy)) * texel, depth - 0.0015);
    }
  }
  return sum / 9.0;
}
fn shadow_factor(i: u32, sp: vec3<f32>) -> f32 {
  if (L[i].shadow.x < 0.5) { return 1.0; }
  let uv = vec2<f32>(sp.x * 0.5 + 0.5, 1.0 - (sp.y * 0.5 + 0.5));
  if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || sp.z > 1.0 || sp.z < 0.0) { return 1.0; }
  let texel = L[i].shadow.y;
  if (i == 0u) { return pcf(shadow0, uv, sp.z, texel); }
  if (i == 1u) { return pcf(shadow1, uv, sp.z, texel); }
  return pcf(shadow2, uv, sp.z, texel);
}

fn smith_vis(n_dot_v: f32, n_dot_l: f32, a2: f32) -> f32 {
  let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
  let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
  return 0.5 / max(gv + gl, 1e-4);
}

fn shade_light(light: Light, sp: vec3<f32>, shadow_tex: u32, albedo: vec3<f32>, f0: vec3<f32>,
              rough: f32, N: vec3<f32>, V: vec3<f32>, W: vec3<f32>) -> vec3<f32> {
  let l_vec = light.pos.xyz - W;
  let dist = max(length(l_vec), 1e-4);
  let Ldir = l_vec / dist;
  let n_dot_l = max(dot(N, Ldir), 0.0);
  if (n_dot_l <= 0.0) { return vec3<f32>(0.0); }
  let vis = shadow_factor(shadow_tex, sp);
  if (vis <= 0.0) { return vec3<f32>(0.0); }
  let radiance = light.color.rgb * light.pos.w * 1.85 / (dist * dist + 1.0); // LIGHT_SCALE=1.85
  let H = normalize(V + Ldir);
  let n_dot_h = max(dot(N, H), 0.0);
  let n_dot_v = max(dot(N, V), 1e-4);
  let a = rough * rough;
  let a2 = a * a;
  let dd = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
  let D = a2 / (3.14159265 * dd * dd);
  let Gv = smith_vis(n_dot_v, n_dot_l, a2);
  let F = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - max(dot(H, V), 0.0), 5.0);
  let spec = D * Gv * F;
  let diffuse = albedo * (1.0 - metallic_mask()) / 3.14159265;
  return (diffuse + spec) * radiance * n_dot_l * vis;
}
// NOTE: metallic is passed via a global set in fs_main to keep helper signatures short:
var<private> g_metallic: f32 = 0.0;
fn metallic_mask() -> f32 { return g_metallic; }

fn perturb_normal(N: vec3<f32>, W: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
  let tn = textureSample(normal_tex, lin_sampler, uv).xyz * 2.0 - 1.0;
  // screen-space cotangent frame (no tangents needed)
  // (brief's `let dpdx = dpdx(W)` shadowed the builtin — renamed locals)
  let pdx = dpdx(W);
  let pdy = dpdy(W);
  let duvdx = dpdx(uv);
  let duvdy = dpdy(uv);
  let det = duvdx.x * duvdy.y - duvdy.x * duvdx.y;
  if (abs(det) < 1e-8) { return N; }
  let inv = 1.0 / det;
  let T = normalize(pdx * duvdy.y * inv - pdy * duvdx.y * inv);
  let B = cross(N, T);
  return normalize(T * tn.x + B * tn.y + N * tn.z);
}

@fragment
fn fs_main(f: VOut) -> @location(0) vec4<f32> {
  var base = M.base_color;
  if (M.flags.x > 0u) { base = base * textureSample(base_tex, lin_sampler, f.uv); }
  var metallic = M.params.x;
  var rough = clamp(M.params.y, 0.045, 1.0);
  if (M.flags.y > 0u) {
    let mr = textureSample(mr_tex, lin_sampler, f.uv);
    metallic = mr.b * metallic;
    rough = mr.g * rough;
  }
  g_metallic = metallic;
  var N = normalize(f.normal);
  if (M.flags.z > 0u) { N = perturb_normal(N, f.world, f.uv); }
  if (M.mode.x == 1u && base.a < M.params.z) { discard; }

  let V = normalize(G.camera_pos.xyz - f.world);
  let albedo = base.rgb;
  let f0 = mix(vec3<f32>(0.04), albedo, metallic);
  var col = albedo * G.env_color.rgb * G.env_color.w * 0.35; // ENV_SCALE=0.35
  col += shade_light(L[0], f.sp0, 0u, albedo, f0, rough, N, V, f.world);
  col += shade_light(L[1], f.sp1, 1u, albedo, f0, rough, N, V, f.world);
  col += shade_light(L[2], f.sp2, 2u, albedo, f0, rough, N, V, f.world);
  if (M.flags.w > 0u) {
    col += textureSample(emissive_tex, lin_sampler, f.uv).rgb * M.emissive.rgb * 2.2;
  } else {
    col += M.emissive.rgb;
  }

  if (G.flags.x == 1u) { // wireframe overlay (barycentric edge factor)
    let d = min(f.bary.x, min(f.bary.y, f.bary.z));
    // Distance to the nearest triangle edge IN PIXELS (d changes ~1 unit per
    // pixel across the triangle). Guard against zero derivative (degenerate
    // barycentric interpolation).
    let px = d / max(fwidth(d), 1e-6);
    // ~1.6px anti-aliased band: pixel centers nearest an edge sit at
    // px ~ 0.5-0.9 (MSAA quantization), so the band must extend past 1px or
    // the lines come out too faint to see (measured on the sparse probe).
    let line = 1.0 - smoothstep(0.6, 1.6, px);
    // Darken the shaded surface toward near-black instead of mixing to a
    // flat color: sparse meshes show crisp neutral lines over shading; dense
    // meshes (sub-pixel triangles, line ~ 1 everywhere) read as a darkened
    // wireframe surface that KEEPS its shading structure and hue. The old
    // flat dark-blue mix turned dense models uniformly blue (user-reported).
    col = mix(col, col * vec3<f32>(0.18), line * 0.85);
  }
  // Task 7 adaptation: the resolved HDR buffer's alpha doubles as a background
  // marker for post.wgsl (dst alpha < 0.5 = background -> untonemapped clear
  // color; >= 0.5 = tonemapped scene). Opaque/mask draws write 1.0 (full
  // coverage). Blend draws are floored at 0.5 so their fragment contribution
  // always takes the tonemap branch — a blend fragment that only partially
  // covers the background must NOT be classified as background, or post.wgsl
  // would discard its rgb contribution entirely (e.g. Task 8's ground plane at
  // opacity 0.4 would be invisible over the background).
  let alpha_out = select(1.0, max(base.a, 0.5), M.mode.x == 2u);
  return vec4<f32>(col, alpha_out);
}
