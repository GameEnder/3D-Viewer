//! NIF (NetImmerse/Gamebryo) loader via ByroRedux's parser (all versions:
//! Morrowind 4.x → Oblivion → FO3/FNV → Skyrim → FO4 → Starfield).
//!
//! Extracts mesh geometry (NiTriShape/NiTriStrips lists, FO4+ BSTriShape),
//! UVs, normals, and node-hierarchy transforms (translation + rotation +
//! scale), baked into world-space vertex positions. Bethesda NIFs are Z-up;
//! the root transform carries a -90° X rotation so models appear upright in
//! the viewer's Y-up space. Skinned meshes render in bind pose.

use std::path::Path;

use glam::{Mat3, Mat4, Vec3, Vec4};

use crate::error::LoadError;
use crate::scene::{Material, Mesh, Scene, Trs};

use byroredux_nif::blocks::node::NiNode;
use byroredux_nif::blocks::strip::destrip;
use byroredux_nif::blocks::tri_shape::{BsTriShape, NiTriShape, NiTriShapeData, NiTriStripsData};
use byroredux_nif::scene::NifScene;
use byroredux_nif::types::NiTransform;

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    let bytes = std::fs::read(path).map_err(LoadError::Io)?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let nif = byroredux_nif::parse_nif(&bytes).map_err(|e| LoadError::Parse {
        format: "NIF",
        detail: format!("{e}"),
    })?;

    let mut scene = Scene::new(name);
    scene.materials.push(Material::default_pbr());

    let mut mesh_count = 0usize;
    let mut warnings = Vec::new();

    if nif.truncated {
        warnings.push(format!(
            "file was truncated during parse ({} blocks dropped); \
             geometry may be incomplete",
            nif.dropped_block_count
        ));
    }

    // Z-up (Gamebryo) → Y-up (viewer): rotate -90° about X.
    let root_fix = Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2);

    if let Some(root_idx) = nif.root_index {
        walk(
            &nif,
            root_idx,
            root_fix,
            &mut scene,
            &mut mesh_count,
            &mut warnings,
        );
    } else {
        // No root index: scan every block for geometry as a fallback.
        for i in 0..nif.len() {
            walk(
                &nif,
                i,
                root_fix,
                &mut scene,
                &mut mesh_count,
                &mut warnings,
            );
        }
    }

    if mesh_count == 0 {
        let mut detail = "no mesh geometry found".to_string();
        if !warnings.is_empty() {
            detail = format!("no meshes loaded: {}", warnings.join("; "));
        }
        return Err(LoadError::Parse {
            format: "NIF",
            detail,
        });
    }
    scene.warnings = warnings;
    Ok(scene)
}

/// Recursively walk the block tree from `block_idx`, extracting geometry.
#[allow(clippy::too_many_arguments)]
fn walk(
    nif: &NifScene,
    block_idx: usize,
    parent_world: Mat4,
    scene: &mut Scene,
    mesh_count: &mut usize,
    warnings: &mut Vec<String>,
) {
    // NiTriShape (and NiTriStrips, which shares its serialization):
    // a mesh whose geometry lives in a separate data block.
    if let Some(shape) = nif.get_as::<NiTriShape>(block_idx) {
        let world = parent_world * byro_transform(&shape.av.transform);
        let Some(data_idx) = shape.data_ref.index() else {
            return;
        };
        let added = if let Some(data) = nif.get_as::<NiTriShapeData>(data_idx) {
            push_mesh(
                &shape_name(shape),
                tri_list_mesh(data, world),
                scene,
                warnings,
            )
        } else if let Some(data) = nif.get_as::<NiTriStripsData>(data_idx) {
            push_mesh(&shape_name(shape), strip_mesh(data, world), scene, warnings)
        } else {
            false
        };
        *mesh_count += added as usize;
    }
    // BsTriShape (FO4/Skyrim SE+): geometry stored inline on the block.
    else if let Some(shape) = nif.get_as::<BsTriShape>(block_idx) {
        let world = parent_world * byro_transform(&shape.av.transform);
        let added = push_mesh(
            &shape_name_bs(shape),
            bs_tri_mesh(shape, world),
            scene,
            warnings,
        );
        *mesh_count += added as usize;
    }
    // NiNode: a group node with children.
    else if let Some(node) = nif.get_as::<NiNode>(block_idx) {
        let world = parent_world * byro_transform(&node.av.transform);
        for child_ref in &node.children {
            if let Some(child_idx) = child_ref.index() {
                walk(nif, child_idx, world, scene, mesh_count, warnings);
            }
        }
    }
}

/// Add a mesh (already world-transformed) to the scene under the root node.
/// Returns true when the mesh was accepted.
fn push_mesh(
    node_name: &str,
    mesh: Result<Mesh, String>,
    scene: &mut Scene,
    warnings: &mut Vec<String>,
) -> bool {
    match mesh {
        Ok(mesh) => {
            let node = scene.add_node(scene.root, Trs::identity(), Some(node_name.to_string()));
            let mi = scene.add_mesh(mesh);
            scene.nodes[node].meshes.push(mi);
            true
        }
        Err(e) => {
            if !node_name.is_empty() {
                warnings.push(format!("{node_name}: {e}"));
            } else {
                warnings.push(e);
            }
            false
        }
    }
}

fn shape_name(shape: &NiTriShape) -> String {
    shape
        .av
        .net
        .name
        .as_ref()
        .map(|n| n.to_string())
        .unwrap_or_else(|| "NiTriShape".into())
}

fn shape_name_bs(shape: &BsTriShape) -> String {
    shape
        .av
        .net
        .name
        .as_ref()
        .map(|n| n.to_string())
        .unwrap_or_else(|| "BSTriShape".into())
}

/// NiTriShapeData: triangle-list geometry.
fn tri_list_mesh(data: &NiTriShapeData, world: Mat4) -> Result<Mesh, String> {
    if data.vertices.is_empty() {
        return Err("mesh has no vertices".into());
    }
    if data.triangles.is_empty() {
        return Err("mesh has no triangles".into());
    }

    let positions = transform_positions(&data.vertices, world);
    let normals = transform_normals(&data.normals, world);
    let uvs = data.uv_sets.first().cloned().unwrap_or_default();
    let indices: Vec<u32> = data
        .triangles
        .iter()
        .flat_map(|t| [u32::from(t[0]), u32::from(t[1]), u32::from(t[2])])
        .collect();
    finish_mesh(positions, normals, uvs, indices)
}

/// NiTriStripsData: triangle strips (Morrowind/Oblivion era).
fn strip_mesh(data: &NiTriStripsData, world: Mat4) -> Result<Mesh, String> {
    if data.vertices.is_empty() {
        return Err("mesh has no vertices".into());
    }
    if data.strips.is_empty() {
        return Err("mesh has no triangle strips".into());
    }

    let positions = transform_positions(&data.vertices, world);
    let normals = transform_normals(&data.normals, world);
    let uvs = data.uv_sets.first().cloned().unwrap_or_default();

    // Strip → triangle list (OpenGL convention; degenerates skipped).
    let indices: Vec<u32> = data
        .strips
        .iter()
        .flat_map(|strip| {
            destrip(strip).flat_map(|[a, b, c]| [u32::from(a), u32::from(b), u32::from(c)])
        })
        .collect();
    if indices.is_empty() {
        return Err("all strips degenerate".into());
    }
    finish_mesh(positions, normals, uvs, indices)
}

/// BsTriShape: FO4+ inline geometry.
fn bs_tri_mesh(shape: &BsTriShape, world: Mat4) -> Result<Mesh, String> {
    if shape.vertices.is_empty() {
        return Err("mesh has no vertices".into());
    }
    if shape.triangles.is_empty() {
        return Err("mesh has no triangles".into());
    }

    let positions = transform_positions(&shape.vertices, world);
    let normals = transform_normals(&shape.normals, world);
    let uvs = shape.uvs.clone();
    let indices: Vec<u32> = shape
        .triangles
        .iter()
        .flat_map(|t| [u32::from(t[0]), u32::from(t[1]), u32::from(t[2])])
        .collect();
    finish_mesh(positions, normals, uvs, indices)
}

fn transform_positions(verts: &[byroredux_nif::types::NiPoint3], world: Mat4) -> Vec<[f32; 3]> {
    verts
        .iter()
        .map(|v| {
            let p = world * Vec4::new(v.x, v.y, v.z, 1.0);
            [p.x, p.y, p.z]
        })
        .collect()
}

fn transform_normals(norms: &[byroredux_nif::types::NiPoint3], world: Mat4) -> Vec<[f32; 3]> {
    // Mirror the position path; empty input means "compute flat normals later"
    // and is preserved as empty so the renderer falls back.
    norms
        .iter()
        .map(|n| {
            let d = world * Vec4::new(n.x, n.y, n.z, 0.0);
            let len = d.length().max(1e-6);
            [d.x / len, d.y / len, d.z / len]
        })
        .collect()
}

fn finish_mesh(
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
) -> Result<Mesh, String> {
    let vertex_count = positions.len();
    for &i in &indices {
        if (i as usize) >= vertex_count {
            return Err(format!(
                "triangle index {i} out of range ({vertex_count} vertices)"
            ));
        }
    }
    let normals = if normals.len() == vertex_count {
        normals
    } else {
        vec![[0.0, 0.0, 0.0]; vertex_count] // renderer recomputes when zeroed
    };
    Ok(Mesh {
        positions,
        normals,
        uvs,
        indices,
        material: 0,
        joints: vec![],
        weights: vec![],
    })
}

/// ByroRedux NiTransform (row-major rotation + translation + uniform scale)
/// → glam Mat4 (column-major).
fn byro_transform(t: &NiTransform) -> Mat4 {
    let r = &t.rotation.rows;
    let rot = Mat3::from_cols(
        Vec3::new(r[0][0], r[1][0], r[2][0]),
        Vec3::new(r[0][1], r[1][1], r[2][1]),
        Vec3::new(r[0][2], r[1][2], r[2][2]),
    );
    Mat4::from_translation(Vec3::new(t.translation.x, t.translation.y, t.translation.z))
        * Mat4::from_mat3(rot)
        * Mat4::from_scale(Vec3::splat(t.scale))
}
