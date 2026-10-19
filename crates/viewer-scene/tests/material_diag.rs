// Diagnostic: print the material albedos our loaders extract for the
// plant FBX and the bee GLB — checking whether the plant's diffuse is
// white (blowout) or mid-gray.
#[test]
#[ignore = "material diagnostic; --ignored --nocapture"]
fn plant_and_bee_albedos() {
    let plant = std::path::Path::new(r"C:\Users\Steven\Desktop\plant_small.fbx");
    if plant.exists() {
        let scene = viewer_scene::load(plant).expect("plant loads");
        eprintln!("plant_small.fbx: {} materials", scene.materials.len());
        for (i, m) in scene.materials.iter().take(8).enumerate() {
            eprintln!(
                "  mat {i}: base=({:.3},{:.3},{:.3},{:.3}) metal={:.2} rough={:.2} tex={}",
                m.base_color[0],
                m.base_color[1],
                m.base_color[2],
                m.base_color[3],
                m.metallic,
                m.roughness,
                m.base_color_texture.is_some()
            );
        }
    }
    let bee = std::path::Path::new(r"C:\Users\Steven\Desktop\Bee.glb");
    if bee.exists() {
        let scene = viewer_scene::load(bee).expect("bee loads");
        eprintln!("Bee.glb: {} materials", scene.materials.len());
        for (i, m) in scene.materials.iter().take(4).enumerate() {
            eprintln!(
                "  mat {i}: base=({:.3},{:.3},{:.3},{:.3}) metal={:.2} rough={:.2} tex={}",
                m.base_color[0],
                m.base_color[1],
                m.base_color[2],
                m.base_color[3],
                m.metallic,
                m.roughness,
                m.base_color_texture.is_some()
            );
        }
    }
}
