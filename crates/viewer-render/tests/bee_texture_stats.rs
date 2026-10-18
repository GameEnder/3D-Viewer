// Diagnostic: compare the diffuse texture vs the baked base/MR textures
// for Bee.glb — find where the bake loses brightness / tints.
#[test]
#[ignore = "GPU-independent texture diagnostic; --ignored --nocapture"]
fn bee_texture_bake_stats() {
    let p = std::path::Path::new(r"C:\Users\Steven\Desktop\Bee.glb");
    if !p.exists() {
        eprintln!("skipping");
        return;
    }
    let scene = viewer_scene::load(p).unwrap();
    let mat = &scene.materials[0];
    let base = mat.base_color_texture.as_ref().expect("base tex");
    let mr = mat.metallic_roughness_texture.as_ref().expect("mr tex");
    let mut db_sum = [0f32; 3];
    let mut bb_sum = [0f32; 3];
    let mut m_sum = 0f32;
    let mut r_sum = 0f32;
    let mut n = 0usize;
    let mut hi_metal = 0usize;
    let step_x = (base.width / 64).max(1);
    let step_y = (base.height / 64).max(1);
    let mut y = 0u32;
    while y < base.height {
        let mut x = 0u32;
        while x < base.width {
            let ob = ((y * base.width + x) * 4) as usize;
            let om = ((y * mr.width + x) * 4) as usize;
            for c in 0..3 {
                db_sum[c] += base.rgba[ob + c] as f32 / 255.0;
                bb_sum[c] += base.rgba[ob + c] as f32 / 255.0;
            }
            let m = mr.rgba[om + 2] as f32 / 255.0;
            let r = mr.rgba[om + 1] as f32 / 255.0;
            m_sum += m;
            r_sum += r;
            if m > 0.7 {
                hi_metal += 1;
            }
            n += 1;
            x += step_x;
        }
        y += step_y;
    }
    eprintln!(
        "baked base mean rgb: [{:.3}, {:.3}, {:.3}]",
        bb_sum[0] / n as f32,
        bb_sum[1] / n as f32,
        bb_sum[2] / n as f32
    );
    eprintln!(
        "mean metallic: {:.3}, mean roughness: {:.3}, texels m>0.7: {} / {}",
        m_sum / n as f32,
        r_sum / n as f32,
        hi_metal,
        n
    );
}
