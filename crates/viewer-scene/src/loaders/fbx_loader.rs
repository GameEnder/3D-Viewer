//! FBX loader (binary FBX 7.4/7.5 via `fbxcel-dom`).
//!
//! Scope: mesh geometry (control points → polygon indices → fan-triangulated)
//! and diffuse color from connected materials. Normals/UVs are not extracted
//! (the fbxcel-dom layer API is complex and experimental); smooth normals
//! are computed by the normalize step, same as STL. Skinned meshes render
//! in bind pose. FBX ASCII format and FBX ≤6.0 are rejected.

use std::path::Path;

use fbxcel_dom::any::AnyDocument;
use fbxcel_dom::v7400::object::TypedObjectHandle;
use fbxcel_dom::v7400::object::geometry::TypedGeometryHandle;

use crate::error::LoadError;
use crate::scene::{Material, Mesh, Scene, Trs};

/// Load a binary FBX: iterate every geometry object, build one scene node
/// per mesh. Individual mesh failures become warnings (one bad mesh should
/// not sink the rest of the file); zero successful meshes is a hard error.
pub fn load(path: &Path) -> Result<Scene, LoadError> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let doc = match AnyDocument::from_seekable_reader(reader).map_err(|e| LoadError::Parse {
        format: "FBX",
        detail: e.to_string(),
    })? {
        AnyDocument::V7400(_ver, doc) => doc,
        _ => {
            return Err(LoadError::Parse {
                format: "FBX",
                detail: "unsupported FBX version (only binary 7.4/7.5 is supported)".into(),
            });
        }
    };

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut scene = Scene::new(name);
    scene.materials.push(Material::default_pbr());

    let mut mesh_count = 0usize;
    let mut warnings = Vec::new();

    for obj in doc.objects() {
        let TypedObjectHandle::Geometry(TypedGeometryHandle::Mesh(mesh)) = obj.get_typed() else {
            continue;
        };

        match load_mesh(&mesh) {
            Ok((mesh_data, material)) => {
                let mat_idx = if let Some(m) = material {
                    scene.materials.push(m);
                    scene.materials.len() - 1
                } else {
                    0
                };
                let mut m = mesh_data;
                m.material = mat_idx;
                let node_name = mesh.name().map(str::to_owned);
                let node = scene.add_node(scene.root, Trs::identity(), node_name);
                let mi = scene.add_mesh(m);
                scene.nodes[node].meshes.push(mi);
                mesh_count += 1;
            }
            Err(e) => {
                warnings.push(format!("mesh {:?}: {}", mesh.name(), e));
            }
        }
    }

    if mesh_count == 0 {
        return Err(LoadError::Parse {
            format: "FBX",
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

fn load_mesh(
    mesh: &fbxcel_dom::v7400::object::geometry::MeshHandle<'_>,
) -> Result<(Mesh, Option<Material>), String> {
    let pv = mesh
        .polygon_vertices()
        .map_err(|e| format!("polygon vertices: {e}"))?;

    // ── Control points (vertex positions) ──
    let positions: Vec<[f32; 3]> = pv
        .raw_control_points()
        .map_err(|e| format!("control points: {e}"))?
        .map(|p| [p.x as f32, p.y as f32, p.z as f32])
        .collect();
    if positions.is_empty() {
        return Err("mesh has no control points".into());
    }

    // ── Polygon vertex indices ──
    // FBX encodes polygons as a flat array where each polygon's LAST
    // control-point index is the bitwise complement (~) of the real index.
    // Split at negative values, fan-triangulate each polygon.
    let raw = pv.raw_polygon_vertices();
    if raw.is_empty() {
        return Err("mesh has no polygon vertices".into());
    }
    let mut indices: Vec<u32> = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    for &v in raw {
        if v < 0 {
            // End of polygon: the real index is !v (bitwise complement).
            current.push((!v) as u32);
            let n = current.len();
            if n >= 3 {
                let first = current[0];
                for i in 1..n - 1 {
                    indices.extend_from_slice(&[first, current[i], current[i + 1]]);
                }
            }
            current.clear();
        } else {
            current.push(v as u32);
        }
    }
    if !current.is_empty() {
        return Err("unterminated polygon (missing negative end marker)".into());
    }
    if indices.is_empty() {
        return Err("no triangles after triangulation (degenerate polygons?)".into());
    }

    // Normals are computed by the normalize step (smooth normals from
    // accumulated face normals — the same path as STL). UVs are not
    // extracted (fbxcel-dom's layer API is complex/experimental).

    let material = load_material(mesh);

    Ok((
        Mesh {
            positions,
            normals: Vec::new(), // filled by normalize()
            uvs: Vec::new(),
            indices,
            material: 0,
            joints: vec![],
            weights: vec![],
        },
        material,
    ))
}

/// Extract the diffuse color from the mesh's first connected Material object
/// by walking the mesh's object-connection graph. Returns None (→ default
/// gray material) when no material is connected or the color is unreadable.
fn load_material(mesh: &fbxcel_dom::v7400::object::geometry::MeshHandle<'_>) -> Option<Material> {
    use fbxcel_dom::v7400::object::property::loaders::F64Arr3Loader;

    for dest in mesh.destination_objects() {
        let Some(handle) = dest.object_handle() else {
            continue;
        };
        if let TypedObjectHandle::Material(mat) = handle.get_typed() {
            // FBX materials store the diffuse color as `DiffuseColor` (or
            // legacy `Color`) under `Properties70` → `P` nodes.
            if let Some(props) = mat.direct_properties() {
                for name in ["DiffuseColor", "Color"] {
                    if let Some(prop) = props.get_property(name)
                        && let Ok([r, g, b]) = prop.load_value(F64Arr3Loader)
                    {
                        return Some(Material {
                            // FBX diffuse colors are linear by convention;
                            // passed through without sRGB decoding.
                            base_color: [r as f32, g as f32, b as f32, 1.0],
                            ..Material::default_pbr()
                        });
                    }
                }
            }
        }
    }
    None
}
