// crates/viewer-scene/tests/ma_load.rs
//! Maya ASCII (.ma) loader tests with a synthetic scene fixture. The
//! fixture mirrors the structure of real Maya exports: `createNode` DAG
//! (transform + mesh pair), a `.vt` vectorArray, a `.ed` doubleArray of
//! edge triplets (start, end, flag), per-face `.fc[i]` polyFaces with
//! signed 1-based edge references and `mu` UV mappings, and a `.uvpt`
//! float2Array. A transform with translate/rotate/scale above the mesh
//! exercises world-transform baking.

/// A unit cube under a transform: t=(1,2,3), r=(0,90,0) degrees, s=(2,2,2).
///
/// Vertex layout (shared, 8):
///   v0(-.5,-.5,-.5) v1(.5,-.5,-.5) v2(.5,.5,-.5) v3(-.5,.5,-.5)  (back)
///   v4(-.5,-.5,.5)  v5(.5,-.5,.5)  v6(.5,.5,.5)  v7(-.5,.5,.5)  (front)
/// Edges (1-based): e1=0-1 e2=1-2 e3=2-3 e4=3-0 e5=4-5 e6=5-6 e7=6-7
///   e8=7-4 e9=0-4 e10=1-5 e11=2-6 e12=3-7
/// Every face winding is outward-verified via cross products.
fn build_cube_scene() -> String {
    let edges = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    let verts = [
        (-0.5, -0.5, -0.5),
        (0.5, -0.5, -0.5),
        (0.5, 0.5, -0.5),
        (-0.5, 0.5, -0.5),
        (-0.5, -0.5, 0.5),
        (0.5, -0.5, 0.5),
        (0.5, 0.5, 0.5),
        (-0.5, 0.5, 0.5),
    ];
    let faces: [&[&str]; 6] = [
        // Edge refs per the real-MayA convention (verified by ring closure
        // AND FBX-render comparison: `k` = edge k END->START; `-(k+1)` =
        // edge k START->END; edge 0 forward = -1):
        // (all windings outward-verified via cross products)
        &["-5", "-6", "-7", "-8"], // front  (+Z): 4,5,6,7
        &["0", "3", "2", "1"],     // back   (-Z): 1,0,3,2
        &["-1", "-10", "4", "8"],  // bottom (-Y): 0,1,5,4
        &["-12", "6", "10", "-3"], // top    (+Y): 3,7,6,2
        &["-2", "10", "5", "9"],   // right  (+X): 1,2,6,5
        &["-9", "7", "11", "-4"],  // left   (-X): 0,4,7,3
    ];

    let mut s = String::new();
    s.push_str("//Maya ASCII 2024 scene\n");
    s.push_str("requires maya \"2024\";\n");
    s.push_str("currentUnit -l centimeter -a degree -t film;\n");
    s.push('\n');
    s.push_str("createNode transform -n \"pCube1\";\n");
    s.push_str("setAttr \".t\" -type \"double3\" 1 2 3;\n");
    s.push_str("setAttr \".r\" -type \"double3\" 0 90 0;\n");
    s.push_str("setAttr \".s\" -type \"double3\" 2 2 2;\n");
    s.push_str("createNode mesh -n \"pCubeShape1\" -p \"pCube1\";\n");

    // .vt: count then N x 3 floats.
    s.push_str("setAttr \".vt\" -type \"vectorArray\" 8 ");
    for (x, y, z) in verts {
        s.push_str(&format!("{x} {y} {z} "));
    }
    s.push_str(";\n");

    // .ed: total double count (3 per edge) then (start, end, flag) triples.
    s.push_str(&format!(
        "setAttr \".ed\" -type \"doubleArray\" {} ",
        edges.len() * 3
    ));
    for (a, b) in edges {
        s.push_str(&format!("{a} {b} 0 "));
    }
    s.push_str(";\n");

    // .uvpt: count then N x 2 floats.
    s.push_str("setAttr \".uvpt\" -type \"float2Array\" 4 0 0 1 0 1 1 0 1 ;\n");

    // One polyFaces entry per face.
    for (i, face) in faces.iter().enumerate() {
        s.push_str(&format!(
            "setAttr \".fc[{i}]\" -type \"polyFaces\" f 4 {} mu 0 4 0 1 2 3;\n",
            face.join(" ")
        ));
    }
    s
}

/// The cube's 8 corners after T(1,2,3) * Ry(90deg) * S(2), hand-computed:
/// scale, then rotate (x,z)->(z,-x), then translate.
const EXPECTED_CORNERS: [[f32; 3]; 8] = [
    [0.0, 1.0, 4.0],
    [0.0, 1.0, 2.0],
    [0.0, 3.0, 2.0],
    [0.0, 3.0, 4.0],
    [2.0, 1.0, 4.0],
    [2.0, 1.0, 2.0],
    [2.0, 3.0, 2.0],
    [2.0, 3.0, 4.0],
];

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("viewer-ma-tests");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

#[test]
fn loads_maya_ascii_cube_with_transform_and_uvs() {
    let path = temp_path("cube.ma");
    std::fs::write(&path, build_cube_scene()).unwrap();
    let scene = viewer_scene::load(&path).unwrap();

    assert_eq!(scene.meshes.len(), 1, "one mesh expected");
    let mesh = &scene.meshes[0];

    // Face-vertex soup: 6 quad faces fan-triangulate into 12 triangles,
    // each emitting 3 of its own vertices (36 total, 8 unique corners).
    assert_eq!(mesh.positions.len(), 36);
    assert_eq!(scene.tri_count, 12);
    assert_eq!(scene.vert_count, 36);
    assert!(mesh.indices.is_empty(), "non-indexed face-vertex soup");

    // Unique positions == the transformed cube corners.
    let mut unique: Vec<[f32; 3]> = Vec::new();
    for p in &mesh.positions {
        if !unique.iter().any(|q| q == p) {
            unique.push(*p);
        }
    }
    assert_eq!(
        unique.len(),
        8,
        "36 triangle-vertices collapse to 8 corners"
    );
    for want in &EXPECTED_CORNERS {
        assert!(
            unique
                .iter()
                .any(|got| got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-5)),
            "missing transformed corner {want:?} in {unique:?}"
        );
    }

    // Face 0 (front) emits its corners first: 4,5,6,7 with UVs 0,1,2,3.
    // v4 transformed == (2,1,4); its UV point is (0,0).
    assert!(
        (mesh.positions[0][0] - 2.0).abs() < 1e-5
            && (mesh.positions[0][1] - 1.0).abs() < 1e-5
            && (mesh.positions[0][2] - 4.0).abs() < 1e-5
    );
    assert_eq!(mesh.uvs.len(), 36);
    assert!((mesh.uvs[0][0] - 0.0).abs() < 1e-6 && (mesh.uvs[0][1] - 0.0).abs() < 1e-6);
    assert!((mesh.uvs[1][0] - 1.0).abs() < 1e-6 && (mesh.uvs[1][1] - 0.0).abs() < 1e-6);

    // Smooth normals computed by the normalize step (Maya rarely stores .n):
    // one per triangle-vertex, filled in because .ma ships without them.
    assert_eq!(mesh.normals.len(), 36);
}

#[test]
fn maya_ascii_without_mesh_is_parse_error() {
    let path = temp_path("empty.ma");
    std::fs::write(&path, "//Maya ASCII 2024 scene\nrequires maya \"2024\";\n").unwrap();
    let err = viewer_scene::load(&path).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("no mesh"), "unexpected error: {msg}");
}

#[test]
fn maya_ascii_garbage_is_parse_error() {
    let path = temp_path("garbage.ma");
    std::fs::write(&path, "this is ! not @@ valid MEL at all\n").unwrap();
    let err = viewer_scene::load(&path).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("Maya"), "error should name the format: {msg}");
}

/// Signed volume of a triangle soup (positive = outward/CCW convention).
/// Signed volume of a triangle soup (positive = outward/CCW convention).
#[allow(dead_code)]
fn signed_volume(positions: &[[f32; 3]]) -> f32 {
    let mut sum = 0.0f32;
    for tri in positions.as_chunks::<3>().0 {
        let a = glam::Vec3::from(tri[0]);
        let b = glam::Vec3::from(tri[1]);
        let c = glam::Vec3::from(tri[2]);
        sum += a.dot(b.cross(c));
    }
    sum / 6.0
}

/// Compare the .ma against the same model as .fbx (both on the author's
/// Desktop; auto-skips when absent). Diagnostic for the display mismatch.
#[test]
fn compare_ma_against_fbx_reference() {
    let fbx = std::path::Path::new(r"C:\Users\Steven\Desktop\plant_small.fbx");
    let ma = std::path::Path::new(r"C:\Users\Steven\Desktop\plant_small.ma");
    if !fbx.exists() || !ma.exists() {
        eprintln!("skipping (absent)");
        return;
    }
    let fbx_scene = viewer_scene::load(fbx).expect("fbx loads");
    let ma_scene = viewer_scene::load(ma).expect("ma loads");

    for (label, scene) in [("fbx", &fbx_scene), ("ma", &ma_scene)] {
        eprintln!(
            "--- {label}: {} meshes, {} tris, bounds {:?}..{:?}",
            scene.meshes.len(),
            scene.tri_count,
            scene.bounds.min.to_array(),
            scene.bounds.max.to_array()
        );
        let world = viewer_scene::world_transforms(scene, &[]);
        for (i, node) in scene.nodes.iter().enumerate() {
            if node.meshes.is_empty() {
                continue;
            }
            for &mi in &node.meshes {
                let m = &scene.meshes[mi];
                let mut mn = glam::Vec3::splat(f32::MAX);
                let mut mx = glam::Vec3::splat(f32::MIN);
                for p in &m.positions {
                    let w = world[i] * glam::Vec4::new(p[0], p[1], p[2], 1.0);
                    let w = w.truncate();
                    mn = mn.min(w);
                    mx = mx.max(w);
                }
                eprintln!(
                    "  node {i} {:?}: mesh {} with {} tris, world bounds {:?}..{:?}",
                    node.name.as_deref().unwrap_or("<unnamed>"),
                    mi,
                    m.positions.len() / 3,
                    mn.to_array(),
                    mx.to_array()
                );
            }
        }
    }
}
