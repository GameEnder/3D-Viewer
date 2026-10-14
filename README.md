# 3D Viewer (Rust)

A desktop 3D-model viewer built with eframe/egui and a custom wgpu renderer: PBR
shading with up to three shadow-casting lights, an animated ground disc with
grid overlay, HDR post-processing (ACES tonemap, exposure/contrast), orbit /
pan / dolly camera, and glTF animation playback with a scrubbing animation bar.
Models load through a `viewer-scene` crate with per-format loaders and are
uploaded to the GPU by `viewer-render`; `viewer-app` wires it all together with
an egui UI (file picker, toolbar, panels, error dialogs).

## Build and run

```sh
cargo run -p viewer-app
```

Requires rustc >= 1.95 (eframe 0.36); `rust-toolchain.toml` pins `stable` so a
fresh clone resolves a compatible toolchain automatically.

## Supported formats

GLB, GLTF, OBJ, STL, PLY, 3MF. FBX files are rejected with a friendly
"not supported by this viewer" message.

## Calibration harness

Headless GPU calibration suite (renders reference scenes offscreen and checks
the encoded output); excluded from the default test run:

```sh
cargo test -p viewer-render --release -- --ignored --nocapture
```

## Tests

```sh
cargo test --workspace
```
