// crates/viewer-scene/tests/nif_load.rs
//! NIF loader tests using synthetic in-memory NIF binaries (no game data).
//! The fixture layout follows Gamebryo v20.2.0.7 / user_version 12 /
//! bsver 100 (Skyrim SE) serialization.

fn w8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}
fn w16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn w32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn wf32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn wsstr(buf: &mut Vec<u8>, s: &str) {
    w32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}
/// BSStreamHeader short string: u8 length (incl. null) + chars + null.
fn wshort(buf: &mut Vec<u8>, s: &str) {
    w8(buf, s.len() as u8 + 1);
    buf.extend_from_slice(s.as_bytes());
    w8(buf, 0);
}

/// Build a minimal Skyrim SE NIF: NiNode root -> NiTriShape (translated
/// (1,2,3) and scaled 2x) -> NiTriShapeData with one triangle.
fn build_skyrim_mesh_nif() -> Vec<u8> {
    let mut nif = Vec::new();

    // Header line
    nif.extend_from_slice(b"Gamebryo File Format, Version 20.2.0.7\n");
    w32(&mut nif, 0x14020007); // version
    w8(&mut nif, 1); // little-endian
    w32(&mut nif, 12); // user_version
    w32(&mut nif, 3); // num_blocks
    // BSStreamHeader (bsver=100)
    w32(&mut nif, 100); // bsver
    wshort(&mut nif, "Test"); // author
    wshort(&mut nif, ""); // process_script (bsver < 131)
    wshort(&mut nif, ""); // export_script

    // Block type table: NiNode, NiTriShape, NiTriShapeData
    w16(&mut nif, 3);
    wsstr(&mut nif, "NiNode");
    wsstr(&mut nif, "NiTriShape");
    wsstr(&mut nif, "NiTriShapeData");
    w16(&mut nif, 0); // block 0 -> NiNode
    w16(&mut nif, 1); // block 1 -> NiTriShape
    w16(&mut nif, 2); // block 2 -> NiTriShapeData

    let block_sizes_offset = nif.len();
    w32(&mut nif, 0); // placeholder: NiNode size
    w32(&mut nif, 0); // placeholder: NiTriShape size
    w32(&mut nif, 0); // placeholder: NiTriShapeData size

    // String table: "Scene Root" (0), "Cube" (1)
    w32(&mut nif, 2);
    w32(&mut nif, 10);
    wsstr(&mut nif, "Scene Root");
    wsstr(&mut nif, "Cube");
    w32(&mut nif, 0); // num_groups

    // Block 0: NiNode root
    let b0 = nif.len();
    w32(&mut nif, 0); // name = "Scene Root"
    w32(&mut nif, 0); // num_extra_data
    w32(&mut nif, 0xFFFF_FFFF); // controller_ref = -1
    w32(&mut nif, 0x0E); // flags
    wf32(&mut nif, 0.0); // translation
    wf32(&mut nif, 0.0);
    wf32(&mut nif, 0.0);
    for v in [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
        wf32(&mut nif, v); // rotation (identity)
    }
    wf32(&mut nif, 1.0); // scale
    w32(&mut nif, 0xFFFF_FFFF); // collision_ref = -1
    w32(&mut nif, 1); // num_children
    w32(&mut nif, 1); // child 0 -> block 1 (NiTriShape)
    w32(&mut nif, 0); // num_effects (bsver < 130)
    let s0 = (nif.len() - b0) as u32;

    // Block 1: NiTriShape (bsver=100: no properties list; shader/alpha refs present)
    let b1 = nif.len();
    w32(&mut nif, 1); // name = "Cube"
    w32(&mut nif, 0); // num_extra_data
    w32(&mut nif, 0xFFFF_FFFF); // controller_ref
    w32(&mut nif, 0x0F); // flags
    wf32(&mut nif, 1.0); // translation (1, 2, 3)
    wf32(&mut nif, 2.0);
    wf32(&mut nif, 3.0);
    for v in [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
        wf32(&mut nif, v); // rotation (identity)
    }
    wf32(&mut nif, 2.0); // scale = 2
    w32(&mut nif, 0xFFFF_FFFF); // collision_ref
    w32(&mut nif, 2); // data_ref -> block 2
    w32(&mut nif, 0xFFFF_FFFF); // skin_instance_ref
    w32(&mut nif, 0); // num_materials
    w32(&mut nif, 0); // active_material_index
    w8(&mut nif, 0); // dirty flag (v20.2.0.7)
    w32(&mut nif, 0xFFFF_FFFF); // shader_property_ref (bsver > 34)
    w32(&mut nif, 0xFFFF_FFFF); // alpha_property_ref
    let s1 = (nif.len() - b1) as u32;

    // Block 2: NiTriShapeData (3 vertices, 1 triangle, 1 UV set, normals)
    let b2 = nif.len();
    w32(&mut nif, 0); // group_id (since 10.1.0.114)
    w16(&mut nif, 3); // num_vertices
    w8(&mut nif, 0); // keep_flags
    w8(&mut nif, 0); // compress_flags
    w8(&mut nif, 1); // has_vertices
    for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        wf32(&mut nif, v); // vertices
    }
    w16(&mut nif, 0x0001); // data_flags: Has UV (BSGeometryDataFlags bit 0)
    w32(&mut nif, 0); // material_crc (bsver > 34)
    w8(&mut nif, 1); // has_normals
    for _ in 0..3 {
        wf32(&mut nif, 0.0); // normals: +Z in NIF space
        wf32(&mut nif, 0.0);
        wf32(&mut nif, 1.0);
    }
    wf32(&mut nif, 0.5); // center
    wf32(&mut nif, 0.5);
    wf32(&mut nif, 0.0);
    wf32(&mut nif, 1.0); // radius
    w8(&mut nif, 0); // has_vertex_colors
    for uv in [[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0]] {
        wf32(&mut nif, uv[0]); // one UV set
        wf32(&mut nif, uv[1]);
    }
    w16(&mut nif, 0); // consistency_flags
    w32(&mut nif, 0xFFFF_FFFF); // additional_data_ref (since 20.0.0.4)
    w16(&mut nif, 1); // num_triangles
    w32(&mut nif, 3); // num_triangle_points
    w8(&mut nif, 1); // has_triangles (since 10.0.1.3)
    w16(&mut nif, 0); // triangle 0: [0, 1, 2]
    w16(&mut nif, 1);
    w16(&mut nif, 2);
    w16(&mut nif, 0); // num_match_groups
    let s2 = (nif.len() - b2) as u32;

    // Patch block sizes.
    for (i, s) in [s0, s1, s2].into_iter().enumerate() {
        let off = block_sizes_offset + i * 4;
        nif[off..off + 4].copy_from_slice(&s.to_le_bytes());
    }

    nif
}

/// Valid header, zero blocks: parses but has no geometry.
fn build_empty_nif() -> Vec<u8> {
    let mut nif = Vec::new();
    nif.extend_from_slice(b"Gamebryo File Format, Version 20.2.0.7\n");
    w32(&mut nif, 0x14020007);
    w8(&mut nif, 1);
    w32(&mut nif, 12);
    w32(&mut nif, 0); // num_blocks = 0
    w32(&mut nif, 100); // bsver
    wshort(&mut nif, "Test");
    wshort(&mut nif, "");
    wshort(&mut nif, "");
    w16(&mut nif, 0); // num_block_types
    w32(&mut nif, 0); // num_strings
    w32(&mut nif, 0); // max_string_length
    w32(&mut nif, 0); // num_groups
    nif
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("viewer-nif-tests");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

#[test]
fn loads_skyrim_nif_with_transform_and_geometry() {
    let path = temp_path("skyrim_mesh.nif");
    std::fs::write(&path, build_skyrim_mesh_nif()).unwrap();
    let scene = viewer_scene::load(&path).unwrap();

    assert_eq!(scene.meshes.len(), 1, "one mesh expected");
    let mesh = &scene.meshes[0];

    // Node-hierarchy transform baked: shape translate (1,2,3) + scale 2 in
    // NIF space, then Z-up -> Y-up ((x,y,z) -> (x,z,-y)).
    let expected = [[1.0f32, 3.0, -2.0], [3.0, 3.0, -2.0], [1.0, 3.0, -4.0]];
    for (got, want) in mesh.positions.iter().zip(expected) {
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-5, "position {got:?} != {want:?}");
        }
    }

    // NIF +Z normal maps to viewer +Y.
    for n in &mesh.normals {
        assert!(
            (n[0] - 0.0).abs() < 1e-5 && (n[1] - 1.0).abs() < 1e-5 && (n[2] - 0.0).abs() < 1e-5,
            "normal {n:?} should be +Y after the Z-up fix"
        );
    }

    assert_eq!(mesh.indices, vec![0, 1, 2]);
    assert_eq!(mesh.uvs.len(), 3);
    assert!((mesh.uvs[1][0] - 1.0).abs() < 1e-6);

    // Shape node carries the block name.
    let shape_node = scene
        .nodes
        .iter()
        .find(|n| n.name.as_deref() == Some("Cube"))
        .expect("node named Cube");
    assert!(!shape_node.meshes.is_empty());

    assert_eq!(scene.tri_count, 1);
    assert_eq!(scene.vert_count, 3);
}

#[test]
fn garbage_nif_is_parse_error() {
    let path = temp_path("garbage.nif");
    std::fs::write(&path, b"\x00\x01\x02 not a nif at all").unwrap();
    let err = viewer_scene::load(&path).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("NIF"), "error should mention NIF: {msg}");
}

#[test]
fn nif_without_geometry_is_parse_error() {
    let path = temp_path("empty.nif");
    std::fs::write(&path, build_empty_nif()).unwrap();
    let err = viewer_scene::load(&path).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("no mesh geometry") || msg.contains("no meshes"),
        "error should explain missing geometry: {msg}"
    );
}

#[test]
fn truncated_nif_reports_no_geometry() {
    // Half a header: parse fails (truncation at the binary header level).
    let path = temp_path("truncated.nif");
    std::fs::write(&path, &build_skyrim_mesh_nif()[..30]).unwrap();
    let err = viewer_scene::load(&path).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("NIF"), "error should mention NIF: {msg}");
}
