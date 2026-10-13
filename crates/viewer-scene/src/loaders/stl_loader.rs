// crates/viewer-scene/src/loaders/stl_loader.rs
use std::path::Path;

use crate::error::LoadError;
use crate::scene::{Mesh, Scene};

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    let bytes = std::fs::read(path)?;
    let tris = if is_binary(&bytes) {
        parse_binary(&bytes)?
    } else {
        parse_ascii(&bytes)?
    };
    if tris.is_empty() {
        return Err(LoadError::Parse {
            format: "STL",
            detail: "no facets found".into(),
        });
    }
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut scene = Scene::new(name);
    scene.materials.push(crate::scene::Material::default_pbr());
    let mut positions = Vec::with_capacity(tris.len() * 9);
    for t in &tris {
        for v in [t.a, t.b, t.c] {
            positions.push(v);
        }
    }
    let mesh = Mesh {
        positions,
        normals: vec![],
        uvs: vec![],
        indices: vec![],
        material: 0,
    };
    let node = scene.add_node(
        scene.root,
        crate::scene::Trs::identity(),
        Some("stl".into()),
    );
    let mi = scene.add_mesh(mesh);
    scene.nodes[node].meshes.push(mi);
    Ok(scene)
}

#[derive(Clone, Copy)]
struct Tri {
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
}

fn is_binary(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        return false;
    }
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    bytes.len() == 84 + n * 50
}

fn parse_binary(bytes: &[u8]) -> Result<Vec<Tri>, LoadError> {
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    let mut tris = Vec::with_capacity(n);
    let mut off = 84usize;
    for _ in 0..n {
        if off + 50 > bytes.len() {
            return Err(LoadError::Parse {
                format: "STL",
                detail: "truncated facet".into(),
            });
        }
        let f = |i: usize| {
            f32::from_le_bytes([
                bytes[off + i],
                bytes[off + i + 1],
                bytes[off + i + 2],
                bytes[off + i + 3],
            ])
        };
        let a = [f(12), f(16), f(20)];
        let b = [f(24), f(28), f(32)];
        let c = [f(36), f(40), f(44)];
        tris.push(Tri { a, b, c });
        off += 50;
    }
    Ok(tris)
}

fn parse_ascii(bytes: &[u8]) -> Result<Vec<Tri>, LoadError> {
    let text = std::str::from_utf8(bytes).map_err(|_| LoadError::Parse {
        format: "STL",
        detail: "invalid UTF-8 in ASCII STL".into(),
    })?;
    let mut tris = Vec::new();
    let mut verts: Vec<[f32; 3]> = Vec::new();
    for line in text.lines() {
        let tok: Vec<&str> = line.split_whitespace().collect();
        match tok.first() {
            Some(&"vertex") if tok.len() >= 4 => {
                let p: [f32; 3] = [
                    tok[1].parse().map_err(|_| bad("vertex x"))?,
                    tok[2].parse().map_err(|_| bad("vertex y"))?,
                    tok[3].parse().map_err(|_| bad("vertex z"))?,
                ];
                verts.push(p);
                if verts.len() == 3 {
                    tris.push(Tri {
                        a: verts[0],
                        b: verts[1],
                        c: verts[2],
                    });
                    verts.clear();
                }
            }
            _ => {}
        }
    }
    Ok(tris)
}

fn bad(what: &'static str) -> LoadError {
    LoadError::Parse {
        format: "STL",
        detail: format!("bad {what} value"),
    }
}
