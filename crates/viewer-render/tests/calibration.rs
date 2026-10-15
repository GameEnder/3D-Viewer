//! Headless calibration harness (Task 18b, controller-implemented).
//!
//! Ignored by default (requires a GPU adapter); run with:
//! `cargo test -p viewer-render --release -- --ignored --nocapture`
//!
//! Measures the four Task 12 A/B targets through the REAL render chain
//! (background quad -> ground -> post) and prints them:
//! - background at the viewport corner (target ~97/255 encoded)
//! - background near the top-center (target ~110/255 encoded)
//! - ground pool near the disc center, away from the model (target ~119/255)
//! - ground rim near the disc edge (target ~37/255)
//!
//! Ground pixels are measured on a scene whose single mesh is a tiny triangle
//! at the world origin, so most of the disc is unshadowed pool. The scene is
//! built directly (not through `load`) with bounds matching the tiny mesh, so
//! the renderer sees a loaded scene and draws the disc.

use std::sync::Arc;

use egui_wgpu::wgpu;

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Default theme rig (duplicated from viewer-app::themes — viewer-render
/// cannot depend on the app crate; keep in sync with its `light_pos`).
fn default_rig() -> [viewer_render::LightInput; 3] {
    std::array::from_fn(|i| {
        const RADIUS: f32 = 2.2;
        let elev = 40.0f32.to_radians();
        let az = (120.0 * i as f32).to_radians();
        let lin = [srgb_to_linear(1.0); 3];
        viewer_render::LightInput {
            pos: [
                RADIUS * elev.cos() * az.sin(),
                RADIUS * elev.sin(),
                RADIUS * elev.cos() * az.cos(),
            ],
            color: lin,
            intensity: 1.0,
            shadows: true,
        }
    })
}

fn tiny_scene() -> Arc<viewer_scene::Scene> {
    let mut scene = viewer_scene::Scene::new("calib".into());
    scene.materials.push(viewer_scene::Material::default_pbr());
    // A small triangle just above the disc plane: the harness measures the
    // GROUND, and the renderer only needs a loaded scene to draw the disc.
    let mesh = viewer_scene::Mesh {
        positions: vec![
            [-0.02, 0.001, -0.02],
            [0.02, 0.001, -0.02],
            [0.0, 0.001, 0.03],
        ],
        normals: vec![[0.0, 1.0, 0.0]; 3],
        uvs: vec![],
        indices: vec![0, 1, 2],
        material: 0,
    };
    let node = scene.add_node(scene.root, viewer_scene::Trs::identity(), None);
    let mi = scene.add_mesh(mesh);
    scene.nodes[node].meshes.push(mi);
    // Bounds consistent with the tiny mesh (bypassing normalize(); the
    // renderer reads bounds for the shadow fit and turntable pivot only).
    scene.bounds = viewer_scene::Aabb {
        min: glam::Vec3::new(-0.02, 0.0, -0.02),
        max: glam::Vec3::new(0.02, 0.001, 0.03),
    };
    Arc::new(scene)
}

fn sample(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// A single large triangle FACING the harness camera (sparse wireframe
/// probe). The calibration camera orbits at rotation -90 (eye ~ (-3, 1.15, 0)
/// looking along +X), so the triangle lives in the Y/Z plane at x = -0.5;
/// a z=0 "vertical" triangle is viewed EDGE-ON there and never rasterizes
/// (the first probe version measured the background gradient instead).
fn big_triangle_scene() -> Arc<viewer_scene::Scene> {
    let mut scene = viewer_scene::Scene::new("big_tri".into());
    scene.materials.push(viewer_scene::Material::default_pbr());
    let mesh = viewer_scene::Mesh {
        positions: vec![[-0.5, -0.8, -0.8], [-0.5, -0.8, 0.8], [-0.5, 0.9, 0.0]],
        normals: vec![[1.0, 0.0, 0.0]; 3],
        uvs: vec![],
        indices: vec![0, 1, 2],
        material: 0,
    };
    let node = scene.add_node(scene.root, viewer_scene::Trs::identity(), None);
    let mi = scene.add_mesh(mesh);
    scene.nodes[node].meshes.push(mi);
    scene.bounds = viewer_scene::Aabb {
        min: glam::Vec3::new(-0.5, -0.8, -0.8),
        max: glam::Vec3::new(-0.5, 0.9, 0.8),
    };
    Arc::new(scene)
}

/// A dense subdivided grid whose triangles are sub-pixel at the harness
/// camera (dense wireframe probe — reproduces the user-reported "wireframe
/// makes the model blue" on the 1.15M-triangle bird).
fn dense_grid_scene(n: u32) -> Arc<viewer_scene::Scene> {
    let mut scene = viewer_scene::Scene::new("dense".into());
    scene.materials.push(viewer_scene::Material::default_pbr());
    let half = 0.7f32;
    let step = 2.0 * half / n as f32;
    // Grid of (n x n) quads -> 2 triangles each, as an index buffer.
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let x = -half + step * i as f32;
            let z = -half + step * j as f32;
            positions.push([x, 0.0, z]);
        }
    }
    let stride = n + 1;
    for j in 0..n {
        for i in 0..n {
            let v = |a: u32, b: u32| b * stride + a;
            let (a, b, c, d) = (v(i, j), v(i + 1, j), v(i, j + 1), v(i + 1, j + 1));
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let mesh = viewer_scene::Mesh {
        positions,
        normals: vec![[0.0, 1.0, 0.0]; ((n + 1) * (n + 1)) as usize],
        uvs: vec![],
        indices,
        material: 0,
    };
    let node = scene.add_node(scene.root, viewer_scene::Trs::identity(), None);
    let mi = scene.add_mesh(mesh);
    scene.nodes[node].meshes.push(mi);
    scene.bounds = viewer_scene::Aabb {
        min: glam::Vec3::new(-half, 0.0, -half),
        max: glam::Vec3::new(half, 0.0, half),
    };
    Arc::new(scene)
}

/// Render `scene` with the given wireframe flag and return
/// (mean_gray, mean_r, mean_b) of the central 120x120 region (the surface
/// fills it at the harness camera).
fn mean_center_gray(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: Arc<viewer_scene::Scene>,
    wireframe: bool,
) -> (f32, f32, f32) {
    let (w, h) = (400u32, 300u32);
    let mut renderer = viewer_render::Renderer::new(device, wgpu::TextureFormat::Bgra8Unorm);
    renderer.resize(device, w, h);
    renderer.set_scene(device, queue, scene);
    let lights = default_rig();
    let lights = lights.map(|mut l| {
        l.shadows = false;
        l
    });
    renderer.queue_frame(viewer_render::FrameParams {
        view_proj: calibration_camera(w, h),
        camera_pos: [0.0, 0.85, 3.1],
        turntable_deg: 0.0,
        model_scale: 1.0,
        lights,
        env_color: [srgb_to_linear(1.0); 3],
        env_intensity: 0.4,
        anim: None,
        settings: viewer_render::RenderSettings {
            clear_color: viewer_render::DEFAULT_CLEAR_COLOR,
            wireframe,
            ground: false,
            grid: false,
            bloom: false,
            exposure: 0.5,
            contrast: 1.66,
            ground_opacity: 0.4,
            shadows: false,
        },
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.request_screenshot();
    renderer.prepare_render(device, queue, &mut encoder);
    queue.submit(Some(encoder.finish()));
    let img = renderer
        .collect_screenshot(device)
        .expect("screenshot readback");
    let rgba = img.into_raw();
    let (x0, y0, x1, y1) = (140u32, 90u32, 260u32, 210u32);
    let (mut sum, mut sr, mut sb, mut count) = (0.0f32, 0.0f32, 0.0f32, 0u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = sample(&rgba, w, x, y);
            sum += (p[0] as u32 + p[1] as u32 + p[2] as u32) as f32 / 3.0;
            sr += p[0] as f32;
            sb += p[2] as f32;
            count += 1;
        }
    }
    let n = count as f32;
    (sum / n, sr / n, sb / n)
}

#[test]
#[ignore = "headless GPU probe; run with --ignored"]
fn wireframe_sparse_triangle_shows_structure() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("no GPU adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).expect("no GPU device");

    // Sparse triangle: the wireframe must show BOTH dark edge pixels and
    // bright shaded interior pixels (structure, not a uniform wash).
    let img_mean_dark = {
        let (w, h) = (400u32, 300u32);
        let mut renderer = viewer_render::Renderer::new(&device, wgpu::TextureFormat::Bgra8Unorm);
        renderer.resize(&device, w, h);
        renderer.set_scene(&device, &queue, big_triangle_scene());
        let mut lights = default_rig();
        for l in &mut lights {
            l.shadows = false;
        }
        renderer.queue_frame(viewer_render::FrameParams {
            view_proj: calibration_camera(w, h),
            camera_pos: [0.0, 0.85, 3.1],
            turntable_deg: 0.0,
            model_scale: 1.0,
            lights,
            env_color: [srgb_to_linear(1.0); 3],
            env_intensity: 0.4,
            anim: None,
            settings: viewer_render::RenderSettings {
                clear_color: viewer_render::DEFAULT_CLEAR_COLOR,
                wireframe: true,
                ground: false,
                grid: false,
                bloom: false,
                exposure: 0.5,
                contrast: 1.66,
                ground_opacity: 0.4,
                shadows: false,
            },
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.request_screenshot();
        renderer.prepare_render(&device, &queue, &mut encoder);
        queue.submit(Some(encoder.finish()));
        let img = renderer
            .collect_screenshot(&device)
            .expect("screenshot readback");
        img.into_raw()
    };
    let (mut min_g, mut max_g) = (255u32, 0u32);
    for y in 0..300 {
        for x in 0..400 {
            let p = sample(&img_mean_dark, 400, x, y);
            let g = (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3;
            min_g = min_g.min(g);
            max_g = max_g.max(g);
        }
    }
    println!("sparse wireframe (full frame): min {min_g}, max {max_g}");
    // Light-edge wireframe (matches the original's "Triangles" view): the
    // thin pale edges push the brightest pixels well ABOVE the shaded
    // surface, while the surface shading itself stays intact below.
    assert!(
        max_g > 170 && (90..=115).contains(&min_g),
        "sparse wireframe must show light edges over a shaded surface \
         (min {min_g}, max {max_g})"
    );
}

#[test]
#[ignore = "headless GPU probe; run with --ignored"]
fn wireframe_dense_surface_keeps_shading() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("no GPU adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).expect("no GPU device");

    let scene = dense_grid_scene(160); // 51,200 sub-pixel triangles
    let (plain, _, _) = mean_center_gray(&device, &queue, scene.clone(), false);
    let (wire, mr, mb) = mean_center_gray(&device, &queue, scene, true);
    println!("dense mean: plain {plain:.1}, wireframe {wire:.1} (r {mr:.1} vs b {mb:.1})");
    // The user-reported bug history: the wireframe first washed dense models
    // uniformly blue, then (darkening fix) still cost ~50% of the shading.
    // With the light-edge style + density fade the surface keeps its shading
    // (the pale lines lift the mean only slightly); lock that band in.
    assert!(
        wire >= 0.9 * plain && wire <= 1.25 * plain,
        "dense wireframe alters the surface shading too much: plain \
         {plain:.1} vs wireframe {wire:.1}"
    );
    assert!(
        (mr - mb).abs() <= 6.0,
        "dense wireframe must stay hue-neutral (r {mr:.1} vs b {mb:.1} — \
         the old flat dark-blue line color tinted the model blue)"
    );
}

#[test]
#[ignore = "headless GPU probe; run with --ignored"]
fn theme_thumbnail_renders_model() {
    // The preview scene + one render: the thumbnail must contain the lit
    // model (bright pixels well above the dark background) and some shadow.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("no GPU adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).expect("no GPU device");

    let lights = default_rig();
    let img = viewer_render::preview::render(
        &device,
        &queue,
        96,
        viewer_render::FrameParams {
            view_proj: viewer_render::preview::camera_view_proj(),
            camera_pos: viewer_render::preview::camera_pos(),
            turntable_deg: 0.0,
            model_scale: 1.0,
            lights,
            env_color: [srgb_to_linear(1.0); 3],
            env_intensity: 0.4,
            anim: None,
            settings: viewer_render::RenderSettings {
                clear_color: [0.03, 0.03, 0.035],
                wireframe: false,
                ground: true,
                grid: false,
                bloom: false,
                exposure: 0.5,
                contrast: 1.66,
                ground_opacity: 0.4,
                shadows: true,
            },
        },
    );
    let (mut min_g, mut max_g) = (255u32, 0u32);
    for px in img.as_chunks::<4>().0 {
        let g = (px[0] as u32 + px[1] as u32 + px[2] as u32) / 3;
        min_g = min_g.min(g);
        max_g = max_g.max(g);
    }
    println!("thumbnail: min {min_g}, max {max_g}");
    assert!(
        max_g > 140 && min_g < 60,
        "thumbnail must show a lit model over a dark background \
         (min {min_g}, max {max_g})"
    );
}

#[test]
#[ignore = "headless GPU calibration harness; run with --ignored"]
fn calibration_measurements() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("no GPU adapter for calibration harness");
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
        .expect("no GPU device for calibration harness");

    let (w, h) = (400u32, 300u32);
    let mut renderer = viewer_render::Renderer::new(&device, wgpu::TextureFormat::Bgra8Unorm);
    renderer.resize(&device, w, h);
    renderer.set_scene(&device, &queue, tiny_scene());

    renderer.queue_frame(viewer_render::FrameParams {
        view_proj: calibration_camera(w, h),
        camera_pos: [0.0, 0.85, 3.1],
        turntable_deg: 0.0,
        model_scale: 1.0,
        lights: default_rig(),
        env_color: [srgb_to_linear(1.0); 3],
        env_intensity: 0.4,
        anim: None,
        settings: viewer_render::RenderSettings {
            clear_color: viewer_render::DEFAULT_CLEAR_COLOR,
            wireframe: false,
            ground: true,
            grid: false,
            bloom: false,
            exposure: 0.5,
            contrast: 1.66,
            ground_opacity: 0.4,
            shadows: true,
        },
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.request_screenshot();
    renderer.prepare_render(&device, &queue, &mut encoder);
    queue.submit(Some(encoder.finish()));

    let img = renderer
        .collect_screenshot(&device)
        .expect("screenshot readback");
    let rgba = img.into_raw();

    // Background corner (top-left) and top-edge center (geometry-derived
    // expectations below), plus a center-sensitive sample just above the
    // disc's horizon (the true gradient center is behind the disc on screen).
    let corner = sample(&rgba, w, 4, 4);
    let top_center = sample(&rgba, w, w / 2, 4);
    let above_horizon = sample(&rgba, w, w / 2, 118);
    // Ground pool: offset from the screen center (where the model sits) but
    // still well inside the disc.
    let pool = sample(&rgba, w, w / 2 + 60, h / 2 + 24);
    // Ground deep rim: ON-disc (the disc edge projects to x~385 at this
    // framing), inside the falloff/fade band.
    let rim = sample(&rgba, w, 375, h / 2);

    println!(
        "bg corner     rgb({}, {}, {})",
        corner[0], corner[1], corner[2]
    );
    println!(
        "bg top-center rgb({}, {}, {})",
        top_center[0], top_center[1], top_center[2]
    );
    println!(
        "bg above-horizon rgb({}, {}, {})",
        above_horizon[0], above_horizon[1], above_horizon[2]
    );
    println!("ground pool   rgb({}, {}, {})", pool[0], pool[1], pool[2]);
    println!("ground rim    rgb({}, {}, {})", rim[0], rim[1], rim[2]);

    // Tight gates only for the deterministic background (exact gradient
    // round-trip). The ground targets are tuned via LIGHT_SCALE against the
    // printed numbers.
    let gray = |p: [u8; 4]| (p[0] as i32 + p[1] as i32 + p[2] as i32) / 3;
    assert!(
        (gray(corner) - 97).abs() <= 3,
        "bg corner {} vs target ~97",
        gray(corner)
    );
    assert!(
        (gray(top_center) - 101).abs() <= 3,
        "bg top-center {} vs target ~101 (geometry-derived: uv distance \
         ~0.688 from the gradient center at this sample point)",
        gray(top_center)
    );
    // Center-sensitive sample (t ~ 0.15 from the gradient center): pins the
    // center constant far more tightly than the top-edge sample.
    assert!(
        (gray(above_horizon) - 108).abs() <= 3,
        "bg above-horizon {} vs target ~108",
        gray(above_horizon)
    );
    // Ground pool: LIGHT_SCALE calibrated so the pool reads ~119/255 encoded
    // (the original app's measured pool brightness).
    assert!(
        (gray(pool) - 119).abs() <= 6,
        "ground pool {} vs target ~119 (LIGHT_SCALE calibration)",
        gray(pool)
    );
    // Rim/edge: our disc extends beyond the viewport at this framing, so the
    // original's absolute edge value (~37) is not observable here; gate the
    // visible falloff instead (the pool-to-edge darkening must be evident).
    assert!(
        gray(rim) <= gray(pool) - 10,
        "ground rim {} shows no visible falloff vs pool {}",
        gray(rim),
        gray(pool)
    );
}

/// View-projection for a fixed orbit pose (elevation 15deg, rotation -90deg,
/// fov 45deg) — mirrors OrbitCamera; viewer-render cannot depend on the app
/// crate.
fn calibration_camera(w: u32, h: u32) -> [[f32; 4]; 4] {
    use glam::{Mat4, Vec3};
    let target = Vec3::new(0.0, 0.35, 0.0);
    let el = 15.0f32.to_radians();
    let ro = (-90.0f32).to_radians();
    let dir = Vec3::new(el.cos() * ro.sin(), el.sin(), el.cos() * ro.cos());
    let eye = target + dir * 3.1;
    let view = Mat4::look_at_rh(eye, target, Vec3::Y);
    let proj = Mat4::perspective_rh(45.0f32.to_radians(), w as f32 / h as f32, 0.05, 100.0);
    (proj * view).to_cols_array_2d()
}
