// crates/viewer-render/src/uniforms.rs
//! CPU-side mirrors of the WGSL uniform structs. Field offsets must match the
//! shader declarations byte-for-byte (see shaders/*.wgsl in this crate).

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GlobalsU {
    pub view_proj: [[f32; 4]; 4], // 0
    pub camera_pos: [f32; 4],     // 64  xyz + unused
    pub env_color: [f32; 4],      // 80  rgb + intensity
    pub flags: [u32; 4], // 96  x=wireframe, y=grid, z=exposure bits, w=contrast bits (ground.wgsl)
    pub pad: [f32; 4],   // 112 x=ground opacity, yzw=background rgb linear (ground.wgsl)
} // 128

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LightU {
    pub pos: [f32; 4],   // xyz + intensity
    pub color: [f32; 4], // rgb + unused
    pub view_proj: [[f32; 4]; 4],
    pub shadow: [f32; 4], // x=shadow enabled flag, y=texel size, zw unused
} // 112 bytes (16-aligned)

pub const LIGHT_COUNT: usize = 3;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ModelU {
    pub model: [[f32; 4]; 4],
    pub normal_mat: [[f32; 4]; 4],
} // 128

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MaterialU {
    pub base_color: [f32; 4],
    pub params: [f32; 4],   // metallic, roughness, alpha_cutoff, unused
    pub emissive: [f32; 4], // rgb + unused
    pub flags: [u32; 4],    // has_base, has_mr, has_normal, has_emissive
    pub mode: [u32; 4],     // x = alpha_mode (0 opaque, 1 mask, 2 blend)
} // 80

/// Post-process uniforms.
///
/// Task 7 adaptation: `clear_color` (linear rgb of the theme background) was
/// added so post.wgsl can output the untouched theme color for background
/// pixels (HDR alpha < 0.5). ACES + contrast would otherwise crush the dark
/// clear color to black, contradicting the ported constant rgb(69,74,80).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PostU {
    pub exposure: f32,
    pub contrast: f32,
    pub bloom_weight: f32,
    pub pad: f32,
    pub clear_color: [f32; 4], // rgb linear + unused
} // 32

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub bary: [f32; 3],
} // 44 bytes, no padding
pub const BARY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ShadowVpU {
    pub view_proj: [[f32; 4]; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_stride_matches_gpu_layout() {
        assert_eq!(size_of::<Vertex>(), 44);
    }

    #[test]
    fn light_array_stride_is_16_aligned() {
        assert_eq!(size_of::<LightU>(), 112);
        assert_eq!(size_of::<LightU>() * LIGHT_COUNT, 336);
    }

    #[test]
    fn post_u_carries_clear_color() {
        assert_eq!(size_of::<PostU>(), 32);
    }
}
