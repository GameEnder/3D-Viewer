// crates/viewer-scene/src/loaders/gltf_loader.rs
//! glTF 2.0 / GLB loader (used for both `.gltf` and `.glb` — the `gltf`
//! crate handles container differences transparently).
//!
//! This is the richest format we support: a node hierarchy with TRS
//! transforms, PBR materials with textures, and keyframe animations. The
//! loader preserves all of it:
//!
//! - Nodes are created in **BFS order** so parents always precede children
//!   (the Scene invariant `parent index < child index` needs this), then
//!   re-parented to their true parents in a second pass.
//! - glTF primitives become one Mesh each, hung on their node.
//! - Animations are converted to our `Channel`/`Keyframes` shape, with
//!   CUBICSPLINE's interleaved in-tangent/value/out-tangent layout
//!   de-interleaved into parallel vectors.
//!
//! Throughout, corrupt data is *rejected* (bounds-checked indices, accessor
//! bounds, key-count cross-checks) rather than passed downstream where it
//! would panic the GPU upload or produce NaN transforms during playback.

use std::collections::VecDeque;
use std::path::Path;

use glam::{Quat, Vec3};
use gltf::accessor::{DataType, Dimensions};
use gltf::buffer::Data as BufferData;

use crate::animation::{Animation, Channel, Interpolation, Keyframes, TrsProperty};
use crate::error::LoadError;
use crate::scene::{AlphaMode, Image, Material, Mesh, Scene, Trs};

/// Placeholder for a texture that exists in the material but failed to
/// decode — loud, unmistakable, and keeps the GPU path branch-free.
const MAGENTA: [u8; 4] = [255, 0, 255, 255];

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    // `import` loads the JSON/binary plus every referenced buffer and
    // image into memory in one call.
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

    // --- skins: collect joint node lists + inverse bind matrices ---
    // Skins are numbered by document iteration order; nodes reference
    // them via Node::skin (the glTF node's skin index). A skin whose
    // joints are not all in the scene graph is skipped with a warning
    // (rendering it would deform through missing identity joints).
    {
        let mut remap: Vec<Option<usize>> = Vec::new();
        for skin in doc.skins() {
            let all: Vec<gltf::Node> = skin.joints().collect();
            let joints: Vec<usize> = all.iter().filter_map(|j| map[j.index()]).collect();
            if joints.len() != all.len() {
                scene.warnings.push(format!(
                    "skin '{}' references nodes outside the scene graph; skipped",
                    skin.name().unwrap_or("?")
                ));
                remap.push(None);
                continue;
            }
            let inverse_bind = match skin.inverse_bind_matrices() {
                Some(acc) => read_mat4s(&acc, &buffers)?,
                None => vec![],
            };
            scene
                .skins
                .push(crate::skinning::Skin::new(joints, inverse_bind));
            remap.push(Some(scene.skins.len() - 1));
        }
        // assign Node::skin through the node map (BFS-created indices)
        for node in &order {
            let Some(gskin) = node.skin() else {
                continue;
            };
            let Some(our_idx) = map[node.index()] else {
                continue;
            };
            scene.nodes[our_idx].skin = remap[gskin.index()];
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
            // anim_eval indexes k.values[i]/[j] by times segment indices, so a
            // sampler whose output accessor carries a different key count than
            // its input would panic (or NaN) during playback. Reject at load.
            if keys.values.len() != keys.times.len() {
                return Err(LoadError::Parse {
                    format: "glTF",
                    detail: format!(
                        "animation sampler output count {} != input time count {}",
                        keys.values.len(),
                        keys.times.len()
                    ),
                });
            }
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

/// Map a glTF PBR material onto ours. Nearly 1:1: glTF's metallic-roughness
/// model IS the model our renderer implements; textures route through
/// `texture_or_magenta` so a broken image never becomes a hard failure.
///
/// Legacy materials using KHR_materials_pbrSpecularGlossiness are
/// CONVERTED, not approximated: when a specularGlossinessTexture is
/// present, a per-texel bake solves the metallic-roughness equivalent
/// (see `bake_spec_gloss`) — a factors-only fallback maps the scalars
/// through the same solve. Skipping the bake (e.g. using the raw
/// glossiness factor as roughness) produces near-mirror materials that
/// blow out under the light rig.
fn convert_material(
    mat: &gltf::Material,
    images: &[gltf::image::Data],
    warnings: &mut Vec<String>,
) -> Material {
    let pbr = mat.pbr_metallic_roughness();
    let mut material = if let Some(sg) = mat.pbr_specular_glossiness() {
        let df = sg.diffuse_factor();
        let sf = sg.specular_factor();
        let gf = sg.glossiness_factor();
        let diff_src = sg.diffuse_texture().map(|t| t.texture().source());
        let sg_src = sg
            .specular_glossiness_texture()
            .map(|t| t.texture().source());
        let sf4 = [sf[0], sf[1], sf[2], 1.0];
        match bake_spec_gloss(diff_src, sg_src, df, sf4, gf, images) {
            Ok((base, mr)) => Material {
                // The baked textures carry color/metallic/roughness; the
                // factors pass them through at 1.0 (alpha keeps the
                // diffuse factor so alphaMode still works).
                base_color: [1.0, 1.0, 1.0, df[3]],
                base_color_texture: Some(base),
                metallic: 1.0,
                roughness: 1.0,
                metallic_roughness_texture: Some(mr),
                ..Material::default_pbr()
            },
            Err(e) => {
                warnings.push(format!("spec-gloss conversion: {e}"));
                scalar_spec_gloss(&pbr, Some(sg))
            }
        }
    } else {
        scalar_spec_gloss(&pbr, None)
    };
    material.normal_map = texture_or_magenta(
        mat.normal_texture().map(|t| t.texture().source()),
        images,
        warnings,
        "normalTexture",
    );
    material.emissive = mat.emissive_factor();
    material.emissive_texture = texture_or_magenta(
        mat.emissive_texture().map(|t| t.texture().source()),
        images,
        warnings,
        "emissiveTexture",
    );
    material.alpha_mode = match mat.alpha_mode() {
        gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
        gltf::material::AlphaMode::Mask => AlphaMode::Mask(mat.alpha_cutoff().unwrap_or(0.5)),
        gltf::material::AlphaMode::Blend => AlphaMode::Blend,
    };
    material.double_sided = mat.double_sided();
    material
}

/// Factors-only mapping for spec-gloss materials without a
/// specularGlossinessTexture (or when the bake fails): solve metallic and
/// roughness from the scalar factors.
fn scalar_spec_gloss(
    pbr: &gltf::material::PbrMetallicRoughness,
    sg: Option<gltf::material::PbrSpecularGlossiness>,
) -> Material {
    let Some(sg) = sg else {
        return Material {
            base_color: pbr.base_color_factor(),
            base_color_texture: None,
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            ..Material::default_pbr()
        };
    };
    let d = sg.diffuse_factor();
    let s = sg.specular_factor();
    let s4 = [s[0], s[1], s[2], 1.0];
    let m = solve_metallic_channels(d, s4);
    let b = base_from_diffuse(d, s4, m);
    Material {
        base_color: b,
        metallic: m,
        roughness: (1.0 - sg.glossiness_factor()).clamp(0.045, 1.0),
        ..Material::default_pbr()
    }
}

/// sRGB -> linear for one channel.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// linear -> sRGB for one channel.
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Solve the metallic fraction that reconciles a spec-gloss texel with
/// the metallic-roughness model. Matching the diffuse term
/// (`base * (1 - m) = D`) and the specular F0 (`mix(0.04, base, m) = S`)
/// gives `0.04 m² + (D + S - 0.08) m + (0.04 - S) = 0` per channel;
/// take the valid root, averaged over RGB. Specular below the 4%
/// dielectric minimum forces m = 0.
fn solve_metallic_channels(d: [f32; 4], s: [f32; 4]) -> f32 {
    const MIN_SPEC: f32 = 0.04;
    let mut sum = 0.0f32;
    for c in 0..3 {
        if s[c] < MIN_SPEC {
            continue;
        }
        let a = MIN_SPEC;
        let b = d[c] + s[c] - 2.0 * MIN_SPEC;
        let cc = MIN_SPEC - s[c];
        let disc = (b * b - 4.0 * a * cc).max(0.0);
        sum += ((-b + disc.sqrt()) / (2.0 * a)).clamp(0.0, 1.0);
    }
    (sum / 3.0).clamp(0.0, 1.0)
}

/// Diffuse-derived base color: `base = D / (1 - metallic)`, clamped.
/// Fully metallic texels (division ~0) fall back to the specular color —
/// for a metal the F0 IS the base color.
fn base_from_diffuse(d: [f32; 4], s: [f32; 4], m: f32) -> [f32; 4] {
    if m >= 0.999 {
        return [
            s[0].clamp(0.0, 1.0),
            s[1].clamp(0.0, 1.0),
            s[2].clamp(0.0, 1.0),
            d[3],
        ];
    }
    let inv = 1.0 / (1.0 - m);
    [
        (d[0] * inv).clamp(0.0, 1.0),
        (d[1] * inv).clamp(0.0, 1.0),
        (d[2] * inv).clamp(0.0, 1.0),
        d[3],
    ]
}

/// Per-texel KHR_materials_pbrSpecularGlossiness -> metallic-roughness
/// bake. Produces the two textures our MR renderer needs:
///
/// - base color: `D / (1 - m)` (sRGB-encoded), alpha from the diffuse
/// - metallic-roughness: G = roughness (1 - glossiness), B = metallic
///   (linear, matching the glTF MR convention)
///
/// The math per texel is `solve_metallic_channels` + `base_from_diffuse`;
/// all color inputs are sRGB-decoded first, and the baked base color is
/// re-encoded (our color textures sample as sRGB). Texture sizes may
/// differ between diffuse and spec-gloss — the output uses the diffuse
/// grid, nearest-sampling the spec-gloss.
fn bake_spec_gloss(
    diff_src: Option<gltf::Image>,
    sg_src: Option<gltf::Image>,
    diffuse_factor: [f32; 4],
    specular_factor: [f32; 4],
    glossiness_factor: f32,
    images: &[gltf::image::Data],
) -> Result<(Image, Image), String> {
    let Some(sg_src) = sg_src else {
        return Err("no specularGlossinessTexture to bake".into());
    };
    let sg_img = decode_image(&images[sg_src.index()]).map_err(|e| e.to_string())?;
    let diff_img = match diff_src {
        Some(src) => decode_image(&images[src.index()]).ok(),
        None => None,
    };
    // Output grid: the diffuse texture when present, else the SG texture.
    let (w, h) = diff_img
        .as_ref()
        .map(|i| (i.width, i.height))
        .unwrap_or((sg_img.width, sg_img.height));
    if w == 0 || h == 0 {
        return Err("empty spec-gloss texture".into());
    }

    let mut base_out = vec![0u8; (w * h * 4) as usize];
    let mut mr_out = vec![0u8; (w * h * 4) as usize];
    let sample = |img: Option<&Image>, x: u32, y: u32| -> Option<[u8; 4]> {
        let img = img?;
        let sx = (x * img.width.max(1) / w.max(1)).min(img.width - 1);
        let sy = (y * img.height.max(1) / h.max(1)).min(img.height - 1);
        let o = ((sy * img.width + sx) * 4) as usize;
        Some([
            img.rgba[o],
            img.rgba[o + 1],
            img.rgba[o + 2],
            img.rgba[o + 3],
        ])
    };

    for y in 0..h {
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            let df = sample(diff_img.as_ref(), x, y);
            let sg_texel = sample(Some(&sg_img), x, y).unwrap_or([255, 255, 255, 255]);
            // Diffuse: texture texel (sRGB -> linear) or the factor.
            let d_lin: [f32; 4] = match df {
                Some(t) => [
                    srgb_to_linear(t[0] as f32 / 255.0),
                    srgb_to_linear(t[1] as f32 / 255.0),
                    srgb_to_linear(t[2] as f32 / 255.0),
                    t[3] as f32 / 255.0,
                ],
                None => diffuse_factor,
            };
            let d = [
                d_lin[0] * diffuse_factor[0],
                d_lin[1] * diffuse_factor[1],
                d_lin[2] * diffuse_factor[2],
                d_lin[3] * diffuse_factor[3],
            ];
            // Specular rgb (sRGB -> linear), glossiness alpha (linear).
            let s = [
                srgb_to_linear(sg_texel[0] as f32 / 255.0) * specular_factor[0],
                srgb_to_linear(sg_texel[1] as f32 / 255.0) * specular_factor[1],
                srgb_to_linear(sg_texel[2] as f32 / 255.0) * specular_factor[2],
                1.0,
            ];
            let g = (sg_texel[3] as f32 / 255.0 * glossiness_factor).clamp(0.0, 1.0);
            let m = solve_metallic_channels(d, s);
            let b = base_from_diffuse(d, s, m);
            let rough = (1.0 - g).clamp(0.045, 1.0);
            base_out[o] = (linear_to_srgb(b[0]) * 255.0).round() as u8;
            base_out[o + 1] = (linear_to_srgb(b[1]) * 255.0).round() as u8;
            base_out[o + 2] = (linear_to_srgb(b[2]) * 255.0).round() as u8;
            base_out[o + 3] = (b[3] * 255.0).round() as u8;
            // MR convention: G = roughness, B = metallic.
            mr_out[o] = 255;
            mr_out[o + 1] = (rough * 255.0).round() as u8;
            mr_out[o + 2] = (m * 255.0).round() as u8;
            mr_out[o + 3] = 255;
        }
    }
    Ok((
        Image {
            width: w,
            height: h,
            rgba: base_out,
        },
        Image {
            width: w,
            height: h,
            rgba: mr_out,
        },
    ))
}

/// Fetch + decode a texture, or substitute a 1x1 magenta placeholder with a
/// warning. Missing/broken textures degrade visually but never fail the load.
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

/// Convert decoded image bytes to our RGBA8 `Image`. The glTF crate decodes
/// most formats for us; only GPU-compressed sources (KTX2/Basis) remain
/// unsupported.
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

/// Convert one glTF primitive into a Mesh. POSITION is mandatory; missing
/// normals/UVs default to empty (normalize synthesizes normals later), and a
/// missing index buffer means "sequential" per the glTF spec.
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
    // Skin binding (JOINTS_0/WEIGHTS_0), per shared vertex. Missing
    // either half leaves the mesh unskinned (bind pose).
    let joints: Vec<[u16; 4]> = reader
        .read_joints(0)
        .map(|r| r.into_u16().collect())
        .unwrap_or_default();
    let weights: Vec<[f32; 4]> = reader
        .read_weights(0)
        .map(|r| r.into_f32().collect())
        .unwrap_or_default();
    let (joints, weights) = if joints.len() == positions.len() && weights.len() == positions.len() {
        (joints, weights)
    } else {
        (vec![], vec![])
    };
    // Malformed files can carry an index accessor referencing vertices beyond
    // POSITION — the GPU upload (gpu_scene::upload, m.positions[vi]) and even
    // normalize's bounds pass would panic on it. Reject at load, like the
    // PLY/3MF loaders do for their face indices.
    for &ix in &indices {
        if ix as usize >= positions.len() {
            return Err(LoadError::Parse {
                format: "glTF",
                detail: format!(
                    "primitive index out of range: index {ix} >= {} POSITION vertices",
                    positions.len()
                ),
            });
        }
    }
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
        joints,
        weights,
    })
}

/// Read a MAT4 accessor (16 little-endian f32s per matrix) — used for
/// skin inverse-bind matrices. Column-major per the glTF spec, which
/// matches glam's memory layout.
fn read_mat4s(acc: &gltf::Accessor, buffers: &[BufferData]) -> Result<Vec<glam::Mat4>, LoadError> {
    if acc.data_type() != DataType::F32 || acc.dimensions() != Dimensions::Mat4 {
        return Err(LoadError::Parse {
            format: "glTF",
            detail: "inverse bind matrices must be f32 MAT4".into(),
        });
    }
    let bytes = accessor_slice(acc, buffers)?;
    Ok(bytes
        .as_chunks::<64>()
        .0
        .iter()
        .map(|c| {
            let f: [f32; 16] = std::array::from_fn(|i| {
                f32::from_le_bytes([c[i * 4], c[i * 4 + 1], c[i * 4 + 2], c[i * 4 + 3]])
            });
            glam::Mat4::from_cols_array(&f)
        })
        .collect())
}

// --- raw accessor helpers (manual decode; no crate-internal APIs) ---
// The animation samplers below read accessors by hand because the crate's
// iterator-based readers don't expose CUBICSPLINE's triple-element layout.

/// Borrow the raw bytes of a dense accessor, bounds-checked. Sparse
/// accessors (explicitly allowed by glTF for animation data) are rejected:
/// they are rare in practice and would silently read zeros here.
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

/// Read an animation sampler's input (timestamps): must be f32 SCALAR.
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

/// Read an animation sampler's output (keyframe values). Rotation is a
/// quaternion (VEC4); translation/scale are VEC3, widened to [f32; 4] so all
/// properties share one value type. `times` is filled by the caller from
/// the matching input accessor.
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
            // The output accessor's element count is 3x the number of keys, and
            // the spec requires at least 2 keys. Malformed counts would truncate
            // to empty/short values and NaN transforms during playback.
            if !acc.count().is_multiple_of(3) || acc.count() < 6 {
                return Err(LoadError::Parse {
                    format: "glTF",
                    detail: format!(
                        "CUBICSPLINE output accessor holds {} elements; expected 3 per key for at least 2 keys (>= 6)",
                        acc.count()
                    ),
                });
            }
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
