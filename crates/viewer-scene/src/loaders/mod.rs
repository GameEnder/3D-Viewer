// crates/viewer-scene/src/loaders/mod.rs
pub mod fbx_loader;
pub mod gltf_loader;
pub mod nif_loader;
pub mod obj_loader;
pub mod ply_loader;
pub mod stl_loader;
pub mod three_mf;

use crate::error::LoadError;
use crate::scene::Scene;

pub fn load(path: &std::path::Path) -> Result<Scene, LoadError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let mut scene = match ext.as_deref() {
        Some("glb") | Some("gltf") => gltf_loader::load(path)?,
        Some("obj") => obj_loader::load(path)?,
        Some("stl") => stl_loader::load(path)?,
        Some("ply") => ply_loader::load(path)?,
        Some("3mf") => three_mf::load(path)?,
        Some("nif") => nif_loader::load(path)?,
        Some("fbx") => fbx_loader::load(path)?,
        other => {
            return Err(LoadError::Parse {
                format: "file",
                detail: format!("unsupported extension {other:?}"),
            });
        }
    };
    crate::normalize::normalize(&mut scene);
    Ok(scene)
}
