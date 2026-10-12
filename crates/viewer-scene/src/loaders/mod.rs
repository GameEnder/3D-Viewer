// crates/viewer-scene/src/loaders/mod.rs
pub mod gltf_loader;

use crate::error::LoadError;
use crate::scene::Scene;

pub fn load(path: &std::path::Path) -> Result<Scene, LoadError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("glb") | Some("gltf") => gltf_loader::load(path),
        Some("obj") => Err(LoadError::UnsupportedFormat(
            "OBJ (arrives in Task 14)".into(),
        )),
        Some("stl") => Err(LoadError::UnsupportedFormat(
            "STL (arrives in Task 15)".into(),
        )),
        Some("ply") => Err(LoadError::UnsupportedFormat(
            "PLY (arrives in Task 16)".into(),
        )),
        Some("3mf") => Err(LoadError::UnsupportedFormat(
            "3MF (arrives in Task 19)".into(),
        )),
        Some("fbx") => Err(LoadError::UnsupportedFormat("FBX".into())),
        other => Err(LoadError::Parse {
            format: "file",
            detail: format!("unsupported extension {other:?}"),
        }),
    }
}
