//! Theme-thumbnail support (UI redesign): a small preview mesh in the style
//! of the original app's lighting-theme swatches (a cone in front of a
//! sphere, standing on the ground plane) plus an offscreen render helper the
//! app uses to produce one thumbnail per lighting theme at startup.

use std::sync::Arc;

use egui_wgpu::wgpu;
use viewer_scene::{Material, Mesh, Scene, Trs};

/// UV sphere: radius, center, rings/segments, pushed into `positions`/`normals`/`indices`.
fn push_sphere(
    out: &mut Mesh,
    center: [f32; 3],
    radius: f32,
    rings: u32,
    segments: u32,
    base_index: u32,
) {
    let mut pos = Vec::new();
    let mut idx = Vec::new();
    for r in 0..=rings {
        let phi = std::f32::consts::PI * r as f32 / rings as f32;
        for s in 0..=segments {
            let theta = 2.0 * std::f32::consts::PI * s as f32 / segments as f32;
            let (sp, cp) = phi.sin_cos();
            let (st, ct) = theta.sin_cos();
            let n = [sp * ct, cp, sp * st];
            pos.push([
                center[0] + radius * n[0],
                center[1] + radius * n[1],
                center[2] + radius * n[2],
            ]);
        }
    }
    for r in 0..rings {
        for s in 0..segments {
            let a = base_index + r * (segments + 1) + s;
            let b = a + segments + 1;
            idx.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    out.normals.extend(pos.iter().map(|p| {
        let n = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        [n[0] / len, n[1] / len, n[2] / len]
    }));
    out.positions.extend(pos);
    out.indices.extend(idx.iter().map(|i| i + base_index));
}

/// Cone (no base cap — it sits on the ground): apex up, centered at `center`.
fn push_cone(
    out: &mut Mesh,
    center: [f32; 3],
    radius: f32,
    height: f32,
    segments: u32,
    base_index: u32,
) {
    let apex = [center[0], center[1] + height, center[2]];
    let mut ring = Vec::new();
    for s in 0..segments {
        let theta = 2.0 * std::f32::consts::PI * s as f32 / segments as f32;
        let (st, ct) = theta.sin_cos();
        ring.push([center[0] + radius * ct, center[1], center[2] + radius * st]);
    }
    let mut pos = Vec::new();
    let mut nrm = Vec::new();
    let mut idx = Vec::new();
    let slope = radius / height;
    let inv = 1.0 / (1.0 + slope * slope).sqrt();
    for s in 0..segments as usize {
        let p0 = ring[s];
        let p1 = ring[(s + 1) % ring.len()];
        let (st, ct) = (2.0 * std::f32::consts::PI * (s as f32 + 0.5) / segments as f32).sin_cos();
        // side normal: outward + up-component
        let n = [ct, slope * inv, st];
        let n = [n[0] * inv, n[1] * inv, n[2] * inv];
        pos.extend_from_slice(&[apex, p1, p0]);
        for _ in 0..3 {
            nrm.push(n);
        }
        let a = base_index + (s * 3) as u32;
        idx.extend_from_slice(&[a, a + 1, a + 2]);
    }
    out.positions.extend(pos);
    out.normals.extend(nrm);
    out.indices.extend(idx);
}

/// The preview model: a cone in front of a sphere, base on the ground plane
/// (the original app's theme-swatch motif).
pub fn scene() -> Arc<Scene> {
    let mut scene = Scene::new("preview".into());
    scene.materials.push(Material {
        base_color: [0.62, 0.62, 0.63, 1.0],
        metallic: 0.1,
        roughness: 0.45,
        ..Material::default_pbr()
    });
    let mut mesh = Mesh {
        positions: vec![],
        normals: vec![],
        uvs: vec![],
        indices: vec![],
        material: 0,
    };
    let sphere_base = mesh.positions.len() as u32;
    push_sphere(&mut mesh, [-0.18, 0.42, -0.1], 0.34, 14, 22, sphere_base);
    let cone_base = mesh.positions.len() as u32;
    push_cone(&mut mesh, [0.16, 0.0, 0.12], 0.3, 0.85, 26, cone_base);
    let node = scene.add_node(scene.root, Trs::identity(), Some("preview".into()));
    let mi = scene.add_mesh(mesh);
    scene.nodes[node].meshes.push(mi);
    scene.bounds = viewer_scene::Aabb {
        min: glam::Vec3::new(-0.52, 0.0, -0.44),
        max: glam::Vec3::new(0.52, 0.94, 0.44),
    };
    Arc::new(scene)
}

/// Offscreen render of one frame into an RGBA image (the thumbnail path —
/// the same chain the viewport uses: shadows, tonemap, everything).
pub fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    params: super::FrameParams,
) -> image::RgbaImage {
    let mut renderer = super::Renderer::new(device, wgpu::TextureFormat::Bgra8Unorm);
    renderer.resize(device, size, size);
    renderer.set_scene(device, queue, scene());
    renderer.queue_frame(params);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.request_screenshot();
    renderer.prepare_render(device, queue, &mut encoder);
    queue.submit(Some(encoder.finish()));
    renderer
        .collect_screenshot(device)
        .expect("thumbnail readback")
}

/// Camera for the thumbnail: fixed pleasant angle framing the preview model
/// (mirrors OrbitCamera's spherical model; viewer-render cannot depend on the
/// app crate).
pub fn camera_view_proj() -> [[f32; 4]; 4] {
    use glam::{Mat4, Vec3};
    let target = Vec3::new(0.0, 0.35, 0.0);
    let el = 18.0f32.to_radians();
    let ro = (-35.0f32).to_radians();
    let dir = Vec3::new(el.cos() * ro.sin(), el.sin(), el.cos() * ro.cos());
    let eye = target + dir * 2.9;
    let view = Mat4::look_at_rh(eye, target, Vec3::Y);
    let proj = Mat4::perspective_rh(35.0f32.to_radians(), 1.0, 0.05, 100.0);
    (proj * view).to_cols_array_2d()
}

/// Camera position matching [`camera_view_proj`] (for specular highlights).
pub fn camera_pos() -> [f32; 3] {
    let el = 18.0f32.to_radians();
    let ro = (-35.0f32).to_radians();
    let target = glam::Vec3::new(0.0, 0.35, 0.0);
    let dir = glam::Vec3::new(el.cos() * ro.sin(), el.sin(), el.cos() * ro.cos());
    (target + dir * 2.9).to_array()
}
