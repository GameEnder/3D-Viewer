// crates/viewer-scene/src/normalize.rs
use glam::{Mat4, Quat, Vec3};

use crate::aabb::Aabb;
use crate::scene::{Mesh, Scene, Trs};

/// Compose local node transforms into world matrices.
/// `overrides` (indexed by node) replace node TRS for animated nodes.
pub fn world_transforms(scene: &Scene, overrides: &[Option<Trs>]) -> Vec<Mat4> {
    let mut out = vec![Mat4::IDENTITY; scene.nodes.len()];
    for (i, node) in scene.nodes.iter().enumerate() {
        let local = overrides
            .get(i)
            .and_then(|o| o.as_ref())
            .unwrap_or(&node.transform)
            .to_mat4();
        out[i] = match node.parent {
            Some(p) => out[p] * local,
            None => local,
        };
    }
    out
}

/// Smooth per-vertex normals from face normals (only when normals are absent).
pub fn compute_normals(mesh: &mut Mesh) {
    if !mesh.normals.is_empty() || mesh.positions.is_empty() {
        return;
    }
    let mut acc = vec![Vec3::ZERO; mesh.positions.len()];
    let mut face = |a: usize, b: usize, c: usize| {
        let pa = Vec3::from(mesh.positions[a]);
        let ab = Vec3::from(mesh.positions[b]) - pa;
        let ac = Vec3::from(mesh.positions[c]) - pa;
        let n = ab.cross(ac);
        for &i in &[a, b, c] {
            acc[i] += n;
        }
    };
    if mesh.indices.is_empty() {
        for t in 0..mesh.positions.len() / 3 {
            face(t * 3, t * 3 + 1, t * 3 + 2);
        }
    } else {
        for t in mesh.indices.as_chunks::<3>().0 {
            face(t[0] as usize, t[1] as usize, t[2] as usize);
        }
    }
    mesh.normals = acc
        .into_iter()
        .map(|n| {
            if n.length_squared() < 1e-12 {
                [0.0, 1.0, 0.0]
            } else {
                n.normalize().to_array()
            }
        })
        .collect();
}

/// Mirrors the original's `Model.PositionAlignBase`: unit bounding sphere,
/// model base on the ground plane (y=0), centered on x/z.
pub fn normalize(scene: &mut Scene) {
    for mesh in &mut scene.meshes {
        compute_normals(mesh);
    }
    // fold meshes under their node world transforms:
    let world = world_transforms(scene, &[]);
    let mut all = Vec::new();
    for (node_idx, node) in scene.nodes.iter().enumerate() {
        for &mi in &node.meshes {
            let m = &scene.meshes[mi];
            for p in &m.positions {
                all.push((world[node_idx] * glam::Vec4::new(p[0], p[1], p[2], 1.0)).truncate());
            }
        }
    }
    // guard: Aabb::from_points on an empty iterator yields an inverted box
    if all.is_empty() {
        scene.bounds = Aabb {
            min: Vec3::splat(-0.5),
            max: Vec3::splat(0.5),
        };
        return;
    }
    let bounds = Aabb::from_points(all.iter().map(|v| v.to_array()));
    let center = bounds.center();
    let radius = bounds.bounding_sphere_radius();
    let s = 1.0 / radius;
    // root transform: scale to unit sphere, then place base on ground and center x/z
    let translation = Vec3::new(-center.x * s, -bounds.min.y * s, -center.z * s);
    scene.nodes[scene.root].transform = Trs {
        translation,
        rotation: Quat::IDENTITY,
        scale: Vec3::splat(s),
    };
    // Recompute bounds in the FINAL coordinate space (with the fitted root
    // transform applied) — these are the bounds the camera frames against.
    let world = world_transforms(scene, &[]);
    let mut all = Vec::new();
    for (node_idx, node) in scene.nodes.iter().enumerate() {
        for &mi in &node.meshes {
            let m = &scene.meshes[mi];
            for p in &m.positions {
                all.push((world[node_idx] * glam::Vec4::new(p[0], p[1], p[2], 1.0)).truncate());
            }
        }
    }
    scene.bounds = Aabb::from_points(all.iter().map(|v| v.to_array()));
}

#[cfg(test)]
mod tests {
    use crate::normalize::{compute_normals, normalize, world_transforms};
    use crate::scene::{Mesh, Scene, Trs};
    use glam::{Vec3, Vec4};

    fn mesh_cube() -> Mesh {
        Mesh {
            positions: vec![
                [-0.5; 3],
                [0.5, -0.5, -0.5],
                [-0.5, 0.5, -0.5],
                [0.5, 0.5, 0.5],
            ],
            normals: vec![],
            uvs: vec![],
            indices: vec![0, 1, 2, 1, 3, 2],
            material: 0,
            joints: vec![],
            weights: vec![],
        }
    }

    #[test]
    fn normalize_unit_sphere_base_on_ground() {
        let mut scene = Scene::new("t".into());
        scene.materials.push(crate::scene::Material::default_pbr());
        let mi = scene.add_mesh(mesh_cube());
        scene.nodes[0].meshes.push(mi);
        normalize(&mut scene);
        // bounding sphere radius must be ~0.866 (half-diagonal of unit cube) * scale(1/0.866)=1.0
        assert!((scene.bounds.bounding_sphere_radius() - 1.0).abs() < 1e-4);
        // base of the model sits at y=0, centered on x/z
        assert!(scene.bounds.min.y.abs() < 1e-5);
        assert!(scene.bounds.center().x.abs() < 1e-5);
        assert!(scene.bounds.center().z.abs() < 1e-5);
    }

    #[test]
    fn world_transforms_composes_hierarchy() {
        let mut scene = Scene::new("t".into());
        let parent = scene.add_node(
            scene.root,
            Trs {
                translation: Vec3::new(1.0, 0.0, 0.0),
                ..Trs::identity()
            },
            None,
        );
        let _child = scene.add_node(
            parent,
            Trs {
                translation: Vec3::new(0.0, 2.0, 0.0),
                ..Trs::identity()
            },
            None,
        );
        let w = world_transforms(&scene, &[]);
        let c = w[2] * Vec4::new(0.0, 0.0, 0.0, 1.0);
        assert!((c.x - 1.0).abs() < 1e-6 && c.y == 2.0);
    }

    #[test]
    fn compute_normals_accumulates_smooth() {
        let mut m = Mesh {
            positions: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            normals: vec![],
            uvs: vec![],
            indices: vec![],
            material: 0,
            joints: vec![],
            weights: vec![],
        };
        compute_normals(&mut m);
        let n = m.normals[0];
        let inv = 1.0 / 3f32.sqrt();
        assert!(
            (n[0] - inv).abs() < 1e-5 && (n[1] - inv).abs() < 1e-5 && (n[2] - inv).abs() < 1e-5
        );
    }
}
