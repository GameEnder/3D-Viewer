// crates/viewer-scene/src/skinning.rs
//! Skeletal skinning: skins deform a mesh by binding each vertex to up to
//! four joints of a skeleton with weights. The deforming matrix per
//! vertex is `sum(weight_i * world_joint_i * inverse_bind_i)`, where the
//! inverse-bind matrices cancel the skeleton's bind-pose transforms so
//! that at the bind pose the skin is exactly the identity.
//!
//! The viewer skins on the CPU each animated frame (viewer-render writes
//! the results into the existing vertex buffer) — no shader changes, and
//! shadows/wireframe stay correct because the geometry itself moves.

use glam::Mat4;

/// One glTF skin: the skeleton. `joints[i]` is a scene-node index (the
/// animated joint node) paired with `inverse_bind[i]`. Mesh
/// `Mesh::joints` values index into THIS list.
#[derive(Clone, Debug, Default)]
pub struct Skin {
    /// Scene-node indices of the joints, in glTF skin order.
    pub joints: Vec<usize>,
    /// Inverse bind matrix per joint (the inverse of the joint's world
    /// transform at bind time).
    pub inverse_bind: Vec<Mat4>,
}

impl Skin {
    /// A skin with missing/short inverse-bind data defaults to identity
    /// matrices (legal in glTF: the bind pose is the node's own frame).
    pub fn new(joints: Vec<usize>, inverse_bind: Vec<Mat4>) -> Self {
        let inverse_bind = if inverse_bind.len() == joints.len() {
            inverse_bind
        } else {
            vec![Mat4::IDENTITY; joints.len()]
        };
        Self {
            joints,
            inverse_bind,
        }
    }
}

/// Compute the per-joint skinning matrices for one skin:
/// `world[node] * inverse_bind[i]`, using world transforms already
/// composed with any animation overrides applied.
///
/// NOTE: these produce world-space skinning. The renderer applies the
/// root node's transform (normalize fit) through the model matrix, so it
/// skins in PRE-root space via [`joint_matrices_local`] instead.
pub fn joint_matrices(skin: &Skin, world: &[Mat4]) -> Vec<Mat4> {
    skin.joints
        .iter()
        .zip(&skin.inverse_bind)
        .map(|(&node, &ib)| world.get(node).copied().unwrap_or(Mat4::IDENTITY) * ib)
        .collect()
}

/// Joint skinning matrices with the ROOT node's transform stripped:
/// `inverse(world[root]) * world[joint] * inverse_bind[i]`. Use when the
/// caller also applies the root transform downstream (the renderer's
/// model matrix) — skinned vertices then stay in pre-root space and the
/// root is applied exactly once.
pub fn joint_matrices_local(skin: &Skin, world: &[Mat4], root: usize) -> Vec<Mat4> {
    let root_inv = world.get(root).copied().unwrap_or(Mat4::IDENTITY).inverse();
    skin.joints
        .iter()
        .zip(&skin.inverse_bind)
        .map(|(&node, &ib)| root_inv * world.get(node).copied().unwrap_or(Mat4::IDENTITY) * ib)
        .collect()
}

/// Skin one mesh's shared vertices: returns (positions, normals).
/// Unskinned meshes (empty `joints`) are returned unchanged (bind pose).
/// Normals are transformed by the weighted matrix's linear part and
/// re-normalized; absent normals (computed later by `normalize`) stay
/// empty.
pub fn skin_mesh(mesh: &crate::scene::Mesh, matrices: &[Mat4]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    if mesh.joints.is_empty() || matrices.is_empty() {
        return (mesh.positions.clone(), mesh.normals.clone());
    }
    let has_normals = mesh.normals.len() == mesh.positions.len();
    let mut positions = Vec::with_capacity(mesh.positions.len());
    let mut normals = Vec::with_capacity(if has_normals { mesh.normals.len() } else { 0 });
    for (vi, &p) in mesh.positions.iter().enumerate() {
        let Some(j) = mesh.joints.get(vi) else {
            positions.push(p);
            if has_normals {
                normals.push(mesh.normals[vi]);
            }
            continue;
        };
        let w = mesh
            .weights
            .get(vi)
            .copied()
            .unwrap_or([1.0, 0.0, 0.0, 0.0]);
        // Weighted sum of the (up to) four bound joint matrices.
        let mut m = Mat4::ZERO;
        for k in 0..4 {
            if w[k] > 0.0
                && let Some(jm) = matrices.get(j[k] as usize)
            {
                m += *jm * w[k];
            }
        }
        let v = m * glam::Vec4::new(p[0], p[1], p[2], 1.0);
        positions.push([v.x, v.y, v.z]);
        if has_normals {
            let tn = m.transform_vector3(glam::Vec3::from(mesh.normals[vi]));
            let len = tn.length().max(1e-6);
            normals.push([tn.x / len, tn.y / len, tn.z / len]);
        }
    }
    (positions, normals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Mesh, Scene, Trs};
    use glam::{Quat, Vec3};

    /// Minimal skinned scene: `joint_count` joint nodes (identity) under
    /// the root, plus a holder node with a 2-vertex skinned mesh bound to
    /// joint 0 (or 50/50 between joints 0 and 1 when blending).
    fn skinned_scene(
        joint_transforms: &[Trs],
        joints: [[u16; 4]; 2],
        weights: [[f32; 4]; 2],
    ) -> (Scene, usize) {
        let mut scene = Scene::new("test".into());
        let mut joint_ids = Vec::new();
        for (i, t) in joint_transforms.iter().enumerate() {
            let id = scene.add_node(scene.root, *t, Some(format!("joint{i}")));
            joint_ids.push(id);
        }
        let joint_count = joint_ids.len();
        scene
            .skins
            .push(Skin::new(joint_ids, vec![Mat4::IDENTITY; joint_count]));
        let skin_idx = scene.skins.len() - 1;
        let holder = scene.add_node(scene.root, Trs::identity(), Some("mesh_node".into()));
        scene.nodes[holder].skin = Some(skin_idx);
        let mesh = Mesh {
            positions: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 2],
            uvs: vec![],
            indices: vec![0, 1, 0],
            material: 0,
            joints: joints.to_vec(),
            weights: weights.to_vec(),
        };
        let mi = scene.add_mesh(mesh);
        scene.nodes[holder].meshes.push(mi);
        (scene, skin_idx)
    }

    /// At the bind pose (identity joints, identity inverse binds),
    /// skinning must return the vertices and normals unchanged.
    #[test]
    fn bind_pose_is_identity() {
        let (scene, skin_idx) = skinned_scene(
            &[Trs::identity()],
            [[0, 0, 0, 0]; 2],
            [[1.0, 0.0, 0.0, 0.0]; 2],
        );
        let world = crate::normalize::world_transforms(&scene, &[]);
        let m = joint_matrices(&scene.skins[skin_idx], &world);
        assert_eq!(m, vec![Mat4::IDENTITY]);
        let (pos, nrm) = skin_mesh(&scene.meshes[0], &m);
        assert_eq!(pos, scene.meshes[0].positions);
        assert_eq!(nrm, scene.meshes[0].normals);
    }

    /// A joint moved after bind (identity bind matrices) drags its fully
    /// weighted vertices along by exactly the joint's world transform.
    #[test]
    fn joint_translation_moves_vertices() {
        let t = Trs {
            translation: Vec3::new(2.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        };
        let (scene, skin_idx) = skinned_scene(&[t], [[0, 0, 0, 0]; 2], [[1.0, 0.0, 0.0, 0.0]; 2]);
        let world = crate::normalize::world_transforms(&scene, &[]);
        let m = joint_matrices(&scene.skins[skin_idx], &world);
        let (pos, nrm) = skin_mesh(&scene.meshes[0], &m);
        assert!((pos[0][0] - 3.0).abs() < 1e-5, "{pos:?}"); // 1 + 2
        assert!((pos[1][0] - 2.0).abs() < 1e-5, "{pos:?}"); // 0 + 2
        assert!(
            (nrm[0][2] - 1.0).abs() < 1e-5,
            "normal unchanged by translation"
        );
    }

    /// 50/50 weights between a moved and an unmoved joint blend halfway.
    #[test]
    fn weights_blend_between_joints() {
        let moved = Trs {
            translation: Vec3::new(4.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        };
        let (scene, skin_idx) = skinned_scene(
            &[Trs::identity(), moved],
            [[0, 1, 0, 0], [0, 1, 0, 0]],
            [[0.5, 0.5, 0.0, 0.0]; 2],
        );
        let world = crate::normalize::world_transforms(&scene, &[]);
        let m = joint_matrices(&scene.skins[skin_idx], &world);
        let (pos, _) = skin_mesh(&scene.meshes[0], &m);
        assert!((pos[0][0] - 3.0).abs() < 1e-5, "{pos:?}"); // 1 + (0+4)/2
        assert!((pos[1][0] - 2.0).abs() < 1e-5, "{pos:?}"); // 0 + (0+4)/2
    }

    /// Unskinned meshes pass through untouched.
    #[test]
    fn unskinned_mesh_is_untouched() {
        let mesh = Mesh {
            positions: vec![[1.0, 2.0, 3.0]],
            normals: vec![[0.0, 1.0, 0.0]],
            uvs: vec![],
            indices: vec![],
            material: 0,
            joints: vec![],
            weights: vec![],
        };
        let (pos, nrm) = skin_mesh(&mesh, &[Mat4::IDENTITY]);
        assert_eq!(pos, mesh.positions);
        assert_eq!(nrm, mesh.normals);
    }
}
