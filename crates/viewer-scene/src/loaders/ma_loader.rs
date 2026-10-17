// crates/viewer-scene/src/loaders/ma_loader.rs
//! Maya ASCII (.ma) loader via the `maya-mel` crate.
//!
//! A `.ma` scene is a stream of MEL commands: `createNode` builds a DAG of
//! `transform` and `mesh` nodes (a mesh shape hangs under a transform via
//! `-p`), `setAttr` writes attributes onto the most recently created node,
//! and `currentUnit` declares the file's units. Geometry lives in the mesh
//! attributes:
//!
//! - `.vt`   shared vertex positions — two dialects:
//!   - typed whole-array: `-type "vectorArray" <count> <x y z>...`
//!   - real exports (2013/2015 verified): a sizing line
//!     `setAttr -s <N> ".vt";` followed by per-range data lines
//!     `setAttr ".vt[a:b]" <raw floats...>` with NO count and NO `-type`
//! - `.ed`   edges as (start, end, flag) triplets — same two dialects
//!   (`-type "doubleArray" <total> <triplets>` / sized + raw ranges)
//! - `.fc[a:b]` / `.face` `-type "polyFaces"` (continuation ranges may omit
//!   `-type`): `f <edgeCount> <edge refs...>` + `mu <uvSet> <count> <uv
//!   point idx per face-vertex...>`; holes `h ...` are parsed past and
//!   ignored. Edge references (verified against real Maya 2013/2015
//!   exports): `k` (>= 0) = edge `k` forward; `-k` = edge `k-1`
//!   reversed — edge 0 reversed is `-1`. One entry may carry many `f`
//!   records (real files pack ~500 per range line).
//! - `.uvst[0].uvsp[a:b]` (modern) / `.uvpt` (older) UV points:
//!   `-type "float2"` raw ranges, or `"float2Array" <count>` whole-array
//! - `.t`/`.r`/`.s` `-type "double3"` transform translate / rotate (Euler
//!   XYZ, in `currentUnit` angle units — degrees by default) / scale,
//!   plus `.rp`/`.sp` rotate/scale pivots (default zero). The local
//!   matrix is `T(t) * T(rp) * R * T(-rp) * T(sp) * S * T(-sp)`; rotate
//!   pivot translations (`.rpt`/`.spt`) and non-XYZ rotate orders are
//!   ignored.
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
//! rotate-pivot translations (`.rpt`/`.spt`) and non-XYZ rotate orders are
//! ignored, `rename` after creation is not tracked, references and
//! instancing are not followed, and only UV set 0 is read.

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
    /// Rotate / scale pivots (None = zero).
    rp: Option<[f64; 3]>,
    sp: Option<[f64; 3]>,
    /// Shared vertex positions.
    vt: Vec<[f32; 3]>,
    /// Edge list: (start vertex, end vertex), 0-based.
    ed: Vec<(usize, usize)>,
    /// Faces: (signed 1-based edge refs, per-face-vertex UV point indices).
    faces: Vec<FaceRecord>,
    /// UV points (`.uvst[0].uvsp` / `.uvpt`).
    uvpt: Vec<[f32; 2]>,
}

/// One polyFaces face record: (signed 1-based edge refs, per-face-vertex
/// UV point indices).
type FaceRecord = (Vec<i64>, Vec<usize>);

impl MaNode {
    fn new(name: Option<String>, is_mesh: bool, parent: Option<String>) -> Self {
        Self {
            name,
            is_mesh,
            parent,
            t: None,
            r: None,
            s: None,
            rp: None,
            sp: None,
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

/// Apply one `setAttr` to its target node. Relative paths (leading dot)
/// target the most recently created node; absolute `node.attr` paths
/// resolve by name. The meaningful attr suffix is the last dot-separated
/// component with any `[...]` index stripped (`.uvst[0].uvsp[3:9]` ->
/// `uvsp`).
#[allow(clippy::too_many_lines)]
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
    let ty = sa.type_name.as_ref().and_then(|i| text(i, src));
    let values: Vec<&str> = sa.values.iter().filter_map(|i| text(i, src)).collect();

    let (target, attr_full) = if attr_path.starts_with('.') {
        // Relative: every dot-separated component belongs to the attr.
        match current {
            Some(i) => (i, attr_path),
            None => return Ok(()),
        }
    } else {
        // Absolute "node.attr": resolve the node by name.
        match attr_path.rsplit_once('.') {
            Some((node_name, attr)) => {
                let name = clean_name(node_name);
                match nodes.iter().position(|n| n.name.as_deref() == Some(name)) {
                    Some(i) => (i, attr),
                    None => return Ok(()), // unknown target: ignore silently
                }
            }
            None => return Ok(()),
        }
    };

    // Suffix: strip leading dots, take the last dot component, cut at '['.
    let suffix_pos = attr_full.rfind('.').map_or(0, |i| i + 1);
    let suffix = attr_full[suffix_pos..]
        .split('[')
        .next()
        .unwrap_or(attr_full);
    // Element range `[a:b]` (or `[i]`) attached to the SUFFIX component
    // only — never an earlier index bracket like the uv-set in
    // `.uvst[0].uvsp` (which otherwise masquerades as range 0:0).
    let range = parse_range(attr_full, suffix_pos);

    let node = &mut nodes[target];
    match suffix {
        // Vertex positions. Typed whole-array writes carry a count token
        // and replace; real per-range data lines carry raw floats and append.
        "vt" => match (ty, range) {
            (Some("vectorArray"), _) => node.vt = parse_vector_array(&values)?,
            (None, Some(range)) => {
                node.vt
                    .extend(parse_raw_floats3(&values, range, "vertex array")?);
            }
            _ => {}
        },
        // Edges: (start, end, flag) triplets.
        "ed" => match (ty, range) {
            (Some("doubleArray"), _) => node.ed = parse_edge_array(&values)?,
            (None, Some(range)) => node.ed.extend(parse_raw_edges(&values, range)?),
            _ => {}
        },
        // UV points: modern `.uvst[0].uvsp` ranges (float2) and older
        // `.uvpt` whole arrays (float2Array).
        "uvpt" | "uvsp" => match (ty, range) {
            (Some("float2Array"), _) => node.uvpt = parse_float2_array(&values)?,
            (Some("float2"), Some(range)) | (None, Some(range)) => {
                node.uvpt
                    .extend(parse_raw_floats2(&values, range, "UV point array")?);
            }
            _ => {}
        },
        // Faces: one entry can carry many `f` records (both dialects);
        // continuation ranges may omit `-type "polyFaces"`. Sizing lines
        // (`-s N ".fc";`, no values) parse to zero records silently.
        "fc" | "face" => match ty {
            Some("polyFaces") | None => {
                let records = parse_poly_faces(&values)?;
                if records.is_empty() && !values.is_empty() {
                    warnings.push(format!("skipping empty polyFaces entry on {attr_path}"));
                }
                node.faces.extend(records);
            }
            _ => {}
        },
        // Transform channels (both dialects type them "double3").
        "t" if ty == Some("double3") => node.t = parse_double3(&values),
        "r" if ty == Some("double3") => node.r = parse_double3(&values),
        "s" if ty == Some("double3") => node.s = parse_double3(&values),
        "rp" if ty == Some("double3") => node.rp = parse_double3(&values),
        "sp" if ty == Some("double3") => node.sp = parse_double3(&values),
        _ => {}
    }
    Ok(())
}

/// Parse an element range `[a:b]` (or a single `[i]`) into an inclusive
/// (start, end) pair, searching only at/after `from` so index brackets on
/// earlier components (`.uvst[0].uvsp[0:249]`) are not misread.
fn parse_range(attr: &str, from: usize) -> Option<(usize, usize)> {
    let start = attr.get(from..)?.find('[')? + from;
    let body = attr.get(start + 1..)?.split(']').next()?;
    if let Some((a, b)) = body.split_once(':') {
        Some((a.parse().ok()?, b.parse().ok()?))
    } else {
        let i: usize = body.parse().ok()?;
        Some((i, i))
    }
}

/// Raw per-range position data: exactly `count` triples of floats.
fn parse_raw_floats3(
    values: &[&str],
    range: (usize, usize),
    what: &str,
) -> Result<Vec<[f32; 3]>, LoadError> {
    let count = range.1 - range.0 + 1;
    if values.len() != count * 3 {
        return Err(bad(&format!(
            "{what} range {}:{} holds {} numbers, expected {}",
            range.0,
            range.1,
            values.len(),
            count * 3
        )));
    }
    let mut out = Vec::with_capacity(count);
    for c in values.as_chunks::<3>().0 {
        out.push([
            parse_f64(c[0], what)? as f32,
            parse_f64(c[1], what)? as f32,
            parse_f64(c[2], what)? as f32,
        ]);
    }
    Ok(out)
}

/// Raw per-range UV data: exactly `count` pairs.
fn parse_raw_floats2(
    values: &[&str],
    range: (usize, usize),
    what: &str,
) -> Result<Vec<[f32; 2]>, LoadError> {
    let count = range.1 - range.0 + 1;
    if values.len() != count * 2 {
        return Err(bad(&format!(
            "{what} range {}:{} holds {} numbers, expected {}",
            range.0,
            range.1,
            values.len(),
            count * 2
        )));
    }
    let mut out = Vec::with_capacity(count);
    for c in values.as_chunks::<2>().0 {
        out.push([parse_f64(c[0], what)? as f32, parse_f64(c[1], what)? as f32]);
    }
    Ok(out)
}

/// Raw per-range edge data: exactly `count` (start, end, flag) triplets.
fn parse_raw_edges(
    values: &[&str],
    range: (usize, usize),
) -> Result<Vec<(usize, usize)>, LoadError> {
    let count = range.1 - range.0 + 1;
    if values.len() != count * 3 {
        return Err(bad(&format!(
            "edge range {}:{} holds {} numbers, expected {}",
            range.0,
            range.1,
            values.len(),
            count * 3
        )));
    }
    values
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| {
            Ok((
                parse_f64(c[0], "edge array")? as usize,
                parse_f64(c[1], "edge array")? as usize,
            ))
        })
        .collect()
}

/// Parse a single f64 with a context label.
fn parse_f64(raw: &str, what: &str) -> Result<f64, LoadError> {
    raw.parse::<f64>()
        .map_err(|_| bad(&format!("bad number {raw:?} in {what}")))
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

/// One polyFaces entry can hold MANY face records (real files pack 500
/// per range line, the synthetic `.fc[i]` form holds one): every `f`
/// starts a new record, `mu` attaches UV indices to the current record,
/// holes `h ...` are parsed past and ignored (v1 renders the face
/// without the hole).
fn parse_poly_faces(values: &[&str]) -> Result<Vec<FaceRecord>, LoadError> {
    let mut records: Vec<FaceRecord> = Vec::new();
    let mut i = 0usize;
    while i < values.len() {
        match values[i] {
            "f" => {
                i += 1;
                let count = next_int(values, &mut i, "face edge count")?;
                let mut refs = Vec::with_capacity(count.max(0) as usize);
                for _ in 0..count {
                    refs.push(next_int(values, &mut i, "face edge ref")?);
                }
                if refs.len() >= 3 {
                    records.push((refs, Vec::new()));
                }
            }
            "mu" => {
                i += 1;
                let _set = next_int(values, &mut i, "mu uv set")?;
                let count = next_int(values, &mut i, "mu count")?;
                let mut uvs = Vec::with_capacity(count.max(0) as usize);
                for _ in 0..count {
                    uvs.push(next_int(values, &mut i, "mu uv index")? as usize);
                }
                if let Some(last) = records.last_mut() {
                    last.1 = uvs;
                }
            }
            "h" => {
                // Hole: parse past it.
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
    Ok(records)
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

/// One node's local matrix, pivot-aware:
/// `T(t) * T(rp) * R * T(-rp) * T(sp) * S * T(-sp)`
/// (rotations pivot around `rp`, scaling around `sp`; both default to the
/// origin). Rotate is Euler XYZ in the file's angle unit (degrees unless
/// currentUnit declared radians); rotate-pivot translations and
/// non-XYZ rotate orders are ignored.
fn trs_matrix(node: &MaNode, angle_in_radians: bool) -> glam::Mat4 {
    let v3 = |o: &Option<[f64; 3]>| {
        o.map(|v| Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32))
            .unwrap_or(Vec3::ZERO)
    };
    let t = v3(&node.t);
    let sp = v3(&node.sp);
    let rp = v3(&node.rp);
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

    glam::Mat4::from_translation(t)
        * glam::Mat4::from_translation(rp)
        * glam::Mat4::from_quat(rot)
        * glam::Mat4::from_translation(-rp)
        * glam::Mat4::from_translation(sp)
        * glam::Mat4::from_scale(s)
        * glam::Mat4::from_translation(-sp)
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
        // Oriented vertex chain: the START vertex of each oriented edge.
        // Edge references (verified against real Maya 2013/2015 exports,
        // 1806/1806 faces close): positive or zero `k` = edge `k` forward;
        // negative `-k` = edge `k-1` reversed — so edge 0 reversed is
        // written as -1, and plain 0 is always edge 0 forward.
        let mut ring: Vec<usize> = Vec::with_capacity(refs.len());
        for &r in refs {
            let edge_err = |r: i64| {
                format!(
                    "face references edge {r} but mesh has {} edges",
                    node.ed.len()
                )
            };
            let start = if r >= 0 {
                let e = r as usize;
                node.ed.get(e).ok_or_else(|| edge_err(r))?.0
            } else {
                let e = (r.unsigned_abs() - 1) as usize;
                node.ed.get(e).ok_or_else(|| edge_err(r))?.1
            };
            ring.push(start);
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
