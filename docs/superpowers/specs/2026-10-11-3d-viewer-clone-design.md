# 3D Viewer — Rust Clone Design Spec

- **Date:** 2026-10-11
- **Status:** Approved in chat; pending user review
- **Target repo:** `H:\Coding Projects\3D-Viewer`

## 1. Background — how Microsoft 3D Viewer works

Findings from reverse engineering `Microsoft.Microsoft3DViewer 7.2602.8012.0`
(installed package; manifest, asset configs, and component inventory read
directly from the appx directory):

| Component | Role |
|---|---|
| `3DViewer.exe` | 19 KB UWP stub; real logic in `3DViewer.dll` (17 MB, C++/WinRT + .NET Native) |
| `Mira.Core.Engine.UWP.dll` | 3D engine "Mira": PBR forward renderer, 3-light rig + environment, per-light 512² shadow maps, bloom/vignette/tone mapping, planar reflections, wireframe, glTF animation, turntable |
| `Microsoft.glTF.Toolkit.UWP.dll` | glTF/GLB loading (upstream open source: `microsoft/glTF-Toolkit`) |
| `OnlineMediaComponent.dll` | "3D Library" search against Hubble (`hubble.officeapps.live.com`, successor of Remix3D) |
| XAML UI (WinUI 2.7) | Left toolbar; settings panel (Model / Lighting / Camera / Effects); animation bar for animated models |
| `View3D.ResourceResolver.exe` | Full-trust broker resolving external resources (e.g. textures referenced by local models) |
| `ShellPreviewHandler3D.dll` | Explorer preview handler COM server |

- **Formats:** 3MF, FBX, OBJ, STL, PLY, GLTF, GLB — non-glTF formats are
  transcoded to glTF in memory (`ModelTranscodingService`).
- **The engine's full tunable parameter surface** is shipped as
  `Assets/Config/DefaultConfiguration.json` (312 keys): camera orbit stored as
  `(distance, elevation, rotation)`; 3 lights × (color, intensity, shadow
  config); ground plane opacity; exposure/contrast/tone mapping; bloom; wireframe.
- **9 lighting themes** ship in `Assets/Lighting/Themes.json` (Default,
  Campfire, Cotton Candy, Glow in the Dark, Moonlight, Paint3D, Silhouette,
  Studio, Sunset) — each defines 3 light colors + intensities, environment
  color + intensity, and a light-rig rotation.
- Default load normalization: `Model.PositionAlignBase = true` (model base
  sits on the ground plane) + `Model.RelativeModelLoadScale`.

## 2. Scope

**Goal:** a native Rust desktop application that reproduces the core 3D
model-viewing experience of Microsoft 3D Viewer.

**In scope**
- Load GLTF/GLB, OBJ, STL, PLY (first-class); 3MF as a stretch goal
- PBR forward rendering with the 3-light rig + environment lighting
- The 9 lighting themes, ported verbatim
- Ground plane with shadow reception, soft shadow maps per light
- Ground-plane grid overlay toggle (from the original's `Renderer.Grid`)
- Orbit camera matching the app's spherical model; tone mapping, exposure,
  contrast, bloom; wireframe mode; turntable animation
- glTF node-TRS animations with play/pause/scrub UI
- Screenshot to PNG
- Drag-and-drop + native file open dialog

**Out of scope (explicitly)**
- FBX (no mature pure-Rust loader; friendly error message)
- Mixed Reality (camera passthrough) mode
- 3D Library / online search (Hubble API), Model shelf
- Paint / decal tools, Print 3D, shell preview handler, file associations
- KTX2/Basis compressed textures (clear error); remote glTF resource URIs
- Skinned-mesh animation (skinned meshes render in bind pose; node-TRS
  animations play). Marked as future work.

## 3. Technology decisions

| Decision | Choice | Rationale |
|---|---|---|
| UI + rendering | `eframe` (egui) on the **wgpu** backend; scene rendered via `egui_wgpu::CallbackTrait` | Immediate-mode panels/sliders for tool UI; shares the surface with correct resize/DPI handling |
| Rendering core | Hand-rolled forward renderer (no engine) | Mirrors Mira's parameter surface 1:1; no framework-fighting; ~2–3k LOC of well-understood code |
| Format loaders | `gltf` (GLTF/GLB), `tobj` (OBJ+MTL), hand-written STL/PLY parsers; 3MF via `zip`+`quick-xml` (stretch) | Pure Rust, clean build; per approved approach |
| Math | `glam` | Standard, SIMD, wgpu-ecosystem default |
| Texture decode | `image` | PNG/JPEG/BMP required by glTF spec |
| Dialogs | `rfd` | Native Windows file dialog |
| Errors | `thiserror` in libs, friendly dialog mapping in app | |
| Platform | Windows-first (DX12/Vulkan via wgpu), code kept cross-platform | |

Alternatives considered and rejected: Bevy (ECS framework-fighting, heavy
builds, egui integration churn), `three-d` crate (fixed material/lighting model,
egui overlay integration friction). See chat log 2026-10-11.

## 4. Architecture

Cargo workspace, three crates with strict one-way dependencies
(`app → render → scene`):

```
3d-viewer/
├─ Cargo.toml            # [workspace]
├─ crates/
│  ├─ viewer-scene/      # unified Scene data model + format loaders (no GPU)
│  ├─ viewer-render/     # wgpu renderer: passes, shaders, lights, post-fx (no UI)
│  └─ viewer-app/        # eframe shell: window, panels, camera controller, state
└─ docs/superpowers/specs/   # this document
```

- `viewer-scene`: pure data + parsing. Testable headless.
- `viewer-render`: consumes `Scene`; owns GPU resources, render passes,
  frame scheduling. No input/UI knowledge beyond a `RenderSettings` struct.
- `viewer-app`: input handling, panels, loading state machine, ties
  renderer and scene together.

## 5. Scene model & loader pipeline (viewer-scene)

```rust
pub struct Scene {
    pub nodes: Vec<Node>,
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub animations: Vec<Animation>,
    pub bounds: Aabb,          // post-normalization
}
pub struct Node {
    pub name: Option<String>,
    pub transform: Trs,          // translation/rotation(quat)/scale
    pub children: Vec<usize>,
    pub mesh: Option<usize>,
}
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,      // empty if absent
    pub indices: Vec<u32>,        // triangles, lists
    pub material: usize,
}
pub struct Material {
    pub base_color: [f32; 4],             // linear
    pub base_color_texture: Option<Image>,
    pub metallic: f32, pub roughness: f32,
    pub metallic_roughness_texture: Option<Image>,
    pub normal_map: Option<Image>,
    pub emissive: [f32; 3], pub emissive_texture: Option<Image>,
    pub alpha_mode: AlphaMode,             // Opaque | Mask(cutoff) | Blend
    pub double_sided: bool,
}
pub struct Animation {
    pub name: String, pub duration: f32,
    pub channels: Vec<Channel>,           // target node, property (T/R/S), keys
}
```

**Loader behavior**
- **GLTF/GLB:** full node hierarchy; buffers resolved (embedded + external
  relative to file); textures decoded via `image`; animations with
  step/linear/cubic-spline interpolation; cameras/lights in the file ignored
  (viewer imposes its own rig, as the original does). Normal-tangents computed
  from UV gradients when no data.
- **OBJ:** `tobj`; MTL mapping: `Kd`→base color, `Ns`/`Ks`→roughness
  heuristic (`roughness = clamp(1 - Ns/1000)`), `map_Kd`→base texture.
  Single root node.
- **STL:** hand parser (binary + ASCII); per-face normals accumulated into
  smooth vertex normals.
- **PLY:** hand parser (ASCII + binary little-endian; `vertex` element with
  x/y/z/nx/ny/nz/red/green properties, `face` element with property lists).
  Colors → per-mesh base color average if no material.
- **3MF (stretch):** unzip `[Content_Types].xml`-validated package, parse
  `3D/3dmodel.model` XML, extract triangles + base colors.
- **FBX:** rejected up front → `LoadError::UnsupportedFormat` with a
  friendly dialog.

**Normalization (all formats), mirroring the original's
`Model.PositionAlignBase`:**
1. Compute world-space AABB.
2. Uniform scale so bounding-sphere radius = **1.0**.
3. Translate: center on x/z = 0, min-y = 0 (base on ground plane).
4. Camera targets the bounds center. *(Our canonical scale differs from the
   original's internal one — its `Camera.Distance = 1.75` is relative to its
   own normalization. We calibrate framing constants to visually match; see §7.)*

**Loading is async:** worker thread + channel; UI shows spinner state.
Texture decode happens in the loader (CPU), GPU upload on the render thread.

## 6. Renderer (viewer-render)

Forward pass, HDR (`rgba16f`) intermediate target, then post chain:

1. **Shadow pass** — 512² depth map per enabled light (size from original's
   config), orthographic projection fit to model bounds, PCF 3×3 softening.
2. **Opaque pass** — PBR metallic-roughness: Lambertian diffuse + GGX
   specular (Smith visibility, Schlick Fresnel); 3 punctual lights
   (color × intensity, shadows) + flat environment-color ambient; ground
   plane disc (radius ~2.5 model units) with radial-gradient alpha
   (center opacity 0.4 from `GroundPlane.Opacity`), receives shadows;
   clear color `rgb(69, 74, 80)` (from `Renderer.ClearColor*`).
3. **Transparent pass** — `AlphaMode::Blend` meshes, sorted back-to-front;
   `Mask` handled in opaque pass with alpha cutoff 0.5.
4. **Post pass** — ACES tone mapping; exposure (default 0.5 EV) and
   contrast (1.66) from the original's config; bloom (threshold + separable
   blur, weight 0.11); background vignette out of scope.
5. **Wireframe** — barycentric attribute per triangle + fragment-shader
   edge factor (wgpu has no geometry shaders); rendered as an overlay mode.
6. **Turntable** — animated Y-rotation of the model root (~30°/s default,
   slider-configurable; exact original rate not extracted — runtime
   observation would be needed, chosen to look equivalent).
7. **MSAA 4×** on geometry passes, resolved before post (nice-to-have, M3).

**Light rig semantics** (matching the app): 3 lights fixed in the upper
hemisphere around the model (equal 120° spacing, elevation ≈ 40°, radius
≈ 2.2 model units); a theme's `LightRotation` rotates the whole rig.
Theme intensities are normalized: **100 = 1.0** reference multiplier.

GPU resources: one vertex/index buffer set per mesh, bind groups per
material (textures) and per frame (camera/lights/shadow maps). Rebuild on
scene change only.

## 7. Camera & input (viewer-app)

Spherical orbit state, exactly the original's parameterization:

- `distance` (default **3.0** under our canonical scale — calibrated to
  visually match the original's `1.75` framing of a unit-normalized model),
  `elevation` (default **15°**), `rotation` (default **−90°**), `FOV`
  (default **45°**).
- **LMB drag** → orbit (elevation clamped ±89°); **wheel** → dolly
  (clamped near/far around model bounds); **RMB drag or Shift+drag** →
  screen-space pan; **double-click** → reset framing.
- Perspective only. Up = +Y.

## 8. UI (eframe/egui)

Mirrors the original's structure:

- **Left vertical toolbar:** Open, Turntable (toggle), Screenshot, Settings
  (toggles panel). (Library/MR/paint excluded per scope.)
- **Settings panel** (slide-in, sectioned like the original):
  - *Model:* filename, tri/vert counts, rotation offset, scale slider
  - *Lighting:* 9 theme swatches, per-light enable + intensity, environment intensity
  - *Camera:* FOV, exposure, contrast, reset-view
  - *Effects:* wireframe, ground plane, grid, bloom
- **Animation bar** (bottom; only when animated): clip selector, play/pause,
  scrubber, speed.
- Native open dialog (`rfd`); drag-and-drop onto the window loads.
- Loading spinner in viewport; errors as modal dialogs.

## 9. Ported data (verbatim from the appx)

- **Engine defaults** (`DefaultConfiguration.json`):
  clear color `rgb(69, 74, 80)`; ground opacity **0.4**; exposure **0.5**;
  contrast **1.66**; bloom weight **0.11**; FOV **45°**; shadow map size
  **512**; camera elevation **15°** / rotation **−90°**.
- **9 themes** (Appendix A) as a `const` table in `viewer-app`
  (the original's `Themes.json` uses a 0–255 color scale and percent-like
  intensities; `LightRigRotation`/`LightRotation` key inconsistency
  normalized to one field).
- Not shipped: MDL2 icon fonts, theme thumbnail PNGs, cubemap (default theme
  is flat-gray environment; IBL is future work).

## 10. Error handling & app states

- `LoadError::{UnsupportedFormat, Parse { format, detail }, MissingTexture,
  Io}` — FBX gets a "not supported" dialog; parse errors name the format and
  a one-line detail; missing textures are **non-fatal**: 1×1 magenta
  substitute + warning count in the status line.
- glTF relative resource URIs resolved against the model file's directory
  (the job of the original's ResourceResolver broker); `http(s)://` URIs
  rejected.
- App state machine: `Idle | Loading | Ready(Scene) | Failed(LoadError)`.

## 11. Testing strategy

- **viewer-scene:** fixture files in `crates/viewer-scene/tests/assets/`
  (tiny OBJ+MTL, binary + ASCII STL, PLY, hand-built minimal GLB);
  assertions on counts, AABB, material mapping, normalization; interpolation
  unit tests (step/linear/cubic-spline).
- **Pure math:** camera orbit ↔ view matrix, turntable, normal accumulation
  — unit tests.
- **Renderer:** lighting math functions unit-tested where pure; full GPU
  pipeline verified by a manual smoke checklist per milestone (no portable
  golden-image tests on wgpu/Windows).
- **Gate:** `cargo fmt --check`, `cargo clippy --deny warnings`, `cargo test`
  must pass each milestone.

## 12. Milestones

1. **M1 Skeleton** — workspace; window + wgpu surface + egui overlay; empty
   viewport with clear color; open-file plumbing (dialog + dnd, logs path).
2. **M2 Viewer** — GLTF/GLB load; PBR opaque pass; orbit camera; default
   light rig + ground plane; model normalization.
3. **M3 Fidelity** — shadow maps + PCF; 9 themes; tone mapping/exposure/
   contrast; bloom; wireframe; turntable; MSAA.
4. **M4 Formats & polish** — OBJ/STL/PLY loaders; glTF animations + bar;
   screenshot PNG; error dialogs/status line; 3MF stretch.

## 13. Known limitations / future work

- Skinned meshes render in bind pose (no joint palettes yet)
- No IBL (environment is flat color; original uses a DDS cubemap)
- No remote texture URIs, no KTX2/Basis, no FBX
- Exact original turntable rate unverified (needs runtime observation)
- Future: Hubble library search, MR mode would be separate projects

## Appendix A — Theme data (from `Assets/Lighting/Themes.json`)

Colors are RGB 0–255; intensity on the original's ~percent scale (100 = 1.0).

| Theme | L0 RGB / I | L1 RGB / I | L2 RGB / I | Env RGB / I | Rig rot (°) |
|---|---|---|---|---|---|
| Default | 255,255,255 / 100 | 255,255,255 / 100 | 255,255,255 / 100 | 255,255,255 / 40 | 0 |
| Campfire | 53,34,122 / 48 | 255,255,188 / 15 | 255,68,0 / 200 | 33,37,104 / 31 | 145.5 |
| Cotton Candy | 255,40,201 / 160 | 110,234,226 / 164 | 211,46,46 / 66 | 161,198,197 / 30 | 189.4 |
| Glow in the Dark | 45,255,52 / 180 | 232,232,232 / 19 | 0,0,0 / 0 | 0,2,158 / 82 | 345 |
| Moonlight | 255,250,173 / 97 | 52,46,119 / 74 | 255,255,255 / 13 | 0,35,150 / 34 | 348 |
| Paint3D | 255,194,168 / 88 | 73,76,255 / 117 | 0,0,0 / 0 | 255,255,255 / 58 | 225 |
| Silhouette | 0,0,0 / 0 | 255,255,255 / 200 | 255,255,255 / 0 | 255,255,255 / 3 | 240.8 |
| Studio | 151,199,201 / 111 | 255,251,232 / 92 | 255,248,239 / 43 | 211,211,211 / 103 | 155.5 |
| Sunset | 255,144,0 / 200 | 84,106,255 / 170 | 0,0,0 / 0 | 81,68,147 / 64 | 46 |

*(Theme colors in the source JSON are stored as named keys; values above
are re-ordered R,G,B.)*

## Appendix B — Key extracted config values

| Config key (original) | Value | Used for |
|---|---|---|
| `Camera.FieldOfView (Degrees)` | 45 | default FOV |
| `/Internal/Camera.Elevation (degrees)` | 15 | default elevation |
| `/Internal/Camera.Rotation (degrees)` | −90 | default orbit rotation |
| `/Internal/Camera.Distance` | 1.75 | (their scale; we recalibrate §7) |
| `Camera.ExposureValue` | 0.5 | exposure default |
| `Camera.Contrast` | 1.66 | contrast default |
| `Renderer.ClearColor {R,G,B}` | 0.2706 / 0.2902 / 0.3137 | background |
| `GroundPlane.Opacity` | 0.4 | ground plane alpha |
| `ImageProcessing.BloomWeight` | 0.11 | bloom weight |
| `Lighting.Light{0,1,2}_ShadowBufferSize` | 512 | shadow map size |
| `Model.PositionAlignBase` | true | load normalization |
