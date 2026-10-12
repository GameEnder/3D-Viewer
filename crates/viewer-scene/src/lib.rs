pub mod aabb;
pub mod animation;
pub mod error;
pub mod loaders;
pub mod scene;

pub use aabb::Aabb;
pub use animation::{Animation, Channel, Interpolation, Keyframes, TrsProperty};
pub use error::LoadError;
pub use scene::{AlphaMode, Image, Material, Mesh, Node, Scene, Trs};

/// Load a 3D model by extension. Normalizes it (Task 5 wires normalize in).
pub fn load(path: &std::path::Path) -> Result<Scene, LoadError> {
    loaders::load(path)
}
