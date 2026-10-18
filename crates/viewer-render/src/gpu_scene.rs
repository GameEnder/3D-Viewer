// crates/viewer-render/src/gpu_scene.rs
//! CPU scene -> GPU upload.
//!
//! The upload deliberately DE-INDEXES every mesh into a flat triangle soup:
//! each vertex gets a barycentric coordinate baked in (`BARY`), which is
//! what lets pbr.wgsl draw wireframe edges and per-triangle effects purely
//! in the fragment shader — no topology buffers, no extra passes.
//!
//! One `GpuMesh` per CPU mesh, one draw entry `(mesh, node)` per node-mesh
//! pairing (glTF-style: the same mesh instanced on two nodes draws twice,
//! each with its node's world matrix in the instance slot).

use std::sync::Arc;

use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;
use viewer_scene::{AlphaMode, Image, Scene};

use crate::uniforms::{BARY, MaterialU, Vertex};

/// Per-skinned-mesh attachment: everything the CPU skinning pass needs to
/// re-derive the triangle soup from the shared vertices each frame. The
/// soup layout is deterministic (same triangle walk as `upload`), so the
/// skinning pass re-derives it rather than storing per-corner data.
pub struct GpuSkin {
    /// Skin index into `scene.skins`.
    pub skin_index: usize,
    /// Per-shared-vertex joint indices and weights (from the CPU Mesh).
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
}

/// One uploaded mesh: vertex/index buffers plus the draw-time material
/// slot and per-material flags the render pass sorts on (`blend` meshes
/// go to the transparent pass, `double_sided` disables backface culling).
pub struct GpuMesh {
    pub vertex_buf: wgpu::Buffer,
    pub index_buf: wgpu::Buffer,
    pub index_count: u32,
    pub material: usize,
    pub blend: bool, // material alpha_mode == Blend
    pub double_sided: bool,
    /// Present on skinned meshes: CPU-skinning attachment (see module docs).
    pub skin: Option<GpuSkin>,
    /// Index into `scene.meshes` of the CPU mesh this draw was built from
    /// (the skinning pass reads its shared vertices).
    pub cpu_mesh: usize,
}

/// One uploaded material: a ready-to-bind group holding the uniform
/// buffer, all texture views (or shared fallbacks), and the sampler.
pub struct GpuMaterial {
    pub bind_group: wgpu::BindGroup,
}

/// The full uploaded scene. `draws` fixes the draw order once so the render
/// pass is a simple loop; `scene` keeps the CPU data alive for ray-free
/// CPU-side queries (stats, bounds).
pub struct GpuScene {
    pub meshes: Vec<GpuMesh>,
    pub materials: Vec<GpuMaterial>,
    pub scene: Arc<Scene>,
    /// (mesh_id, node_id) per draw
    pub draws: Vec<(usize, usize)>,
}

/// Interleaved vertex layout matching `Vertex`: pos(0) normal(1) uv(2)
/// barycentric(3), 44 bytes per vertex.
pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: 44,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                format: wgpu::VertexFormat::Float32x3,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                offset: 12,
                format: wgpu::VertexFormat::Float32x3,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                offset: 24,
                format: wgpu::VertexFormat::Float32x2,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                offset: 32,
                format: wgpu::VertexFormat::Float32x3,
                shader_location: 3,
            },
        ],
    }
}

/// Upload all meshes: de-index to a triangle soup (each triangle's three
/// corners get barycentrics [1,0,0]/[0,1,0]/[0,0,1]) and rebuild a
/// sequential index buffer. Missing per-vertex data falls back safely
/// (up-facing normal, zero UV) rather than failing the upload.
pub fn upload(device: &wgpu::Device, _queue: &wgpu::Queue, scene: Arc<Scene>) -> GpuScene {
    let mut meshes = Vec::new();
    let mut draws = Vec::new();
    for (node_id, node) in scene.nodes.iter().enumerate() {
        for &mi in &node.meshes {
            let m = &scene.meshes[mi];
            // de-index into triangle soup so each triangle carries barycentric
            let tri_iter: Box<dyn Iterator<Item = [usize; 3]>> = if m.indices.is_empty() {
                Box::new((0..m.positions.len() / 3).map(|t| [t * 3, t * 3 + 1, t * 3 + 2]))
            } else {
                Box::new(
                    m.indices
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|c| [c[0] as usize, c[1] as usize, c[2] as usize]),
                )
            };
            let mut verts = Vec::new();
            let mut indices = Vec::new();
            // Skinned meshes remember which shared vertex every soup
            // vertex came from, so the per-frame CPU skinning pass can
            // rewrite the buffer (positions + normals move; UVs and
            // barycentrics are per-corner and stay).
            let skinned = !m.joints.is_empty() && node.skin.is_some();
            for tri in tri_iter {
                for (k, vi) in tri.iter().enumerate() {
                    verts.push(Vertex {
                        pos: m.positions[*vi],
                        normal: m.normals.get(*vi).copied().unwrap_or([0.0, 1.0, 0.0]),
                        uv: m.uvs.get(*vi).copied().unwrap_or([0.0, 0.0]),
                        bary: BARY[k],
                    });
                    indices.push(indices.len() as u32);
                }
            }
            let skin = if skinned {
                Some(GpuSkin {
                    skin_index: node.skin.unwrap(),
                    joints: m.joints.clone(),
                    weights: m.weights.clone(),
                })
            } else {
                None
            };
            let mat = scene.materials.get(m.material);
            let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vertices"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("indices"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            });
            meshes.push(GpuMesh {
                vertex_buf,
                index_buf,
                index_count: indices.len() as u32,
                material: m.material,
                blend: matches!(mat.map(|m| &m.alpha_mode), Some(AlphaMode::Blend)),
                double_sided: mat.map(|m| m.double_sided).unwrap_or(false),
                skin,
                cpu_mesh: mi,
            });
            draws.push((meshes.len() - 1, node_id));
        }
    }
    GpuScene {
        meshes,
        materials: Vec::new(),
        scene,
        draws,
    }
}

/// Shared sampler + fallback views needed to build material bind groups.
pub struct MaterialResources<'a> {
    pub bgl: &'a wgpu::BindGroupLayout,
    pub lin_sampler: &'a wgpu::Sampler,
    pub fallback_base: &'a wgpu::TextureView,
    pub fallback_mr: &'a wgpu::TextureView,
    pub fallback_normal: &'a wgpu::TextureView,
    pub fallback_emissive: &'a wgpu::TextureView,
}

/// Upload one bind group per material. Absent textures borrow the shared
/// 1x1 fallback views (white for base/MR so factors pass through,
/// flat-blue for normals, black for emissive) — the shader never needs
/// branches for missing textures.
pub fn upload_materials(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &Scene,
    res: &MaterialResources<'_>,
) -> Vec<GpuMaterial> {
    scene
        .materials
        .iter()
        .map(|m| {
            let (alpha_cutoff, mode) = match m.alpha_mode {
                AlphaMode::Opaque => (0.0, 0u32),
                AlphaMode::Mask(c) => (c, 1u32),
                AlphaMode::Blend => (0.0, 2u32),
            };
            let u = MaterialU {
                base_color: m.base_color,
                params: [m.metallic, m.roughness, alpha_cutoff, 0.0],
                emissive: [m.emissive[0], m.emissive[1], m.emissive[2], 0.0],
                flags: [
                    u32::from(m.base_color_texture.is_some()),
                    u32::from(m.metallic_roughness_texture.is_some()),
                    u32::from(m.normal_map.is_some()),
                    u32::from(m.emissive_texture.is_some()),
                ],
                mode: [mode, 0, 0, 0],
            };
            let ub = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("material_u"),
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            // glTF color textures are sRGB-encoded; MR/normal maps are data.
            // Absent textures share the renderer's 1x1 fallback views.
            let base = match &m.base_color_texture {
                Some(img) => upload_texture(device, queue, Some(img), [255, 255, 255, 255], true),
                None => res.fallback_base.clone(),
            };
            let mr = match &m.metallic_roughness_texture {
                Some(img) => upload_texture(device, queue, Some(img), [255, 255, 255, 255], false),
                None => res.fallback_mr.clone(),
            };
            let normal = match &m.normal_map {
                Some(img) => upload_texture(device, queue, Some(img), [128, 128, 255, 255], false),
                None => res.fallback_normal.clone(),
            };
            let emissive = match &m.emissive_texture {
                Some(img) => upload_texture(device, queue, Some(img), [0, 0, 0, 255], true),
                None => res.fallback_emissive.clone(),
            };
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("material_bg"),
                layout: res.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: ub.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&base),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&mr),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&normal),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&emissive),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(res.lin_sampler),
                    },
                ],
            });
            GpuMaterial { bind_group }
        })
        .collect()
}

/// Create a 2D RGBA texture from an image (or a 1x1 solid fallback pixel)
/// and upload its pixels. `srgb` picks the view format: color textures are
/// sRGB (the GPU decodes on sample), data textures (MR/normal) stay linear
/// so their numeric channels survive.
pub fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    img: Option<&Image>,
    fallback: [u8; 4],
    srgb: bool,
) -> wgpu::TextureView {
    let format = if srgb {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    };
    let (w, h, data): (u32, u32, Vec<u8>) = match img {
        Some(i) => (i.width.max(1), i.height.max(1), i.rgba.clone()),
        None => (1, 1, fallback.to_vec()),
    };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("material_tex"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // wgpu 30 requires COPY_DST usage for write_texture; the texture above
    // carries it. queue.write_texture returns () in wgpu 30.
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    tex.create_view(&Default::default())
}
