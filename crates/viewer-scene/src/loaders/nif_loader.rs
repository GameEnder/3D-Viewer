//! NIF (NetImmerse/Gamebryo) loader via the `nif` crate (targets version 20.0.0.4).
//!
//! Extracts mesh geometry (NiTriShape + NiTriStrips, both expanded to triangle
//! lists), UVs, normals, and diffuse color from NiMaterialProperty. Node
//! hierarchy transforms (translation + rotation + scale) are composed from
//! the root and baked into the vertex positions. Skinned meshes render in
//! bind pose. Texture files are not loaded (diffuse color only).

use std::path::Path;

use glam::{Mat3, Mat4, Vec3, Vec4};

use crate::error::LoadError;
use crate::scene::{AlphaMode, Material, Mesh, Scene, Trs};

/// Check the NIF magic string + version and give a precise error for
/// unsupported versions. The `nif` crate targets version 20.0.0.4
/// (magic "Gamebryo File Format, Version 20.0.0.4"); the much more common
/// "NetImmerse File Format" magic (versions up to 10.x) is NOT supported
/// by the crate, and we say so instead of dumping a binrw backtrace.
fn check_nif_version(bytes: &[u8]) -> Result<(), LoadError> {
    let magic = &bytes[..bytes.len().min(28)];

    if magic.starts_with(b"NetImmerse File Format") {
        // Extract the version string for a helpful message.
        let version_str = String::from_utf8_lossy(magic)
            .split("Version ")
            .nth(1)
            .unwrap_or("unknown")
            .to_string();
        return Err(LoadError::Parse {
            format: "NIF",
            detail: format!(
                "NetImmerse NIF (version {version_str}) is not supported — this viewer \
                 only supports Gamebryo NIF 20.0.0.4 or later"
            ),
        });
    }

    if !magic.starts_with(b"Gamebryo File Format") {
        return Err(LoadError::Parse {
            format: "NIF",
            detail: "not a valid NIF file (bad magic)".into(),
        });
    }

    // Check the specific version after the magic (bytes 28..32 = version u32).
    if let Some(version_bytes) = bytes.get(28..32) {
        let version = u32::from_le_bytes([
            version_bytes[0],
            version_bytes[1],
            version_bytes[2],
            version_bytes[3],
        ]);
        // The crate asserts version == 0x14000004 (20.0.0.4).
        if version != 0x1400_0004 {
            let v1 = (version >> 24) & 0xFF;
            let v2 = (version >> 16) & 0xFF;
            let v3 = (version >> 8) & 0xFF;
            let v4 = version & 0xFF;
            return Err(LoadError::Parse {
                format: "NIF",
                detail: format!(
                    "NIF version {v1}.{v2}.{v3}.{v4} is not supported — \
                     this viewer only supports version 20.0.0.4"
                ),
            });
        }
    }

    Ok(())
}

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    let bytes = std::fs::read(path)?;

    // Detect the NIF version from the magic string BEFORE calling the parser,
    // so we can give a precise error message for the common mismatch
    // (older NetImmerse files vs the Gamebryo 20.0 the crate targets).
    check_nif_version(&bytes)?;

    let mut cursor = std::io::Cursor::new(bytes);
    let nif = nif::Nif::parse(&mut cursor).map_err(|e| LoadError::Parse {
        format: "NIF",
        detail: e.to_string(),
    })?;

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut scene = Scene::new(name);
    scene.materials.push(Material::default_pbr());

    let mut mesh_count = 0usize;
    let mut warnings = Vec::new();

    // Walk the block tree from roots, accumulating world transforms.
    let root_indices: Vec<usize> = nif.roots().map(|(i, _)| i).collect();
    for root_idx in root_indices {
        collect_shapes(
            &nif,
            root_idx,
            Mat4::IDENTITY,
            &mut scene,
            &mut mesh_count,
            &mut warnings,
        );
    }

    if mesh_count == 0 {
        return Err(LoadError::Parse {
            format: "NIF",
            detail: if warnings.is_empty() {
                "no mesh geometry found".into()
            } else {
                format!("no meshes loaded: {}", warnings.join("; "))
            },
        });
    }
    scene.warnings = warnings;
    Ok(scene)
}

/// Recursively walk the block tree, extracting geometry with accumulated world transforms.
fn collect_shapes(
    nif: &nif::Nif,
    block_idx: usize,
    parent_world: Mat4,
    scene: &mut Scene,
    mesh_count: &mut usize,
    warnings: &mut Vec<String>,
) {
    let Some(block) = nif.blocks.get(block_idx) else {
        return;
    };

    // Compute this block's world transform = parent_world * local_transform.
    let local = local_transform(block);
    let world = parent_world * local;

    // Skip hidden shapes.
    if let Some(av) = block.av_object()
        && av.is_hidden()
    {
        return;
    }

    // Extract geometry from NiTriShape / NiTriStrips.
    if block.geometry().is_some()
        && let Some((data, triangles)) = block.triangles(&nif.blocks)
    {
        match build_mesh(nif, block, data, &triangles, world) {
            Ok((mesh, material)) => {
                let mat_idx = if let Some(m) = material {
                    scene.materials.push(m);
                    scene.materials.len() - 1
                } else {
                    0
                };
                let mut m = mesh;
                m.material = mat_idx;
                let node_name = block
                    .object_net()
                    .map(|o| o.name.to_string_lossy().into_owned());
                let node = scene.add_node(scene.root, Trs::identity(), node_name);
                let mi = scene.add_mesh(m);
                scene.nodes[node].meshes.push(mi);
                *mesh_count += 1;
            }
            Err(e) => {
                let shape_name = block
                    .object_net()
                    .map(|o| o.name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                warnings.push(format!("shape '{shape_name}': {e}"));
            }
        }
    }

    // Recurse into children.
    if let Some(child_refs) = block.child_refs() {
        for child_ref in child_refs {
            if let Some(child_idx) = child_ref.index() {
                collect_shapes(nif, child_idx, world, scene, mesh_count, warnings);
            }
        }
    }
}

/// Build the local transform matrix from a block's NiAvObject fields.
/// The NIF rotation is a row-major 3×3 matrix stored as a flat [f32; 9]:
/// [m11 m12 m13, m21 m22 m23, m31 m32 m33].
fn local_transform(block: &nif::blocks::Block) -> Mat4 {
    let Some(av) = block.av_object() else {
        return Mat4::IDENTITY;
    };
    let t = Vec3::new(av.translation.x, av.translation.y, av.translation.z);
    let rm = &av.rotation.row_major;
    // glam Mat3 columns from row-major NIF data:
    // col_x = (m11, m21, m31) = elements [0], [3], [6]
    // col_y = (m12, m22, m32) = elements [1], [4], [7]
    // col_z = (m13, m23, m33) = elements [2], [5], [8]
    let rot = Mat3::from_cols(
        Vec3::new(rm[0], rm[3], rm[6]),
        Vec3::new(rm[1], rm[4], rm[7]),
        Vec3::new(rm[2], rm[5], rm[8]),
    );
    Mat4::from_translation(t) * Mat4::from_mat3(rot) * Mat4::from_scale(Vec3::splat(av.scale))
}

/// Extract geometry data from NiGeometryData + triangles, applying the world transform.
fn build_mesh(
    nif: &nif::Nif,
    shape_block: &nif::blocks::Block,
    data: &nif::blocks::NiGeometryData,
    triangles: &[nif::common::Triangle],
    world: Mat4,
) -> Result<(Mesh, Option<Material>), String> {
    let positions_raw = data.vertices.as_ref().ok_or("no vertices")?;
    if positions_raw.is_empty() {
        return Err("mesh has no vertices".into());
    }
    if triangles.is_empty() {
        return Err("mesh has no triangles".into());
    }

    // Positions: apply the world transform.
    let positions: Vec<[f32; 3]> = positions_raw
        .iter()
        .map(|v| {
            let p = world * Vec4::new(v.x, v.y, v.z, 1.0);
            [p.x, p.y, p.z]
        })
        .collect();

    // Normals: apply the rotation part and renormalize.
    let normals: Vec<[f32; 3]> = data
        .normals
        .as_ref()
        .map(|ns| {
            ns.iter()
                .map(|n| {
                    let dir = world * Vec4::new(n.x, n.y, n.z, 0.0);
                    let len = dir.length().max(1e-6);
                    [dir.x / len, dir.y / len, dir.z / len]
                })
                .collect()
        })
        .unwrap_or_default();

    // UVs: first UV set if present.
    let uvs: Vec<[f32; 2]> = data
        .uv_sets
        .first()
        .map(|s| s.uvs.iter().map(|uv| [uv.u, uv.v]).collect())
        .unwrap_or_default();

    // Triangle indices (NIF Triangle fields are a, b, c — u16).
    let indices: Vec<u32> = triangles
        .iter()
        .flat_map(|t| [t.a as u32, t.b as u32, t.c as u32])
        .collect();

    // Bounds-check indices.
    let vertex_count = positions.len();
    for &i in &indices {
        if (i as usize) >= vertex_count {
            return Err(format!(
                "triangle index {i} out of range ({vertex_count} vertices)"
            ));
        }
    }

    let material = load_material(nif, shape_block);

    Ok((
        Mesh {
            positions,
            normals,
            uvs,
            indices,
            material: 0, // caller sets this
        },
        material,
    ))
}

/// Walk the shape's property_refs to find a NiMaterialProperty and extract
/// the diffuse color + alpha.
fn load_material(nif: &nif::Nif, shape_block: &nif::blocks::Block) -> Option<Material> {
    let properties = shape_block.properties(&nif.blocks)?;
    for (_, prop) in properties {
        if let nif::blocks::Block::NiMaterialProperty(mat) = prop {
            let diffuse = mat.color_diffuse;
            let base_color = [diffuse.r, diffuse.g, diffuse.b, mat.alpha];
            let alpha_mode = if mat.alpha < 1.0 {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            };
            return Some(Material {
                base_color,
                roughness: (1.0 - mat.glossiness / 1000.0).clamp(0.05, 1.0),
                alpha_mode,
                ..Material::default_pbr()
            });
        }
    }
    None
}
