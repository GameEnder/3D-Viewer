// crates/viewer-render/tests/bee_diag.rs
//! Diagnostic for the Bee.glb scale + animation report: renders the bee
//! headlessly (bind pose and mid-animation), checks pixel coverage, and
//! verifies the skinning math produces sane positions at both times.
//! Auto-skips when the author's Desktop file is absent.

use std::sync::Arc;

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn default_rig() -> [viewer_render::LightInput; 3] {
    std::array::from_fn(|i| {
        const RADIUS: f32 = 2.2;
        let elev = 40.0f32.to_radians();
        let az = (120.0 * i as f32).to_radians();
        viewer_render::LightInput {
            pos: [
                RADIUS * elev.cos() * az.sin(),
                RADIUS * elev.sin(),
                RADIUS * elev.cos() * az.cos(),
            ],
            color: [srgb_to_linear(1.0); 3],
            intensity: 1.0,
            shadows: false,
        }
    })
}

#[test]
#[ignore = "GPU diagnostic; run with --ignored --nocapture"]
fn bee_scale_and_skinning_diag() {
    let path = std::path::Path::new(r"C:\Users\Steven\Desktop\Bee.glb");
    if !path.exists() {
        eprintln!("skipping: Bee.glb absent");
        return;
    }
    let scene = Arc::new(viewer_scene::load(path).expect("bee loads"));
    eprintln!(
        "scene: {} meshes, {} skins, {} anims, bounds {:?}..{:?}",
        scene.meshes.len(),
        scene.skins.len(),
        scene.animations.len(),
        scene.bounds.min.to_array(),
        scene.bounds.max.to_array()
    );
    let mesh = &scene.meshes[0];
    eprintln!(
        "mesh: {} shared verts, {} joints, {} weights",
        mesh.positions.len(),
        mesh.joints.len(),
        mesh.weights.len()
    );
    let skin = &scene.skins[0];
    eprintln!(
        "skin: {} joints, IBMs: {}",
        skin.joints.len(),
        skin.inverse_bind.len()
    );

    // --- skinning math check at bind pose and mid-animation ---
    let world = viewer_scene::world_transforms(&scene, &[]);
    let mats = viewer_scene::skinning::joint_matrices_local(skin, &world, scene.root);
    let (bind_pos, _) = viewer_scene::skinning::skin_mesh(mesh, &mats);
    // At bind pose the skinned positions must equal the raw ones.
    let mut max_err = 0.0f32;
    for (a, b) in bind_pos.iter().zip(mesh.positions.iter()) {
        for i in 0..3 {
            max_err = max_err.max((a[i] - b[i]).abs());
        }
    }
    eprintln!("bind pose max error: {max_err} (should be ~0)");
    // Skinned bounds at bind pose:
    let mut mn = glam::Vec3::splat(f32::MAX);
    let mut mx = glam::Vec3::splat(f32::MIN);
    for p in &bind_pos {
        let v = glam::Vec3::from(*p);
        mn = mn.min(v);
        mx = mx.max(v);
    }
    eprintln!(
        "skinned bind bounds: {:?}..{:?}",
        mn.to_array(),
        mx.to_array()
    );

    // At t=0.5 of clip 0:
    let overrides = viewer_scene::evaluate(&scene, 0, 0.5);
    eprintln!(
        "overrides at t=0.5: {} nodes",
        overrides.iter().filter(|o| o.is_some()).count()
    );
    let world = viewer_scene::world_transforms(&scene, &overrides);
    let mats = viewer_scene::skinning::joint_matrices_local(skin, &world, scene.root);
    let (anim_pos, _) = viewer_scene::skinning::skin_mesh(mesh, &mats);
    let mut mn2 = glam::Vec3::splat(f32::MAX);
    let mut mx2 = glam::Vec3::splat(f32::MIN);
    for p in &anim_pos {
        let v = glam::Vec3::from(*p);
        mn2 = mn2.min(v);
        mx2 = mx2.max(v);
    }
    eprintln!(
        "skinned t=0.5 bounds: {:?}..{:?}",
        mn2.to_array(),
        mx2.to_array()
    );
    // How much did vertices move?
    let mut moved = 0.0f32;
    for (a, b) in anim_pos.iter().zip(bind_pos.iter()) {
        for i in 0..3 {
            moved = moved.max((a[i] - b[i]).abs());
        }
    }
    eprintln!("max vertex movement at t=0.5: {moved}");

    // --- GPU render at bind pose ---
    let instance = egui_wgpu::wgpu::Instance::new(
        egui_wgpu::wgpu::InstanceDescriptor::new_without_display_handle(),
    );
    let adapter =
        pollster::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).expect("device");

    let render = |scene: Arc<viewer_scene::Scene>, anim: Option<(usize, f32)>, label: &str| {
        let (w, h) = (400u32, 300u32);
        let mut renderer =
            viewer_render::Renderer::new(&device, egui_wgpu::wgpu::TextureFormat::Bgra8Unorm);
        renderer.resize(&device, w, h);
        renderer.set_scene(&device, &queue, scene);
        let center = glam::Vec3::new(0.0, 0.5, 0.0);
        let (vp, cam) = viewer_render::preview::camera_at(15.0, -90.0, 2.6, center);
        renderer.queue_frame(viewer_render::FrameParams {
            view_proj: vp,
            camera_pos: cam,
            turntable_deg: 0.0,
            model_scale: 1.0,
            lights: default_rig(),
            env_color: [srgb_to_linear(1.0); 3],
            env_intensity: 0.4,
            anim,
            settings: viewer_render::RenderSettings {
                clear_color: viewer_render::DEFAULT_CLEAR_COLOR,
                wireframe: false,
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
        let img = renderer.collect_screenshot(&device).expect("readback");
        // Count non-background pixels.
        let mut count = 0usize;
        for p in img.pixels() {
            let is_bg =
                p[0].abs_diff(p[1]) < 6 && p[0].abs_diff(p[2]) < 6 && p[0] < 125 && p[0] > 80;
            if !is_bg {
                count += 1;
            }
        }
        eprintln!("{label}: {count} visible pixels / {}", w * h);
        img
    };

    let scene2 = Arc::new(viewer_scene::load(path).unwrap());
    let _bind_img = render(scene2.clone(), None, "bind pose");
    let scene3 = Arc::new(viewer_scene::load(path).unwrap());
    let anim_img = render(scene3, Some((0, 0.5)), "t=0.5 clip 0");

    // Diff between bind and animated:
    let bind_img = _bind_img;
    let mut diff = 0usize;
    for (a, b) in bind_img.pixels().zip(anim_img.pixels()) {
        if a[0].abs_diff(b[0]) > 8 || a[1].abs_diff(b[1]) > 8 || a[2].abs_diff(b[2]) > 8 {
            diff += 1;
        }
    }
    eprintln!("bind vs t=0.5 diff: {diff} pixels (0 = animation has no visual effect)");
}
