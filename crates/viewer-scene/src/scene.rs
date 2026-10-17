// crates/viewer-scene/src/scene.rs
//! The format-neutral scene model every loader produces and the renderer
//! consumes. Keeping this in its own crate (no GPU, no UI types) is what
//! lets loaders be unit-tested headlessly and the renderer stay agnostic.
//!
//! The scene is a flat node array with index-based parent/children links
//! rather than a recursive tree: cache-friendly, trivially serializable,
//! and cheap to walk. One invariant makes transform composition a single
//! forward pass: **a node's index is always greater than its parent's**
//! (loaders create nodes parent-first, e.g. glTF's BFS order).

use glam::{Quat, Vec3};

use crate::aabb::Aabb;
use crate::animation::Animation;

/// Translation-Rotation-Scale transform. Stored decomposed (not as a
/// matrix) because animations tween these channels independently, and the
/// bottom playback bar lerps between TRS keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}
impl Trs {
    pub fn identity() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
    /// Compose into a column-major Mat4 for the GPU. Scale is applied
    /// first, then rotation, then translation (SRT order).
    pub fn to_mat4(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// How a material's alpha reaches the screen. `Mask` discards fragments
/// below the cutoff (cheap, no sorting); `Blend` interpolates (needs the
/// transparent pass).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    Mask(f32),
    Blend,
}

/// CPU-side texture: sRGB-encoded RGBA8, row-major, no padding.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>, // sRGB-encoded RGBA8
}

#[derive(Clone, Debug)]
pub struct Material {
    pub base_color: [f32; 4], // linear
    pub base_color_texture: Option<Image>,
    pub metallic: f32,
    pub roughness: f32,
    pub metallic_roughness_texture: Option<Image>,
    pub normal_map: Option<Image>,
    pub emissive: [f32; 3], // linear
    pub emissive_texture: Option<Image>,
    pub alpha_mode: AlphaMode,
    pub double_sided: bool,
}
impl Material {
    pub fn default_pbr() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8, 1.0],
            base_color_texture: None,
            metallic: 0.0,
            roughness: 0.5,
            metallic_roughness_texture: None,
            normal_map: None,
            emissive: [0.0; 3],
            emissive_texture: None,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub material: usize,
}

/// One transform-hierarchy node. `meshes` holds indices into
/// `Scene::meshes` — one entry per draw call (glTF primitives, 3MF
/// material splits, FBX meshes all map onto this).
#[derive(Clone, Debug)]
pub struct Node {
    pub name: Option<String>,
    pub transform: Trs,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub meshes: Vec<usize>, // one draw per entry; glTF primitives map here
}

/// The whole loaded model. `bounds` is filled by `normalize`, as are
/// missing normals; `tri_count`/`vert_count` accumulate in `add_mesh`
/// for the Model tab's stats.
#[derive(Debug)]
pub struct Scene {
    pub root: usize,
    pub nodes: Vec<Node>, // invariant: parent index < child index
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub animations: Vec<Animation>,
    pub bounds: Aabb,
    pub warnings: Vec<String>,
    pub source_name: String,
    pub tri_count: u32,
    pub vert_count: u32,
}
impl Scene {
    /// New scene containing only the identity root node at index 0.
    pub fn new(source_name: String) -> Self {
        Self {
            root: 0,
            nodes: vec![Node {
                name: Some("root".into()),
                transform: Trs::identity(),
                parent: None,
                children: vec![],
                meshes: vec![],
            }],
            meshes: vec![],
            materials: vec![],
            animations: vec![],
            bounds: Aabb {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            },
            warnings: vec![],
            source_name,
            tri_count: 0,
            vert_count: 0,
        }
    }
    /// Append a node under `parent`. The caller is responsible for the
    /// parent-first invariant (children after parents in `nodes`).
    pub fn add_node(&mut self, parent: usize, transform: Trs, name: Option<String>) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(Node {
            name,
            transform,
            parent: Some(parent),
            children: vec![],
            meshes: vec![],
        });
        self.nodes[parent].children.push(idx);
        idx
    }
    /// Register a mesh, updating tri/vertex stats. Non-indexed meshes
    /// (STL, 3MF point soup) count `positions.len()/3` triangles.
    pub fn add_mesh(&mut self, mesh: Mesh) -> usize {
        let tris = if mesh.indices.is_empty() {
            mesh.positions.len() / 3
        } else {
            mesh.indices.len() / 3
        };
        self.tri_count += tris as u32;
        self.vert_count += mesh.positions.len() as u32;
        self.meshes.push(mesh);
        self.meshes.len() - 1
    }
}
