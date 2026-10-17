# 3D Viewer (Rust)

A desktop 3D-model viewer for Windows, built with eframe/egui and a custom
wgpu renderer: PBR shading with up to three shadow-casting lights, animated
ground disc with grid overlay, HDR post-processing (ACES tonemap, exposure,
contrast, bloom), wireframe overlay, orbit / pan / dolly camera, and glTF
animation playback with a scrubbing timeline.

Models load through the `viewer-scene` crate (one loader per format, no GPU
types), are uploaded to the GPU by `viewer-render`, and `viewer-app` wires it
all together with an egui UI in the style of the original Microsoft 3D
Viewer: traditional menu bar, icon-tabbed settings panel, always-visible
playback bar, and GPU-rendered theme / viewpoint thumbnails.

## Features

- **Renderer** — PBR metallic/roughness shading, 9 verbatim lighting themes
  with per-light color/intensity controls, 512px PCF shadow maps, ACES
  tonemap with exposure/contrast, half-resolution bloom, density-aware
  wireframe, calibrated radial-gradient background and ground disc.
- **Formats** — GLB, GLTF, OBJ (+MTL), STL (binary + ASCII), PLY (ASCII +
  binary), 3MF, binary FBX, and NIF.
- **Animation** — glTF keyframe playback (Linear / Step / CubicSpline) with
  a play/pause/scrub/speed bar.
- **Windows integration** — set as the default app for all supported file
  types from `Edit → Preferences` (writes Windows-validated UserChoice
  assignments — including types normally protected by Windows); Explorer
  "Open with 3D Viewer" context-menu label; self-contained release exe
  (static CRT); per-machine MSI installer.

## Supported formats

| Format | Notes |
| --- | --- |
| GLB / GLTF | Full hierarchy, PBR materials, animations |
| OBJ (+ MTL) | Diffuse color/texture; shininess mapped to roughness |
| STL | Binary and ASCII dialects (auto-detected) |
| PLY | ASCII and little-endian binary; vertex colors tint the material |
| 3MF | ZIP/XML package; materials and multi-object builds |
| FBX | Binary 7.4/7.5 only (via [fbxcel-dom]); ASCII and ≤ 6.0 rejected with a friendly error |
| NIF | All Gamebryo/NetImmerse versions (Morrowind → Starfield) via ByroRedux |

## Build and run

```sh
cargo run -p viewer-app
```

Requires rustc >= 1.95 (eframe 0.36); `rust-toolchain.toml` pins `stable` so a
fresh clone resolves a compatible toolchain automatically.

Release builds are standalone: static CRT, stripped, no runtime dependencies
(see `.cargo/config.toml`).

```sh
cargo build --release -p viewer-app
```

## Installer (Windows MSI)

`installer\build.ps1` builds a per-machine MSI
(`installer\bin\3DViewer-<version>-x64.msi`) that installs to
`C:\Program Files\3D Viewer`, adds a Start Menu shortcut, and registers the
machine-wide ProgID + "Open with" entries for every supported file type.
Requires the WiX v3 toolset on PATH (or pass `-WixBin` pointing at a WiX
`bin` folder).

## Tests

```sh
cargo test --workspace
```

Loader tests build synthetic format fixtures in-memory (including a
hand-assembled Skyrim-SE NIF and a Morrowind 4.0.0.2 NIF), so no external
game assets are needed.

### Calibration harness

Headless GPU calibration suite (renders reference scenes offscreen and checks
the encoded output); excluded from the default test run:

```sh
cargo test -p viewer-render --release -- --ignored --nocapture
```

## Credits

This project stands on these libraries (direct dependencies):

- [eframe](https://github.com/emilk/egui) / [egui](https://github.com/emilk/egui) (MIT OR Apache-2.0) — GUI framework
- [wgpu](https://github.com/gfx-rs/wgpu) (MIT OR Apache-2.0) — GPU abstraction, used via the `egui-wgpu` re-export
- [glam](https://github.com/bitshifter/glam-rs) (MIT OR Apache-2.0) — vectors, matrices, quaternions
- [gltf](https://github.com/gltf-rs/gltf) (MIT OR Apache-2.0) — GLB/glTF parsing
- [tobj](https://github.com/Twinklebear/tobj) (MIT) — OBJ + MTL parsing
- [image](https://github.com/image-rs/image) (MIT OR Apache-2.0) — texture decoding
- [zip](https://github.com/zip-rs/zip2) (MIT) — 3MF container unpacking
- [quick-xml](https://github.com/tafia/quick-xml) (MIT) — 3MF XML parsing
- [fbxcel-dom](https://github.com/lo48576/fbxcel-dom) (MIT OR Apache-2.0) — binary FBX parsing
- [byroredux-nif](https://github.com/matiaszanolli/ByroRedux) (license: see repository) — NIF parsing, all Gamebryo versions
- [rfd](https://github.com/PolyMeilex/rfd) (MIT) — native file dialogs
- [thiserror](https://github.com/dtolnay/thiserror) (MIT OR Apache-2.0) — error types
- [bytemuck](https://github.com/Lokathor/bytemuck) (Zlib OR Apache-2.0 OR MIT) — zero-copy GPU buffer casting
- [pollster](https://github.com/zesterer/pollster) (Apache-2.0/MIT) — blocking on GPU futures
- [winreg](https://github.com/gentoo90/winreg) (MIT) — registry access for file associations
- [winresource](https://github.com/byron/winresource) (MIT) — build-time exe icon and version resources

Installer tooling: [WiX Toolset](https://wixtoolset.org/) v3 (`.NET`
Foundation) compiles the MSI.

Special thanks: the Windows UserChoice hash implemented in
`viewer-app/src/user_choice.rs` was reverse-engineered from
[SetUserFTA](https://setuserfta.com/) by Christoph Kolbicz, and validated
against output vectors captured from that tool.

## License

This project is licensed under the
[Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0.txt).
