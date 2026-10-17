// crates/viewer-scene/src/loaders/ma_loader.rs
//! Maya ASCII (.ma) loader via the `maya-mel` crate.
//!
//! A `.ma` scene is a stream of MEL commands: `createNode` builds a DAG of
//! `transform` and `mesh` nodes (a mesh shape hangs under a transform via
//! `-p`), `setAttr` writes attributes onto the most recently created node,
//! and `currentUnit` declares the file's units. Geometry lives in the mesh
//! attributes:
//!
//! - `.vt`   `-type "vectorArray"`   shared vertex positions (count + N*3)
//! - `.ed`   `-type "doubleArray"`   edge triplets (start, end, flag; the
//!   count token is the total double count)
//! - `.fc[i]`/`.face` `-type "polyFaces"` one face record per entry:
//!   `f <edgeCount> <signed 1-based edge refs...> [mu <uvSet> <count>
//!   <uv point idx per face-vertex...>] [h <count> <holes...>]`
//!   Negative edge references traverse the edge backwards.
//! - `.uvpt` `-type "float2Array"`   UV points (count + N*2)
//! - `.t`/`.r`/`.s` `-type "double3"` transform translate / rotate (Euler
//!   XYZ, in `currentUnit` angle units — degrees by default) / scale
//!
//! The mesh is emitted as a face-vertex soup (positions duplicated per
//! face, like STL but with UVs), so UV seams stay exact and no
//! vertex-sharing decision has to be made. Ancestor transforms are baked
//! into world space; Maya is Y-up natively, so there is no up-axis fix.
//! Normals are usually absent in `.ma` — `normalize` computes smooth ones,
//! as with STL/FBX. Materials are the default PBR gray (Maya shading
//! networks are out of scope).
//!
//! Known v1 simplifications (warnings are NOT emitted for these):
//! rotate pivots (`.rp`/`.sp`) and non-XYZ rotate orders are ignored,
//! `rename` after creation is not tracked, references and instancing are
//! not followed, and only UV set 0 is read.

use std::path::Path;

use glam::{Quat, Vec3};

use crate::error::LoadError;
use crate::scene::{Material, Mesh, Scene, Trs};

use maya_mel::maya::model::{
    MayaCreateNodeCommand, MayaRawShellItem, MayaSetAttrCommand, MayaSpecializedCommand,
    MayaTopLevelItem,
};
use maya_mel::syntax::SourceView;
use maya_mel::{ParseBudgets, ParseOptions, collect_top_level_facts, parse_file_with_options};

const FMT: &str = "Maya ASCII";

fn bad(detail: &str) -> LoadError {
    LoadError::Parse {
        format: FMT,
        detail: detail.into(),
    }
}

/// Lexical text of a raw shell item (quotes stripped for strings).
fn text<'a>(item: &MayaRawShellItem, src: SourceView<'a>) -> Option<&'a str> {
    item.value_text(src)
}

/// One DAG node while walking the command stream.
struct MaNode {
    name: Option<String>,
    is_mesh: bool,
    parent: Option<String>,
    /// (t, r, s) transform channels (None = identity part).
    t: Option<[f64; 3]>,
    r: Option<[f64; 3]>,
    s: Option<[f64; 3]>,
    /// Shared vertex positions.
    vt: Vec<[f32; 3]>,
    /// Edge list: (start vertex, end vertex), 0-based.
    ed: Vec<(usize, usize)>,
    /// Faces: (signed 1-based edge refs, per-face-vertex UV point indices).
    faces: Vec<(Vec<i64>, Vec<usize>)>,
    /// UV points.
    uvpt: Vec<[f32; 2]>,
}

impl MaNode {
    fn new(name: Option<String>, is_mesh: bool, parent: Option<String>) -> Self {
        Self {
            name,
            is_mesh,
            parent,
            t: None,
            r: None,
            s: None,
            vt: vec![],
            ed: vec![],
            faces: vec![],
            uvpt: vec![],
        }
    }
}

/// Strip Maya dag-path decoration from a node name: `|a|b` -> `a|b`
/// (full-path lookups still match because createNode parents use the
/// same spelling the file does).
fn clean_name(s: &str) -> &str {
    s.strip_prefix('|').unwrap_or(s)
}

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    // Unlimited budgets: real .ma scenes can be huge, and every array is
    // bounds-checked during extraction anyway.
    let options = ParseOptions {
        budgets: ParseBudgets::unlimited(),
        ..ParseOptions::default()
    };
    let parsed = parse_file_with_options(path, options)
        .map_err(|e| bad(&format!("cannot read scene: {e}")))?;
    if let Some(err) = parsed.errors.first() {
        return Err(bad(&format!(
            "parse error at offset {}: {}",
            maya_mel::syntax::range_start(err.range),
            err.message
        )));
    }
    let facts = collect_top_level_facts(&parsed);
    let src = parsed.source_view();

    let mut nodes: Vec<MaNode> = Vec::new();
    // Index of the node a relative `setAttr` targets (last created node).
    let mut current: Option<usize> = None;
    // Rotate channels are degrees unless currentUnit says otherwise.
    let mut angle_in_radians = false;
    let mut warnings: Vec<String> = Vec::new();

    for item in &facts.items {
        let MayaTopLevelItem::Command(cmd) = item else {
            continue;
        };
        match cmd.specialized.as_ref() {
            Some(MayaSpecializedCommand::CurrentUnit(cu)) => {
                if let Some(a) = cu.angle.as_ref().and_then(|i| text(i, src)) {
                    angle_in_radians = a.eq_ignore_ascii_case("radian");
                }
            }
            Some(MayaSpecializedCommand::CreateNode(cn)) => {
                let node = register_node(cn, src, &mut nodes);
                current = Some(node);
            }
            Some(MayaSpecializedCommand::SetAttr(sa)) => {
                apply_set_attr(sa, src, &mut nodes, current, &mut warnings)?;
            }
            _ => {}
        }
    }

    // ---- resolve node name -> index for parent lookups ----
    let by_name: std::collections::HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.name.as_ref().map(|nm| (nm.clone(), i)))
        .collect();

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut scene = Scene::new(name);
    scene.materials.push(Material::default_pbr());
    let mut mesh_count = 0usize;

    for (idx, node) in nodes.iter().enumerate() {
        if !node.is_mesh || node.faces.is_empty() {
            continue;
        }
        // World transform = ancestors' TRS composed root-most first.
        let world = world_transform(idx, &nodes, &by_name, angle_in_radians, &mut warnings);
        match build_mesh(node, world) {
            Ok(mesh) => {
                let n = scene.add_node(scene.root, Trs::identity(), node.name.clone());
                let mi = scene.add_mesh(mesh);
                scene.nodes[n].meshes.push(mi);
                mesh_count += 1;
            }
            Err(e) => warnings.push(format!(
                "mesh {}: {e}",
                node.name.as_deref().unwrap_or("<unnamed>")
            )),
        }
    }

    if mesh_count == 0 {
        let mut detail = "no mesh geometry found".to_string();
        if !warnings.is_empty() {
            detail = format!("no meshes loaded: {}", warnings.join("; "));
        }
        return Err(bad(&detail));
    }
    scene.warnings = warnings;
    Ok(scene)
}

/// Append a node from a `createNode` command and return its index.
fn register_node(
    cn: &MayaCreateNodeCommand,
    src: SourceView<'_>,
    nodes: &mut Vec<MaNode>,
) -> usize {
    let is_mesh = cn
        .node_type
        .as_ref()
        .and_then(|i| i.value_text(src))
        .is_some_and(|t| t == "mesh");
    let name = cn
        .name
        .as_ref()
        .and_then(|i| i.value_text(src))
        .map(|s| clean_name(s).to_owned());
    let parent = cn
        .parent
        .as_ref()
        .and_then(|i| i.value_text(src))
        .map(|s| clean_name(s).to_owned());
    nodes.push(MaNode::new(name, is_mesh, parent));
    nodes.len() - 1
}

/// Apply one `setAttr` to its target node (last created node for relative
/// paths like `.vt`, name-resolved for absolute ones like `shape.vt`).
fn apply_set_attr(
    sa: &MayaSetAttrCommand,
    src: SourceView<'_>,
    nodes: &mut [MaNode],
    current: Option<usize>,
    warnings: &mut Vec<String>,
) -> Result<(), LoadError> {
    let Some(attr_path) = sa.attr_path.as_ref().and_then(|i| text(i, src)) else {
        return Ok(());
    };
    // Absolute form "nodeName.attr" resolves by name; relative ".attr"
    // targets the most recently created node.
    let (target, attr) = match attr_path.rsplit_once('.') {
        Some((node_name, attr)) if !node_name.is_empty() => {
            let name = clean_name(node_name);
            match nodes.iter().position(|n| n.name.as_deref() == Some(name)) {
                Some(i) => (i, attr),
                None => return Ok(()), // unknown target: ignore silently
            }
        }
        _ => match current {
            // Relative paths still carry the leading dot: ".vt" -> "vt".
            Some(i) => (i, attr_path.strip_prefix('.').unwrap_or(attr_path)),
            None => return Ok(()),
        },
    };
    // Split off component suffixes: ".fc[3]" -> "fc".
    let suffix = attr.split('[').next().unwrap_or(attr);

    let ty = sa.type_name.as_ref().and_then(|i| text(i, src));
    let values = sa
        .values
        .iter()
        .filter_map(|i| text(i, src))
        .collect::<Vec<_>>();
    let node = &mut nodes[target];

    match suffix {
        "vt" if ty == Some("vectorArray") => {
            node.vt = parse_vector_array(&values)?;
        }
        "ed" if ty == Some("doubleArray") => {
            node.ed = parse_edge_array(&values)?;
        }
        "uvpt" if ty == Some("float2Array") => {
            node.uvpt = parse_float2_array(&values)?;
        }
        "fc" | "face" if ty == Some("polyFaces") => {
            let face = parse_poly_face(&values)?;
            if let Some(f) = face {
                node.faces.push(f);
            } else {
                warnings.push(format!(
                    "skipping degenerate polyFaces entry on {attr_path}"
                ));
            }
        }
        "t" if ty == Some("double3") => {
            node.t = parse_double3(&values);
        }
        "r" if ty == Some("double3") => {
            node.r = parse_double3(&values);
        }
        "s" if ty == Some("double3") => {
            node.s = parse_double3(&values);
        }
        _ => {}
    }
    Ok(())
}

/// `vectorArray` values: [count, x y z ...] -> positions.
fn parse_vector_array(values: &[&str]) -> Result<Vec<[f32; 3]>, LoadError> {
    let nums = parse_f64s(values, "vertex array")?;
    if nums.is_empty() {
        return Ok(vec![]);
    }
    let count = nums[0] as usize;
    let data = &nums[1..];
    if data.len() < count * 3 {
        return Err(bad(&format!(
            "vertex array declares {count} entries but holds only {}",
            data.len() / 3
        )));
    }
    Ok(data[..count * 3]
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
        .collect())
}

/// `doubleArray` edge values: [totalDoubleCount, (start, end, flag) ...].
fn parse_edge_array(values: &[&str]) -> Result<Vec<(usize, usize)>, LoadError> {
    let nums = parse_f64s(values, "edge array")?;
    if nums.is_empty() {
        return Ok(vec![]);
    }
    let total = nums[0] as usize;
    let data = &nums[1..];
    if data.len() < total || !total.is_multiple_of(3) {
        return Err(bad(&format!(
            "edge array declares {total} doubles but holds {} (needs triplets)",
            data.len()
        )));
    }
    let edges = data[..total]
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| (c[0] as usize, c[1] as usize))
        .collect();
    Ok(edges)
}

/// `float2Array` values: [count, u v ...] -> UV points.
fn parse_float2_array(values: &[&str]) -> Result<Vec<[f32; 2]>, LoadError> {
    let nums = parse_f64s(values, "UV point array")?;
    if nums.is_empty() {
        return Ok(vec![]);
    }
    let count = nums[0] as usize;
    let data = &nums[1..];
    if data.len() < count * 2 {
        return Err(bad(&format!(
            "UV point array declares {count} entries but holds only {}",
            data.len() / 2
        )));
    }
    Ok(data[..count * 2]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| [c[0] as f32, c[1] as f32])
        .collect())
}

/// One polyFaces face record: (signed 1-based edge refs, per-face-vertex
/// UV point indices).
type FaceRecord = (Vec<i64>, Vec<usize>);

/// One polyFaces entry: `f <count> <refs...> [mu <set> <count> <idx...>]`
/// (holes `h <count> <refs...>` are parsed past and ignored). Returns the
/// record, or None for a face with no usable geometry.
fn parse_poly_face(values: &[&str]) -> Result<Option<FaceRecord>, LoadError> {
    let mut refs: Vec<i64> = Vec::new();
    let mut uvs: Vec<usize> = Vec::new();
    let mut i = 0usize;
    while i < values.len() {
        match values[i] {
            "f" => {
                i += 1;
                let count = next_int(values, &mut i, "face edge count")?;
                for _ in 0..count {
                    refs.push(next_int(values, &mut i, "face edge ref")?);
                }
            }
            "mu" => {
                i += 1;
                let _set = next_int(values, &mut i, "mu uv set")?;
                let count = next_int(values, &mut i, "mu count")?;
                for _ in 0..count {
                    uvs.push(next_int(values, &mut i, "mu uv index")? as usize);
                }
            }
            "h" => {
                // Hole: parse past it (v1 renders the face without the hole).
                i += 1;
                let count = next_int(values, &mut i, "hole edge count")?;
                for _ in 0..count {
                    next_int(values, &mut i, "hole edge ref")?;
                }
            }
            other => {
                return Err(bad(&format!("unexpected token {other:?} in polyFaces")));
            }
        }
    }
    if refs.len() < 3 {
        return Ok(None);
    }
    Ok(Some((refs, uvs)))
}

/// `double3` values (no count token).
fn parse_double3(values: &[&str]) -> Option<[f64; 3]> {
    if values.len() < 3 {
        return None;
    }
    let v: [f64; 3] = [
        values[0].parse().ok()?,
        values[1].parse().ok()?,
        values[2].parse().ok()?,
    ];
    Some(v)
}

fn next_int(values: &[&str], i: &mut usize, what: &str) -> Result<i64, LoadError> {
    let raw = values
        .get(*i)
        .ok_or_else(|| bad(&format!("truncated {what}")))?;
    *i += 1;
    raw.parse::<i64>()
        .map_err(|_| bad(&format!("bad {what} {raw:?}")))
}

fn parse_f64s(values: &[&str], what: &str) -> Result<Vec<f64>, LoadError> {
    values
        .iter()
        .map(|v| {
            v.parse::<f64>()
                .map_err(|_| bad(&format!("bad number {v:?} in {what}")))
        })
        .collect()
}

/// Compose the world matrix of `idx` from its transform ancestors (t/r/s
/// per node; pivots and rotate orders are ignored — see module docs).
fn world_transform(
    idx: usize,
    nodes: &[MaNode],
    by_name: &std::collections::HashMap<String, usize>,
    angle_in_radians: bool,
    warnings: &mut Vec<String>,
) -> glam::Mat4 {
    // Collect ancestor transforms root-most first (the mesh node itself
    // carries no transform: its parent transform does).
    let mut chain: Vec<usize> = Vec::new();
    let mut cursor = nodes[idx].parent.as_deref();
    let mut depth = 0;
    while let Some(pname) = cursor {
        if depth > 64 {
            warnings.push("transform hierarchy deeper than 64; truncating".into());
            break;
        }
        match by_name.get(pname) {
            Some(&i) => {
                chain.push(i);
                cursor = nodes[i].parent.as_deref();
            }
            None => break,
        }
        depth += 1;
    }
    chain.reverse();

    let mut world = glam::Mat4::IDENTITY;
    for &i in &chain {
        world *= trs_matrix(&nodes[i], angle_in_radians);
    }
    world
}

/// One node's local TRS matrix. Rotate is Euler XYZ in the file's angle
/// unit (degrees unless currentUnit declared radians); pivots are ignored.
fn trs_matrix(node: &MaNode, angle_in_radians: bool) -> glam::Mat4 {
    let t = node
        .t
        .map(|v| Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32))
        .unwrap_or(Vec3::ZERO);
    let rot = node
        .r
        .map(|v| {
            let to_rad = if angle_in_radians {
                1.0
            } else {
                std::f32::consts::TAU / 360.0
            };
            Quat::from_euler(
                glam::EulerRot::XYZ,
                v[0] as f32 * to_rad,
                v[1] as f32 * to_rad,
                v[2] as f32 * to_rad,
            )
        })
        .unwrap_or(Quat::IDENTITY);
    let s = node
        .s
        .map(|v| Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32))
        .unwrap_or(Vec3::ONE);
    glam::Mat4::from_scale_rotation_translation(s, rot, t)
}

/// Build a face-vertex soup mesh: each face emits its own corners (exact
/// UVs, no seam handling needed), fan-triangulated in place.
fn build_mesh(node: &MaNode, world: glam::Mat4) -> Result<Mesh, String> {
    if node.vt.is_empty() {
        return Err("mesh has no vertex array".into());
    }
    // Bounds-check every edge endpoint once.
    for (a, b) in &node.ed {
        if *a >= node.vt.len() || *b >= node.vt.len() {
            return Err(format!(
                "edge endpoint out of range ({a}, {b} >= {})",
                node.vt.len()
            ));
        }
    }

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();

    for (refs, mu) in &node.faces {
        // Oriented vertex chain: the start vertex of each oriented edge
        // (positive ref = a->b, negative = b->a).
        let mut ring: Vec<usize> = Vec::with_capacity(refs.len());
        for &r in refs {
            let e = r.unsigned_abs() as usize;
            if e == 0 || e > node.ed.len() {
                return Err(format!(
                    "face references edge {r} but mesh has {} edges",
                    node.ed.len()
                ));
            }
            let (a, b) = node.ed[e - 1];
            ring.push(if r > 0 { a } else { b });
        }
        // Face-vertex UVs align with the ring order when mu has enough
        // entries; otherwise the face renders without UVs.
        let face_uvs: Vec<[f32; 2]> = if mu.len() >= ring.len() {
            mu.iter()
                .take(ring.len())
                .map(|&u| node.uvpt.get(u).copied().unwrap_or([0.0; 2]))
                .collect()
        } else {
            vec![[0.0; 2]; ring.len()]
        };
        // Fan-triangulate the ring, duplicating per face: each emitted
        // triangle contributes 3 new vertices.
        for k in 1..ring.len().saturating_sub(1) {
            let tri = [ring[0], ring[k], ring[k + 1]];
            let tri_uvs = [
                face_uvs[0],
                face_uvs.get(k).copied().unwrap_or([0.0; 2]),
                face_uvs.get(k + 1).copied().unwrap_or([0.0; 2]),
            ];
            for (vi, uv) in tri.iter().zip(tri_uvs) {
                let p = node.vt[*vi];
                let w = world * glam::Vec4::new(p[0], p[1], p[2], 1.0);
                positions.push([w.x, w.y, w.z]);
                uvs.push(uv);
            }
        }
    }

    if positions.is_empty() {
        return Err("mesh produced no triangles".into());
    }
    Ok(Mesh {
        positions,
        normals: vec![], // filled by normalize
        uvs,
        indices: vec![],
        material: 0,
    })
}
