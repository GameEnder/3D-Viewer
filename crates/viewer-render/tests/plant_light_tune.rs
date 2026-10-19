// Diagnostic: render the bee with several (light-scale, env-scale)
// multipliers and measure the lit/shadow encoded-brightness ratio on the
// model, searching for the original's soft fill (ratio ~0.45-0.5) while
// keeping the calibrated ground-pool brightness.
#[test]
#[ignore = "GPU tuning diagnostic; --ignored --nocapture"]
fn plant_lighting_fill_search() {
    let path = std::path::Path::new(r"C:\Users\Steven\Desktop\plant_small.fbx");
    if !path.exists() {
        eprintln!("skipping: plant_small.fbx absent");
        return;
    }

    use std::sync::Arc;

    fn srgb_to_linear(c: f32) -> f32 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    let scene = Arc::new(viewer_scene::load(path).expect("plant loads"));

    let instance = egui_wgpu::wgpu::Instance::new(
        egui_wgpu::wgpu::InstanceDescriptor::new_without_display_handle(),
    );
    let adapter =
        pollster::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).expect("device");

    // (light_mult, env_mult, exposure, contrast) — target: the bee's lit
    // fur ≈ 200 encoded, shadow belly ≈ 95-105, while the ground pool
    // stays ≈ 119 (the original-measured calibration).
    // (light_mult, env_mult, exposure, contrast) — target: the plant's
    // flat mid-gray look (lit ~135, shadow ~115, ratio ~0.85).
    let candidates: [(f32, f32, f32, f32); 8] = [
        (1.0, 1.0, 1.0, 1.0), // current constants, neutral post
        (0.6, 0.6, 1.0, 1.0),
        (0.45, 0.45, 1.0, 1.0),
        (0.35, 0.35, 1.0, 1.0),
        (0.3, 0.3, 1.0, 1.0),
        (0.3, 0.2, 1.0, 1.0),
        (0.25, 0.15, 1.0, 1.0),
        (0.2, 0.1, 1.0, 1.0),
    ];
    for (lm, em, exposure, contrast) in candidates {
        let (w, h) = (400u32, 300u32);
        let mut renderer =
            viewer_render::Renderer::new(&device, egui_wgpu::wgpu::TextureFormat::Bgra8Unorm);
        renderer.resize(&device, w, h);
        renderer.set_scene(&device, &queue, scene.clone());
        let mut lights = std::array::from_fn(|i| {
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
                intensity: lm,
                shadows: false,
            }
        });
        for l in &mut lights {
            l.shadows = false;
        }
        let center = glam::Vec3::new(0.0, 0.5, 0.0);
        let (vp, cam) = viewer_render::preview::camera_at(15.0, -90.0, 2.6, center);
        renderer.queue_frame(viewer_render::FrameParams {
            view_proj: vp,
            camera_pos: cam,
            turntable_deg: 0.0,
            model_scale: 1.0,
            lights,
            env_color: [srgb_to_linear(1.0); 3],
            env_intensity: 0.4 * em,
            anim: None,
            settings: viewer_render::RenderSettings {
                clear_color: viewer_render::DEFAULT_CLEAR_COLOR,
                wireframe: false,
                ground: false,
                grid: false,
                bloom: false,
                exposure,
                contrast,
                ground_opacity: 0.4,
                shadows: false,
            },
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.request_screenshot();
        renderer.prepare_render(&device, &queue, &mut encoder);
        queue.submit(Some(encoder.finish()));
        let img = renderer.collect_screenshot(&device).expect("readback");

        // Split the MODEL's non-bg pixels at the median height: upper half
        // = lit back, lower half = shadowed belly/legs. (Ground OFF so no
        // disc pixels pollute the bands — matches the user's screenshots.)
        let mut ys: Vec<u32> = Vec::new();
        let mut px_by_y: Vec<(u32, f32)> = Vec::new();
        for y in 0..img.height() {
            for x in 0..img.width() {
                let p = img.get_pixel(x, y);
                let is_bg =
                    p[0].abs_diff(p[1]) < 6 && p[0].abs_diff(p[2]) < 6 && p[0] < 125 && p[0] > 80;
                if !is_bg {
                    ys.push(y);
                    px_by_y.push((y, (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0));
                }
            }
        }
        if ys.is_empty() {
            eprintln!("light×{lm:.2} env×{em:.2}: NO MODEL PIXELS");
            continue;
        }
        let min_y = *ys.iter().min().unwrap();
        let max_y = *ys.iter().max().unwrap();
        let mid = (min_y + max_y) / 2;
        let mut lit_sum = 0f32;
        let mut lit_n = 0usize;
        let mut sh_sum = 0f32;
        let mut sh_n = 0usize;
        for (y, b) in &px_by_y {
            if *y <= mid {
                lit_sum += b;
                lit_n += 1;
            } else {
                sh_sum += b;
                sh_n += 1;
            }
        }
        let lit = lit_sum / lit_n.max(1) as f32;
        let shadow = sh_sum / sh_n.max(1) as f32;
        let ratio = if lit > 0.0 { shadow / lit } else { 0.0 };
        eprintln!(
            "light×{lm:.2} env×{em:.2} exp={exposure} con={contrast}: lit={lit:.0} shadow={shadow:.0} ratio={ratio:.2}"
        );
    }
}
