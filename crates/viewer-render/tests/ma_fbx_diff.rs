// crates/viewer-render/tests/ma_fbx_diff.rs
//! Diagnostic: render the same model loaded from .ma and from .fbx with
//! identical camera + lighting, then compare the images band-by-band
//! (bottom = soil, middle = vase, top = plant) to localize display
//! differences. Auto-skips when the author's Desktop files are absent.

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

fn render_scene_png(
    device: &egui_wgpu::wgpu::Device,
    queue: &egui_wgpu::wgpu::Queue,
    scene: Arc<viewer_scene::Scene>,
    out: &std::path::Path,
) -> image::RgbaImage {
    let (w, h) = (400u32, 300u32);
    let mut renderer =
        viewer_render::Renderer::new(device, egui_wgpu::wgpu::TextureFormat::Bgra8Unorm);
    renderer.resize(device, w, h);
    renderer.set_scene(device, queue, scene.clone());
    let center = scene.bounds.center();
    let (view_proj, cam_pos) = viewer_render::preview::camera_at(15.0, -90.0, 2.6, center);
    renderer.queue_frame(viewer_render::FrameParams {
        view_proj,
        camera_pos: cam_pos,
        turntable_deg: 0.0,
        model_scale: 1.0,
        lights: default_rig(),
        env_color: [srgb_to_linear(1.0); 3],
        env_intensity: 0.4,
        anim: None,
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
    renderer.prepare_render(device, queue, &mut encoder);
    queue.submit(Some(encoder.finish()));
    let img = renderer.collect_screenshot(device).expect("readback");
    img.save(out).expect("save png");
    img
}

/// Mean brightness of a horizontal band [y0, y1) counting only
/// non-background pixels (background encodes ~97-110 gray).
fn band_stats(img: &image::RgbaImage, y0: u32, y1: u32) -> (usize, f32) {
    let mut count = 0usize;
    let mut sum = 0f32;
    for y in y0..y1 {
        for x in 0..img.width() {
            let p = img.get_pixel(x, y);
            // Background gradient is near-gray ~97-110; content differs.
            let is_bg =
                p[0].abs_diff(p[1]) < 6 && p[0].abs_diff(p[2]) < 6 && p[0] < 125 && p[0] > 80;
            if !is_bg {
                count += 1;
                sum += (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0;
            }
        }
    }
    (count, if count > 0 { sum / count as f32 } else { 0.0 })
}

#[test]
#[ignore = "GPU render diagnostic (like the calibration suite); run with \
            `cargo test -p viewer-render --test ma_fbx_diff -- --ignored --nocapture`. \
            Auto-skips without the author's Desktop files."]
fn ma_vs_fbx_render_diff() {
    let fbx_path = std::path::Path::new(r"C:\Users\Steven\Desktop\plant_small.fbx");
    let ma_path = std::path::Path::new(r"C:\Users\Steven\Desktop\plant_small.ma");
    if !fbx_path.exists() || !ma_path.exists() {
        eprintln!("skipping: desktop files absent");
        return;
    }
    let fbx = Arc::new(viewer_scene::load(fbx_path).expect("fbx loads"));
    let ma = Arc::new(viewer_scene::load(ma_path).expect("ma loads"));

    let instance = egui_wgpu::wgpu::Instance::new(
        egui_wgpu::wgpu::InstanceDescriptor::new_without_display_handle(),
    );
    let adapter = pollster::block_on(
        instance.request_adapter(&egui_wgpu::wgpu::RequestAdapterOptions::default()),
    )
    .expect("adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).expect("device");

    let dir = std::env::temp_dir().join("opencode");
    let _ = std::fs::create_dir_all(&dir);
    let fbx_img = render_scene_png(&device, &queue, fbx, &dir.join("plant_fbx.png"));
    let ma_img = render_scene_png(&device, &queue, ma, &dir.join("plant_ma.png"));

    let h = fbx_img.height();
    let bands = [
        ("bottom (soil)", 2 * h / 3, h),
        ("middle (vase)", h / 3, 2 * h / 3),
        ("top (plant)", 0, h / 3),
    ];
    for (name, y0, y1) in bands {
        let (fc, fm) = band_stats(&fbx_img, y0, y1);
        let (mc, mm) = band_stats(&ma_img, y0, y1);
        eprintln!(
            "{name}: fbx {fc} px @ {fm:.1},  ma {mc} px @ {mm:.1}  (px delta {:+}%, brightness delta {:+.1})",
            (mc as i64 - fc as i64) * 100 / (fc.max(1) as i64),
            mm - fm
        );
    }
    // Raw pixel diff count between the two renders.
    let mut diff = 0usize;
    for (a, b) in fbx_img.pixels().zip(ma_img.pixels()) {
        if a[0].abs_diff(b[0]) > 8 || a[1].abs_diff(b[1]) > 8 || a[2].abs_diff(b[2]) > 8 {
            diff += 1;
        }
    }
    eprintln!(
        "total differing pixels: {diff} / {} ({:.1}%)",
        fbx_img.width() * fbx_img.height(),
        100.0 * diff as f32 / (fbx_img.width() * fbx_img.height()) as f32
    );
}
