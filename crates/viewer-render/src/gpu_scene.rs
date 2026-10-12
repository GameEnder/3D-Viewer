// crates/viewer-render/src/gpu_scene.rs
use std::sync::Arc;

use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;
use viewer_scene::{AlphaMode, Image, Scene};

use crate::uniforms::{BARY, MaterialU, Vertex};

pub struct GpuMesh {
    pub vertex_buf: wgpu::Buffer,
    pub index_buf: wgpu::Buffer,
    pub index_count: u32,
    pub material: usize,
    pub blend: bool, // material alpha_mode == Blend
    pub double_sided: bool,
}

pub struct GpuMaterial {
    pub bind_group: wgpu::BindGroup,
}

pub struct GpuScene {
    pub meshes: Vec<GpuMesh>,
    pub materials: Vec<GpuMaterial>,
    pub scene: Arc<Scene>,
    /// (mesh_id, node_id) per draw
    pub draws: Vec<(usize, usize)>,
}

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

/// Build one uniform buffer + bind group per scene material.
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
