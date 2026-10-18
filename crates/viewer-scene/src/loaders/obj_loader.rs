// crates/viewer-scene/src/loaders/obj_loader.rs
//! OBJ + MTL loader. OBJ is a plain-text mesh format (positions/normals/UVs
//! with a shared vertex list and 1-based face indices); MTL is its companion
//! material library. The `tobj` crate does the tokenizing — this module
//! converts tobj's output into our `Scene`/`Mesh`/`Material` types and
//! maps MTL's legacy lighting parameters onto our PBR material model.

use std::path::Path;

use crate::error::LoadError;
use crate::scene::{Image, Material, Mesh, Scene};

/// Convert an sRGB channel (0..1) to linear light.
///
/// OBJ/MTL colors are authored in sRGB (what a modeller picked in a color
/// picker), but the renderer does lighting in linear space. The two-segment
/// curve is the standard IEC sRGB transfer function.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    // `single_index` flattens OBJ's separate position/normal/UV indices
    // into one index list (matching our Mesh layout, which has a single
    // `indices` array); `triangulate` splits n-gon faces into triangles
    // (the renderer only draws triangle lists).
    let (models, materials) = tobj::load_obj(
        path,
        &tobj::LoadOptions {
            single_index: true,
            triangulate: true,
            ..Default::default()
        },
    )
    .map_err(|e| LoadError::Parse {
        format: "OBJ",
        detail: e.to_string(),
    })?;
    // tobj reports .mtl load failures separately from geometry failures.
    let materials = materials.map_err(|e| LoadError::Parse {
        format: "OBJ",
        detail: format!("MTL: {e}"),
    })?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut scene = Scene::new(name);
    // Texture paths inside an .mtl are relative to the .mtl file's folder,
    // so remember the source directory before resolving them.
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();

    for mat in &materials {
        let converted = convert_mtl(mat, &dir, &mut scene.warnings);
        scene.materials.push(converted);
    }
    // OBJ files without any .mtl reference still need one material to draw.
    if scene.materials.is_empty() {
        scene.materials.push(Material::default_pbr());
    }
    // All OBJ groups land under a single node: the format has no transform
    // hierarchy, everything is stored in one shared coordinate space.
    let node = scene.add_node(
        scene.root,
        crate::scene::Trs::identity(),
        Some("obj".into()),
    );
    for m in &models {
        let mesh = &m.mesh;
        // A group may reference a material by index; clamp to the range we
        // actually converted so a corrupt file cannot index out of bounds.
        let material = mesh
            .material_id
            .unwrap_or(0)
            .min(scene.materials.len().saturating_sub(1));
        // tobj hands back flat [x,y,z, x,y,z, ...] buffers already laid out
        // exactly like our Mesh wants; chunk into triples without copying.
        let our = scene.add_mesh(Mesh {
            positions: mesh.positions.as_chunks::<3>().0.to_vec(),
            normals: mesh.normals.as_chunks::<3>().0.to_vec(),
            uvs: mesh.texcoords.as_chunks::<2>().0.to_vec(),
            indices: mesh.indices.clone(),
            material,
            joints: vec![],
            weights: vec![],
        });
        scene.nodes[node].meshes.push(our);
    }
    Ok(scene)
}

/// Map one MTL material onto our PBR material.
///
/// MTL predates PBR, so the mapping is heuristic:
/// - `Kd` (diffuse color) becomes base color, sRGB-decoded to linear.
/// - `Ns` (shininess 0..1000) is inverted into roughness — shiny means
///   smooth, so 0.95 specular-exponent scale flips to low roughness.
/// - `Ks` (specular color) above 0.6 luminance reads as "metal-ish";
///   MTL has no metallic channel at all.
fn convert_mtl(mat: &tobj::Material, dir: &Path, warnings: &mut Vec<String>) -> Material {
    let kd = mat.diffuse.unwrap_or([0.8, 0.8, 0.8]);
    let mut m = Material {
        base_color: [
            srgb_to_linear(kd[0]),
            srgb_to_linear(kd[1]),
            srgb_to_linear(kd[2]),
            1.0,
        ],
        roughness: mat
            .shininess
            .map(|ns| (1.0 - ns / 1000.0).clamp(0.05, 1.0))
            .unwrap_or(0.5),
        metallic: 0.0,
        ..Material::default_pbr()
    };
    if let Some(ks) = mat.specular {
        // Rec.709 luminance — a perceptual average of the specular color.
        let lum = 0.2126 * ks[0] + 0.7152 * ks[1] + 0.0722 * ks[2];
        if lum > 0.6 {
            m.metallic = 0.5;
        }
    }
    // `map_Kd` is the diffuse texture; resolve it relative to the source
    // directory and surface load failures as warnings (the mesh still loads
    // with the plain diffuse color).
    if let Some(map_kd) = &mat.diffuse_texture {
        match load_image(&dir.join(map_kd)) {
            Ok(img) => m.base_color_texture = Some(img),
            Err(e) => warnings.push(format!("map_Kd {map_kd}: {e}")),
        }
    }
    m
}

/// Decode an image file into our simple RGBA8 image type.
/// `image::load_from_memory` sniffs the format from the bytes, so PNG, JPEG,
/// BMP, TGA etc. all work without matching on extensions.
fn load_image(path: &Path) -> Result<Image, LoadError> {
    let bytes = std::fs::read(path)?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| LoadError::Parse {
            format: "image",
            detail: e.to_string(),
        })?
        .to_rgba8();
    Ok(Image {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}
