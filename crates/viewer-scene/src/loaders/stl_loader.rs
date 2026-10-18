// crates/viewer-scene/src/loaders/stl_loader.rs
//! STL loader (both dialects: binary and ASCII).
//!
//! STL is a triangle-soup format: no index buffer, no UVs, no material —
//! every facet simply carries three vertices. We therefore emit a
//! non-indexed `Mesh` (empty `indices`, three `positions` per triangle) and
//! leave `normals` empty; `normalize::compute_normals` fills them in later
//! from face geometry.
//!
//! The two dialects are distinguished by size arithmetic, not by content:
//! a binary STL is exactly `84 + 50 * n` bytes (80-byte header, 4-byte
//! triangle count, then 50 bytes per facet).

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
    // An STL with zero facets is almost certainly a truncated or corrupt
    // file rather than a legitimate empty model — reject it explicitly.
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
    // STL carries no material info; give the mesh the default PBR gray.
    scene.materials.push(crate::scene::Material::default_pbr());
    // Triangle soup: 9 floats (3 vertices) per facet, no index buffer.
    let mut positions = Vec::with_capacity(tris.len() * 9);
    for t in &tris {
        for v in [t.a, t.b, t.c] {
            positions.push(v);
        }
    }
    let mesh = Mesh {
        positions,
        normals: vec![], // computed later by normalize::compute_normals
        uvs: vec![],
        indices: vec![],
        material: 0,
        joints: vec![],
        weights: vec![],
    };
    // STL has no hierarchy; everything hangs off one identity node.
    let node = scene.add_node(
        scene.root,
        crate::scene::Trs::identity(),
        Some("stl".into()),
    );
    let mi = scene.add_mesh(mesh);
    scene.nodes[node].meshes.push(mi);
    Ok(scene)
}

/// One STL facet: three corner positions.
#[derive(Clone, Copy)]
struct Tri {
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
}

/// Decide binary vs ASCII without parsing.
///
/// A binary STL's total length must equal exactly `84 + 50 * (declared
/// facet count)` — an ASCII file would have to accidentally satisfy that
/// exact-size equation (and start with 80 arbitrary header bytes) to be
/// misclassified, which never happens in practice.
fn is_binary(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        return false;
    }
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    bytes.len() == 84 + n * 50
}

/// Parse a binary STL.
///
/// Layout per facet: 12 bytes normal, 3x12 bytes vertices, 2 bytes
/// "attribute byte count" (universally ignored). We skip the stored facet
/// normal entirely and recompute smooth normals downstream — STL normals
/// are frequently garbage in the wild.
fn parse_binary(bytes: &[u8]) -> Result<Vec<Tri>, LoadError> {
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    let mut tris = Vec::with_capacity(n);
    let mut off = 84usize;
    for _ in 0..n {
        // Guard against a lying count field: stop with a clear error as
        // soon as a facet runs past the end of the file.
        if off + 50 > bytes.len() {
            return Err(LoadError::Parse {
                format: "STL",
                detail: "truncated facet".into(),
            });
        }
        // Little helper reading the little-endian f32 at facet offset `i`.
        let f = |i: usize| {
            f32::from_le_bytes([
                bytes[off + i],
                bytes[off + i + 1],
                bytes[off + i + 2],
                bytes[off + i + 3],
            ])
        };
        // Vertex order inside the 50-byte record: normal(0..12, skipped),
        // v1 at 12, v2 at 24, v3 at 36.
        let a = [f(12), f(16), f(20)];
        let b = [f(24), f(28), f(32)];
        let c = [f(36), f(40), f(44)];
        tris.push(Tri { a, b, c });
        off += 50;
    }
    Ok(tris)
}

/// Parse an ASCII STL.
///
/// Structure: `solid name` ... `facet normal nx ny nz` / `outer loop` /
/// three `vertex x y z` / `endloop` / `endfacet` ... `endsolid`. We ignore
/// all keywords except `vertex` and simply group every consecutive triple
/// of vertices into one triangle — robust against writers that omit or
/// reorder the decorative keywords.
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
                // Every 3 consecutive vertices form one facet.
                if verts.len() == 3 {
                    tris.push(Tri {
                        a: verts[0],
                        b: verts[1],
                        c: verts[2],
                    });
                    verts.clear();
                }
            }
            _ => {} // solid/facet/outer loop/endloop/endfacet/endsolid: ignored
        }
    }
    Ok(tris)
}

/// Error for a malformed float token in ASCII STL.
fn bad(what: &'static str) -> LoadError {
    LoadError::Parse {
        format: "STL",
        detail: format!("bad {what} value"),
    }
}
