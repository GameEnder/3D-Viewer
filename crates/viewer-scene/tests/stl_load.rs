// crates/viewer-scene/tests/stl_load.rs
use std::io::Write;

fn binary_stl_one_triangle() -> Vec<u8> {
    let mut b: Vec<u8> = Vec::new();
    b.extend_from_slice(&[0u8; 80]); // header
    b.extend_from_slice(&1u32.to_le_bytes()); // tri count
    // normal + 3 verts (f32 LE) + attribute
    let n = [0.0f32, 0.0, 1.0];
    let v = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    b.extend_from_slice(&n.map(|f| f.to_le_bytes()).concat());
    for vert in v {
        b.extend_from_slice(&vert.map(|f| f.to_le_bytes()).concat());
    }
    b.extend_from_slice(&0u16.to_le_bytes());
    b
}

const ASCII_STL: &str = r#"
solid test
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
  facet normal 0 0 1
    outer loop
      vertex 0 1 0
      vertex 1 0 0
      vertex 1 1 0
    endloop
  endfacet
endsolid test
"#;

#[test]
fn loads_binary_and_ascii_stl() {
    let dir = std::env::temp_dir().join("viewer-stl-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("t.stl");
    std::fs::File::create(&bin)
        .unwrap()
        .write_all(&binary_stl_one_triangle())
        .unwrap();
    let s1 = viewer_scene::load(&bin).unwrap();
    assert_eq!(s1.meshes[0].positions.len(), 3);
    assert_eq!(s1.tri_count, 1);
    // smooth normals computed: all ≈ +Z
    let n = s1.meshes[0].normals[0];
    assert!(n[2] > 0.99);

    let asc = dir.join("a.stl");
    std::fs::write(&asc, ASCII_STL).unwrap();
    let s2 = viewer_scene::load(&asc).unwrap();
    assert_eq!(s2.meshes[0].positions.len(), 6);
    assert_eq!(s2.tri_count, 2);
    assert!((s2.bounds.bounding_sphere_radius() - 1.0).abs() < 1e-3); // normalized
}
