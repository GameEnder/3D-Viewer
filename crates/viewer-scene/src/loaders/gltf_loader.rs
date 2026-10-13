// crates/viewer-scene/src/loaders/gltf_loader.rs
use std::collections::VecDeque;
use std::path::Path;

use glam::{Quat, Vec3};
use gltf::accessor::{DataType, Dimensions};
use gltf::buffer::Data as BufferData;

use crate::animation::{Animation, Channel, Interpolation, Keyframes, TrsProperty};
use crate::error::LoadError;
use crate::scene::{AlphaMode, Image, Material, Mesh, Scene, Trs};

const MAGENTA: [u8; 4] = [255, 0, 255, 255];

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    let (doc, buffers, images) = gltf::import(path).map_err(|e| LoadError::Parse {
        format: "glTF",
        detail: e.to_string(),
    })?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut scene = Scene::new(name);

    // --- materials ---
    for mat in doc.materials() {
        scene
            .materials
            .push(convert_material(&mat, &images, &mut scene.warnings));
    }
    if scene.materials.is_empty() {
        scene.materials.push(Material::default_pbr());
    }

    // --- node order: BFS from scene roots so parents precede children ---
    let roots: Vec<gltf::Node> = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .map(|s| s.nodes().collect())
        .unwrap_or_default();
    let mut order: Vec<gltf::Node> = Vec::new();
    let mut map: Vec<Option<usize>> = vec![None; doc.nodes().len()]; // gltf idx -> our idx
    let mut queue: VecDeque<gltf::Node> = roots.into();
    while let Some(n) = queue.pop_front() {
        let i = n.index();
        if map[i].is_some() {
            continue;
        }
        map[i] = Some(order.len()); // BFS position — dedupe marker only
        let children: Vec<_> = n.children().collect();
        order.push(n);
        queue.extend(children);
    }
    // create our nodes in BFS order
    for node in &order {
        let (t, r, s) = node.transform().decomposed();
        let trs = Trs {
            translation: Vec3::from(t),
            rotation: Quat::from_xyzw(r[0], r[1], r[2], r[3]),
            scale: Vec3::from(s),
        };
        let idx = scene.add_node(scene.root, trs, node.name().map(str::to_owned));
        // Overwrite the BFS-position dedupe marker with our TRUE scene
        // index: the synthetic root occupies slot 0, so created nodes start
        // at 1. Animation channels and hierarchy reparenting resolve through
        // this map (Task 17 regression: the BFS position pointed one node
        // too early — a playing channel then replaced the root's normalize
        // TRS instead of animating its node).
        map[node.index()] = Some(idx);
        // attach meshes (one per primitive)
        if let Some(mesh) = node.mesh() {
            for prim in mesh.primitives() {
                if prim.mode() != gltf::mesh::Mode::Triangles {
                    scene.warnings.push("non-triangle primitive skipped".into());
                    continue;
                }
                let m = convert_primitive(&prim, &buffers, &mut scene)?;
                let our_mesh = scene.add_mesh(m);
                scene.nodes[idx].meshes.push(our_mesh);
            }
        }
    }
    // reparent: add_node parented everything to root; fix to true parents
    for node in &order {
        for child in node.children() {
            let (Some(our_child), Some(our_parent)) = (map[child.index()], map[node.index()])
            else {
                continue;
            };
            scene.nodes[our_child].parent = Some(our_parent);
            scene.nodes[scene.root].children.retain(|&c| c != our_child);
            scene.nodes[our_parent].children.push(our_child);
        }
    }

    // --- animations ---
    for anim in doc.animations() {
        let mut channels = Vec::new();
        let mut duration = 0.0f32;
        for ch in anim.channels() {
            let Some(our_node) = map[ch.target().node().index()] else {
                continue;
            };
            let property = match ch.target().property() {
                gltf::animation::Property::Translation => TrsProperty::Translation,
                gltf::animation::Property::Rotation => TrsProperty::Rotation,
                gltf::animation::Property::Scale => TrsProperty::Scale,
                gltf::animation::Property::MorphTargetWeights => continue, // morph weights out of scope
            };
            let sampler = ch.sampler();
            let interpolation = match sampler.interpolation() {
                gltf::animation::Interpolation::Linear => Interpolation::Linear,
                gltf::animation::Interpolation::Step => Interpolation::Step,
                gltf::animation::Interpolation::CubicSpline => Interpolation::CubicSpline,
            };
            let times = read_f32(&sampler.input(), &buffers)?;
            duration = duration.max(times.last().copied().unwrap_or(0.0));
            let mut keys = read_keyframes(&sampler.output(), &buffers, interpolation, property)?;
            keys.times = times;
            channels.push(Channel {
                node: our_node,
                property,
                keys,
            });
        }
        if !channels.is_empty() {
            scene.animations.push(Animation {
                name: anim
                    .name()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Animation {}", scene.animations.len() + 1)),
                duration,
                channels,
            });
        }
    }
    Ok(scene)
}

fn convert_material(
    mat: &gltf::Material,
    images: &[gltf::image::Data],
    warnings: &mut Vec<String>,
) -> Material {
    let pbr = mat.pbr_metallic_roughness();
    let factor = pbr.base_color_factor();
    Material {
        base_color: factor,
        base_color_texture: texture_or_magenta(
            pbr.base_color_texture().map(|t| t.texture().source()),
            images,
            warnings,
            "baseColorTexture",
        ),
        metallic: pbr.metallic_factor(),
        roughness: pbr.roughness_factor(),
        metallic_roughness_texture: texture_or_magenta(
            pbr.metallic_roughness_texture()
                .map(|t| t.texture().source()),
            images,
            warnings,
            "metallicRoughnessTexture",
        ),
        normal_map: texture_or_magenta(
            mat.normal_texture().map(|t| t.texture().source()),
            images,
            warnings,
            "normalTexture",
        ),
        emissive: mat.emissive_factor(),
        emissive_texture: texture_or_magenta(
            mat.emissive_texture().map(|t| t.texture().source()),
            images,
            warnings,
            "emissiveTexture",
        ),
        alpha_mode: match mat.alpha_mode() {
            gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
            gltf::material::AlphaMode::Mask => AlphaMode::Mask(mat.alpha_cutoff().unwrap_or(0.5)),
            gltf::material::AlphaMode::Blend => AlphaMode::Blend,
        },
        double_sided: mat.double_sided(),
    }
}

fn texture_or_magenta(
    source: Option<gltf::Image>,
    images: &[gltf::image::Data],
    warnings: &mut Vec<String>,
    name: &str,
) -> Option<Image> {
    let src = source?;
    match images.get(src.index()) {
        Some(data) => match decode_image(data) {
            Ok(img) => Some(img),
            Err(e) => {
                warnings.push(format!("{name}: {e}"));
                Some(Image {
                    width: 1,
                    height: 1,
                    rgba: MAGENTA.to_vec(),
                })
            }
        },
        None => {
            warnings.push(format!("{name}: missing image data"));
            Some(Image {
                width: 1,
                height: 1,
                rgba: MAGENTA.to_vec(),
            })
        }
    }
}

fn decode_image(img: &gltf::image::Data) -> Result<Image, LoadError> {
    let w = img.width;
    let h = img.height;
    match img.format {
        gltf::image::Format::R8G8B8A8 => Ok(Image {
            width: w,
            height: h,
            rgba: img.pixels.clone(),
        }),
        gltf::image::Format::R8G8B8 => {
            let mut rgba = Vec::with_capacity(img.pixels.len() / 3 * 4);
            for px in img.pixels.as_chunks::<3>().0 {
                rgba.extend_from_slice(px);
                rgba.push(255);
            }
            Ok(Image {
                width: w,
                height: h,
                rgba,
            })
        }
        other => Err(LoadError::Parse {
            format: "glTF",
            detail: format!("unsupported image format {other:?} (KTX2/Basis not supported)"),
        }),
    }
}

fn convert_primitive(
    prim: &gltf::Primitive,
    buffers: &[BufferData],
    scene: &mut Scene,
) -> Result<Mesh, LoadError> {
    let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| LoadError::Parse {
            format: "glTF",
            detail: "primitive without POSITION".into(),
        })?
        .collect();
    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|r| r.collect())
        .unwrap_or_default();
    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|r| r.into_f32().collect())
        .unwrap_or_default();
    let indices: Vec<u32> = reader
        .read_indices()
        .map(|r| r.into_u32().collect())
        .unwrap_or_else(|| (0..positions.len() as u32).collect());
    let material = prim
        .material()
        .index()
        .unwrap_or(0)
        .min(scene.materials.len().saturating_sub(1));
    Ok(Mesh {
        positions,
        normals,
        uvs,
        indices,
        material,
    })
}

// --- raw accessor helpers (manual decode; no crate-internal APIs) ---

fn accessor_slice<'a>(
    acc: &gltf::Accessor<'a>,
    buffers: &'a [BufferData],
) -> Result<&'a [u8], LoadError> {
    let view = acc.view().ok_or_else(|| LoadError::Parse {
        format: "glTF",
        detail: "sparse accessor not supported".into(),
    })?;
    let data = &buffers[view.buffer().index()].0;
    let start = view.offset() + acc.offset();
    let stride = acc.size(); // bytes per element (tight packing; animation samplers forbid stride)
    let len = acc.count() * stride;
    if start + len > data.len() {
        return Err(LoadError::Parse {
            format: "glTF",
            detail: "accessor out of bounds".into(),
        });
    }
    Ok(&data[start..start + len])
}

fn read_f32(acc: &gltf::Accessor, buffers: &[BufferData]) -> Result<Vec<f32>, LoadError> {
    if acc.data_type() != DataType::F32 || acc.dimensions() != Dimensions::Scalar {
        return Err(LoadError::Parse {
            format: "glTF",
            detail: "animation input must be f32 SCALAR".into(),
        });
    }
    Ok(accessor_slice(acc, buffers)?
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect())
}

fn read_keyframes(
    acc: &gltf::Accessor,
    buffers: &[BufferData],
    interpolation: Interpolation,
    property: TrsProperty,
) -> Result<Keyframes, LoadError> {
    let is_vec4 = matches!(property, TrsProperty::Rotation);
    let comps = if is_vec4 { 4 } else { 3 };
    if acc.data_type() != DataType::F32 {
        return Err(LoadError::Parse {
            format: "glTF",
            detail: "animation output must be f32".into(),
        });
    }
    let expected = if is_vec4 {
        Dimensions::Vec4
    } else {
        Dimensions::Vec3
    };
    if acc.dimensions() != expected {
        return Err(LoadError::Parse {
            format: "glTF",
            detail: "animation output type mismatch".into(),
        });
    }
    let bytes = accessor_slice(acc, buffers)?;
    let read_v4 = |chunk: &[u8]| -> [f32; 4] {
        let f = |i: usize| {
            let b = &chunk[i * 4..i * 4 + 4];
            f32::from_le_bytes([b[0], b[1], b[2], b[3]])
        };
        let (x, y, z) = (f(0), f(1), f(2));
        let w = if comps == 4 { f(3) } else { 0.0 };
        [x, y, z, w]
    };
    let stride = comps * 4;
    match interpolation {
        Interpolation::CubicSpline => {
            // glTF layout per key: [in-tangent, value, out-tangent] (spec §5.8.2).
            // The output accessor's element count is 3x the number of keys.
            let n = acc.count() / 3;
            let mut values = Vec::with_capacity(n);
            let mut in_t = Vec::with_capacity(n);
            let mut out_t = Vec::with_capacity(n);
            for k in 0..n {
                let base = k * stride * 3;
                in_t.push(read_v4(&bytes[base..]));
                values.push(read_v4(&bytes[base + stride..]));
                out_t.push(read_v4(&bytes[base + 2 * stride..]));
            }
            Ok(Keyframes {
                times: vec![],
                values,
                in_tangents: in_t,
                out_tangents: out_t,
                interpolation,
            })
        }
        _ => {
            let n = acc.count();
            let values = (0..n).map(|k| read_v4(&bytes[k * stride..])).collect();
            Ok(Keyframes {
                times: vec![],
                values,
                in_tangents: vec![],
                out_tangents: vec![],
                interpolation,
            })
        }
    }
}
