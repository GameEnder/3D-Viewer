# 3D Viewer Clone — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust desktop clone of Microsoft 3D Viewer's core experience: load GLB/GLTF/OBJ/STL/PLY/3MF models, PBR rendering with the original's 3-light rig and 9 lighting themes, per-light shadow maps, orbit camera, wireframe, turntable, glTF animation playback, screenshots.

**Architecture:** Three-crate Cargo workspace. `viewer-scene` (pure data + loaders, no GPU), `viewer-render` (wgpu renderer, no UI), `viewer-app` (eframe/egui shell). One-way deps: app → render → scene. The renderer draws the scene into an offscreen HDR texture during egui-wgpu's `prepare` callback phase (own command encoder, returns command buffers), then `paint` blits that texture into egui's render pass inside the viewport rect. The same offscreen texture is the screenshot source.

**Tech Stack:** Rust edition 2024, eframe/egui-wgpu 0.36 (wgpu 30 via `egui_wgpu::wgpu` re-export — never a standalone wgpu dep), glam, gltf crate 1.x, tobj, image, rfd, bytemuck, thiserror, pollster.

**Spec:** `docs/superpowers/specs/2026-10-11-3d-viewer-clone-design.md`

## Global Constraints

- **Pinned versions:** eframe 0.36 + egui-wgpu 0.36 MUST match (both 0.36.2 at time of writing). Use `egui_wgpu::wgpu` for ALL wgpu types; NEVER add a standalone `wgpu` dependency.
- eframe dep: `{ version = "0.36", default-features = false, features = ["default_fonts", "wgpu", "accesskit"] }`.
- **Verification gate (end of every task):** `cargo fmt --check` then `cargo clippy --workspace --all-targets -- -D warnings` then `cargo test --workspace`. All must pass before the commit step.
- **Git identity:** repo has none configured. Always commit with: `git -c user.name="Steven" -c user.email="steven@local" commit -m "<msg>"`.
- **No unsafe code.** No `unwrap()` in library crates (viewer-scene, viewer-render) — use `expect("...")` or propagate errors. `unwrap()` acceptable in viewer-app for egui/wgpu state known present.
- WGSL shaders: `include_str!` + `wgpu::ShaderSource::Wgsl`.
- Ported constants (exact, from the original app — spec Appendix B): clear color rgb(69,74,80); ground plane opacity 0.4; exposure 0.5 EV; contrast 1.66; bloom weight 0.11; FOV 45°; camera elevation 15°, rotation −90°; shadow map size 512². Theme intensities: 100 = 1.0.
- sRGB: color texture uploads use `*Srgb` formats; scalar textures (MR/normal) use non-srgb. Light/theme colors converted sRGB→linear on CPU.
- Camera: LMB drag orbit, wheel dolly, RMB drag pan, double-click reset. Perspective only.
- Commit style: `feat:`/`fix:`/`test:`/`chore:`.
- Windows-first; keep code cross-platform (no winapi).
- When eframe/egui-wgpu API details differ from this plan's code (the API churns), check docs.rs for the pinned 0.36 version and adapt — the prepare/paint/CallbackResources pattern itself is stable.

## File Structure

```
Cargo.toml                                # [workspace]
crates/
├─ viewer-scene/
│  ├─ Cargo.toml
│  └─ src/
│     ├─ lib.rs                           # re-exports + load() dispatch by extension
│     ├─ aabb.rs                          # Aabb
│     ├─ scene.rs                         # Trs, Node, Mesh, Material, Image, AlphaMode, Scene
│     ├─ animation.rs                     # Animation, Channel, Keyframes, Interpolation
│     ├─ error.rs                         # LoadError
│     ├─ normalize.rs                     # normalize(), compute_normals(), world_transforms()
│     ├─ anim_eval.rs                      # (Task 17) animation sampling
│     └─ loaders/
│        ├─ mod.rs                        # dispatch + normalize post-call
│        ├─ gltf_loader.rs                # Task 4
│        ├─ obj_loader.rs                 # Task 14
│        ├─ stl_loader.rs                # Task 15
│        ├─ ply_loader.rs                # Task 16
│        └─ three_mf.rs                  # Task 19 (stretch)
├─ viewer-render/
│  ├─ Cargo.toml
│  └─ src/
│     ├─ lib.rs                           # Renderer, FrameParams, LightInput, RenderSettings
│     ├─ gpu_scene.rs                     # GpuScene: vertex/index buffers, materials, textures
│     ├─ uniforms.rs                      # bytemuck uniform structs (must match WGSL exactly)
│     ├─ post.rs                          # bloom chain targets (Task 10)
│     └─ shaders/
│        ├─ pbr.wgsl                      # Task 7 (full final form incl. shadow sampling)
│        ├─ ground.wgsl                   # Task 8
│        ├─ shadow.wgsl                   # Task 9
│        ├─ post.wgsl                     # Task 7 (full form incl. bloom binding)
│        ├─ bloom.wgsl                    # Task 10 (threshold + separable blur)
│        └─ blit.wgsl                     # Task 7
└─ viewer-app/
   ├─ Cargo.toml
   └─ src/
      ├─ main.rs                          # eframe::run_native
      ├─ app.rs                           # ViewerApp: state machine, update(), input, loading
      ├─ camera.rs                        # OrbitCamera
      ├─ paint.rs                         # ScenePaint callback (prepare+paint), blit install
      ├─ panels.rs                        # toolbar, settings sections, animation bar, dialogs
      └─ themes.rs                        # 9-theme const table + rig math
```

---

### Task 1: Cargo workspace scaffold

**Files:**
- Create: `Cargo.toml`, `crates/viewer-scene/Cargo.toml`, `crates/viewer-scene/src/lib.rs`, `crates/viewer-render/Cargo.toml`, `crates/viewer-render/src/lib.rs`, `crates/viewer-app/Cargo.toml`, `crates/viewer-app/src/main.rs`

**Interfaces:**
- Produces: compiling workspace; crate names `viewer-scene` (lib `viewer_scene`), `viewer-render` (lib `viewer_render`), `viewer-app` (bin `viewer`). All later tasks build here.

- [ ] **Step 1: Create workspace root manifest**

```toml
# Cargo.toml
[workspace]
resolver = "3"
members = ["crates/viewer-scene", "crates/viewer-render", "crates/viewer-app"]

[workspace.dependencies]
eframe = { version = "0.36", default-features = false, features = ["default_fonts", "wgpu", "accesskit"] }
egui-wgpu = "0.36"
glam = "0.30"
gltf = "1"
tobj = "4"
image = "0.25"
rfd = "0.15"
thiserror = "2"
bytemuck = { version = "1", features = ["derive"] }
pollster = "0.4"
viewer-scene = { path = "crates/viewer-scene" }
viewer-render = { path = "crates/viewer-render" }
```

- [ ] **Step 2: Create viewer-scene crate**

```toml
# crates/viewer-scene/Cargo.toml
[package]
name = "viewer-scene"
version = "0.1.0"
edition = "2024"

[dependencies]
glam.workspace = true
thiserror.workspace = true
gltf.workspace = true
tobj.workspace = true
image.workspace = true
```

```rust
// crates/viewer-scene/src/lib.rs
// Modules land in later tasks. This file grows: mod aabb; mod scene; ...
```

- [ ] **Step 3: Create viewer-render crate**

```toml
# crates/viewer-render/Cargo.toml
[package]
name = "viewer-render"
version = "0.1.0"
edition = "2024"

[dependencies]
egui-wgpu.workspace = true   # provides the wgpu 30 re-export; NO standalone wgpu dep
viewer-scene.workspace = true
glam.workspace = true
bytemuck.workspace = true
pollster.workspace = true
image.workspace = true
```

```rust
// crates/viewer-render/src/lib.rs
// Renderer lands in Task 7.
```

- [ ] **Step 4: Create viewer-app crate**

```toml
# crates/viewer-app/Cargo.toml
[package]
name = "viewer-app"
version = "0.1.0"
edition = "2024"

[dependencies]
eframe.workspace = true
egui-wgpu.workspace = true
viewer-scene.workspace = true
viewer-render.workspace = true
glam.workspace = true
rfd.workspace = true
image.workspace = true

[[bin]]
name = "viewer"
path = "src/main.rs"
```

```rust
// crates/viewer-app/src/main.rs
fn main() {
    println!("viewer scaffold");
}
```

- [ ] **Step 5: Verify the gate**

Run: `cargo fmt` then `cargo clippy --workspace --all-targets -- -D warnings` then `cargo test --workspace` then `cargo run -p viewer-app`
Expected: all pass; prints "viewer scaffold".

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates
git -c user.name="Steven" -c user.email="steven@local" commit -m "chore: scaffold three-crate workspace"
```

---

### Task 2: eframe window shell + wgpu paint callback

**Files:**
- Modify: `crates/viewer-app/src/main.rs`
- Create: `crates/viewer-app/src/app.rs`, `crates/viewer-app/src/paint.rs`

**Interfaces:**
- Produces: `ViewerApp::new(cc: &eframe::CreationContext<'_>) -> Self` (grows in later tasks); `paint::install(&RenderState)` inserts GPU pipeline objects into `render_state.renderer.write().callback_resources` — this is the established sharing pattern for all later tasks. The `SolidPainter` callback is throwaway scaffolding replaced in Task 7.

- [ ] **Step 1: main.rs — run eframe with wgpu backend**

```rust
// crates/viewer-app/src/main.rs
mod app;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("3D Viewer")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "3D Viewer",
        options,
        Box::new(|cc| Ok(Box::new(app::ViewerApp::new(cc)))),
    )
}
```

- [ ] **Step 2: paint.rs — solid-color pipeline + install()**

```rust
// crates/viewer-app/src/paint.rs
use std::sync::Arc;
use egui_wgpu::wgpu;
use egui_wgpu::RenderState;

/// Stored in egui_wgpu callback_resources; retrieved by paint callbacks.
pub struct SolidPipeline(pub wgpu::RenderPipeline);

/// Insert shared GPU resources once at app creation.
pub fn install(state: &Arc<RenderState>) {
    let device = &state.device;
    let format = state.target_format;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("solid.wgsl"),
        source: wgpu::ShaderSource::Wgsl(SOLID_WGSL.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("solid_pipeline"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        multiview: None,
        cache: None,
    });
    state.renderer.write().callback_resources.insert(SolidPipeline(pipeline));
}

const SOLID_WGSL: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0), vec2<f32>(3.0, 1.0), vec2<f32>(-1.0, 1.0));
    return vec4<f32>(pos[vi], 0.0, 1.0);
}
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // linear values for rgb(69,74,80) — target is *Srgb on Windows, hardware re-encodes.
    // If the window background looks wrong vs the reference app, the surface format
    // lacks the Srgb suffix: apply pow(c, 1.0/2.2) here instead.
    return vec4<f32>(0.0592, 0.0677, 0.0797, 1.0);
}
"#;
```

- [ ] **Step 3: app.rs — ViewerApp with viewport paint callback**

```rust
// crates/viewer-app/src/app.rs
use std::sync::Arc;
use eframe::egui;
use egui_wgpu::{Callback, CallbackResources, CallbackTrait, RenderState};

use crate::paint;

pub struct ViewerApp {
    #[allow(dead_code)] // used from Task 7 onward
    render_state: Option<Arc<RenderState>>,
}

impl ViewerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // cc.wgpu_render_state is a FIELD in eframe 0.36 (Option<Arc<RenderState>>).
        // If the compiler disagrees, it became a method: cc.wgpu_render_state().
        if let Some(state) = cc.wgpu_render_state.as_ref() {
            paint::install(state);
        }
        Self { render_state: cc.wgpu_render_state.as_ref().map(Arc::clone) }
    }
}

impl eframe::App for ViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default())
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                ui.painter()
                    .add(Callback::new_paint_callback(rect, SolidPainter));
                ui.label("3D Viewer (Rust clone)");
            });
    }
}

struct SolidPainter;

impl CallbackTrait for SolidPainter {
    fn paint(
        &self,
        info: egui_wgpu::epaint::PaintCallbackInfo,
        render_pass: &mut egui_wgpu::wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let pipeline = resources
            .get::<paint::SolidPipeline>()
            .expect("SolidPipeline installed at startup");
        let vp = info.viewport_in_pixels();
        render_pass.set_pipeline(&pipeline.0);
        render_pass.set_viewport(
            vp.left_px as f32,
            vp.top_px as f32,
            vp.width_px as f32,
            vp.height_px as f32,
            0.0,
            1.0,
        );
        render_pass.draw(0..3, 0..1);
    }
}
```

- [ ] **Step 4: Verify manually**

Run: `cargo run -p viewer-app`
Expected: a 1280×800 window, viewport painted rgb(69,74,80) (matches the original app's background), "3D Viewer (Rust clone)" label on top, no console errors. Resize/maximize — color must follow the full window.

- [ ] **Step 5: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/viewer-app
git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: eframe window shell with wgpu paint callback"
```

---

### Task 3: viewer-scene core types

**Files:**
- Create: `crates/viewer-scene/src/{aabb.rs, scene.rs, animation.rs, error.rs}`
- Modify: `crates/viewer-scene/src/lib.rs`
- Test: `crates/viewer-scene/src/aabb.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces (used by every later task — keep names exact):
  - `pub struct Aabb { pub min: Vec3, pub max: Vec3 }` with `from_points`, `center`, `bounding_sphere_radius`
  - `pub struct Trs { translation: Vec3, rotation: Quat, scale: Vec3 }` + `identity()`, `to_mat4()`
  - `pub enum AlphaMode { Opaque, Mask(f32), Blend }`
  - `pub struct Image { width: u32, height: u32, rgba: Vec<u8> }` (RGBA8, sRGB-encoded bytes)
  - `pub struct Material { base_color: [f32;4], base_color_texture: Option<Image>, metallic: f32, roughness: f32, metallic_roughness_texture: Option<Image>, normal_map: Option<Image>, emissive: [f32;3], emissive_texture: Option<Image>, alpha_mode: AlphaMode, double_sided: bool }` + `Material::default_pbr()`
  - `pub struct Mesh { positions: Vec<[f32;3]>, normals: Vec<[f32;3]>, uvs: Vec<[f32;2]>, indices: Vec<u32>, material: usize }`
  - `pub struct Node { name: Option<String>, transform: Trs, parent: Option<usize>, children: Vec<usize>, meshes: Vec<usize> }`
  - `pub struct Scene { root: usize, nodes: Vec<Node>, meshes: Vec<Mesh>, materials: Vec<Material>, animations: Vec<Animation>, bounds: Aabb, warnings: Vec<String>, source_name: String, tri_count: u32, vert_count: u32 }` with `Scene::new(source_name)`, `add_node(parent, transform, name) -> usize`, `add_mesh(mesh) -> usize`
  - `pub struct Animation { name: String, duration: f32, channels: Vec<Channel> }`
  - `pub struct Channel { node: usize, property: TrsProperty, keys: Keyframes }`
  - `pub enum TrsProperty { Translation, Rotation, Scale }`
  - `pub enum Interpolation { Step, Linear, CubicSpline }`
  - `pub struct Keyframes { times: Vec<f32>, values: Vec<[f32;4]>, in_tangents: Vec<[f32;4]>, out_tangents: Vec<[f32;4]>, interpolation: Interpolation }` — `values`: T/S = xyz+pad; R = quat xyzw (glTF order). For Step/Linear, tangent vecs are empty; for CubicSpline all three are per-key (spec: output = [in-tangent, value, out-tangent] per key).
  - `pub enum LoadError { UnsupportedFormat(String), Parse { format: &'static str, detail: String }, Io(std::io::Error) }`

- [ ] **Step 1: aabb.rs with tests**

```rust
// crates/viewer-scene/src/aabb.rs
use glam::Vec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn from_points(points: impl Iterator<Item = [f32; 3]>) -> Self {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for p in points {
            let p = Vec3::from(p);
            min = min.min(p);
            max = max.max(p);
        }
        Self { min, max }
    }
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn bounding_sphere_radius(&self) -> f32 {
        (self.max - self.center()).length().max(1e-6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unit_box() {
        let a = Aabb::from_points([[-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]].into_iter());
        assert_eq!(a.min, Vec3::splat(-0.5));
        assert_eq!(a.max, Vec3::splat(0.5));
        assert_eq!(a.center(), Vec3::ZERO);
        assert!((a.bounding_sphere_radius() - 0.5 * 3f32.sqrt()).abs() < 1e-5);
    }
}
```

- [ ] **Step 2: scene.rs**

```rust
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
        Self { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }
    pub fn to_mat4(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rot_translation(self.scale, self.rotation, self.translation)
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
            bounds: Aabb { min: Vec3::ZERO, max: Vec3::ZERO },
            warnings: vec![],
            source_name,
            tri_count: 0,
            vert_count: 0,
        }
    }
    pub fn add_node(&mut self, parent: usize, transform: Trs, name: Option<String>) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(Node { name, transform, parent: Some(parent), children: vec![], meshes: vec![] });
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
```

- [ ] **Step 3: animation.rs**

```rust
// crates/viewer-scene/src/animation.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrsProperty {
    Translation,
    Rotation,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Step,
    Linear,
    CubicSpline,
}

#[derive(Clone, Debug)]
pub struct Keyframes {
    pub times: Vec<f32>,
    /// T/S: xyz + 0 pad. R: quaternion xyzw (glTF order). One per key.
    pub values: Vec<[f32; 4]>,
    /// CubicSpline only (glTF output layout per key: [in-tangent, value, out-tangent]).
    pub in_tangents: Vec<[f32; 4]>,
    pub out_tangents: Vec<[f32; 4]>,
    pub interpolation: Interpolation,
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub node: usize,
    pub property: TrsProperty,
    pub keys: Keyframes,
}

#[derive(Clone, Debug)]
pub struct Animation {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<Channel>,
}
```

- [ ] **Step 4: error.rs + lib.rs wiring**

```rust
// crates/viewer-scene/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("{0} files are not supported by this viewer")]
    UnsupportedFormat(String),
    #[error("{format} parse error: {detail}")]
    Parse { format: &'static str, detail: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

```rust
// crates/viewer-scene/src/lib.rs
pub mod aabb;
pub mod animation;
pub mod error;
pub mod scene;

pub use aabb::Aabb;
pub use animation::{Animation, Channel, Interpolation, Keyframes, TrsProperty};
pub use error::LoadError;
pub use scene::{AlphaMode, Image, Material, Mesh, Node, Scene, Trs};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p viewer-scene`
Expected: `aabb::tests::unit_box` passes; workspace compiles.

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/viewer-scene
git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: scene core types (Scene/Node/Mesh/Material/Animation)"
```


---

### Task 4: glTF/GLB loader + load dispatcher

**Files:**
- Create: `crates/viewer-scene/src/loaders/mod.rs`, `crates/viewer-scene/src/loaders/gltf_loader.rs`
- Modify: `crates/viewer-scene/src/lib.rs`
- Test: `crates/viewer-scene/tests/gltf_load.rs` (fixture built in-code)

**Interfaces:**
- Consumes: Task 3 types.
- Produces:
  - `pub fn load(path: &std::path::Path) -> Result<Scene, LoadError>` re-exported from crate root — THE entry point used by viewer-app forever.
  - `loaders::gltf_loader::load(path) -> Result<Scene, LoadError>`.
  - Invariant established here and kept by all loaders: nodes ordered parent-before-child; single synthetic root (index 0) holds model roots; normals may be empty (filled by `compute_normals` in Task 5).

- [ ] **Step 1: lib.rs dispatch**

```rust
// crates/viewer-scene/src/lib.rs — add:
pub mod loaders;

pub use scene::{AlphaMode, Image, Material, Mesh, Node, Scene, Trs};

/// Load a 3D model by extension. Normalizes it (Task 5 wires normalize in).
pub fn load(path: &std::path::Path) -> Result<Scene, LoadError> {
    loaders::load(path)
}
```

```rust
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
        Some("obj") => Err(LoadError::UnsupportedFormat("OBJ (arrives in Task 14)".into())),
        Some("stl") => Err(LoadError::UnsupportedFormat("STL (arrives in Task 15)".into())),
        Some("ply") => Err(LoadError::UnsupportedFormat("PLY (arrives in Task 16)".into())),
        Some("3mf") => Err(LoadError::UnsupportedFormat("3MF (arrives in Task 19)".into())),
        Some("fbx") => Err(LoadError::UnsupportedFormat("FBX".into())),
        other => Err(LoadError::Parse {
            format: "file",
            detail: format!("unsupported extension {other:?}"),
        }),
    }
}
```

- [ ] **Step 2: gltf_loader.rs implementation**

```rust
// crates/viewer-scene/src/loaders/gltf_loader.rs
use std::path::Path;

use gltf::buffer::Data as BufferData;
use glam::{Quat, Vec3};

use crate::animation::{Animation, Channel, Interpolation, Keyframes, TrsProperty};
use crate::error::LoadError;
use crate::scene::{AlphaMode, Image, Material, Mesh, Scene, Trs};

const MAGENTA: [u8; 4] = [255, 0, 255, 255];

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    let (doc, buffers, images) = gltf::import(path)
        .map_err(|e| LoadError::Parse { format: "glTF", detail: e.to_string() })?;
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut scene = Scene::new(name);

    // --- materials ---
    for mat in doc.materials() {
        scene.materials.push(convert_material(&mat, &images, &mut scene.warnings));
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
    let mut queue: std::collections::VecDeque<gltf::Node> = roots.into();
    while let Some(n) = queue.pop_front() {
        let i = n.index();
        if map[i].is_some() {
            continue;
        }
        map[i] = Some(order.len());
        order.push(n);
        queue.extend(n.children());
    }
    // create our nodes in BFS order
    for node in &order {
        let (t, r, s) = node.transform().decomposed();
        let trs = Trs {
            translation: t.into(),
            rotation: Quat::from_xyzw(r[0], r[1], r[2], r[3]),
            scale: s.into(),
        };
        let idx = scene.add_node(scene.root, trs, node.name().map(str::to_owned));
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
        let child_of_root = map[node.index()];
        for child in node.children() {
            let (Some(our_child), Some(our_parent)) = (map[child.index()], map[node.index()]) else {
                continue;
            };
            let _ = child_of_root;
            scene.nodes[our_child].parent = Some(our_parent);
            scene.nodes[scene.root].children.retain(|&c| c != our_child);
            scene.nodes[our_parent].children.push(our_child);
        }
    }

    // --- animations ---
    for anim in doc.animations() {
        let mut channels = Vec::new();
        let mut duration = 0.0f32;
        for ch in anim.channels() {
            let Some(our_node) = map[ch.target().node().index()] else { continue };
            let property = match ch.target().path() {
                gltf::animation::Property::Translation => TrsProperty::Translation,
                gltf::animation::Property::Rotation => TrsProperty::Rotation,
                gltf::animation::Property::Scale => TrsProperty::Scale,
                gltf::animation::Property::Weights => continue, // morph weights out of scope
            };
            let sampler = ch.sampler();
            let interpolation = match sampler.interpolation() {
                gltf::animation::Interpolation::Linear => Interpolation::Linear,
                gltf::animation::Interpolation::Step => Interpolation::Step,
                gltf::animation::Interpolation::CubicSpline => Interpolation::CubicSpline,
            };
            let times = read_f32(&sampler.input(), &buffers)?;
            duration = duration.max(times.last().copied().unwrap_or(0.0));
            let keys = read_keyframes(&sampler.output(), &buffers, interpolation, property)?;
            channels.push(Channel { node: our_node, property, keys });
        }
        if !channels.is_empty() {
            scene.animations.push(Animation {
                name: anim.name().map(str::to_owned)
                    .unwrap_or_else(|| format!("Animation {}", scene.animations.len() + 1)),
                duration,
                channels,
            });
        }
    }
    Ok(scene)
}

fn convert_material(
    mat: &gltf::Material,
    images: &[gltf::image::Data],
    warnings: &mut Vec<String>,
) -> Material {
    let pbr = mat.pbr_metallic_roughness();
    let factor = pbr.base_color_factor();
    Material {
        base_color: factor,
        base_color_texture: texture_or_magenta(
            pbr.base_color_texture().map(|t| t.texture().source()),
            images,
            warnings,
            "baseColorTexture",
        ),
        metallic: pbr.metallic_factor(),
        roughness: pbr.roughness_factor(),
        metallic_roughness_texture: texture_or_magenta(
            pbr.metallic_roughness_texture().map(|t| t.texture().source()),
            images,
            warnings,
            "metallicRoughnessTexture",
        ),
        normal_map: texture_or_magenta(
            mat.normal_texture().map(|t| t.texture().source()),
            images,
            warnings,
            "normalTexture",
        ),
        emissive: mat.emissive_factor(),
        emissive_texture: texture_or_magenta(
            mat.emissive_texture().map(|t| t.texture().source()),
            images,
            warnings,
            "emissiveTexture",
        ),
        alpha_mode: match mat.alpha_mode() {
            gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
            gltf::material::AlphaMode::Mask => AlphaMode::Mask(mat.alpha_cutoff()),
            gltf::material::AlphaMode::Blend => AlphaMode::Blend,
        },
        double_sided: mat.double_sided(),
    }
}

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
                Some(Image { width: 1, height: 1, rgba: MAGENTA.to_vec() })
            }
        },
        None => {
            warnings.push(format!("{name}: missing image data"));
            Some(Image { width: 1, height: 1, rgba: MAGENTA.to_vec() })
        }
    }
}

fn decode_image(img: &gltf::image::Data) -> Result<Image, LoadError> {
    let w = img.width;
    let h = img.height;
    match img.format {
        gltf::ImageFormat::R8G8B8A8 => Ok(Image { width: w, height: h, rgba: img.pixels.clone() }),
        gltf::ImageFormat::R8G8B8 => {
            let mut rgba = Vec::with_capacity(img.pixels.len() / 3 * 4);
            for px in img.pixels.chunks_exact(3) {
                rgba.extend_from_slice(px);
                rgba.push(255);
            }
            Ok(Image { width: w, height: h, rgba })
        }
        other => Err(LoadError::Parse {
            format: "glTF",
            detail: format!("unsupported image format {other:?} (KTX2/Basis not supported)"),
        }),
    }
}

fn convert_primitive(
    prim: &gltf::Primitive,
    buffers: &[BufferData],
    scene: &mut Scene,
) -> Result<Mesh, LoadError> {
    let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| LoadError::Parse { format: "glTF", detail: "primitive without POSITION".into() })?
        .collect();
    let normals: Vec<[f32; 3]> = reader.read_normals().map(|r| r.collect()).unwrap_or_default();
    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|r| r.map(|c| c.into_f32()).collect())
        .unwrap_or_default();
    let indices: Vec<u32> = reader
        .read_indices()
        .map(|r| r.into_u32().collect())
        .unwrap_or_else(|| (0..positions.len() as u32).collect());
    let material = prim.material().index().unwrap_or(0).min(scene.materials.len().saturating_sub(1));
    Ok(Mesh { positions, normals, uvs, indices, material })
}

// --- raw accessor helpers (manual decode; no crate-internal APIs) ---

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
    let stride = acc.size(); // bytes per element (handles byteStride internally)
    let len = acc.count() * stride;
    if start + len > data.len() {
        return Err(LoadError::Parse { format: "glTF", detail: "accessor out of bounds".into() });
    }
    Ok(&data[start..start + len])
}

fn read_f32(acc: &gltf::Accessor, buffers: &[BufferData]) -> Result<Vec<f32>, LoadError> {
    if acc.component_type() != gltf::accessor::ComponentType::F32
        || acc.type() != gltf::accessor::Type::Scalar
    {
        return Err(LoadError::Parse { format: "glTF", detail: "animation input must be f32 SCALAR".into() });
    }
    Ok(accessor_slice(acc, buffers)?
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

fn read_keyframes(
    acc: &gltf::Accessor,
    buffers: &[BufferData],
    interpolation: Interpolation,
    property: TrsProperty,
) -> Result<Keyframes, LoadError> {
    let is_vec4 = matches!(property, TrsProperty::Rotation);
    let comps = if is_vec4 { 4 } else { 3 };
    if acc.component_type() != gltf::accessor::ComponentType::F32 {
        return Err(LoadError::Parse { format: "glTF", detail: "animation output must be f32".into() });
    }
    let expected = gltf::accessor::Type::Vec4.filter(|_| is_vec4).or(Some(gltf::accessor::Type::Vec3)).filter(|_| !is_vec4);
    if acc.type() != expected.unwrap() {
        return Err(LoadError::Parse { format: "glTF", detail: "animation output type mismatch".into() });
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
    let n = acc.count();
    match interpolation {
        Interpolation::CubicSpline => {
            // glTF layout per key: [in-tangent, value, out-tangent] (spec §5.8.2)
            let mut values = Vec::with_capacity(n);
            let mut in_t = Vec::with_capacity(n);
            let mut out_t = Vec::with_capacity(n);
            for k in 0..n {
                let base = k * stride * 3;
                in_t.push(read_v4(&bytes[base..]));
                values.push(read_v4(&bytes[base + stride..]));
                out_t.push(read_v4(&bytes[base + 2 * stride..]));
            }
            Ok(Keyframes { times: vec![], values, in_tangents: in_t, out_tangents: out_t, interpolation })
        }
        _ => {
            let values = (0..n).map(|k| read_v4(&bytes[k * stride..])).collect();
            Ok(Keyframes { times: vec![], values, in_tangents: vec![], out_tangents: vec![], interpolation })
        }
    }
}
```

Note: `Keyframes.times` is filled by the caller (the animation loop above already has `times` — assign `keys.times = times;` after `read_keyframes` returns; keep the struct field order stable).

- [ ] **Step 3: integration test with hand-built GLB fixture**

```rust
// crates/viewer-scene/tests/gltf_load.rs
use std::io::Write;

fn build_cube_glb(with_animation: bool) -> Vec<u8> {
    let positions: Vec<f32> = vec![
        -0.5, -0.5, -0.5, 0.5, -0.5, -0.5, -0.5, 0.5, -0.5, 0.5, 0.5, -0.5,
        -0.5, -0.5, 0.5, 0.5, -0.5, 0.5, -0.5, 0.5, 0.5, 0.5, 0.5, 0.5,
    ];
    let normals: Vec<f32> = positions.clone();
    let indices: Vec<u16> = vec![
        0, 1, 3, 0, 3, 2, 4, 6, 7, 4, 7, 5, 0, 4, 5, 0, 5, 1,
        1, 5, 7, 1, 7, 3, 3, 7, 6, 3, 6, 2, 2, 6, 4, 2, 4, 0,
    ];
    let times: Vec<f32> = vec![0.0, 1.0];
    let quats: Vec<f32> = vec![
        0.0, 0.0, 0.0, 1.0, // identity
        0.0, 0.3826834, 0.0, 0.9238795, // 90° around Y
    ];
    let mut bin: Vec<u8> = Vec::new();
    for v in &positions { bin.extend_from_slice(&v.to_le_bytes()); }   // 96 B
    for v in &normals { bin.extend_from_slice(&v.to_le_bytes()); }      // 96 B
    for i in &indices { bin.extend_from_slice(&i.to_le_bytes()); }      // 72 B
    bin.extend_from_slice(&0.0f32.to_le_bytes());                        // anim time 0
    bin.extend_from_slice(&1.0f32.to_le_bytes());                        // anim time 1  (view 3: 8 B)
    for q in &quats { bin.extend_from_slice(&q.to_le_bytes()); }        // 32 B (view 4)

    let mut json = String::from(r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0,"name":"cube"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1},"indices":2,"material":0}]}],"materials":[{"pbrMetallicRoughness":{"baseColorFactor":[1.0,0.0,0.0,1.0],"metallicFactor":0.0,"roughnessFactor":0.5},"alphaMode":"MASK","alphaCutoff":0.4}],"accessors":[{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]},{"bufferView":1,"componentType":5126,"count":8,"type":"VEC3"},{"bufferView":2,"componentType":5123,"count":36,"type":"SCALAR"}"#);
    if with_animation {
        json.push_str(r#"},{"bufferView":3,"componentType":5126,"count":2,"type":"SCALAR","max":[1.0]},{"bufferView":4,"componentType":5126,"count":2,"type":"VEC4"#);
    }
    json.push_str(r#"}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":96},{"buffer":0,"byteOffset":96,"byteLength":96},{"buffer":0,"byteOffset":192,"byteLength":72}"#);
    if with_animation {
        json.push_str(r#",{"buffer":0,"byteOffset":264,"byteLength":8},{"buffer":0,"byteOffset":272,"byteLength":32}"#);
    }
    json.push_str(r#"}],"buffers":[{"byteLength":304}]"#);
    if with_animation {
        json.push_str(r#,"animations":[{"channels":[{"sampler":0,"target":{"node":0,"path":"rotation"}}],"samplers":[{"input":3,"interpolation":"LINEAR","output":4}]}]}"#);
    }

    let json_bytes = json.as_bytes().to_vec();
    let pad_json = (4 - json_bytes.len() % 4) % 4;
    let pad_bin = (4 - bin.len() % 4) % 4;
    let total = 12 + 8 + json_bytes.len() + pad_json + 8 + bin.len() + pad_bin;

    let mut glb: Vec<u8> = Vec::new();
    glb.extend_from_slice(&0x46546C67u32.to_le_bytes()); // magic "glTF"
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&((json_bytes.len() + pad_json) as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // "JSON"
    glb.extend_from_slice(&json_bytes);
    glb.extend(std::iter::repeat(b' ').take(pad_json));
    glb.extend_from_slice(&((bin.len() + pad_bin) as u32).to_le_bytes());
    glb.extend_from_slice(&0x004E4942u32.to_le_bytes()); // "BIN"
    glb.extend_from_slice(&bin);
    glb.extend(std::iter::repeat(0u8).take(pad_bin));
    glb
}

fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("viewer-scene-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(bytes).unwrap();
    p
}

#[test]
fn loads_glb_mesh_material_animation() {
    let path = write_temp("cube.glb", &build_cube_glb(true));
    let scene = viewer_scene::load(&path).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.meshes[0].positions.len(), 8);
    assert_eq!(scene.meshes[0].indices.len(), 36);
    assert_eq!(scene.materials[0].base_color, [1.0, 0.0, 0.0, 1.0]);
    assert!(matches!(
        scene.materials[0].alpha_mode,
        viewer_scene::AlphaMode::Mask(c) if (c - 0.4).abs() < 1e-6
    ));
    assert_eq!(scene.nodes.len(), 2); // synthetic root + cube
    assert_eq!(scene.nodes[1].name.as_deref(), Some("cube"));
    assert!(scene.nodes[1].meshes.len() == 1);
    assert_eq!(scene.animations.len(), 1);
    let a = &scene.animations[0];
    assert_eq!(a.channels.len(), 1);
    assert_eq!(a.duration, 1.0);
    assert_eq!(a.channels[0].keys.times, vec![0.0, 1.0]);
    assert_eq!(a.channels[0].keys.values.len(), 2);
    assert_eq!(scene.tri_count, 12);
}

#[test]
fn rejects_fbx_with_friendly_error() {
    let err = viewer_scene::load(std::path::Path::new("model.fbx")).unwrap_err();
    assert!(matches!(err, viewer_scene::LoadError::UnsupportedFormat(_)));
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p viewer-scene`
Expected: all pass, including the two new integration tests.

- [ ] **Step 5: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/viewer-scene
git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: glTF/GLB loader with animations and dispatch"
```


---

### Task 5: Normalization, world transforms, smooth normals

**Files:**
- Create: `crates/viewer-scene/src/normalize.rs`
- Modify: `crates/viewer-scene/src/lib.rs`, `crates/viewer-scene/src/loaders/mod.rs`
- Test: `crates/viewer-scene/src/normalize.rs` (inline tests)

**Interfaces:**
- Consumes: Task 3 `Scene`, Task 4 loaders.
- Produces:
  - `pub fn world_transforms(scene: &Scene, overrides: &[Option<Trs>]) -> Vec<glam::Mat4>` — overrides indexed by node (empty slice = none). Renderer + normalize both consume this.
  - `pub fn normalize(scene: &mut Scene)` — bounding sphere radius → 1.0, base at y=0, centered on x/z; stores result in `scene.bounds`; fills missing normals.
  - `pub fn compute_normals(mesh: &mut Mesh)` — smooth normals by accumulation.
  - `loaders::load` now calls `normalize` before returning — the app never sees un-normalized scenes.

- [ ] **Step 1: Write failing tests inside normalize.rs**

```rust
#[cfg(test)]
mod tests {
    use crate::scene::{Mesh, Scene, Trs};
    use crate::normalize::{compute_normals, normalize, world_transforms};
    use glam::{Vec3, Vec4};

    fn mesh_cube() -> Mesh {
        Mesh {
            positions: vec![[-0.5; 3], [0.5, -0.5, -0.5], [-0.5, 0.5, -0.5], [0.5, 0.5, 0.5]],
            normals: vec![],
            uvs: vec![],
            indices: vec![0, 1, 2, 1, 3, 2],
            material: 0,
        }
    }

    #[test]
    fn normalize_unit_sphere_base_on_ground() {
        let mut scene = Scene::new("t".into());
        scene.materials.push(crate::scene::Material::default_pbr());
        scene.nodes[0].meshes.push(scene.add_mesh(mesh_cube()));
        normalize(&mut scene);
        // bounding sphere radius must be ~0.866 (half-diagonal of unit cube) * scale(1/0.866)=1.0
        assert!((scene.bounds.bounding_sphere_radius() - 1.0).abs() < 1e-4);
        // base of the model sits at y=0, centered on x/z
        assert!(scene.bounds.min.y.abs() < 1e-5);
        assert!(scene.bounds.center().x.abs() < 1e-5);
        assert!(scene.bounds.center().z.abs() < 1e-5);
    }

    #[test]
    fn world_transforms_composes_hierarchy() {
        let mut scene = Scene::new("t".into());
        let parent = scene.add_node(scene.root, Trs { translation: Vec3::new(1.0, 0.0, 0.0), ..Trs::identity() }, None);
        let _child = scene.add_node(parent, Trs { translation: Vec3::new(0.0, 2.0, 0.0), ..Trs::identity() }, None);
        let w = world_transforms(&scene, &[]);
        let c = w[2] * Vec4::new(0.0, 0.0, 0.0, 1.0);
        assert!((c.x - 1.0).abs() < 1e-6 && c.y == 2.0);
    }

    #[test]
    fn compute_normals_accumulates_smooth() {
        let mut m = Mesh {
            positions: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            normals: vec![],
            uvs: vec![],
            indices: vec![],
            material: 0,
        };
        compute_normals(&mut m);
        let n = m.normals[0];
        let inv = 1.0 / 3f32.sqrt();
        assert!((n[0] - inv).abs() < 1e-5 && (n[1] - inv).abs() < 1e-5 && (n[2] - inv).abs() < 1e-5);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p viewer-scene normalize`
Expected: FAIL — `normalize` module doesn't exist yet.

- [ ] **Step 3: Implement normalize.rs**

```rust
// crates/viewer-scene/src/normalize.rs
use glam::{Mat4, Quat, Vec3};

use crate::aabb::Aabb;
use crate::scene::{Mesh, Scene, Trs};

/// Compose local node transforms into world matrices.
/// `overrides` (indexed by node) replace node TRS for animated nodes.
pub fn world_transforms(scene: &Scene, overrides: &[Option<Trs>]) -> Vec<Mat4> {
    let mut out = vec![Mat4::IDENTITY; scene.nodes.len()];
    for (i, node) in scene.nodes.iter().enumerate() {
        let local = overrides
            .get(i)
            .and_then(|o| o.as_ref())
            .unwrap_or(&node.transform)
            .to_mat4();
        out[i] = match node.parent {
            Some(p) => out[p] * local,
            None => local,
        };
    }
    out
}

/// Smooth per-vertex normals from face normals (only when normals are absent).
pub fn compute_normals(mesh: &mut Mesh) {
    if !mesh.normals.is_empty() || mesh.positions.is_empty() {
        return;
    }
    let mut acc = vec![Vec3::ZERO; mesh.positions.len()];
    let mut face = |a: usize, b: usize, c: usize| {
        let pa = Vec3::from(mesh.positions[a]);
        let ab = Vec3::from(mesh.positions[b]) - pa;
        let ac = Vec3::from(mesh.positions[c]) - pa;
        let n = ab.cross(ac);
        for &i in &[a, b, c] {
            acc[i] += n;
        }
    };
    if mesh.indices.is_empty() {
        for t in 0..mesh.positions.len() / 3 {
            face(t * 3, t * 3 + 1, t * 3 + 2);
        }
    } else {
        for t in mesh.indices.chunks_exact(3) {
            face(t[0] as usize, t[1] as usize, t[2] as usize);
        }
    }
    mesh.normals = acc
        .into_iter()
        .map(|n| if n.length_squared() < 1e-12 { [0.0, 1.0, 0.0] } else { n.normalize().to_array() })
        .collect();
}

/// Mirrors the original's `Model.PositionAlignBase`: unit bounding sphere,
/// model base on the ground plane (y=0), centered on x/z.
pub fn normalize(scene: &mut Scene) {
    for mesh in &mut scene.meshes {
        compute_normals(mesh);
    }
    let world = world_transforms(scene, &[]);
    let points = scene
        .meshes
        .iter()
        .flat_map(|m| m.positions.iter().map(|p| Vec3::from(*p)))
        .zip(std::iter::repeat(()));
    // fold meshes under their node world transforms:
    let mut all = Vec::new();
    for (node_idx, node) in scene.nodes.iter().enumerate() {
        for &mi in &node.meshes {
            let m = &scene.meshes[mi];
            for p in &m.positions {
                all.push((world[node_idx] * glam::Vec4::new(p[0], p[1], p[2], 1.0)).truncate());
            }
        }
    }
    let _ = points; // (unused helper removed; `all` is authoritative)
    if all.is_empty() {
        scene.bounds = Aabb { min: Vec3::splat(-0.5), max: Vec3::splat(0.5) };
        return;
    }
    let bounds = Aabb::from_points(all.iter().map(|v| v.to_array()));
    let center = bounds.center();
    let radius = bounds.bounding_sphere_radius();
    let s = 1.0 / radius;
    // root transform: scale to unit sphere, then place base on ground and center x/z
    let translation = Vec3::new(-center.x * s, -bounds.min.y * s, -center.z * s);
    scene.nodes[scene.root].transform = Trs {
        translation,
        rotation: Quat::IDENTITY,
        scale: Vec3::splat(s),
    };
    let world = world_transforms(scene, &[]);
    let mut all = Vec::new();
    for (node_idx, node) in scene.nodes.iter().enumerate() {
        for &mi in &node.meshes {
            let m = &scene.meshes[mi];
            for p in &m.positions {
                all.push((world[node_idx] * glam::Vec4::new(p[0], p[1], p[2], 1.0)).truncate());
            }
        }
    }
    scene.bounds = Aabb::from_points(all.iter().map(|v| v.to_array()));
}
```

- [ ] **Step 4: Wire into lib.rs and loaders/mod.rs**

```rust
// lib.rs — add:
pub mod normalize;
pub use normalize::{compute_normals, normalize, world_transforms};
```

```rust
// loaders/mod.rs — replace load():
pub fn load(path: &std::path::Path) -> Result<Scene, LoadError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let mut scene = match ext.as_deref() {
        Some("glb") | Some("gltf") => gltf_loader::load(path)?,
        Some("obj") => return Err(LoadError::UnsupportedFormat("OBJ (arrives in Task 14)".into())),
        Some("stl") => return Err(LoadError::UnsupportedFormat("STL (arrives in Task 15)".into())),
        Some("ply") => return Err(LoadError::UnsupportedFormat("PLY (arrives in Task 16)".into())),
        Some("3mf") => return Err(LoadError::UnsupportedFormat("3MF (arrives in Task 19)".into())),
        Some("fbx") => return Err(LoadError::UnsupportedFormat("FBX".into())),
        other => {
            return Err(LoadError::Parse {
                format: "file",
                detail: format!("unsupported extension {other:?}"),
            })
        }
    };
    crate::normalize::normalize(&mut scene);
    Ok(scene)
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p viewer-scene`
Expected: all pass (old + new normalize tests).

- [ ] **Step 6: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates/viewer-scene
git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: model normalization, world transforms, smooth normals"
```

---

### Task 6: Orbit camera

**Files:**
- Create: `crates/viewer-app/src/camera.rs`
- Modify: `crates/viewer-app/src/main.rs` (add `mod camera;`)
- Test: `crates/viewer-app/src/camera.rs` (inline tests)

**Interfaces:**
- Consumes: `viewer_scene::Aabb` via glam types (bounds passed in from app).
- Produces:
  - `pub struct OrbitCamera { pub target: Vec3, pub distance: f32, pub elevation_deg: f32, pub rotation_deg: f32, pub fov_y_deg: f32 }`
  - `DEFAULT_ELEVATION_DEG = 15.0`, `DEFAULT_ROTATION_DEG = -90.0`, `DEFAULT_FOV_DEG = 45.0` (exact values from the original app's config)
  - `OrbitCamera::default_for(center: Vec3, radius: f32)` → distance = `radius / sin(fov/2) * 1.25`
  - `eye() -> Vec3`, `view_matrix() -> Mat4`, `proj_matrix(aspect: f32) -> Mat4`, `orbit(dx, dy)`, `dolly(mult: f32)`, `pan(dx, dy, aspect)`, `fit(center, radius)`, `view_proj(aspect) -> [[f32;4];4]`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Vec3, Vec4};

    #[test]
    fn spherical_eye_matches_rotation_elevation() {
        let c = OrbitCamera::default_for(Vec3::ZERO, 1.0);
        // rotation −90° → eye on −X side (mod axis flip), elevation 15° above horizon
        assert!((c.rotation_deg - (-90.0)).abs() < 1e-4);
        assert!((c.elevation_deg - 15.0).abs() < 1e-4);
        let eye = c.eye();
        assert!(eye.x < 0.0 && eye.y > 0.0 && eye.z.abs() < 1e-4);
    }

    #[test]
    fn target_maps_to_clip_center() {
        let c = OrbitCamera::default_for(Vec3::new(0.0, 0.5, 0.0), 1.0);
        let vp = c.view_proj(16.0 / 9.0);
        let clip = vp * Vec4::new(0.0, 0.5, 0.0, 1.0);
        assert!(clip.w > 0.0);
        assert!((clip.x / clip.w).abs() < 1e-5);
        assert!((clip.y / clip.w).abs() < 1e-5);
        assert!(clip.z / clip.w > 0.0 && clip.z / clip.w < 1.0); // reverse... no: standard 0..1 depth
    }

    #[test]
    fn dolly_clamps() {
        let mut c = OrbitCamera::default_for(Vec3::ZERO, 1.0);
        let before = c.distance;
        c.dolly(0.001);
        assert!(c.distance > before * 0.5); // clamped, not collapsed
        c.dolly(100.0);
        assert!(c.distance < 50.0);
    }
}
```

- [ ] **Step 2: Run to verify failure** — Run: `cargo test -p viewer-app camera` (expect compile error).

- [ ] **Step 3: Implement camera.rs**

```rust
// crates/viewer-app/src/camera.rs
use glam::{Mat4, Vec3, Vec4};

pub const DEFAULT_ELEVATION_DEG: f32 = 15.0;
pub const DEFAULT_ROTATION_DEG: f32 = -90.0;
pub const DEFAULT_FOV_DEG: f32 = 45.0;

/// Spherical orbit camera matching the original's (distance, elevation, rotation) model.
#[derive(Clone, Copy, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    pub elevation_deg: f32,
    pub rotation_deg: f32,
    pub fov_y_deg: f32,
}

impl OrbitCamera {
    pub fn fit(&mut self, center: Vec3, radius: f32) {
        self.target = center;
        self.elevation_deg = DEFAULT_ELEVATION_DEG;
        self.rotation_deg = DEFAULT_ROTATION_DEG;
        self.fov_y_deg = DEFAULT_FOV_DEG;
        self.distance = (radius / (self.fov_y_deg * 0.5).to_radians().sin()) * 1.25;
    }

    pub fn default_for(center: Vec3, radius: f32) -> Self {
        let mut c = Self { target: center, distance: 3.0, elevation_deg: DEFAULT_ELEVATION_DEG, rotation_deg: DEFAULT_ROTATION_DEG, fov_y_deg: DEFAULT_FOV_DEG };
        c.fit(center, radius);
        c
    }

    pub fn eye(&self) -> Vec3 {
        let el = self.elevation_deg.to_radians();
        let ro = self.rotation_deg.to_radians();
        let dir = Vec3::new(el.cos() * ro.sin(), el.sin(), el.cos() * ro.cos());
        self.target + dir * self.distance
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Y)
    }

    pub fn proj_matrix(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y_deg.to_radians(), aspect.max(0.01), 0.05, 100.0)
    }

    pub fn view_proj(&self, aspect: f32) -> [[f32; 4]; 4] {
        (self.proj_matrix(aspect) * self.view_matrix()).to_cols_array_2d()
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.rotation_deg -= dx * 0.5;
        self.elevation_deg = (self.elevation_deg + dy * 0.5).clamp(-89.0, 89.0);
    }

    pub fn dolly(&mut self, mult: f32) {
        self.distance = (self.distance / mult.max(0.01)).clamp(0.4, 30.0);
    }

    pub fn pan(&mut self, dx: f32, dy: f32, aspect: f32) {
        let scale = self.distance * (self.fov_y_deg.to_radians() * 0.5).tan() / 500.0;
        let view = self.view_matrix();
        let right = Vec3::new(view.x_axis.x, view.y_axis.x, view.z_axis.x); // view row 0
        let up = Vec3::new(view.x_axis.y, view.y_axis.y, view.z_axis.y); // view row 1
        self.target += right * (-dx * scale * aspect.max(0.01).min(3.0)) + up * (dy * scale);
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p viewer-app`
Expected: 3 camera tests pass.

- [ ] **Step 5: Gate + commit**

```bash
git add crates/viewer-app/src/camera.rs crates/viewer-app/src/main.rs
git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: orbit camera matching original spherical model"
```


---

### Task 7: Renderer core — PBR pass, HDR + tonemap, blit integration

The big one. Creates `viewer-render::Renderer` (GPU scene upload, PBR forward pass with the 3-light rig, HDR target with MSAA 4× resolve, ACES post with exposure/contrast, final `target_format` texture), and the app-side `ScenePaint` callback that renders in `prepare` and blits in `paint`. Shadow sampling and bloom bindings are included in the shaders NOW but bound to 1×1 dummies — Task 9/10 swap in real resources without touching shader code again.

**Files:**
- Create: `crates/viewer-render/src/{uniforms.rs, gpu_scene.rs, shaders/pbr.wgsl, shaders/post.wgsl, shaders/blit.wgsl}`
- Modify: `crates/viewer-render/src/lib.rs`
- Modify: `crates/viewer-app/src/{app.rs, paint.rs, main.rs}`

**Interfaces:**
- Consumes: Task 3/5 `viewer_scene::{Scene, Material, AlphaMode}`, Task 5 `world_transforms`.
- Produces (used by Tasks 8–13, 18):
  ```rust
  pub struct LightInput { pub pos: [f32; 3], pub color: [f32; 3], pub intensity: f32, pub shadows: bool }
  pub struct RenderSettings { pub clear_color: [f32; 3], pub wireframe: bool, pub ground: bool,
      pub grid: bool, pub bloom: bool, pub exposure: f32, pub contrast: f32, pub ground_opacity: f32 }
  pub struct FrameParams { pub view_proj: [[f32; 4]; 4], pub camera_pos: [f32; 3],
      pub turntable_deg: f32, pub lights: [LightInput; 3], pub env_color: [f32; 3],
      pub env_intensity: f32, pub settings: RenderSettings, pub anim: Option<(usize, f32)> }
  pub struct Renderer { /* internal */ }
  impl Renderer {
      pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self
      pub fn set_scene(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: Arc<viewer_scene::Scene>)
      pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32)
      pub fn queue_frame(&mut self, params: FrameParams)
      pub fn prepare_render(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) // records all passes into final texture
      pub fn blit(&self, render_pass: &mut wgpu::RenderPass<'static>, viewport: [f32; 4])
      pub fn request_screenshot(&mut self)   // Task 13 collects
      pub fn collect_screenshot(&mut self, device: &wgpu::Device) -> Option<image::RgbaImage>
  }
  ```
  App side: `paint::SharedRenderer(pub Arc<parking_lot-free std::sync::Mutex<Renderer>>)` inserted into callback_resources; `ScenePaint` callback struct.
- Tuning constants (documented in code, adjust at smoke): `LIGHT_SCALE = 8.0` (theme intensity 1.0 → this multiplier), `ENV_SCALE = 0.35`.

- [ ] **Step 1: uniforms.rs — bytemuck structs mirroring WGSL exactly**

```rust
// crates/viewer-render/src/uniforms.rs
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GlobalsU {
    pub view_proj: [[f32; 4]; 4], // 0
    pub camera_pos: [f32; 4],      // 64
    pub env_color: [f32; 4],       // 80  rgb + intensity
    pub flags: [u32; 4],           // 96  x=wireframe, y=grid, z=unused, w=unused
    pub pad: [f32; 4],             // 112
}                                  // 128

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LightU {
    pub pos: [f32; 4],            // xyz + intensity
    pub color: [f32; 4],          // rgb + unused
    pub view_proj: [[f32; 4]; 4],
    pub shadow: [f32; 4],         // x=shadow enabled flag, y=texel size, zw unused
}                                 // 112 bytes (16-aligned)

pub const LIGHT_COUNT: usize = 3;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ModelU {
    pub model: [[f32; 4]; 4],
    pub normal_mat: [[f32; 4]; 4],
} // 128

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MaterialU {
    pub base_color: [f32; 4],
    pub params: [f32; 4],   // metallic, roughness, alpha_cutoff, unused
    pub emissive: [f32; 4], // rgb + unused
    pub flags: [u32; 4],    // has_base, has_mr, has_normal, has_emissive
    pub mode: [u32; 4],     // x = alpha_mode (0 opaque, 1 mask, 2 blend)
} // 80

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PostU {
    pub exposure: f32,
    pub contrast: f32,
    pub bloom_weight: f32,
    pub pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub bary: [f32; 3],
} // 44 bytes, no padding
pub const BARY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ShadowVpU {
    pub view_proj: [[f32; 4]; 4],
}
```

- [ ] **Step 2: shaders/pbr.wgsl — final form (shadow sampling vs dummies), wireframe included**

```wgsl
// crates/viewer-render/src/shaders/pbr.wgsl
struct Globals {
  view_proj: mat4x4<f32>,
  camera_pos: vec4<f32>,
  env_color: vec4<f32>,   // rgb + intensity
  flags: vec4<u32>,       // x=wireframe
  pad: vec4<f32>,
};
struct Light {
  pos: vec4<f32>,        // xyz + intensity
  color: vec4<f32>,      // rgb
  view_proj: mat4x4<f32>,
  shadow: vec4<f32>,     // x=enabled, y=texel
};
@group(0) @binding(0) var<uniform> G: Globals;
@group(0) @binding(1) var<uniform> L: array<Light, 3>;
@group(0) @binding(2) var shadow0: texture_depth_2d;
@group(0) @binding(3) var shadow1: texture_depth_2d;
@group(0) @binding(4) var shadow2: texture_depth_2d;
@group(0) @binding(5) var shadow_cmp: sampler_comparison;

struct ModelU { model: mat4x4<f32>, normal_mat: mat4x4<f32> };
@group(2) @binding(0) var<uniform> MU: ModelU;

struct MaterialU {
  base_color: vec4<f32>,
  params: vec4<f32>,    // metallic, roughness, alpha_cutoff
  emissive: vec4<f32>,
  flags: vec4<u32>,     // has_base, has_mr, has_normal, has_emissive
  mode: vec4<u32>,      // x=alpha_mode
};
@group(1) @binding(0) var<uniform> M: MaterialU;
@group(1) @binding(1) var base_tex: texture_2d<f32>;
@group(1) @binding(2) var mr_tex: texture_2d<f32>;
@group(1) @binding(3) var normal_tex: texture_2d<f32>;
@group(1) @binding(4) var emissive_tex: texture_2d<f32>;
@group(1) @binding(5) var lin_sampler: sampler;

struct VIn {
  @location(0) pos: vec3<f32>,
  @location(1) normal: vec3<f32>,
  @location(2) uv: vec2<f32>,
  @location(3) bary: vec3<f32>,
};
struct VOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) world: vec3<f32>,
  @location(1) normal: vec3<f32>,
  @location(2) uv: vec2<f32>,
  @location(3) bary: vec3<f32>,
  @location(4) sp0: vec3<f32>,
  @location(5) sp1: vec3<f32>,
  @location(6) sp2: vec3<f32>,
};

@vertex
fn vs_main(v: VIn) -> VOut {
  var o: VOut;
  let world = MU.model * vec4(v.pos, 1.0);
  o.world = world.xyz;
  o.clip = G.view_proj * world;
  o.normal = normalize((MU.normal_mat * vec4(v.normal, 0.0)).xyz);
  o.uv = v.uv;
  o.bary = v.bary;
  let s0 = L[0].view_proj * world;
  let s1 = L[1].view_proj * world;
  let s2 = L[2].view_proj * world;
  o.sp0 = s0.xyz / max(s0.w, 1e-6);
  o.sp1 = s1.xyz / max(s1.w, 1e-6);
  o.sp2 = s2.xyz / max(s2.w, 1e-6);
  return o;
}

fn pcf(tex: texture_depth_2d, uv: vec2<f32>, depth: f32, texel: f32) -> f32 {
  var sum = 0.0;
  for (var dy: i32 = -1; dy <= 1; dy++) {
    for (var dx: i32 = -1; dx <= 1; dx++) {
      sum += textureSampleCompare(tex, shadow_cmp, uv + vec2<f32>(f32(dx), f32(dy)) * texel, depth - 0.0015);
    }
  }
  return sum / 9.0;
}
fn shadow_factor(i: u32, sp: vec3<f32>) -> f32 {
  if (L[i].shadow.x < 0.5) { return 1.0; }
  let uv = vec2<f32>(sp.x * 0.5 + 0.5, 1.0 - (sp.y * 0.5 + 0.5));
  if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || sp.z > 1.0 || sp.z < 0.0) { return 1.0; }
  let texel = L[i].shadow.y;
  if (i == 0u) { return pcf(shadow0, uv, sp.z, texel); }
  if (i == 1u) { return pcf(shadow1, uv, sp.z, texel); }
  return pcf(shadow2, uv, sp.z, texel);
}

fn smith_vis(n_dot_v: f32, n_dot_l: f32, a2: f32) -> f32 {
  let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
  let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
  return 0.5 / max(gv + gl, 1e-4);
}

fn shade_light(light: Light, sp: vec3<f32>, shadow_tex: u32, albedo: vec3<f32>, f0: vec3<f32>,
              rough: f32, N: vec3<f32>, V: vec3<f32>, W: vec3<f32>) -> vec3<f32> {
  let l_vec = light.pos.xyz - W;
  let dist = max(length(l_vec), 1e-4);
  let Ldir = l_vec / dist;
  let n_dot_l = max(dot(N, Ldir), 0.0);
  if (n_dot_l <= 0.0) { return vec3<f32>(0.0); }
  let vis = shadow_factor(shadow_tex, sp);
  if (vis <= 0.0) { return vec3<f32>(0.0); }
  let radiance = light.color.rgb * light.pos.w * 8.0 / (dist * dist + 1.0); // LIGHT_SCALE=8.0
  let H = normalize(V + Ldir);
  let n_dot_h = max(dot(N, H), 0.0);
  let n_dot_v = max(dot(N, V), 1e-4);
  let a = rough * rough;
  let a2 = a * a;
  let dd = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
  let D = a2 / (3.14159265 * dd * dd);
  let Gv = smith_vis(n_dot_v, n_dot_l, a2);
  let F = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - max(dot(H, V), 0.0), 5.0);
  let spec = D * Gv * F;
  let diffuse = albedo * (1.0 - metallic_mask()) / 3.14159265;
  return (diffuse + spec) * radiance * n_dot_l * vis;
}
// NOTE: metallic is passed via a global set in fs_main to keep helper signatures short:
var<private> g_metallic: f32 = 0.0;
fn metallic_mask() -> f32 { return g_metallic; }

fn perturb_normal(N: vec3<f32>, W: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
  let tn = textureSample(normal_tex, lin_sampler, uv).xyz * 2.0 - 1.0;
  // screen-space cotangent frame (no tangents needed)
  let dpdx = dpdx(W);
  let dpdy = dpdy(W);
  let duvdx = dpdx(uv);
  let duvdy = dpdy(uv);
  let det = duvdx.x * duvdy.y - duvdy.x * duvdx.y;
  if (abs(det) < 1e-8) { return N; }
  let inv = 1.0 / det;
  let T = normalize(dpdx * duvdy.y * inv - dpdy * duvdx.y * inv);
  let B = cross(N, T);
  return normalize(T * tn.x + B * tn.y + N * tn.z);
}

@fragment
fn fs_main(f: VOut) -> @location(0) vec4<f32> {
  var base = M.base_color;
  if (M.flags.x > 0u) { base = base * textureSample(base_tex, lin_sampler, f.uv); }
  var metallic = M.params.x;
  var rough = clamp(M.params.y, 0.045, 1.0);
  if (M.flags.y > 0u) {
    let mr = textureSample(mr_tex, lin_sampler, f.uv);
    metallic = mr.b * metallic;
    rough = mr.g * rough;
  }
  g_metallic = metallic;
  var N = normalize(f.normal);
  if (M.flags.z > 0u) { N = perturb_normal(N, f.world, f.uv); }
  if (M.mode.x == 1u && base.a < M.params.z) { discard; }

  let V = normalize(G.camera_pos.xyz - f.world);
  let albedo = base.rgb;
  let f0 = mix(vec3<f32>(0.04), albedo, metallic);
  var col = albedo * G.env_color.rgb * G.env_color.w * 0.35; // ENV_SCALE=0.35
  col += shade_light(L[0], f.sp0, 0u, albedo, f0, rough, N, V, f.world);
  col += shade_light(L[1], f.sp1, 1u, albedo, f0, rough, N, V, f.world);
  col += shade_light(L[2], f.sp2, 2u, albedo, f0, rough, N, V, f.world);
  if (M.flags.w > 0u) {
    col += textureSample(emissive_tex, lin_sampler, f.uv).rgb * M.emissive.rgb * 2.2;
  } else {
    col += M.emissive.rgb;
  }

  if (G.flags.x == 1u) { // wireframe overlay (barycentric edge factor)
    let d = min(f.bary.x, min(f.bary.y, f.bary.z));
    let line = 1.0 - smoothstep(0.6, 1.4, d / fwidth(d));
    col = mix(col, vec3<f32>(0.05, 0.06, 0.09), line * 0.9);
  }
  return vec4<f32>(col, base.a);
}
```

Note: `fwidth` availability — naga supports `fwidth`. If the toolchain rejects `fwidth`, replace with a constant threshold: `let line = 1.0 - smoothstep(0.02, 0.03, d);`.

- [ ] **Step 3: shaders/post.wgsl and shaders/blit.wgsl**

```wgsl
// crates/viewer-render/src/shaders/post.wgsl
struct PostU { exposure: f32, contrast: f32, bloom_weight: f32, pad: f32 };
@group(0) @binding(0) var scene_tex: texture_2d<f32>;   // resolved HDR rgba16f
@group(0) @binding(1) var bloom_tex: texture_2d<f32>;   // half-res chain result (dummy 1x1 black in Task 7)
@group(0) @binding(2) var lin_sampler: sampler;
@group(0) @binding(3) var<uniform> P: PostU;

struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
  var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(3.0, 1.0), vec2<f32>(-1.0, 1.0));
  let xy = p[vi];
  var o: VOut;
  o.pos = vec4<f32>(xy, 0.0, 1.0);
  o.uv = vec2<f32>((xy.x + 1.0) * 0.5, 1.0 - (xy.y + 1.0) * 0.5);
  return o;
}

fn aces(x: vec3<f32>) -> vec3<f32> {
  let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
  return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
  var col = textureSample(scene_tex, lin_sampler, v.uv).rgb;
  if (P.bloom_weight > 0.0) {
    col += textureSample(bloom_tex, lin_sampler, v.uv).rgb * P.bloom_weight;
  }
  col *= exp2(P.exposure);
  col = aces(col);
  col = (col - vec3<f32>(0.5)) * P.contrast + vec3<f32>(0.5);
  return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
```

```wgsl
// crates/viewer-render/src/shaders/blit.wgsl
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
  var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(3.0, 1.0), vec2<f32>(-1.0, 1.0));
  let xy = p[vi];
  var o: VOut;
  o.pos = vec4<f32>(xy, 0.0, 1.0);
  o.uv = vec2<f32>((xy.x + 1.0) * 0.5, 1.0 - (xy.y + 1.0) * 0.5);
  return o;
}
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var lin_sampler: sampler;
@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
  return textureSample(src, lin_sampler, v.uv);
}
```

Note: if the viewport appears upside down after integration, the uv.y flip convention differs on your setup — toggle the `1.0 -` in `post.wgsl`'s uv OR in `blit.wgsl` (exactly one of them), not both.

- [ ] **Step 4: gpu_scene.rs — upload**

```rust
// crates/viewer-render/src/gpu_scene.rs
use std::sync::Arc;
use egui_wgpu::wgpu;
use viewer_scene::{AlphaMode, Image, Scene};

use crate::uniforms::{Bary etc. — see below};

pub struct GpuMesh {
    pub vertex_buf: wgpu::Buffer,
    pub index_buf: wgpu::Buffer,
    pub index_count: u32,
    pub material: usize,
    pub blend: bool, // material alpha_mode == Blend
    pub double_sided: bool,
}

pub struct GpuMaterial {
    pub bind_group: wgpu::BindGroup,
}

pub struct GpuScene {
    pub meshes: Vec<GpuMesh>,
    pub materials: Vec<GpuMaterial>,
    pub scene: Arc<Scene>,
    /// (mesh_id, node_id) per draw
    pub draws: Vec<(usize, usize)>,
}

pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: 44,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute { offset: 0, format: wgpu::VertexFormat::Float32x3, shader_location: 0 },
            wgpu::VertexAttribute { offset: 12, format: wgpu::VertexFormat::Float32x3, shader_location: 1 },
            wgpu::VertexAttribute { offset: 24, format: wgpu::VertexFormat::Float32x2, shader_location: 2 },
            wgpu::VertexAttribute { offset: 32, format: wgpu::VertexFormat::Float32x3, shader_location: 3 },
        ],
    }
}

pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, scene: Arc<Scene>) -> GpuScene {
    let mut meshes = Vec::new();
    let mut draws = Vec::new();
    for (node_id, node) in scene.nodes.iter().enumerate() {
        for &mi in &node.meshes {
            let m = &scene.meshes[mi];
            // de-index into triangle soup so each triangle carries barycentric
            let tri_iter: Box<dyn Iterator<Item = [usize; 3]>> = if m.indices.is_empty() {
                Box::new((0..m.positions.len() / 3).map(|t| [t * 3, t * 3 + 1, t * 3 + 2]))
            } else {
                Box::new(m.indices.chunks_exact(3).map(|c| [c[0] as usize, c[1] as usize, c[2] as usize]))
            };
            let mut verts = Vec::new();
            let mut indices = Vec::new();
            for tri in tri_iter {
                for (k, vi) in tri.iter().enumerate() {
                    verts.push(crate::uniforms::Vertex {
                        pos: m.positions[*vi],
                        normal: m.normals.get(*vi).copied().unwrap_or([0.0, 1.0, 0.0]),
                        uv: m.uvs.get(*vi).copied().unwrap_or([0.0, 0.0]),
                        bary: crate::uniforms::BARY[k],
                    });
                    indices.push(indices.len() as u32);
                }
            }
            let mat = scene.materials.get(m.material);
            let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vertices"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("indices"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            });
            meshes.push(GpuMesh {
                vertex_buf,
                index_buf,
                index_count: indices.len() as u32,
                material: m.material,
                blend: matches!(mat.map(|m| &m.alpha_mode), Some(AlphaMode::Blend)),
                double_sided: mat.map(|m| m.double_sided).unwrap_or(false),
            });
            draws.push((meshes.len() - 1, node_id));
        }
    }
    GpuScene { meshes, materials: Vec::new(), scene, draws }
}

pub fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    img: Option<&Image>,
    fallback: [u8; 4],
    srgb: bool,
) -> wgpu::TextureView {
    let format = if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
    let (w, h, data): (u32, u32, Vec<u8>) = match img {
        Some(i) => (i.width.max(1), i.height.max(1), i.rgba.clone()),
        None => (1, 1, fallback.to_vec()),
    };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("material_tex"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &data,
        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    tex.create_view(&Default::default())
}
```

- [ ] **Step 5: lib.rs — Renderer**

`Renderer::new` creates: bind group layouts (globals incl. shadows; material; model dynamic), pipelines (opaque-cullback, opaque-doublesided, blend), post pipeline (HDR→target_format), blit pipeline (target_format→target_format), uniform buffers (globals, lights, model with dynamic-offset-capable size, post), 1×1 dummy depth textures ×3 + comparison sampler, 1×1 dummy bloom texture, linear sampler, and 1×1 texture-fallback set (white for base/mr, normal-neutral (128,128,255), black emissive).

`resize(w, h)`: recreate `hdr_msaa` (rgba16float, sample_count 4, RENDER_ATTACHMENT), `hdr` resolve (rgba16float, RENDER_ATTACHMENT | TEXTURE_BINDING), `depth_msaa` (depth24plus 4x, RENDER_ATTACHMENT) + `depth resolve not needed`, `final` (target_format, RENDER_ATTACHMENT | TEXTURE_BINDING | COPY_SRC), `bloom_a/b` (Task 10 — create now as 1×1 dummies), `blit_bind_group` for final texture.

`set_scene`: `gpu_scene = Some(gpu_scene::upload(...))`, upload material uniforms/textures, build material bind groups; build model uniform buffer sized `draws.len() * 256` (min 256).

`prepare_render(device, encoder)`:
1. If `self.size == 0` → return. If no gpu_scene → render pass clearing final to clear_color via post pipeline with a black 1×1 scene texture; return.
2. `queue.write_buffer(globals, ...)` with queued `FrameParams` (world transforms from `viewer_scene::world_transforms(scene, overrides)` + turntable post-multiplied around bounds center; `overrides` from anim eval (Task 17; `None` until then).
3. Model uniform per draw slot: `[model, normal_mat]` where `normal_mat` = inverse-transpose model3x3 padded.
4. Scene pass → `hdr_msaa` (resolve → `hdr`), depth `depth_msaa`, clear color from settings: for each draw: pick pipeline (blend? ground after opaque; blend draws sorted far→near by world-pos · view forward); `set_bind_group(0, globals_bg)`, `set_bind_group(2, model_bg, [slot*256])`, `set_bind_group(1, material_bg)`, `draw_indexed(0..count)`.
5. Post pass → `final`: `set_bind_group(0, post_bg)` (scene_tex = hdr view, bloom_tex = dummy, post uniform = exposure/contrast/bloom_weight 0.0 for now — Task 10 feeds real weight).
6. Screenshot: if requested, `encoder.copy_texture_to_buffer(final → staging)`.

`blit(render_pass, [x,y,w,h])`: `set_pipeline(blit)`, `set_bind_group(0, blit_bg)`, `set_viewport`, `draw(0..3)`.

The full `lib.rs` is ~450 lines of mechanical wgpu wiring following the structure above; follow the shader/binding contract exactly (globals group 0 bindings 0-5; material group 1 bindings 0-5; model group 2 binding 0 dynamic offset). For dummy depth textures: `depth32float` 1×1 cleared to 1.0 (write via a tiny render pass with load-op clear); bind as shadow0/1/2 with `shadow.x = 0` (disabled) in light uniforms.

- [ ] **Step 6: app integration — paint.rs + app.rs**

```rust
// crates/viewer-app/src/paint.rs — replace Task 2 content, keep SolidPipeline REMOVED
use std::sync::{Arc, Mutex};
use egui_wgpu::{Callback, CallbackResources, CallbackTrait, RenderState};
use viewer_render::Renderer;

/// Shared between app update() and the wgpu paint callbacks.
pub struct SharedRenderer(pub Arc<Mutex<Renderer>>);

pub fn install(state: &Arc<RenderState>, renderer: Arc<Mutex<Renderer>>) {
    state.renderer.write().callback_resources.insert(SharedRenderer(renderer));
}

pub struct ScenePaint;

impl CallbackTrait for ScenePaint {
    fn prepare(
        &self,
        device: &egui_wgpu::wgpu::Device,
        queue: &egui_wgpu::wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut egui_wgpu::wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<egui_wgpu::wgpu::CommandBuffer> {
        let shared = resources.get::<SharedRenderer>().expect("SharedRenderer installed");
        let mut r = shared.0.lock().expect("renderer mutex");
        r.prepare_render(device, queue, egui_encoder);
        Vec::new()
    }

    fn paint(
        &self,
        info: egui_wgpu::epaint::PaintCallbackInfo,
        render_pass: &mut egui_wgpu::wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let shared = resources.get::<SharedRenderer>().expect("SharedRenderer installed");
        let r = shared.0.lock().expect("renderer mutex");
        let vp = info.viewport_in_pixels();
        r.blit(render_pass, [vp.left_px as f32, vp.top_px as f32, vp.width_px as f32, vp.height_px as f32]);
    }
}
```

Note: `prepare_render` signature here gains `&Queue` (update the Produces block: `prepare_render(&mut self, device, queue, encoder)`).

app.rs gains (replacing the Task 2 stub): camera field (`OrbitCamera`), renderer creation on first update (needs `render_state.device` — Renderer::new + SharedRenderer install once), central panel:
```rust
// inside update(), before panels:
let rect = ui.available_rect_before_wrap();
let cb = egui_wgpu::Callback::new_paint_callback(rect, paint::ScenePaint);
ui.painter().add(cb);
```
Also per-frame: `renderer.queue_frame(FrameParams { ... })` built from camera (aspect = rect aspect), default lights (3 white ×1.0 at rig positions, rig rotation 0), env gray 0.4, settings with the ported constants (exposure 0.5, contrast 1.66, clear color, ground=false until Task 8); `renderer.resize(device, px_w, px_h)` when panel size changes (multiply by `ctx.pixels_per_point()`).

- [ ] **Step 7: Manual smoke verification**

Run: `cargo run -p viewer-app` with a temporary dev-only auto-load: in `ViewerApp::new`, spawn the loader on `crates/viewer-scene/tests` cube fixture path if it exists (remove after verification — Task 8 replaces with real file opening).
Expected: red cube visible, lit from three sides (brighter top-left), ACES-tonemapped, background rgb(69,74,80); drag orbits (wire camera input minimally: allocate_rect + drag_delta → camera.orbit), wheel dollies. No panics on resize.

- [ ] **Step 8: Gate + commit**

Run: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

```bash
git add crates
git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: PBR renderer with HDR tonemap and egui-wgpu integration"
```


---

### Task 8: Ground plane + async loading + file open (M2 complete)

**Files:**
- Create: `crates/viewer-render/src/shaders/ground.wgsl`
- Modify: `crates/viewer-render/src/lib.rs`, `crates/viewer-app/src/{app.rs, panels.rs (new)}`

**Interfaces:**
- Consumes: Task 7 Renderer (`RenderSettings.ground`, `.grid`, `.ground_opacity` already in FrameParams; globals flags.y = grid).
- Produces:
  - Ground disc mesh generated in `Renderer` (unit fan, radius 2.6, 64 segments, `uv.x` = radial 0..1), drawn with `ground.wgsl` using the same group 0 (globals/lights/shadows) and a constant identity model bind (group 2 slot 0).
  - App: `AppState { Idle, Loading, Ready(Arc<Scene>), Failed(String) }`, worker-thread loading via `std::sync::mpsc`, `rfd::FileDialog` open button, drag-and-drop (`ctx.input(|i| i.raw.dropped_files.clone())`), spinner while loading.

- [ ] **Step 1: ground.wgsl**

```wgsl
// crates/viewer-render/src/shaders/ground.wgsl
// Uses the SAME Globals/Light/shadow bindings as pbr.wgsl (group 0, bindings 0-5).
struct VIn { @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) bary: vec3<f32> };
struct VOut {
  @builtin(position) clip: vec4<f32>,
  @location(0) world: vec3<f32>,
  @location(1) uv: vec2<f32>,
  @location(2) sp0: vec3<f32>,
  @location(3) sp1: vec3<f32>,
  @location(4) sp2: vec3<f32>,
};
@vertex
fn vs_main(v: VIn) -> VOut {
  var o: VOut;
  let world = vec4<f32>(v.pos, 1.0); // identity model
  o.world = world.xyz;
  o.clip = G.view_proj * world;
  o.uv = v.uv;
  let s0 = L[0].view_proj * world; o.sp0 = s0.xyz / max(s0.w, 1e-6);
  let s1 = L[1].view_proj * world; o.sp1 = s1.xyz / max(s1.w, 1e-6);
  let s2 = L[2].view_proj * world; o.sp2 = s2.xyz / max(s2.w, 1e-6);
  return o;
}

fn pcf_g(tex: texture_depth_2d, uv: vec2<f32>, depth: f32, texel: f32) -> f32 {
  var sum = 0.0;
  for (var dy: i32 = -1; dy <= 1; dy++) {
    for (var dx: i32 = -1; dx <= 1; dx++) {
      sum += textureSampleCompare(tex, shadow_cmp, uv + vec2<f32>(f32(dx), f32(dy)) * texel, depth - 0.0015);
    }
  }
  return sum / 9.0;
}

@fragment
fn fs_main(f: VOut) -> @location(0) vec4<f32> {
  let radial = f.uv.x;
  let opacity = G.env_color.w * 0.0 + 0.4; // placeholder replaced by uniform below
  // NOTE: ground opacity arrives via Globals.pad.x (set CPU-side from RenderSettings.ground_opacity).
  let ground_alpha = G.pad.x * (1.0 - smoothstep(0.75, 1.0, radial));
  var col = vec3<f32>(0.55, 0.55, 0.55) * G.env_color.rgb * G.env_color.w * 0.35;
  // simple lambert per light with shadows
  let N = vec3<f32>(0.0, 1.0, 0.0);
  for (var i: u32 = 0u; i < 3u; i = i + 1u) {
    let l_vec = L[i].pos.xyz - f.world;
    let dist = max(length(l_vec), 1e-4);
    let Ldir = l_vec / dist;
    let n_dot_l = max(dot(N, Ldir), 0.0);
    var vis = 1.0;
    if (L[i].shadow.x > 0.5 && n_dot_l > 0.0) {
      var uv = vec2<f32>(0.0, 0.0);
      var sp = vec3<f32>(0.0, 0.0, 0.0);
      if (i == 0u) { sp = f.sp0; } else if (i == 1u) { sp = f.sp1; } else { sp = f.sp2; }
      uv = vec2<f32>(sp.x * 0.5 + 0.5, 1.0 - (sp.y * 0.5 + 0.5));
      if (uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0 && sp.z <= 1.0 && sp.z >= 0.0) {
        let texel = L[i].shadow.y;
        if (i == 0u) { vis = pcf_g(shadow0, uv, sp.z, texel); }
        else if (i == 1u) { vis = pcf_g(shadow1, uv, sp.z, texel); }
        else { vis = pcf_g(shadow2, uv, sp.z, texel); }
      }
    }
    col += vec3<f32>(0.55) * L[i].color.rgb * L[i].pos.w * 8.0 / (dist * dist + 1.0) * n_dot_l * vis / 3.14159265;
  }
  if (G.flags.y == 1u) { // grid overlay
    let g = abs(fract(f.world.xz * 2.0 - 0.5) - 0.5) / max(fwidth(f.world.xz * 2.0), vec2<f32>(1e-5));
    let line = 1.0 - min(min(g.x, g.y), 1.0);
    col = mix(col, vec3<f32>(0.3, 0.32, 0.36), line * 0.6);
  }
  return vec4<f32>(col, ground_alpha);
}
```

- [ ] **Step 2: Renderer — ground pipeline + disc + draw**

In `Renderer::new`: create ground pipeline — same vertex layout, `depth_write: false`, blend alpha {src_alpha, one_minus_src_alpha}, fragment target rgba16float. Generate disc vertices (65 verts, fan soup with barycentric), own vertex/index buffer. In `prepare_render`, after opaques and before blend meshes: if `settings.ground` set bind groups (globals group 0 + model group 2 slot 0 = identity) and draw. `G.pad.x` ← `RenderSettings.ground_opacity` (0.4 default).

- [ ] **Step 3: app.rs — state machine + worker loading + open/drop**

```rust
// app.rs additions (exact shape; fill fields from Task 7's struct)
pub enum AppState {
    Idle,
    Loading,
    Ready(std::sync::Arc<viewer_scene::Scene>),
    Failed(String),
}

// in ViewerApp:
fn open_path(&mut self, ctx: &egui::Context, path: std::path::PathBuf) {
    self.state = AppState::Loading;
    let (tx, rx) = std::sync::mpsc::channel::<Result<viewer_scene::Scene, viewer_scene::LoadError>>();
    std::thread::spawn(move || {
        let _ = tx.send(viewer_scene::load(&path));
    });
    self.load_rx = Some(rx);
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
}

fn poll_loading(&mut self, ctx: &egui::Context) {
    if let Some(rx) = &self.load_rx {
        if let Ok(result) = rx.try_recv() {
            self.load_rx = None;
            match result {
                Ok(scene) => {
                    self.camera.fit(scene.bounds.center(), scene.bounds.bounding_sphere_radius());
                    let arc = std::sync::Arc::new(scene);
                    if let Some(rs) = &self.render_state {
                        let shared = rs.renderer.read().callback_resources.get::<paint::SharedRenderer>().cloned();
                        if let Some(shared) = shared {
                            let r = shared.0.lock().unwrap();
                            // set_scene needs device+queue: fetch from render_state
                            r.lock().set_scene(&rs.device, &rs.queue, arc.clone());
                        }
                    }
                    self.state = AppState::Ready(arc);
                }
                Err(e) => self.state = AppState::Failed(e.to_string()),
            }
        }
    }
}
```
(The SharedRenderer Arc cloning pattern above: `SharedRenderer` stores `Arc<Mutex<Renderer>>` and CallbackResources `get` returns a reference — clone the inner Arc out.)

Drag & drop + open button in `panels.rs`:
```rust
// in update(): handle drops
let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
if let Some(file) = dropped.first() {
    if let Some(path) = &file.path { self.open_path(ctx, path.clone()); }
}
// toolbar stub (full toolbar in Task 12): a simple "Open" button for now
if ui.button("Open").clicked() {
    if let Some(p) = rfd::FileDialog::new()
        .add_filter("3D models", &["glb", "gltf", "obj", "stl", "ply", "3mf"])
        .pick_file() { self.open_path(ctx, p); }
}
```
Spinner: `ui.put(rect.center() - vec2(20.0, 20.0), egui::Spinner::new())` while `Loading`.

- [ ] **Step 4: Manual smoke (M2 milestone checklist)**

Run `cargo run -p viewer-app`; open the test cube GLB via the Open dialog (generate one by running the Task 4 test and copying the temp file, or any real GLB you have).
Verify: [ ] model loads with spinner; [ ] sits on a soft radial-gradient ground disc at 40% opacity; [ ] LMB orbit / wheel dolly / RMB pan / double-click reset; [ ] resize window — model and ground keep aspect; [ ] background rgb(69,74,80).

- [ ] **Step 5: Gate + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: ground plane, async loading, file open — M2 complete"
```

---

### Task 9: Shadow maps + PCF

**Files:**
- Create: `crates/viewer-render/src/shaders/shadow.wgsl`
- Modify: `crates/viewer-render/src/lib.rs`

**Interfaces:**
- Consumes: Task 7 shader contract (shadow sampling already present, dummy textures bound, `LightU.shadow.x = 0`).
- Produces: real 512² `depth32float` maps per light; `LightU.shadow = [1.0, 1.0/512.0, 0, 0]` when `LightInput.shadows && settings.shadows enabled`; light ortho fit around `scene.bounds` (radius * 1.25, near 0.05, far `dist + 2.5*r`).

- [ ] **Step 1: shadow.wgsl**

```wgsl
// crates/viewer-render/src/shaders/shadow.wgsl
struct ShadowVp { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> SVP: ShadowVp;
struct ModelU { model: mat4x4<f32>, normal_mat: mat4x4<f32> };
@group(1) @binding(0) var<uniform> MU: ModelU;

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
  return SVP.view_proj * MU.model * vec4<f32>(pos, 1.0);
}
```

- [ ] **Step 2: Renderer — shadow pass**

In `Renderer::new`: 3× depth textures 512² (`depth32float`, RENDER_ATTACHMENT | TEXTURE_BINDING), 3 view+buffers, `shadow.wgsl` pipeline (depth-only, `depth_bias: 2`, `slope_scaled_depth_bias: 1` — constants tuned at smoke), 3 tiny uniform buffers for light view-proj + 3 bind groups, 3 shadow bind groups for group 0 slot (swap the existing dummy textures for real ones in the group 0 bind group).

In `prepare_render`, per light with shadows enabled: compute `Mat4::look_at_rh(light_pos, bounds.center(), Vec3::Y)` + `Mat4::orthographic_rh(-r, r, -r, r, 0.05, dist + 2.5 * r)` where `r = bounds.bounding_sphere_radius() * 1.25`; write to uniform; record depth pass (clear 1.0) drawing all non-blend meshes with the same model dynamic offsets; set `LightU.shadow.x = 1.0` for that light. Lights without shadows keep the dummy (or their stale map) and `shadow.x = 0.0`.

Note the `view` up-vector: if light dir ≈ +Y (top light), use `Vec3::Z` as look_at up to avoid degeneracy.

- [ ] **Step 3: Manual smoke**

Run with a test model. Verify: model casts 3 soft shadows on the ground disc; shadows track light rig rotation; no acne (adjust bias constants if needed); shadow edges are PCF-softened.

- [ ] **Step 4: Gate + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: 512px per-light shadow maps with PCF"
```

---

### Task 10: Bloom chain

**Files:**
- Create: `crates/viewer-render/src/shaders/bloom.wgsl`, `crates/viewer-render/src/post.rs`
- Modify: `crates/viewer-render/src/lib.rs`

**Interfaces:**
- Consumes: Task 7 post.wgsl (already samples `bloom_tex`), resolved `hdr` view.
- Produces: half-res `bloom_a`/`bloom_b` (rgba16float, RENDER_ATTACHMENT | TEXTURE_BINDING); threshold→blur-H→blur-V chain; `PostU.bloom_weight = 0.11` when `RenderSettings.bloom` (exact original default) else `0.0`.

- [ ] **Step 1: bloom.wgsl**

```wgsl
// crates/viewer-render/src/shaders/bloom.wgsl
struct BlurParams { direction: vec2<f32>, threshold: f32, pad: f32 };
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var lin_sampler: sampler;
@group(0) @binding(2) var<uniform> B: BlurParams;

struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
  var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(3.0, 1.0), vec2<f32>(-1.0, 1.0));
  let xy = p[vi];
  var o: VOut;
  o.pos = vec4<f32>(xy, 0.0, 1.0);
  o.uv = vec2<f32>((xy.x + 1.0) * 0.5, 1.0 - (xy.y + 1.0) * 0.5);
  return o;
}
const WEIGHTS = array<f32, 5>(0.227027, 0.1945946, 0.1216216, 0.054054, 0.016216);
@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
  // threshold pass runs when B.threshold >= 0.0; blur passes use threshold = -1.0
  if (B.threshold >= 0.0) {
    let c = textureSample(src, lin_sampler, v.uv).rgb;
    let lum = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    let bright = max(c - vec3<f32>(B.threshold), vec3<f32>(0.0));
    let k = select(0.0, 1.0, lum > B.threshold);
    return vec4<f32>(bright * k, 1.0);
  }
  var sum = textureSample(src, lin_sampler, v.uv).rgb * WEIGHTS[0];
  for (var i = 1u; i < 5u; i = i + 1u) {
    let off = B.direction * f32(i);
    sum += textureSample(src, lin_sampler, v.uv + off).rgb * WEIGHTS[i];
    sum += textureSample(src, lin_sampler, v.uv - off).rgb * WEIGHTS[i];
  }
  return vec4<f32>(sum, 1.0);
}
```

- [ ] **Step 2: Renderer — chain**

`resize` recreates `bloom_a/b` at half final size (min 1). `prepare_render` after resolve, before post (only when `settings.bloom`): threshold pass `hdr → bloom_a` (threshold 1.0), blur `bloom_a → bloom_b` (direction = (1/w, 0)), blur `bloom_b → bloom_a` (direction = (0, 1/h)). Post bind group's `bloom_tex` = `bloom_a` view. `PostU.bloom_weight = 0.11`.

- [ ] **Step 3: Manual smoke**

Load an emissive/bright model (or temporarily boost exposure). Verify soft glow around highlights, no blocky edges, and that toggling bloom (a debug key is fine until Task 12) changes output.

- [ ] **Step 4: Gate + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: bloom threshold/blur chain at original 0.11 weight"
```


---

### Task 11: The 9 lighting themes + Lighting panel

**Files:**
- Create: `crates/viewer-app/src/themes.rs`
- Modify: `crates/viewer-app/src/{panels.rs, app.rs, main.rs}`

**Interfaces:**
- Consumes: `viewer_render::{LightInput, FrameParams}`; spec Appendix A data (verbatim).
- Produces:
  ```rust
  pub struct ThemeLight { pub rgb: [u8; 3], pub intensity: f32 }   // intensity: 100 = 1.0
  pub struct Theme { pub name: &'static str, pub lights: [ThemeLight; 3], pub env: ThemeLight, pub rig_rot_deg: f32 }
  pub const THEMES: [Theme; 9];
  pub fn light_pos(i: usize, rig_rot_deg: f32) -> [f32; 3]  // 120° spacing, elev 40°, radius 2.2
  pub fn frame_lights(theme: &Theme, user_mult: [f32; 3], env_mult: f32) -> ([LightInput; 3], ([f32; 3], f32))
  ```
  App fields: `theme_idx: usize`, `light_mult: [f32; 3]`, `env_mult: f32`.

- [ ] **Step 1: themes.rs — exact data from `Themes.json`**

```rust
// crates/viewer-app/src/themes.rs
use viewer_render::LightInput;

pub struct ThemeLight { pub rgb: [u8; 3], pub intensity: f32 }
pub struct Theme { pub name: &'static str, pub lights: [ThemeLight; 3], pub env: ThemeLight, pub rig_rot_deg: f32 }

const fn t(rgb: [u8; 3], i: f32) -> ThemeLight { ThemeLight { rgb, intensity: i } }

pub const THEMES: [Theme; 9] = [
    Theme { name: "Default",       lights: [t([255,255,255],100.0), t([255,255,255],100.0), t([255,255,255],100.0)], env: t([255,255,255],40.0), rig_rot_deg: 0.0 },
    Theme { name: "Campfire",      lights: [t([53,34,122],48.0),   t([255,255,188],15.0),  t([255,68,0],200.0)],   env: t([33,37,104],31.0),  rig_rot_deg: 145.5 },
    Theme { name: "Cotton Candy",  lights: [t([255,40,201],160.0), t([110,234,226],164.0), t([211,46,46],66.0)],   env: t([161,198,197],30.0), rig_rot_deg: 189.4 },
    Theme { name: "Glow in the Dark", lights: [t([45,255,52],180.0), t([232,232,232],19.0), t([0,0,0],0.0)],       env: t([0,2,158],82.0),   rig_rot_deg: 345.0 },
    Theme { name: "Moonlight",      lights: [t([255,250,173],97.0), t([52,46,119],74.0),  t([255,255,255],13.0)],  env: t([0,35,150],34.0),  rig_rot_deg: 348.0 },
    Theme { name: "Paint3D",        lights: [t([255,194,168],88.0), t([73,76,255],117.0), t([0,0,0],0.0)],         env: t([255,255,255],58.0), rig_rot_deg: 225.0 },
    Theme { name: "Silhouette",     lights: [t([0,0,0],0.0),        t([255,255,255],200.0), t([255,255,255],0.0)],  env: t([255,255,255],3.0),  rig_rot_deg: 240.8 },
    Theme { name: "Studio",         lights: [t([151,199,201],111.0), t([255,251,232],92.0), t([255,248,239],43.0)], env: t([211,211,211],103.0), rig_rot_deg: 155.5 },
    Theme { name: "Sunset",         lights: [t([255,144,0],200.0),  t([84,106,255],170.0), t([0,0,0],0.0)],         env: t([81,68,147],64.0),  rig_rot_deg: 46.0 },
];

fn srgb_to_linear(c: u8) -> f32 {
    let v = c as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}
fn lin3(rgb: [u8; 3]) -> [f32; 3] {
    [srgb_to_linear(rgb[0]), srgb_to_linear(rgb[1]), srgb_to_linear(rgb[2])]
}

/// Light rig: 3 lights, 120° apart, elevation 40°, radius 2.2 around the model.
pub fn light_pos(i: usize, rig_rot_deg: f32) -> [f32; 3] {
    const RADIUS: f32 = 2.2;
    let elev = 40.0f32.to_radians();
    let az = (rig_rot_deg + 120.0 * i as f32).to_radians();
    [RADIUS * elev.cos() * az.sin(), RADIUS * elev.sin(), RADIUS * elev.cos() * az.cos()]
}

/// Theme intensities: 100 = 1.0 multiplier (original's percent scale).
pub fn frame_lights(theme: &Theme, user_mult: [f32; 3], env_mult: f32) -> ([LightInput; 3], ([f32; 3], f32)) {
    let lights = std::array::from_fn(|i| LightInput {
        pos: light_pos(i, theme.rig_rot_deg),
        color: lin3(theme.lights[i].rgb),
        intensity: theme.lights[i].intensity / 100.0 * user_mult[i],
        shadows: theme.lights[i].intensity > 0.0 && user_mult[i] > 0.0,
    });
    let env = (lin3(theme.env.rgb), theme.env.intensity / 100.0 * env_mult);
    (lights, env)
}
```

- [ ] **Step 2: theme unit test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nine_themes_and_rig_geometry() {
        assert_eq!(THEMES.len(), 9);
        assert_eq!(THEMES[0].name, "Default");
        assert_eq!(THEMES[8].name, "Sunset");
        let p = light_pos(0, 0.0);
        assert!((p[1] - 2.2 * 40.0f32.to_radians().sin()).abs() < 1e-4); // y ≈ 1.414
        assert!(p[2] > 1.5); // in front (+Z)
        let q = light_pos(1, 0.0);
        let angle = (q[0].atan2(q[2]) - p[0].atan2(p[2])).to_degrees();
        assert!((angle - 120.0).abs() < 0.01);
        let (lights, env) = frame_lights(&THEMES[7], [1.0; 3], 1.0);
        assert!(lights[0].shadows);
        let (_, e) = env;
        assert!((e - 1.03).abs() < 1e-4); // Studio env 103/100
    }
}
```

- [ ] **Step 3: Lighting panel (panels.rs)**

Right settings panel section "Lighting": theme swatch grid (3 columns of buttons; each button's fill = theme's light-0 color as visual identity, `ui.add(egui::Button::new(name).fill(...))`), selected theme highlighted; below: per-light intensity sliders (DragValue 0..3), environment intensity slider (0..3), a "Shadows" toggle mapping to `LightInput.shadows` override.

- [ ] **Step 4: Wire into app + smoke**

App builds `FrameParams.lights` via `frame_lights(&THEMES[self.theme_idx], self.light_mult, self.env_mult)`. Smoke: cycle all 9 themes on a test model — Campfire glows orange/teal, Silhouette produces a bright rim on dark, Studio is neutral. Compare side-by-side with reference screenshots from the original app.

- [ ] **Step 5: Gate + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: 9 original lighting themes + lighting panel"
```

---

### Task 12: Wireframe + Effects panel + left toolbar

**Files:**
- Modify: `crates/viewer-app/src/{panels.rs, app.rs}`

**Interfaces:**
- Consumes: Task 7 `RenderSettings` flags (shader already handles wireframe via `G.flags.x`, grid via `G.flags.y`).
- Produces: left vertical toolbar (Open, Turntable toggle — non-functional until Task 13, Screenshot — until Task 13, Settings toggle); Effects panel section: wireframe, ground plane, grid, bloom checkboxes bound to app state feeding `RenderSettings`.

- [ ] **Step 1: Toolbar (panels.rs)**

```rust
pub fn toolbar(ui: &mut egui::Ui, app: &mut ViewerApp) {
    ui.set_width(56.0);
    if ui.button("Open").clicked() {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("3D models", &["glb", "gltf", "obj", "stl", "ply", "3mf"])
            .pick_file() { app.request_open(p); }
    }
    app.turntable = ui.toggle_button("Turntable", app.turntable);  // wired in Task 13
    if ui.button("Screenshot").clicked() { app.request_screenshot(); }  // wired in Task 13
    app.settings_open = ui.toggle_button("Settings", app.settings_open);
}
```
Note: egui has no `toggle_button` builtin — implement a 3-line helper: `Button::new(text)` with `.fill(selected color)` that returns the clicked bool. Drag-and-drop and keyboard: keep existing behavior. Turntable/Screenshot buttons set pending flags that Task 13 consumes; until then they're inert (that's why Task 12 precedes 13 without breaking).

- [ ] **Step 2: Effects panel**

Settings panel section "Effects": checkboxes — Wireframe, Ground plane (default on), Grid (default on, matching the original's visible grid over the ground), Bloom (default on at 0.11). Each writes straight into the per-frame `RenderSettings`.

- [ ] **Step 3: Smoke + gate + commit**

Verify wireframe overlay (dark lines over lit surfaces), grid lines over the ground disc, toolbar layout on the left edge.
```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: wireframe/effects panel and left toolbar"
```

---

### Task 13: Turntable + Camera/Model panels + screenshot (M3 complete)

**Files:**
- Modify: `crates/viewer-app/src/{app.rs, panels.rs}`

**Interfaces:**
- Consumes: `FrameParams.turntable_deg`; Task 12 toolbar buttons.
- Produces: turntable at default 30°/s (configurable slider, honest note: exact original rate not extracted); Model panel (name, tri/vert counts, rotation offset slider, scale slider); Camera panel (FOV, exposure, contrast, reset view); screenshot → PNG via `rfd::FileDialog::save_file`.

- [ ] **Step 1: Turntable**

App fields: `turntable_deg: f32`, `turntable_speed: f32 = 30.0` (deg/s). In `update()`:
```rust
if self.turntable {
    let dt = ctx.input(|i| i.unstable_dt).min(0.1);
    self.turntable_deg = (self.turntable_deg + self.turntable_speed * dt) % 360.0;
    ctx.request_repaint();
}
```
`FrameParams.turntable_deg` already applied by the renderer as a Y rotation around the bounds center.

- [ ] **Step 2: Model + Camera panels (panels.rs)**

Model: `scene.source_name`, tri/vert counts (from `Scene`), rotation offset slider (0..360, adds to `turntable_deg` static offset), scale slider (0.1..10, multiplies model matrix — renderer applies as extra uniform: pass via `RenderSettings`? No — add `FrameParams.model_scale: f32` defaulting 1.0; renderer scales about bounds center). Camera: FOV (20..90, default 45), exposure (−3..3, default 0.5), contrast (0.2..3.0, default 1.66), "Reset view" button → `camera.fit(bounds)`.
(This adds one field to `FrameParams` — update Task 7's struct accordingly.)

- [ ] **Step 3: Screenshot**

App: `request_screenshot()` sets renderer flag via SharedRenderer. Each frame after `prepare`-side render, call `renderer.collect_screenshot(&device)`; when `Some(img)`:
```rust
if let Some(path) = rfd::FileDialog::new()
    .set_file_name("screenshot.png")
    .add_filter("PNG", &["png"])
    .save_file() {
    let _ = img.save(&path); // image::RgbaImage::save
}
```
`collect_screenshot` (viewer-render): copy `final` texture → staging buffer (COPY_SRC already set), `pollster::block_on(map_async(MapMode::Read))` + `device.poll(PollType::Wait)`, convert BGRA→RGBA if `target_format` is `Bgra8*` (check format string), build `image::RgbaImage`. Exclude the egui UI — screenshot is the 3D scene only, matching the original's behavior of capturing the model view.

- [ ] **Step 4: M3 milestone smoke checklist**

[ ] Turntable spins ~12 s/rev, light rig stays fixed; [ ] rotation offset + scale sliders work; [ ] FOV/exposure/contrast sliders update live; [ ] screenshot PNG saved and opens correctly (colors match screen, not flipped — if flipped vertically, the readback row order needs reversing; fix in `collect_screenshot`); [ ] all 9 themes still correct; [ ] fmt/clippy/test green.

- [ ] **Step 5: Gate + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: turntable, camera/model panels, screenshot — M3 complete"
```


---

### Task 14: OBJ loader (tobj)

**Files:**
- Create: `crates/viewer-scene/src/loaders/obj_loader.rs`
- Modify: `crates/viewer-scene/src/loaders/mod.rs`
- Test: `crates/viewer-scene/tests/obj_load.rs` + fixture `tests/assets/cube.obj`, `tests/assets/cube.mtl`

**Interfaces:**
- Consumes: Task 3 types, Task 5 normalize (via dispatch).
- Produces: `obj_loader::load(path) -> Result<Scene, LoadError>`; dispatch wires `Some("obj")`. MTL mapping: `Kd`→base_color (sRGB→linear), `map_Kd`→base color texture (decoded via `image` crate), `Ns`→`roughness = clamp(1 − Ns/1000, 0.05, 1)`, `Ks`→metallic heuristic (`luminance(Ks)`>0.6 → 0.5 else 0.0).

- [ ] **Step 1: Write fixtures**

```
# crates/viewer-scene/tests/assets/cube.obj
mtllib cube.mtl
o cube
v -0.5 -0.5 -0.5
v 0.5 -0.5 -0.5
v -0.5 0.5 -0.5
v 0.5 0.5 -0.5
v -0.5 -0.5 0.5
v 0.5 -0.5 0.5
v -0.5 0.5 0.5
v 0.5 0.5 0.5
usemtl red
f 1 3 2  2 3 4
f 5 6 8  5 8 7
f 1 5 7  1 7 3
f 2 4 8  2 8 6
f 3 7 8  3 8 4
f 1 2 6  1 6 5
```

```
# crates/viewer-scene/tests/assets/cube.mtl
newmtl red
Kd 1.0 0.1 0.1
Ks 0.1 0.1 0.1
Ns 250.0
```

- [ ] **Step 2: Write failing test**

```rust
// crates/viewer-scene/tests/obj_load.rs
use std::path::Path;

fn fixture(p: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/assets").join(p)
}

#[test]
fn loads_obj_with_mtl() {
    let scene = viewer_scene::load(&fixture("cube.obj")).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.meshes[0].positions.len(), 8);
    // 12 triangles as soup after tobj triangulation: indices count 36
    assert_eq!(scene.meshes[0].indices.len(), 36);
    let m = &scene.materials[0];
    // Kd 1.0 0.1 0.1 is sRGB → linear ≈ (1.0, 0.01, 0.01)
    assert!(m.base_color[0] > 0.9 && m.base_color[1] < 0.02);
    // Ns 250 → roughness = 1 - 0.25 = 0.75
    assert!((m.roughness - 0.75).abs() < 0.01);
    // normalized: bounding sphere radius ≈ 1
    assert!((scene.bounds.bounding_sphere_radius() - 1.0).abs() < 1e-3);
}
```

- [ ] **Step 3: Run to verify it fails** — `cargo test -p viewer-scene obj_load`

- [ ] **Step 4: Implement obj_loader.rs**

```rust
// crates/viewer-scene/src/loaders/obj_loader.rs
use std::path::Path;

use crate::error::LoadError;
use crate::scene::{Image, Material, Mesh, Scene};

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    let (models, materials) = tobj::load_obj(
        path,
        &tobj::LoadOptions { single_index: true, triangulate: true, ..Default::default() },
    )
    .map_err(|e| LoadError::Parse { format: "OBJ", detail: e.to_string() })?;
    let materials = materials.map_err(|e| LoadError::Parse { format: "OBJ", detail: format!("MTL: {e}") })?;
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut scene = Scene::new(name);

    for mat in &materials {
        scene.materials.push(convert_mtl(mat, path.parent().unwrap_or(Path::new(".")), &mut scene.warnings));
    }
    if scene.materials.is_empty() {
        scene.materials.push(Material::default_pbr());
    }
    let node = scene.add_node(scene.root, crate::scene::Trs::identity(), Some("obj".into()));
    for m in &models {
        let mesh = &m.mesh;
        let material = mesh.material_id.unwrap_or(0).min(scene.materials.len().saturating_sub(1));
        let our = scene.add_mesh(Mesh {
            positions: mesh.positions.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            normals: mesh.normals.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            uvs: mesh.texcoords.chunks_exact(2).map(|c| [c[0], c[1]]).collect(),
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
        base_color: [srgb_to_linear(kd[0]), srgb_to_linear(kd[1]), srgb_to_linear(kd[2]), 1.0],
        roughness: mat.shininess.map(|ns| (1.0 - ns / 1000.0).clamp(0.05, 1.0)).unwrap_or(0.5),
        metallic: 0.0,
        ..Material::default_pbr()
    };
    if let Some(ks) = mat.specular {
        let lum = 0.2126 * ks[0] + 0.7152 * ks[1] + 0.0722 * ks[2];
        if lum > 0.6 { m.metallic = 0.5; }
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
        .map_err(|e| LoadError::Parse { format: "image", detail: e.to_string() })?
        .to_rgba8();
    Ok(Image { width: img.width(), height: img.height(), rgba: img.into_raw() })
}
```

- [ ] **Step 5: Wire dispatch** — replace `Some("obj")` arm with `obj_loader::load(path)`.

- [ ] **Step 6: Run tests** — expect pass. Also load a real-world OBJ if available as manual smoke.

- [ ] **Step 7: Gate + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: OBJ+MTL loader"
```

---

### Task 15: STL loader

**Files:**
- Create: `crates/viewer-scene/src/loaders/stl_loader.rs`
- Modify: `crates/viewer-scene/src/loaders/mod.rs`
- Test: `crates/viewer-scene/tests/stl_load.rs`

**Interfaces:**
- Consumes/Produces as per OBJ task; parser handles binary and ASCII, both → triangle soup (`positions` 3-per-tri, no indices), normals filled by Task 5 `compute_normals` (STL face normals are per-triangle only).

- [ ] **Step 1: Failing test — build binary STL in code**

```rust
// crates/viewer-scene/tests/stl_load.rs
use std::io::Write;

fn binary_stl_one_triangle() -> Vec<u8> {
    let mut b: Vec<u8> = Vec::new();
    b.extend_from_slice(&[0u8; 80]);                 // header
    b.extend_from_slice(&1u32.to_le_bytes());        // tri count
    // normal + 3 verts (f32 LE) + attribute
    let n = [0.0f32, 0.0, 1.0];
    let v = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    b.extend_from_slice(&n.map(|f| f.to_le_bytes()).concat());
    for vert in v { b.extend_from_slice(&vert.map(|f| f.to_le_bytes()).concat()); }
    b.extend_from_slice(&0u16.to_le_bytes());
    b
}

const ASCII_STL: &str = r#"
solid test
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
  facet normal 0 0 1
    outer loop
      vertex 0 1 0
      vertex 1 0 0
      vertex 1 1 0
    endloop
  endfacet
endsolid test
"#;

#[test]
fn loads_binary_and_ascii_stl() {
    let dir = std::env::temp_dir().join("viewer-stl-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("t.stl");
    std::fs::File::create(&bin).unwrap().write_all(&binary_stl_one_triangle()).unwrap();
    let s1 = viewer_scene::load(&bin).unwrap();
    assert_eq!(s1.meshes[0].positions.len(), 3);
    assert_eq!(s1.tri_count, 1);
    // smooth normals computed: all ≈ +Z
    let n = s1.meshes[0].normals[0];
    assert!(n[2] > 0.99);

    let asc = dir.join("a.stl");
    std::fs::write(&asc, ASCII_STL).unwrap();
    let s2 = viewer_scene::load(&asc).unwrap();
    assert_eq!(s2.meshes[0].positions.len(), 6);
    assert_eq!(s2.tri_count, 2);
    assert!((s2.bounds.bounding_sphere_radius() - 1.0).abs() < 1e-3); // normalized
}
```

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement stl_loader.rs**

```rust
// crates/viewer-scene/src/loaders/stl_loader.rs
use std::path::Path;
use crate::error::LoadError;
use crate::scene::{Mesh, Scene};

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    let bytes = std::fs::read(path)?;
    let tris = if is_binary(&bytes) { parse_binary(&bytes)? } else { parse_ascii(&bytes)? };
    if tris.is_empty() {
        return Err(LoadError::Parse { format: "STL", detail: "no facets found".into() });
    }
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut scene = Scene::new(name);
    scene.materials.push(crate::scene::Material::default_pbr());
    let mut positions = Vec::with_capacity(tris.len() * 9);
    for t in &tris {
        for v in [t.a, t.b, t.c] {
            positions.push(v);
        }
    }
    let mesh = Mesh { positions, normals: vec![], uvs: vec![], indices: vec![], material: 0 };
    let node = scene.add_node(scene.root, crate::scene::Trs::identity(), Some("stl".into()));
    let mi = scene.add_mesh(mesh);
    scene.nodes[node].meshes.push(mi);
    Ok(scene)
}

#[derive(Clone, Copy)]
struct Tri { a: [f32; 3], b: [f32; 3], c: [f32; 3] }

fn is_binary(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        return false;
    }
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    bytes.len() == 84 + n * 50
}

fn parse_binary(bytes: &[u8]) -> Result<Vec<Tri>, LoadError> {
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    let mut tris = Vec::with_capacity(n);
    let mut off = 84usize;
    for _ in 0..n {
        if off + 50 > bytes.len() {
            return Err(LoadError::Parse { format: "STL", detail: "truncated facet".into() });
        }
        let f = |i: usize| f32::from_le_bytes([bytes[off + i], bytes[off + i + 1], bytes[off + i + 2], bytes[off + i + 3]]);
        let a = [f(12), f(16), f(20)];
        let b = [f(24), f(28), f(32)];
        let c = [f(36), f(40), f(44)];
        tris.push(Tri { a, b, c });
        off += 50;
    }
    Ok(tris)
}

fn parse_ascii(bytes: &[u8]) -> Result<Vec<Tri>, LoadError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| LoadError::Parse { format: "STL", detail: "invalid UTF-8 in ASCII STL".into() })?;
    let mut tris = Vec::new();
    let mut verts: Vec<[f32; 3]> = Vec::new();
    for line in text.lines() {
        let tok: Vec<&str> = line.split_whitespace().collect();
        match tok.first() {
            Some(&"vertex") if tok.len() >= 4 => {
                let p: [f32; 3] = [
                    tok[1].parse().map_err(|_| bad("vertex x"))?,
                    tok[2].parse().map_err(|_| bad("vertex y"))?,
                    tok[3].parse().map_err(|_| bad("vertex z"))?,
                ];
                verts.push(p);
                if verts.len() == 3 {
                    tris.push(Tri { a: verts[0], b: verts[1], c: verts[2] });
                    verts.clear();
                }
            }
            _ => {}
        }
    }
    Ok(tris)
}

fn bad(what: &'static str) -> LoadError {
    LoadError::Parse { format: "STL", detail: format!("bad {what} value").into() }
}
```

- [ ] **Step 4: Wire dispatch, run tests, gate, commit**

```bash
cargo test -p viewer-scene && cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: STL binary+ascii loader"
```

---

### Task 16: PLY loader

**Files:**
- Create: `crates/viewer-scene/src/loaders/ply_loader.rs`
- Modify: `crates/viewer-scene/src/loaders/mod.rs`
- Test: `crates/viewer-scene/tests/ply_load.rs`

**Interfaces:**
- Same as STL task. PLY scope: `format ascii 1.0` and `binary_little_endian 1.0`; `vertex` element with `x,y,z` required, `nx,ny,nz` optional, `red,green,blue` (uchar) optional; `face` element with `property list uchar int vertex_indices` — polygon fans triangulated.

- [ ] **Step 1: Failing test**

```rust
// crates/viewer-scene/tests/ply_load.rs
const ASCII_PLY: &str = r#"
ply
format ascii 1.0
element vertex 4
property float x
property float y
property float z
property float nx
property float ny
property float nz
property uchar red
property uchar green
property uchar blue
element face 1
property list uchar int vertex_indices
end_header
0 0 0 0 0 1 255 0 0
1 0 0 0 0 1 255 0 0
0 1 0 0 0 1 255 0 0
1 1 0 0 0 1 255 0 0
4 0 1 2 3
"#;

#[test]
fn loads_ascii_ply_with_colors_and_normals() {
    let dir = std::env::temp_dir().join("viewer-ply-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.ply");
    std::fs::write(&p, ASCII_PLY).unwrap();
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.meshes[0].positions.len(), 4);
    assert_eq!(s.tri_count, 2); // quad → 2 tris via fan
    assert_eq!(s.meshes[0].normals[0], [0.0, 0.0, 1.0]);
    assert!((s.materials[0].base_color[0] - 1.0).abs() < 1e-3); // red 255 → linear ≈ 1.0
}
```

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement ply_loader.rs**

Structure (full file ~170 lines):
1. Read whole file; parse header line-by-line until `end_header`: track `format` (ascii/binary_little_endian — anything else → `Parse` error), `element <name> <count>` and subsequent `property <type> <name>` (supporting scalar types `char,uchar,short,ushort,int,uint,float,float32,double` and `property list <count_type> <index_type> <name>`).
2. `parse_ascii`: read `vertex` count × N scalars in property order (whitespace tokens), then `face` count × list entries; fan-triangulate indices `i0, i1, i2..`.
3. `parse_binary`: fixed offsets per property (`element_size` table), vertex block tightly packed; face block sequential with per-face `uchar` count + `int` indices.
4. Map: `x,y,z` → positions (required — error if absent); `nx,ny,nz` → normals (optional, else `compute_normals` fills); `red,green,blue` → average vertex color → material `base_color` (sRGB→linear) if ≥ 60% of vertices carry colors; faces → indices; no faces → soup with empty indices.
5. Build Scene: one node "ply", one mesh, default material (or color-derived), per the STL loader pattern.

Follow the STL loader's file layout exactly (same structure/registration in mod.rs).

- [ ] **Step 4: Wire dispatch, run tests, gate, commit**

```bash
cargo test -p viewer-scene && cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: PLY ascii+binary loader"
```


---

### Task 17: glTF animation evaluation + animation bar

**Files:**
- Create: `crates/viewer-scene/src/anim_eval.rs`
- Modify: `crates/viewer-scene/src/lib.rs`, `crates/viewer-render/src/lib.rs`, `crates/viewer-app/src/{app.rs, panels.rs}`

**Interfaces:**
- Consumes: Task 4 channel data (verified against glTF 2.0 spec: CUBICSPLINE output per key = [in-tangent, value, out-tangent]; interpolated quats must be normalized; tangents scale by segment dt).
- Produces:
  - `pub fn evaluate(scene: &Scene, anim: usize, time: f32) -> Vec<Option<Trs>>` — one entry per node; `time` wraps at `duration`. Step/Linear/CubicSpline for T/R/S (hermite for T/S, slerp for R with normalized result).
  - Renderer: `FrameParams.anim: Option<(clip, time)>` (already declared in Task 7) — `prepare_render` calls `viewer_scene::evaluate` and passes overrides into `world_transforms`.
  - App: `AnimationState { clip: usize, playing: bool, time: f32, speed: f32 }`; bottom animation bar when `scene.animations` non-empty.

- [ ] **Step 1: Failing tests (anim_eval.rs inline)**

```rust
#[cfg(test)]
mod tests {
    use crate::animation::{Animation, Channel, Interpolation, Keyframes, TrsProperty};
    use crate::scene::{Scene, Trs};
    use crate::anim_eval::evaluate;
    use glam::Quat;

    fn scene_with_translation_anim() -> (Scene, usize) {
        let mut s = Scene::new("t".into());
        let node = s.add_node(s.root, Trs::identity(), None);
        s.nodes[node].transform = Trs::identity();
        s.animations.push(Animation {
            name: "move".into(),
            duration: 2.0,
            channels: vec![Channel {
                node,
                property: TrsProperty::Translation,
                keys: Keyframes {
                    times: vec![0.0, 2.0],
                    values: vec![[0.0, 0.0, 0.0, 0.0], [2.0, 0.0, 0.0, 0.0]],
                    in_tangents: vec![],
                    out_tangents: vec![],
                    interpolation: Interpolation::Linear,
                },
            }],
        });
        (s, node)
    }

    #[test]
    fn linear_midpoint() {
        let (s, node) = scene_with_translation_anim();
        let pose = evaluate(&s, 0, 1.0);
        assert_eq!(pose[node].as_ref().unwrap().translation.x, 1.0);
    }

    #[test]
    fn wraps_at_duration() {
        let (s, node) = scene_with_translation_anim();
        let pose = evaluate(&s, 0, 2.5); // wraps to 0.5
        assert!((pose[node].as_ref().unwrap().translation.x - 0.5).abs() < 1e-5);
    }

    #[test]
    fn rotation_slerp_normalized() {
        let mut s = Scene::new("t".into());
        let node = s.add_node(s.root, Trs::identity(), None);
        s.animations.push(Animation {
            name: "spin".into(),
            duration: 1.0,
            channels: vec![Channel {
                node,
                property: TrsProperty::Rotation,
                keys: Keyframes {
                    times: vec![0.0, 1.0],
                    values: vec![[0.0, 0.0, 0.0, 1.0], [0.0, 0.3826834, 0.0, 0.9238795]],
                    in_tangents: vec![],
                    out_tangents: vec![],
                    interpolation: Interpolation::Linear,
                },
            }],
        });
        let pose = evaluate(&s, 0, 0.5);
        let q = pose[node].as_ref().unwrap().rotation;
        assert!((q.length() - 1.0).abs() < 1e-5); // normalized
        let angle = 2.0 * q.w.acos();
        assert!((angle - std::f32::consts::FRAC_PI_4).abs() < 0.01); // 45°
    }

    #[test]
    fn step_holds_value() {
        let (mut s, node) = scene_with_translation_anim();
        s.animations[0].channels[0].keys.interpolation = Interpolation::Step;
        let pose = evaluate(&s, 0, 1.9);
        assert_eq!(pose[node].as_ref().unwrap().translation.x, 0.0); // holds first key
    }
}
```

- [ ] **Step 2: Run to verify failure**

- [ ] **Step 3: Implement anim_eval.rs**

```rust
// crates/viewer-scene/src/anim_eval.rs
use glam::{Quat, Vec3};

use crate::scene::{Scene, Trs};
use crate::animation::{Interpolation, TrsProperty};

/// Evaluate an animation at `time` (wraps at duration). Returns per-node TRS overrides.
pub fn evaluate(scene: &Scene, anim: usize, time: f32) -> Vec<Option<Trs>> {
    let mut out: Vec<Option<Trs>> = vec![None; scene.nodes.len()];
    let Some(a) = scene.animations.get(anim) else { return out };
    let t = if a.duration > 0.0 { time.rem_euclid(a.duration) } else { time };
    for ch in &a.channels {
        let Some(node) = scene.nodes.get(ch.node) else { continue };
        let base = &node.transform;
        let v = sample(&ch.keys, t);
        let trs = out[ch.node].get_or_insert_with(|| *base);
        match ch.property {
            TrsProperty::Translation => trs.translation = Vec3::new(v[0], v[1], v[2]),
            TrsProperty::Rotation => {
                trs.rotation = Quat::from_xyzw(v[0], v[1], v[2], v[3]).normalize();
            }
            TrsProperty::Scale => trs.scale = Vec3::new(v[0], v[1], v[2]),
        }
    }
    out
}

fn find_segment(times: &[f32], t: f32) -> (usize, usize) {
    // returns (i, j) with times[i] <= t <= times[j]
    if times.len() < 2 {
        return (0, times.len().saturating_sub(1));
    }
    for i in 0..times.len() - 1 {
        if t >= times[i] && t <= times[i + 1] {
            return (i, i + 1);
        }
    }
    if t < times[0] { (0, 1) } else { (times.len() - 2, times.len() - 1) }
}

fn sample(k: &crate::animation::Keyframes, t: f32) -> [f32; 4] {
    if k.values.is_empty() { return [0.0; 4]; }
    if k.values.len() == 1 || t <= k.times[0] { return k.values[0]; }
    if t >= *k.times.last().expect("non-empty") { return *k.values.last().expect("non-empty"); }
    let (i, j) = find_segment(&k.times, t);
    let (t0, t1) = (k.times[i], k.times[j]);
    let dt = (t1 - t0).max(1e-6);
    let u = (t - t0) / dt;
    match k.interpolation {
        Interpolation::Step => k.values[i],
        Interpolation::Linear => lerp4(k.values[i], k.values[j], u),
        Interpolation::CubicSpline => {
            // Hermite: h00*p0 + h10*(dt*m0) + h01*p1 + h11*(dt*m1)
            let m0 = k.out_tangents.get(i).copied().unwrap_or([0.0; 4]);
            let m1 = k.in_tangents.get(j).copied().unwrap_or([0.0; 4]);
            let (h00, h10, h01, h11) =
                (2.0 * u * u * u - 3.0 * u * u + 1.0, u * u * u - 2.0 * u * u + u,
                 -2.0 * u * u * u + 3.0 * u * u, u * u * u - u * u);
            let p0 = k.values[i];
            let p1 = k.values[j];
            let mut out = [0.0f32; 4];
            for c in 0..4 {
                out[c] = h00 * p0[c] + h10 * dt * m0[c] + h01 * p1[c] + h11 * dt * m1[c];
            }
            out
        }
    }
}

fn lerp4(a: [f32; 4], b: [f32; 4], u: f32) -> [f32; 4] {
    let mut o = [0.0f32; 4];
    for c in 0..4 { o[c] = a[c] + (b[c] - a[c]) * u; }
    o
}
```

Note: Linear rotation technically requires slerp; component lerp + `normalize()` (done on assignment in `evaluate`) is an acceptable approximation for a viewer and dramatically simpler. The test asserts 45° which lerp+normalize satisfies within tolerance. Document this in code comments.

- [ ] **Step 4: Renderer + app integration**

Renderer `prepare_render`: if `params.anim = Some((clip, t))`, `let overrides = viewer_scene::anim_eval::evaluate(&gpu_scene.scene, clip, t);` then `world_transforms(&scene, &overrides)` (Task 5's signature already accepts overrides).
App `AnimationState`: when `Ready` and animations exist, bottom panel: clip `egui::ComboBox`, play/pause button, `egui::Slider::new(&mut time, 0.0..=duration)`, speed DragValue (0.1..4, default 1.0); `time += dt * speed` while playing; pass `Some((clip, time))` into `FrameParams.anim`. Turntable + animation compose (turntable is the root post-rotation).

- [ ] **Step 5: Smoke with the animated test GLB**

Build the Task 4 fixture (`build_cube_glb(true)`), open it, press play: cube rotates 0→90° over 1 s and loops; scrubber drags update the pose live; speed slider works.

- [ ] **Step 6: Gate + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: glTF animation playback with animation bar"
```

---

### Task 18: Error UX, status line, final polish (M4 complete)

**Files:**
- Modify: `crates/viewer-app/src/{app.rs, panels.rs}`

**Interfaces:**
- Consumes: `LoadError` display strings, `Scene.warnings`.
- Produces: modal error dialog on `AppState::Failed` (egui::Window with message + "OK" → back to Idle; FBX message includes "FBX is not supported — export to GLB, OBJ, STL, PLY or 3MF"); status line bottom-left in viewport when Ready: filename, tri count, vert count, warning count (`⚠ N` if any, tooltip lists them).

- [ ] **Step 1: Implement dialog + status line**

```rust
// in update() top:
if let AppState::Failed(msg) = &self.state {
    egui::Window::new("Could not open model")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(msg);
            if ui.button("OK").clicked() { self.state = AppState::Idle; }
        });
}
// status line: ui.painter().text or a small label widget bottom-left of CentralPanel:
if let AppState::Ready(scene) = &self.state {
    let warn = if scene.warnings.is_empty() { String::new() }
        else { format!(" ({} warnings)", scene.warnings.len()) };
    ui.label(format!("{} — {} tris, {} verts{}", scene.source_name, scene.tri_count, scene.vert_count, warn))
        .on_hover_text(scene.warnings.join("\n"));
}
```

- [ ] **Step 2: Full-project final gate + smoke checklist**

Run the complete gate: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
Manual smoke (all must pass):
[ ] open GLB with textures; [ ] open OBJ with MTL + map_Kd; [ ] open binary + ASCII STL; [ ] open ASCII PLY; [ ] drop file on window; [ ] FBX → friendly modal; [ ] missing-texture model renders with magenta patches and warning count in status; [ ] all 9 themes; [ ] wireframe; [ ] grid toggle; [ ] turntable; [ ] animation play/scrub/speed; [ ] shadows from 3 lights; [ ] bloom on bright emissive; [ ] screenshot PNG (colors correct, not flipped); [ ] resize + maximize; [ ] double-click reset; [ ] exposure/contrast/FOV sliders.

- [ ] **Step 3: Commit (M4 complete)**

```bash
git add crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: error dialogs, status line, final polish — M4 complete"
```

---

### Task 19 (stretch): 3MF loader

**Marked stretch — implement only after Tasks 1-18 are green; skipping does not break anything (dispatcher already returns a clear "not yet" error).**

**Files:**
- Create: `crates/viewer-scene/src/loaders/three_mf.rs`
- Modify: `crates/viewer-scene/src/loaders/mod.rs`, root `Cargo.toml` + `viewer-scene/Cargo.toml` (add `zip = "2"`, `quick-xml = "0.38"` to workspace deps)
- Test: `crates/viewer-scene/tests/three_mf_load.rs`

**Interfaces:**
- Produces: `three_mf::load(path)` — unzip, find `3D/3dmodel.model` (or `*.model` under `3D/`), stream-parse XML: `<mesh><vertices><vertex x= y= z=/>…`, `<triangles><triangle v1= v2= v3=/>…`; `<basematerials><base name= displaycolor= "#RRGGBB"/>`; `<object>`→node, `<build><item objectid=/>`→instance at root. Materials: one per base material, triangle→material mapping from `<triangles>` `pid/p1` attributes when present; else single default.

- [ ] **Step 1: Failing test** — build a minimal valid 3MF zip in code using the `zip` crate (write `[Content_Types].xml`, `_rels/.rels`, `3D/3dmodel.model` with 1 triangle, `displaycolor="#FF0000"`), assert: 3 positions, 1 triangle, base color linear red ≈ (1, 0, 0), normalized bounds.

- [ ] **Step 2: Implement** — `zip::ZipArchive` over the file; `quick_xml::Reader` with a small state machine (events: `Start(vertex) {x,y,z}` → collect; `Start(triangle) {v1,v2,v3}` → collect; `Start(base) {displaycolor}` → materials; `Start(item) {objectid}` → build instancing). Keep XML parsing tolerant (unknown elements skipped). Namespace: 3MF uses `http://schemas.microsoft.com/3dmanufacturing/core/2015/02` — match on local names only.

- [ ] **Step 3: Wire dispatch (`Some("3mf")`), tests, gate, commit**

```bash
cargo test -p viewer-scene && cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add Cargo.toml Cargo.lock crates && git -c user.name="Steven" -c user.email="steven@local" commit -m "feat: 3MF loader (stretch)"
```

---

## Plan Self-Review (completed by plan author)

1. **Spec coverage** — M1 skeleton (Tasks 1–2) ✔; M2 viewer (3–8) ✔; M3 fidelity: shadows 9, themes 11, tonemap 7, bloom 10, wireframe 12, turntable 13, grid 12 ✔; M4: OBJ 14, STL 15, PLY 16, animations 17, screenshot 13, error UX 18, 3MF stretch 19 ✔. Camera/input §7 → Task 6 + Task 8 input wiring ✔. UI §8 → Tasks 11–13, 18 ✔. Ported data §9 → Task 11 + constants in Task 7 ✔. Error handling §10 → Tasks 3 (LoadError), 4 (magenta), 18 (dialogs) ✔. Testing §11 → per-task fixtures + gates ✔. Deferred items (skinning bind-pose, IBL, remote URIs, FBX) match spec §13 ✔.
2. **Placeholder scan** — Task 16 Step 3 and Task 19 Step 2 describe structure rather than full code (deliberate: mechanical parsing following the STL pattern; signatures + behaviors fully specified). All other code steps contain complete code. Acceptable per task granularity; flagged for executors.
3. **Type consistency** — `FrameParams.anim` (Task 7 declares, 17 consumes) ✔; `Renderer::prepare_render` gained `&Queue` in Task 7 Step 6 vs Step 5 signature — Step 6 supersedes ✔; `SharedRenderer` naming consistent 7→8 ✔; `Keyframes` layout (Task 3) matches loader writes (Task 4) and evaluator reads (Task 17) ✔; light rig positions (Task 11) match shadow fit assumptions (Task 9) ✔; `Image` RGBA8-sRGB contract consistent between loaders and `upload_texture` ✔.

