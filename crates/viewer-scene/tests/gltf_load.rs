// crates/viewer-scene/tests/gltf_load.rs
use std::io::Write;

#[derive(Clone, Copy, PartialEq)]
enum Anim {
    None,
    Linear,
    CubicSpline,
}

fn build_cube_glb(with_animation: bool) -> Vec<u8> {
    build_cube_glb_anim(if with_animation {
        Anim::Linear
    } else {
        Anim::None
    })
}

fn build_cube_glb_anim(anim: Anim) -> Vec<u8> {
    let positions: Vec<f32> = vec![
        -0.5, -0.5, -0.5, 0.5, -0.5, -0.5, -0.5, 0.5, -0.5, 0.5, 0.5, -0.5, -0.5, -0.5, 0.5, 0.5,
        -0.5, 0.5, -0.5, 0.5, 0.5, 0.5, 0.5, 0.5,
    ];
    let normals: Vec<f32> = positions.clone();
    let indices: Vec<u16> = vec![
        0, 1, 3, 0, 3, 2, 4, 6, 7, 4, 7, 5, 0, 4, 5, 0, 5, 1, 1, 5, 7, 1, 7, 3, 3, 7, 6, 3, 6, 2,
        2, 6, 4, 2, 4, 0,
    ]
    // Task 9 smoke finding: these faces were wound CW-from-outside (glTF
    // requires CCW), so the cube rendered inside-out under backface culling.
    // Flip every triangle; the load test only checks counts/materials.
    .as_chunks::<3>()
    .0
    .iter()
    .flat_map(|t| [t[0], t[2], t[1]])
    .collect();
    let times: Vec<f32> = vec![0.0, 1.0];
    // CUBICSPLINE output layout per key: [in-tangent, value, out-tangent] (spec §5.8.2)
    let quats: Vec<f32> = match anim {
        Anim::CubicSpline => {
            let mut q: Vec<f32> = Vec::new();
            for v in [[0.0, 0.0, 0.0, 1.0], [0.0, 0.3826834, 0.0, 0.9238795]] {
                q.extend_from_slice(&[0.0f32, 0.0, 0.0, 0.0]); // in-tangent
                q.extend_from_slice(&v); // value
                q.extend_from_slice(&[0.0f32, 0.0, 0.0, 0.0]); // out-tangent
            }
            q
        }
        _ => vec![
            0.0, 0.0, 0.0, 1.0, // identity
            0.0, 0.3826834, 0.0, 0.9238795, // 45° around Y (sin/cos 22.5°)
        ],
    };
    let mut bin: Vec<u8> = Vec::new();
    for v in &positions {
        bin.extend_from_slice(&v.to_le_bytes());
    } // 96 B
    for v in &normals {
        bin.extend_from_slice(&v.to_le_bytes());
    } // 96 B
    for i in &indices {
        bin.extend_from_slice(&i.to_le_bytes());
    } // 72 B
    for t in &times {
        bin.extend_from_slice(&t.to_le_bytes());
    } // 8 B (view 3)
    for q in &quats {
        bin.extend_from_slice(&q.to_le_bytes());
    } // 32/96 B (view 4)

    let mut json = String::from(
        r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0,"name":"cube"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1},"indices":2,"material":0}]}],"materials":[{"pbrMetallicRoughness":{"baseColorFactor":[1.0,0.0,0.0,1.0],"metallicFactor":0.0,"roughnessFactor":0.5},"alphaMode":"MASK","alphaCutoff":0.4}],"accessors":[{"bufferView":0,"componentType":5126,"count":8,"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]},{"bufferView":1,"componentType":5126,"count":8,"type":"VEC3"},{"bufferView":2,"componentType":5123,"count":36,"type":"SCALAR"}"#,
    );
    if anim != Anim::None {
        // CUBICSPLINE output has 3 elements per key
        json.push_str(&format!(
            r#",{{"bufferView":3,"componentType":5126,"count":2,"type":"SCALAR","max":[1.0]}},{{"bufferView":4,"componentType":5126,"count":{},"type":"VEC4"}}"#,
            quats.len() / 4
        ));
    }
    json.push_str(r#"],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":96},{"buffer":0,"byteOffset":96,"byteLength":96},{"buffer":0,"byteOffset":192,"byteLength":72}"#);
    if anim != Anim::None {
        json.push_str(&format!(
            r#",{{"buffer":0,"byteOffset":264,"byteLength":8}},{{"buffer":0,"byteOffset":272,"byteLength":{}}}"#,
            quats.len() * 4
        ));
    }
    json.push_str(&format!(
        r#"],"buffers":[{{"byteLength":{}}}]"#,
        272 + quats.len() * 4
    ));
    if anim != Anim::None {
        let interpolation = if anim == Anim::CubicSpline {
            "CUBICSPLINE"
        } else {
            "LINEAR"
        };
        json.push_str(&format!(
            r#","animations":[{{"channels":[{{"sampler":0,"target":{{"node":0,"path":"rotation"}}}}],"samplers":[{{"input":3,"interpolation":"{interpolation}","output":4}}]}}]"#
        ));
    }
    json.push('}');

    let json_bytes = json.as_bytes().to_vec();
    let pad_json = (4 - json_bytes.len() % 4) % 4;
    let pad_bin = (4 - bin.len() % 4) % 4;
    let total = 12 + 8 + json_bytes.len() + pad_json + 8 + bin.len() + pad_bin;

    let mut glb: Vec<u8> = Vec::new();
    glb.extend_from_slice(&0x46546C67u32.to_le_bytes()); // magic "glTF"
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&((json_bytes.len() + pad_json) as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // "JSON"
    glb.extend_from_slice(&json_bytes);
    glb.extend(std::iter::repeat_n(b' ', pad_json));
    glb.extend_from_slice(&((bin.len() + pad_bin) as u32).to_le_bytes());
    glb.extend_from_slice(&0x004E4942u32.to_le_bytes()); // "BIN"
    glb.extend_from_slice(&bin);
    glb.extend(std::iter::repeat_n(0u8, pad_bin));
    glb
}

fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("viewer-scene-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(bytes).unwrap();
    p
}

/// Minimal two-node GLB (Task 17 review follow-up): scene root -> "parent"
/// -> "child". No meshes/buffers — hierarchy only, so the fixture stays a
/// bare JSON chunk inside the GLB container (buffers are optional in glTF
/// 2.0 when no accessor references them).
fn build_two_node_glb() -> Vec<u8> {
    let json = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"name":"parent","children":[1]},{"name":"child"}]}"#;
    let json_bytes = json.as_bytes().to_vec();
    let pad_json = (4 - json_bytes.len() % 4) % 4;
    let total = 12 + 8 + json_bytes.len() + pad_json;

    let mut glb: Vec<u8> = Vec::new();
    glb.extend_from_slice(&0x46546C67u32.to_le_bytes()); // magic "glTF"
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&((json_bytes.len() + pad_json) as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // "JSON"
    glb.extend_from_slice(&json_bytes);
    glb.extend(std::iter::repeat_n(b' ', pad_json));
    glb
}

/// Pins the reparent path (Task 17 review follow-up): the loader parents
/// every node to the synthetic root first, then fixes up true parents — the
/// child must end up under "parent" (parent field AND parent's children) and
/// detached from the root's children list.
#[test]
fn two_node_glb_reparents_child_under_its_parent() {
    let path = write_temp("hierarchy.glb", &build_two_node_glb());
    let scene = viewer_scene::load(&path).unwrap();
    assert_eq!(scene.nodes.len(), 3); // synthetic root + parent + child
    let parent = scene
        .nodes
        .iter()
        .position(|n| n.name.as_deref() == Some("parent"))
        .expect("parent node created");
    let child = scene
        .nodes
        .iter()
        .position(|n| n.name.as_deref() == Some("child"))
        .expect("child node created");
    assert_ne!(parent, child);
    // BFS creates the parent (scene root's child) before its child.
    assert_eq!(scene.nodes[child].parent, Some(parent));
    assert!(
        !scene.nodes[scene.root].children.contains(&child),
        "child must be detached from the synthetic root"
    );
    assert!(
        scene.nodes[parent].children.contains(&child),
        "parent.children must contain the child"
    );
    assert!(
        scene.nodes[scene.root].children.contains(&parent),
        "the top-level parent stays under the root"
    );
}

#[test]
fn loads_glb_mesh_material_animation() {
    let path = write_temp("cube.glb", &build_cube_glb(true));
    let scene = viewer_scene::load(&path).unwrap();
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.meshes[0].positions.len(), 8);
    assert_eq!(scene.meshes[0].indices.len(), 36);
    assert_eq!(scene.materials[0].base_color, [1.0, 0.0, 0.0, 1.0]);
    assert!(matches!(
        scene.materials[0].alpha_mode,
        viewer_scene::AlphaMode::Mask(c) if (c - 0.4).abs() < 1e-6
    ));
    assert_eq!(scene.nodes.len(), 2); // synthetic root + cube
    assert_eq!(scene.nodes[1].name.as_deref(), Some("cube"));
    assert!(scene.nodes[1].meshes.len() == 1);
    assert_eq!(scene.animations.len(), 1);
    let a = &scene.animations[0];
    assert_eq!(a.channels.len(), 1);
    assert_eq!(a.duration, 1.0);
    assert_eq!(a.channels[0].keys.times, vec![0.0, 1.0]);
    assert_eq!(a.channels[0].keys.values.len(), 2);
    assert_eq!(scene.tri_count, 12);
}

#[test]
fn rejects_fbx_with_friendly_error() {
    let err = viewer_scene::load(std::path::Path::new("model.fbx"))
        .err()
        .expect("FBX must be rejected");
    assert!(matches!(err, viewer_scene::LoadError::UnsupportedFormat(_)));
}

#[test]
fn loads_cubic_spline_animation() {
    let path = write_temp("cube_spline.glb", &build_cube_glb_anim(Anim::CubicSpline));
    let scene = viewer_scene::load(&path).unwrap();
    assert_eq!(scene.animations.len(), 1);
    let keys = &scene.animations[0].channels[0].keys;
    assert_eq!(keys.times, vec![0.0, 1.0]);
    assert_eq!(keys.interpolation, viewer_scene::Interpolation::CubicSpline);
    assert_eq!(
        keys.values,
        vec![[0.0, 0.0, 0.0, 1.0], [0.0, 0.3826834, 0.0, 0.9238795]]
    );
    assert!(keys.in_tangents.iter().all(|t| *t == [0.0f32; 4]));
    assert!(keys.out_tangents.iter().all(|t| *t == [0.0f32; 4]));
    assert_eq!(keys.in_tangents.len(), 2);
    assert_eq!(keys.out_tangents.len(), 2);
}

#[test]
fn animation_channel_targets_the_created_node() {
    // Regression (Task 17 smoke finding): the glTF loader's node map stored
    // the BFS position, but created nodes land at 1 + position (the
    // synthetic root occupies scene slot 0). Channels then targeted the
    // wrong node — here the root, whose TRS normalize() overwrites, so a
    // playing animation would collapse the model instead of rotating the
    // cube node.
    let path = write_temp("cube_probe.glb", &build_cube_glb(true));
    let scene = viewer_scene::load(&path).unwrap();
    assert_eq!(scene.nodes.len(), 2);
    assert_eq!(scene.nodes[1].name.as_deref(), Some("cube"));
    assert_eq!(scene.animations[0].channels[0].node, 1);
}
