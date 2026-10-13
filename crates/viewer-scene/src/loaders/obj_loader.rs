// crates/viewer-scene/src/loaders/obj_loader.rs
use std::path::Path;

use crate::error::LoadError;
use crate::scene::{Image, Material, Mesh, Scene};

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn load(path: &Path) -> Result<Scene, LoadError> {
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
    let materials = materials.map_err(|e| LoadError::Parse {
        format: "OBJ",
        detail: format!("MTL: {e}"),
    })?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut scene = Scene::new(name);
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();

    for mat in &materials {
        let converted = convert_mtl(mat, &dir, &mut scene.warnings);
        scene.materials.push(converted);
    }
    if scene.materials.is_empty() {
        scene.materials.push(Material::default_pbr());
    }
    let node = scene.add_node(
        scene.root,
        crate::scene::Trs::identity(),
        Some("obj".into()),
    );
    for m in &models {
        let mesh = &m.mesh;
        let material = mesh
            .material_id
            .unwrap_or(0)
            .min(scene.materials.len().saturating_sub(1));
        let our = scene.add_mesh(Mesh {
            positions: mesh.positions.as_chunks::<3>().0.to_vec(),
            normals: mesh.normals.as_chunks::<3>().0.to_vec(),
            uvs: mesh.texcoords.as_chunks::<2>().0.to_vec(),
            indices: mesh.indices.clone(),
            material,
        });
        scene.nodes[node].meshes.push(our);
    }
    Ok(scene)
}

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
        let lum = 0.2126 * ks[0] + 0.7152 * ks[1] + 0.0722 * ks[2];
        if lum > 0.6 {
            m.metallic = 0.5;
        }
    }
    if let Some(map_kd) = &mat.diffuse_texture {
        match load_image(&dir.join(map_kd)) {
            Ok(img) => m.base_color_texture = Some(img),
            Err(e) => warnings.push(format!("map_Kd {map_kd}: {e}")),
        }
    }
    m
}

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
