// crates/viewer-scene/src/loaders/three_mf.rs
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use quick_xml::events::{BytesStart, Event};

use crate::error::LoadError;
use crate::scene::{AlphaMode, Material, Mesh, Scene, Trs};

const FMT: &str = "3MF";
const CORE_MODEL: &str = "3D/3dmodel.model";

fn bad(detail: &str) -> LoadError {
    LoadError::Parse {
        format: FMT,
        detail: detail.into(),
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

struct RawTri {
    v: [u32; 3],
    mat: Option<(u32, u32)>, // (pid, p1)
}

struct RawObject {
    name: Option<String>,
    positions: Vec<[f32; 3]>,
    tris: Vec<RawTri>,
}

struct MatGroup {
    id: Option<u32>,
    materials: Vec<Material>,
}

struct Item {
    objectid: u32,
}

struct Model {
    objects: Vec<RawObject>,
    obj_ids: HashMap<u32, usize>,
    groups: Vec<MatGroup>,
    items: Vec<Item>,
    warnings: Vec<String>,
}

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    let file = std::fs::File::open(path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| bad(&format!("not a valid 3MF package: {e}")))?;
    let member = find_model_part(&mut archive)?;
    let data = read_member(&mut archive, &member)?;
    let model = parse_model(&data)?;
    build_scene(model, path)
}

/// The canonical model part first (case-insensitive), else any `*.model`
/// stored under `3D/`.
fn find_model_part(archive: &mut zip::ZipArchive<std::fs::File>) -> Result<String, LoadError> {
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    if let Some(exact) = names.iter().find(|n| n.eq_ignore_ascii_case(CORE_MODEL)) {
        return Ok(exact.clone());
    }
    let mut fallbacks: Vec<&String> = names
        .iter()
        .filter(|n| {
            let lower = n.replace('\\', "/").to_ascii_lowercase();
            lower.starts_with("3d/") && lower.ends_with(".model")
        })
        .collect();
    fallbacks.sort();
    fallbacks.into_iter().next().cloned().ok_or_else(|| {
        bad(&format!(
            "no 3D model part found in archive (expected {CORE_MODEL})"
        ))
    })
}

fn read_member(
    archive: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
) -> Result<Vec<u8>, LoadError> {
    let mut member = archive
        .by_name(name)
        .map_err(|e| bad(&format!("cannot read model part {name:?}: {e}")))?;
    let mut out = Vec::new();
    member
        .read_to_end(&mut out)
        .map_err(|e| bad(&format!("cannot decompress model part {name:?}: {e}")))?;
    Ok(out)
}

/// Flags describing where in the model XML we currently are. Container flags
/// are set on `Start` and cleared on `End`; `Empty` elements (no children)
/// only trigger the leaf handlers.
struct Ctx {
    cur_object: Option<usize>,
    cur_group: Option<usize>,
    in_mesh: bool,
    in_vertices: bool,
    in_triangles: bool,
    in_build: bool,
}

fn attr(e: &BytesStart, name: &[u8]) -> Result<Option<String>, LoadError> {
    for a in e.attributes() {
        let a = a.map_err(|_| bad("malformed attribute"))?;
        if a.key.local_name().as_ref() == name {
            let value = a
                .decode_and_unescape_value(e.decoder())
                .map_err(|_| bad("malformed attribute value"))?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

fn req_f32(e: &BytesStart, key: &[u8], elem: &str, label: &str) -> Result<f32, LoadError> {
    let raw = attr(e, key)?.ok_or_else(|| bad(&format!("<{elem}> missing {label}")))?;
    let v: f32 = raw
        .parse()
        .map_err(|_| bad(&format!("<{elem}> {label}={raw:?} is not a number")))?;
    if !v.is_finite() {
        return Err(bad(&format!("<{elem}> {label}={raw:?} is not finite")));
    }
    Ok(v)
}

fn req_u32(e: &BytesStart, key: &[u8], elem: &str, label: &str) -> Result<u32, LoadError> {
    let raw = attr(e, key)?.ok_or_else(|| bad(&format!("<{elem}> missing {label}")))?;
    raw.parse::<u32>()
        .map_err(|_| bad(&format!("<{elem}> {label}={raw:?} is not a valid index")))
}

fn opt_u32(e: &BytesStart, key: &[u8], elem: &str, label: &str) -> Result<Option<u32>, LoadError> {
    match attr(e, key)? {
        None => Ok(None),
        Some(raw) => raw
            .parse::<u32>()
            .map(Some)
            .map_err(|_| bad(&format!("<{elem}> {label}={raw:?} is not a valid index"))),
    }
}

fn register_object(e: &BytesStart, model: &mut Model) -> Result<usize, LoadError> {
    let id = opt_u32(e, b"id", "object", "id")?;
    let name = attr(e, b"name")?;
    let idx = model.objects.len();
    model.objects.push(RawObject {
        name,
        positions: vec![],
        tris: vec![],
    });
    if let Some(id) = id {
        model.obj_ids.insert(id, idx);
    }
    Ok(idx)
}

fn register_basematerials(e: &BytesStart, model: &mut Model) -> Result<usize, LoadError> {
    let id = opt_u32(e, b"id", "basematerials", "id")?;
    let idx = model.groups.len();
    model.groups.push(MatGroup {
        id,
        materials: vec![],
    });
    Ok(idx)
}

fn on_vertex(e: &BytesStart, model: &mut Model, ctx: &Ctx) -> Result<(), LoadError> {
    if !ctx.in_vertices {
        return Ok(());
    }
    let x = req_f32(e, b"x", "vertex", "x")?;
    let y = req_f32(e, b"y", "vertex", "y")?;
    let z = req_f32(e, b"z", "vertex", "z")?;
    if let Some(oi) = ctx.cur_object {
        model.objects[oi].positions.push([x, y, z]);
    }
    Ok(())
}

fn on_triangle(e: &BytesStart, model: &mut Model, ctx: &Ctx) -> Result<(), LoadError> {
    if !ctx.in_triangles {
        return Ok(());
    }
    let v1 = req_u32(e, b"v1", "triangle", "v1")?;
    let v2 = req_u32(e, b"v2", "triangle", "v2")?;
    let v3 = req_u32(e, b"v3", "triangle", "v3")?;
    let pid = opt_u32(e, b"pid", "triangle", "pid")?;
    let p1 = opt_u32(e, b"p1", "triangle", "p1")?;
    let mat = pid.map(|p| (p, p1.unwrap_or(0)));
    if let Some(oi) = ctx.cur_object {
        model.objects[oi].tris.push(RawTri {
            v: [v1, v2, v3],
            mat,
        });
    }
    Ok(())
}

/// `#RGB`, `#RRGGBB` or `#RRGGBBAA`; alpha is opacity (not linearized).
fn parse_displaycolor(s: &str) -> Option<([f32; 3], f32)> {
    let hex = s.strip_prefix('#')?;
    let full: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => hex.to_owned(),
        _ => return None,
    };
    if !full.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| -> Option<u8> { u8::from_str_radix(&full[i * 2..i * 2 + 2], 16).ok() };
    let r = byte(0)?;
    let g = byte(1)?;
    let b = byte(2)?;
    let a = if full.len() == 8 { byte(3)? } else { 255 };
    Some((
        [
            srgb_to_linear(f32::from(r) / 255.0),
            srgb_to_linear(f32::from(g) / 255.0),
            srgb_to_linear(f32::from(b) / 255.0),
        ],
        f32::from(a) / 255.0,
    ))
}

fn on_base(e: &BytesStart, model: &mut Model, ctx: &Ctx) -> Result<(), LoadError> {
    let Some(gi) = ctx.cur_group else {
        return Ok(());
    };
    let raw = attr(e, b"displaycolor")?;
    let material = match raw.as_deref().and_then(parse_displaycolor) {
        Some((rgb, alpha)) => Material {
            base_color: [rgb[0], rgb[1], rgb[2], alpha],
            alpha_mode: if alpha < 1.0 {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            },
            ..Material::default_pbr()
        },
        None => {
            let shown = raw.as_deref().unwrap_or("<missing>");
            model.warnings.push(format!(
                "ignoring malformed displaycolor {shown:?} (expected #RRGGBB or #RRGGBBAA)"
            ));
            Material::default_pbr()
        }
    };
    model.groups[gi].materials.push(material);
    Ok(())
}

fn on_item(e: &BytesStart, model: &mut Model, ctx: &Ctx) -> Result<(), LoadError> {
    if !ctx.in_build {
        return Ok(());
    }
    let objectid = req_u32(e, b"objectid", "item", "objectid")?;
    if attr(e, b"transform")?.is_some() {
        model
            .warnings
            .push("3MF build item transform is not supported and was ignored".into());
    }
    model.items.push(Item { objectid });
    Ok(())
}

fn parse_model(data: &[u8]) -> Result<Model, LoadError> {
    let mut reader = quick_xml::Reader::from_reader(data);
    let mut model = Model {
        objects: vec![],
        obj_ids: HashMap::new(),
        groups: vec![],
        items: vec![],
        warnings: vec![],
    };
    let mut ctx = Ctx {
        cur_object: None,
        cur_group: None,
        in_mesh: false,
        in_vertices: false,
        in_triangles: false,
        in_build: false,
    };
    loop {
        let ev = reader
            .read_event()
            .map_err(|e| bad(&format!("malformed XML in model part: {e}")))?;
        match ev {
            Event::Start(e) => match e.name().local_name().as_ref() {
                b"object" => {
                    let idx = register_object(&e, &mut model)?;
                    ctx.cur_object = Some(idx);
                }
                b"basematerials" => {
                    let idx = register_basematerials(&e, &mut model)?;
                    ctx.cur_group = Some(idx);
                }
                b"mesh" => ctx.in_mesh = ctx.cur_object.is_some(),
                b"vertices" => ctx.in_vertices = ctx.in_mesh,
                b"triangles" => ctx.in_triangles = ctx.in_mesh,
                b"build" => ctx.in_build = true,
                b"vertex" => on_vertex(&e, &mut model, &ctx)?,
                b"triangle" => on_triangle(&e, &mut model, &ctx)?,
                b"base" => on_base(&e, &mut model, &ctx)?,
                b"item" => on_item(&e, &mut model, &ctx)?,
                _ => {}
            },
            Event::Empty(e) => match e.name().local_name().as_ref() {
                b"object" => {
                    register_object(&e, &mut model)?;
                }
                b"basematerials" => {
                    register_basematerials(&e, &mut model)?;
                }
                b"vertex" => on_vertex(&e, &mut model, &ctx)?,
                b"triangle" => on_triangle(&e, &mut model, &ctx)?,
                b"base" => on_base(&e, &mut model, &ctx)?,
                b"item" => on_item(&e, &mut model, &ctx)?,
                _ => {}
            },
            Event::End(e) => match e.name().local_name().as_ref() {
                b"object" => ctx.cur_object = None,
                b"basematerials" => ctx.cur_group = None,
                b"mesh" => ctx.in_mesh = false,
                b"vertices" => ctx.in_vertices = false,
                b"triangles" => ctx.in_triangles = false,
                b"build" => ctx.in_build = false,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(model)
}

/// Group an object's triangles by material (first-appearance order); an
/// object without triangles keeps its vertices as a point soup.
fn build_object_meshes(
    scene: &mut Scene,
    obj: &RawObject,
    tri_mats: &[Option<usize>],
    default_idx: Option<usize>,
) -> Vec<usize> {
    if obj.tris.is_empty() {
        if obj.positions.is_empty() {
            return vec![];
        }
        let id = scene.add_mesh(Mesh {
            positions: obj.positions.clone(),
            normals: vec![],
            uvs: vec![],
            indices: vec![],
            material: default_idx.unwrap_or(0),
        });
        return vec![id];
    }
    let mut order: Vec<usize> = Vec::new();
    let mut buckets: HashMap<usize, Vec<u32>> = HashMap::new();
    for (t, m) in obj.tris.iter().zip(tri_mats) {
        let mi = match m {
            Some(i) => *i,
            None => default_idx.unwrap_or(0),
        };
        let bucket = buckets.entry(mi).or_insert_with(|| {
            order.push(mi);
            Vec::new()
        });
        bucket.extend_from_slice(&t.v);
    }
    order
        .iter()
        .map(|&mi| {
            scene.add_mesh(Mesh {
                positions: obj.positions.clone(),
                normals: vec![],
                uvs: vec![],
                indices: buckets[&mi].clone(),
                material: mi,
            })
        })
        .collect()
}

fn build_scene(model: Model, path: &Path) -> Result<Scene, LoadError> {
    let Model {
        objects,
        obj_ids,
        groups,
        mut items,
        mut warnings,
    } = model;

    // An empty <build> falls back to instancing every object in resource order.
    if items.is_empty() {
        let mut ids: Vec<(usize, u32)> = obj_ids.iter().map(|(&id, &oi)| (oi, id)).collect();
        ids.sort();
        if !ids.is_empty() {
            warnings.push("model part has no <build> items; instancing all objects".into());
        }
        items = ids
            .into_iter()
            .map(|(_, id)| Item { objectid: id })
            .collect();
    }
    let mut resolved_items: Vec<usize> = Vec::with_capacity(items.len());
    for it in &items {
        let Some(&oi) = obj_ids.get(&it.objectid) else {
            return Err(bad(&format!(
                "build item references unknown object {}",
                it.objectid
            )));
        };
        resolved_items.push(oi);
    }

    // bounds-check every triangle index against the object's vertex array
    for obj in &objects {
        let n = obj.positions.len();
        for t in &obj.tris {
            if t.v.iter().any(|&i| i as usize >= n) {
                return Err(bad(&format!(
                    "triangle index out of range (object has {n} vertices)"
                )));
            }
        }
    }

    // one material per <base>, flattened across <basematerials> resources;
    // triangle pid/p1 maps into the referenced group's slice
    let mut materials: Vec<Material> = Vec::new();
    let mut group_start: HashMap<u32, (usize, usize)> = HashMap::new(); // id -> (group, start)
    for (gi, g) in groups.iter().enumerate() {
        if let Some(id) = g.id {
            group_start.insert(id, (gi, materials.len()));
        }
        materials.extend(g.materials.iter().cloned());
    }
    let mut warned_pids: HashSet<u32> = HashSet::new();
    let tri_mats: Vec<Vec<Option<usize>>> = objects
        .iter()
        .map(|obj| {
            obj.tris
                .iter()
                .map(|t| {
                    let Some((pid, p1)) = t.mat else {
                        return Ok(None);
                    };
                    let Some(&(gi, start)) = group_start.get(&pid) else {
                        // not a basematerials resource (missing, or an
                        // extension group like texture2dgroup)
                        if warned_pids.insert(pid) {
                            warnings.push(format!(
                                "triangle material group {pid} not found; using default material"
                            ));
                        }
                        return Ok(None);
                    };
                    if p1 as usize >= groups[gi].materials.len() {
                        return Err(bad(&format!(
                            "triangle material index p1={p1} out of range in material group {pid}"
                        )));
                    }
                    Ok(Some(start + p1 as usize))
                })
                .collect()
        })
        .collect::<Result<Vec<_>, _>>()?;

    let has_unmapped = resolved_items
        .iter()
        .any(|&oi| tri_mats[oi].iter().any(|m| m.is_none()));
    let default_idx = if materials.is_empty() || has_unmapped {
        materials.push(Material::default_pbr());
        Some(materials.len() - 1)
    } else {
        None
    };

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut scene = Scene::new(name);
    scene.materials = materials;

    let mut built: HashMap<usize, Vec<usize>> = HashMap::new();
    for &oi in &resolved_items {
        built.entry(oi).or_insert_with(|| {
            build_object_meshes(&mut scene, &objects[oi], &tri_mats[oi], default_idx)
        });
    }
    if scene.meshes.is_empty() {
        return Err(bad("no mesh found in model part"));
    }
    for &oi in &resolved_items {
        let node = scene.add_node(scene.root, Trs::identity(), objects[oi].name.clone());
        scene.nodes[node].meshes = built[&oi].clone();
    }
    scene.warnings = warnings;
    Ok(scene)
}
