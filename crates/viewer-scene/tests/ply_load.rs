// crates/viewer-scene/tests/ply_load.rs
const ASCII_PLY: &str = r#"
ply
format ascii 1.0
element vertex 4
property float x
property float y
property float z
property float nx
property float ny
property float nz
property uchar red
property uchar green
property uchar blue
element face 1
property list uchar int vertex_indices
end_header
0 0 0 0 0 1 255 0 0
1 0 0 0 0 1 255 0 0
0 1 0 0 0 1 255 0 0
1 1 0 0 0 1 255 0 0
4 0 1 2 3
"#;

const ASCII_PLY_NO_COLORS: &str = r#"
ply
format ascii 1.0
element vertex 3
property float x
property float y
property float z
element face 1
property list uchar int vertex_indices
end_header
0 0 0
1 0 0
0 1 0
3 0 1 2
"#;

const ASCII_PLY_NO_FACES: &str = r#"
ply
format ascii 1.0
element vertex 6
property float x
property float y
property float z
end_header
0 0 0
1 0 0
0 1 0
1 0 0
1 1 0
0 1 0
"#;

fn binary_ply_rgb_triangle() -> Vec<u8> {
    let header = "\
ply
format binary_little_endian 1.0
element vertex 3
property float x
property float y
property float z
property uchar red
property uchar green
property uchar blue
element face 1
property list uchar int vertex_indices
end_header
";
    let mut b = Vec::new();
    b.extend_from_slice(header.as_bytes());
    let verts = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    for v in verts {
        for c in v {
            b.extend_from_slice(&c.to_le_bytes());
        }
        b.push(255); // red
        b.push(0); // green
        b.push(0); // blue
    }
    b.push(3); // per-face vertex count (uchar)
    for i in 0i32..3 {
        b.extend_from_slice(&i.to_le_bytes());
    }
    b
}

#[test]
fn loads_ascii_ply_with_colors_and_normals() {
    let dir = std::env::temp_dir().join("viewer-ply-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.ply");
    std::fs::write(&p, ASCII_PLY).unwrap();
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.meshes[0].positions.len(), 4);
    assert_eq!(s.tri_count, 2); // quad → 2 tris via fan
    assert_eq!(s.meshes[0].normals[0], [0.0, 0.0, 1.0]);
    assert!((s.materials[0].base_color[0] - 1.0).abs() < 1e-3); // red 255 → linear ≈ 1.0
}

#[test]
fn loads_binary_little_endian_ply() {
    let dir = std::env::temp_dir().join("viewer-ply-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("bin.ply");
    std::fs::write(&p, binary_ply_rgb_triangle()).unwrap();
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.meshes[0].positions.len(), 3);
    assert_eq!(s.meshes[0].positions[0], [0.0, 0.0, 0.0]);
    assert_eq!(s.meshes[0].positions[1], [1.0, 0.0, 0.0]);
    assert_eq!(s.meshes[0].positions[2], [0.0, 1.0, 0.0]);
    assert_eq!(s.tri_count, 1);
    assert!((s.materials[0].base_color[0] - 1.0).abs() < 1e-3); // red 255 → linear ≈ 1.0
    assert!(s.materials[0].base_color[1].abs() < 1e-6);
}

#[test]
fn ply_without_color_properties_uses_default_material() {
    let dir = std::env::temp_dir().join("viewer-ply-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("nc.ply");
    std::fs::write(&p, ASCII_PLY_NO_COLORS).unwrap();
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.tri_count, 1);
    assert_eq!(s.materials[0].base_color, [0.8, 0.8, 0.8, 1.0]);
}

// f64::MAX (0x7FEFFFFFFFFFFFFF) little-endian: parses as a garbage list count
// whose `as usize` cast saturates to usize::MAX, exercising overflow-safe
// skip arithmetic. (All-0xFF mantissa with 0x7FF exponent would be NaN → 0.)
const F64_MAX_LE: [u8; 8] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xEF, 0x7F];

fn binary_ply_with_unknown_element(junk_props: &str, junk_body: &[u8]) -> Vec<u8> {
    let header = format!(
        "ply
format binary_little_endian 1.0
element vertex 1
property float x
property float y
property float z
element junk 1
{junk_props}end_header
"
    );
    let mut b = header.into_bytes();
    b.extend_from_slice(&0.5f32.to_le_bytes());
    b.extend_from_slice(&0.5f32.to_le_bytes());
    b.extend_from_slice(&0.5f32.to_le_bytes());
    b.extend_from_slice(junk_body);
    b
}

fn assert_ply_truncated_error(bytes: &[u8], name: &str) {
    let dir = std::env::temp_dir().join("viewer-ply-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    let err = viewer_scene::load(&p).map(|_| ()).unwrap_err();
    assert!(
        err.to_string().contains("truncated binary data"),
        "unexpected error: {err}"
    );
}

#[test]
fn binary_unknown_element_f64_list_garbage_count_errors_not_overflows() {
    let b = binary_ply_with_unknown_element("property list double uchar blob\n", &F64_MAX_LE);
    assert_ply_truncated_error(&b, "bin-ovf-list.ply");
}

#[test]
fn binary_unknown_element_two_f64_lists_garbage_counts_errors_not_overflows() {
    let mut body = F64_MAX_LE.to_vec();
    body.extend_from_slice(&F64_MAX_LE);
    let b = binary_ply_with_unknown_element(
        "property list double uchar blob\nproperty list double uchar blob2\n",
        &body,
    );
    assert_ply_truncated_error(&b, "bin-ovf-list-list.ply");
}

#[test]
fn binary_unknown_element_f64_list_then_scalar_errors_not_overflows() {
    let mut body = F64_MAX_LE.to_vec();
    body.extend_from_slice(&F64_MAX_LE);
    let b = binary_ply_with_unknown_element(
        "property list double uchar blob\nproperty double pad\n",
        &body,
    );
    assert_ply_truncated_error(&b, "bin-ovf-list-scalar.ply");
}

#[test]
fn ply_without_faces_is_point_soup() {
    let dir = std::env::temp_dir().join("viewer-ply-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("soup.ply");
    std::fs::write(&p, ASCII_PLY_NO_FACES).unwrap();
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.meshes[0].positions.len(), 6);
    assert!(s.meshes[0].indices.is_empty());
    assert_eq!(s.tri_count, 2); // soup: positions/3
}

// Task 16 review regression: a zero-property unknown element consumes
// nothing per record, so a huge declared count used to loop ~forever in the
// skip path. The skip bound (count vs file size) must reject it promptly.
const HUGE_COUNT: usize = 4_000_000_000;

#[test]
fn ascii_unknown_zero_property_element_with_huge_count_errors_not_hangs() {
    let bytes = format!(
        "ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nelement junk {HUGE_COUNT}\nend_header\n0 0 0\n"
    );
    let dir = std::env::temp_dir().join("viewer-ply-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("ascii-huge-skip.ply");
    std::fs::write(&p, bytes).unwrap();
    let err = viewer_scene::load(&p).map(|_| ()).unwrap_err();
    assert!(
        err.to_string().contains("exceeds the file size"),
        "unexpected error: {err}"
    );
}

#[test]
fn binary_unknown_zero_property_element_with_huge_count_errors_not_hangs() {
    let header = format!(
        "ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nelement junk {HUGE_COUNT}\nend_header\n"
    );
    let mut b = header.into_bytes();
    b.extend_from_slice(&0.5f32.to_le_bytes());
    b.extend_from_slice(&0.5f32.to_le_bytes());
    b.extend_from_slice(&0.5f32.to_le_bytes());
    let dir = std::env::temp_dir().join("viewer-ply-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("bin-huge-skip.ply");
    std::fs::write(&p, b).unwrap();
    let err = viewer_scene::load(&p).map(|_| ()).unwrap_err();
    assert!(
        err.to_string().contains("exceeds the file size"),
        "unexpected error: {err}"
    );
}
