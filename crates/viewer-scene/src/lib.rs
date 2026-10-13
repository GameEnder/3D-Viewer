pub mod aabb;
pub mod anim_eval;
pub mod animation;
pub mod error;
pub mod loaders;
pub mod normalize;
pub mod scene;

pub use aabb::Aabb;
pub use anim_eval::evaluate;
pub use animation::{Animation, Channel, Interpolation, Keyframes, TrsProperty};
pub use error::LoadError;
pub use normalize::{compute_normals, normalize, world_transforms};
pub use scene::{AlphaMode, Image, Material, Mesh, Node, Scene, Trs};

/// Load a 3D model by extension. The returned scene is normalized
/// (unit bounding sphere, base at y=0) with smooth normals filled in.
pub fn load(path: &std::path::Path) -> Result<Scene, LoadError> {
    loaders::load(path)
}
