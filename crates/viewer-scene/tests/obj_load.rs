use std::path::Path;

fn fixture(p: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/assets")
        .join(p)
}

#[test]
fn loads_obj_with_mtl() {
    let scene = viewer_scene::load(&fixture("cube.obj")).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.meshes[0].positions.len(), 8);
    // 12 triangles as soup after tobj triangulation: indices count 36
    assert_eq!(scene.meshes[0].indices.len(), 36);
    let m = &scene.materials[0];
    // Kd 1.0 0.1 0.1 is sRGB -> linear ~= (1.0, 0.01, 0.01), then
    // brightness-normalized (max component capped at 0.5).
    assert!(m.base_color[0] > 0.45 && m.base_color[1] < 0.02);
    // Ns 250 -> roughness = 1 - 0.25 = 0.75
    assert!((m.roughness - 0.75).abs() < 0.01);
    // normalized: bounding sphere radius ~= 1
    assert!((scene.bounds.bounding_sphere_radius() - 1.0).abs() < 1e-3);
}

#[test]
fn obj_without_mtl_gets_default_material() {
    let scene = viewer_scene::load(&fixture("no_mtl.obj")).unwrap();
    assert_eq!(scene.materials.len(), 1);
    let m = &scene.materials[0];
    // Default 0.8 gray after brightness normalization (max component
    // capped at 0.5): 0.8 * 0.5/0.8 = 0.5.
    assert_eq!(m.base_color, [0.5, 0.5, 0.5, 1.0]);
    assert!((m.roughness - 0.5).abs() < 1e-6);
    assert!((m.metallic).abs() < 1e-6);
    // no usemtl -> material_id None -> material 0
    assert_eq!(scene.meshes[0].material, 0);
    assert!(scene.warnings.is_empty());
}

#[test]
fn missing_map_kd_pushes_warning_not_error() {
    let scene = viewer_scene::load(&fixture("bad_texture.obj")).unwrap();
    assert_eq!(scene.materials.len(), 1);
    assert!(scene.materials[0].base_color_texture.is_none());
    assert!(
        scene
            .warnings
            .iter()
            .any(|w| w.contains("map_Kd") && w.contains("does_not_exist.png"))
    );
}

/// Task 14 ruling: the metallic branch fires on specular luminance —
/// `0.2126*R + 0.7152*G + 0.0722*B > 0.6` maps to metallic 0.5, at or below
/// stays 0. Ks 0.7 gray (lum 0.7) vs Ks 0.5 gray (lum 0.5) straddle the
/// threshold; MTL order fixes material indices.
#[test]
fn high_ks_luminance_maps_to_metallic_half() {
    let scene = viewer_scene::load(&fixture("metallic.obj")).unwrap();
    assert_eq!(scene.materials.len(), 2);
    let shiny = &scene.materials[0];
    let dull = &scene.materials[1];
    assert!(
        (shiny.metallic - 0.5).abs() < 1e-6,
        "Ks luminance 0.7 > 0.6 must map to metallic 0.5, got {}",
        shiny.metallic
    );
    assert!(
        dull.metallic.abs() < 1e-6,
        "Ks luminance 0.5 <= 0.6 must stay metallic 0, got {}",
        dull.metallic
    );
    // The meshes keep their per-usemtl materials.
    assert_eq!(scene.meshes[0].material, 0);
    assert_eq!(scene.meshes[1].material, 1);
}
