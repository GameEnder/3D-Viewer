// crates/viewer-scene/src/loaders/mod.rs
pub mod gltf_loader;
pub mod obj_loader;
pub mod stl_loader;

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
        Some("ply") => {
            return Err(LoadError::UnsupportedFormat(
                "PLY (arrives in Task 16)".into(),
            ));
        }
        Some("3mf") => {
            return Err(LoadError::UnsupportedFormat(
                "3MF (arrives in Task 19)".into(),
            ));
        }
        Some("fbx") => return Err(LoadError::UnsupportedFormat("FBX".into())),
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
