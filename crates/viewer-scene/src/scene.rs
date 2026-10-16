// crates/viewer-scene/src/scene.rs
use glam::{Quat, Vec3};

use crate::aabb::Aabb;
use crate::animation::Animation;

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
    pub fn to_mat4(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    Mask(f32),
    Blend,
}

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

#[derive(Clone, Debug)]
pub struct Node {
    pub name: Option<String>,
    pub transform: Trs,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub meshes: Vec<usize>, // one draw per entry; glTF primitives map here
}

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
