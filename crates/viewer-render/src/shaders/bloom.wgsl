// crates/viewer-render/src/shaders/bloom.wgsl
// Bloom chain: one shader serving both stages (Task 10), switched by the
// uniform's threshold sentinel:
//   - THRESHOLD pass (threshold >= 0.0): soft bright-pass over the
//     resolved HDR - pixels below the luminance threshold go to zero,
//     brighter ones keep their excess.
//   - BLUR passes (threshold = -1.0): 9-tap separable gaussian along
//     B.direction, run at half resolution via the ping-pong chain in post.rs.
struct BlurParams { direction: vec2<f32>, threshold: f32, pad: f32 };
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var lin_sampler: sampler;
@group(0) @binding(2) var<uniform> B: BlurParams;

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
const WEIGHTS = array<f32, 5>(0.227027, 0.1945946, 0.1216216, 0.054054, 0.016216);
@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
  // threshold pass runs when B.threshold >= 0.0; blur passes use threshold = -1.0
  if (B.threshold >= 0.0) {
    let c = textureSample(src, lin_sampler, v.uv).rgb;
    let lum = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    let bright = max(c - vec3<f32>(B.threshold), vec3<f32>(0.0));
    let k = select(0.0, 1.0, lum > B.threshold);
    return vec4<f32>(bright * k, 1.0);
  }
  var sum = textureSample(src, lin_sampler, v.uv).rgb * WEIGHTS[0];
  for (var i = 1u; i < 5u; i = i + 1u) {
    let off = B.direction * f32(i);
    sum += textureSample(src, lin_sampler, v.uv + off).rgb * WEIGHTS[i];
    sum += textureSample(src, lin_sampler, v.uv - off).rgb * WEIGHTS[i];
  }
  return vec4<f32>(sum, 1.0);
}
