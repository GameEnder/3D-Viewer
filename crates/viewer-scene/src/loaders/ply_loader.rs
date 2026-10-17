// crates/viewer-scene/src/loaders/ply_loader.rs
//! PLY (Stanford Polygon) loader — both ASCII and little-endian binary.
//!
//! PLY is a self-describing format: a text header declares the elements
//! (`vertex`, `face`, or anything else), their counts, and the exact
//! property layout (scalar type + name, or variable-length lists for face
//! indices). The body then follows that schema verbatim — so the loader
//! parses the header into a small type description and walks the body
//! against it, rather than assuming a fixed layout.
//!
//! Only the properties we render are consumed (`x/y/z`, `nx/ny/nz`,
//! `red/green/blue`, face index lists); every other property is skipped
//! in a way that keeps the cursor in sync with the declared schema.
//! Unknown elements are skipped record-by-record with sanity bounds so a
//! corrupt count field can never make the loader hang or run off the file.
//!
//! PLY has no materials, so vertex colors (if present for a majority of
//! vertices) are averaged into one sRGB base color for the whole mesh.

use std::path::Path;

use crate::error::LoadError;
use crate::scene::{Material, Mesh, Scene};

const FMT: &str = "PLY";

pub fn load(path: &Path) -> Result<Scene, LoadError> {
    let bytes = std::fs::read(path)?;
    let header = parse_header(&bytes)?;
    let data = if header.format == Format::Ascii {
        parse_ascii(&bytes, &header)?
    } else {
        parse_binary(&bytes, &header)?
    };
    if data.positions.is_empty() {
        return Err(bad("no vertices found"));
    }
    // Vertex colors -> a single material base color for the whole mesh
    // (PLY has no materials of its own). The 10-vs-6 cross-multiplication
    // tests "colors for at least 60% of vertices" without floats: a file
    // whose colors cover most, but not all, vertices still tints the mesh.
    let mut material = Material::default_pbr();
    if data.colors.len() * 10 >= data.positions.len() * 6 {
        // Average in sRGB, then linearize once — averaging linear values
        // would over-weight bright pixels.
        let mut avg = [0.0f32; 3];
        for c in &data.colors {
            for (a, v) in avg.iter_mut().zip(c) {
                *a += v;
            }
        }
        let n = data.colors.len() as f32;
        for a in &mut avg {
            *a /= n;
        }
        material.base_color = [
            srgb_to_linear(avg[0]),
            srgb_to_linear(avg[1]),
            srgb_to_linear(avg[2]),
            1.0,
        ];
    }
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut scene = Scene::new(name);
    scene.materials.push(material);
    let mesh = Mesh {
        positions: data.positions,
        normals: data.normals,
        uvs: vec![],
        indices: data.indices,
        material: 0,
    };
    // PLY has no hierarchy: one identity node under the scene root.
    let node = scene.add_node(
        scene.root,
        crate::scene::Trs::identity(),
        Some("ply".into()),
    );
    let mi = scene.add_mesh(mesh);
    scene.nodes[node].meshes.push(mi);
    Ok(scene)
}

/// sRGB channel -> linear light (PLY colors are sRGB; the renderer is linear).
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Body encoding. Big-endian binary PLY exists in the spec but is vanishingly
/// rare; we reject it with a clear message rather than misparse it.
#[derive(Clone, Copy, PartialEq)]
enum Format {
    Ascii,
    BinaryLe,
}

/// One of PLY's scalar property types.
#[derive(Clone, Copy)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    /// On-disk size in bytes (used to compute record strides).
    fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }
}

/// Accept both the classic PLY spellings (`char`, `uchar`, ...) and the
/// modern aliases (`int8`, `uint8`, ...).
fn scalar_from_name(s: &str) -> Option<Scalar> {
    Some(match s {
        "char" | "int8" => Scalar::I8,
        "uchar" | "uint8" => Scalar::U8,
        "short" | "int16" => Scalar::I16,
        "ushort" | "uint16" => Scalar::U16,
        "int" | "int32" => Scalar::I32,
        "uint" | "uint32" => Scalar::U32,
        "float" | "float32" => Scalar::F32,
        "double" | "float64" => Scalar::F64,
        _ => return None,
    })
}

/// A property is either a plain scalar or a variable-length list
/// (`list <count type> <index type>` — how face indices are stored).
#[derive(Clone)]
enum PropKind {
    Scalar(Scalar),
    List { count: Scalar, index: Scalar },
}

#[derive(Clone)]
struct Prop {
    name: String,
    kind: PropKind,
}

/// A named element with a declared record count and property layout,
/// e.g. `element vertex 8` / `property float x`.
struct Element {
    name: String,
    count: usize,
    props: Vec<Prop>,
}

/// The parsed header: where the body starts, and how to read it.
struct Header {
    format: Format,
    elements: Vec<Element>,
    body: usize, // byte offset of the data after end_header
}

/// Everything the loaders manage to extract from the body.
#[derive(Default)]
struct PlyData {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 3]>, // sRGB 0..1
    indices: Vec<u32>,
}

/// Parse-error constructor so every failure says "PLY".
fn bad(detail: &str) -> LoadError {
    LoadError::Parse {
        format: FMT,
        detail: detail.into(),
    }
}

/// Parse the text header line-by-line until `end_header`.
///
/// The header is line-oriented even when the body is binary, so we scan for
/// raw `\n` bytes (tolerating CRLF) instead of pulling the whole file into
/// a str — a binary body could legitimately contain invalid UTF-8.
/// `end_header`'s byte offset is recorded so the body parsers know exactly
/// where the data begins.
fn parse_header(bytes: &[u8]) -> Result<Header, LoadError> {
    let mut format: Option<Format> = None;
    let mut elements: Vec<Element> = Vec::new();
    let mut saw_magic = false;
    let mut pos = 0usize;
    loop {
        let Some(line_end) = bytes[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| pos + i)
        else {
            return Err(bad("missing end_header"));
        };
        let mut line = &bytes[pos..line_end];
        pos = line_end + 1;
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        let text = std::str::from_utf8(line).map_err(|_| bad("invalid UTF-8 in header"))?;
        let tok: Vec<&str> = text.split_whitespace().collect();
        match tok.first().copied() {
            None => {}
            Some("ply") if !saw_magic && format.is_none() && elements.is_empty() => {
                saw_magic = true;
            }
            Some("format") => {
                if !saw_magic {
                    return Err(bad("missing ply magic"));
                }
                format = Some(match tok.get(1).copied() {
                    Some("ascii") => Format::Ascii,
                    Some("binary_little_endian") => Format::BinaryLe,
                    _ => {
                        return Err(bad(
                            "unsupported format (only ascii and binary_little_endian)",
                        ));
                    }
                });
            }
            Some("element") => {
                if !saw_magic {
                    return Err(bad("missing ply magic"));
                }
                let name = tok
                    .get(1)
                    .copied()
                    .ok_or_else(|| bad("element missing name"))?;
                let count: usize = tok
                    .get(2)
                    .copied()
                    .unwrap_or_default()
                    .parse()
                    .map_err(|_| bad("bad element count"))?;
                elements.push(Element {
                    name: name.into(),
                    count,
                    props: vec![],
                });
            }
            Some("property") => {
                if !saw_magic {
                    return Err(bad("missing ply magic"));
                }
                let elem = elements
                    .last_mut()
                    .ok_or_else(|| bad("property before element"))?;
                if tok.get(1).copied() == Some("list") {
                    let count = scalar_from_name(tok.get(2).copied().unwrap_or_default())
                        .ok_or_else(|| bad("bad list count type"))?;
                    let index = scalar_from_name(tok.get(3).copied().unwrap_or_default())
                        .ok_or_else(|| bad("bad list index type"))?;
                    let name = tok
                        .get(4)
                        .copied()
                        .ok_or_else(|| bad("list property missing name"))?;
                    elem.props.push(Prop {
                        name: name.into(),
                        kind: PropKind::List { count, index },
                    });
                } else {
                    let t = scalar_from_name(tok.get(1).copied().unwrap_or_default())
                        .ok_or_else(|| bad("unsupported property type"))?;
                    let name = tok
                        .get(2)
                        .copied()
                        .ok_or_else(|| bad("property missing name"))?;
                    elem.props.push(Prop {
                        name: name.into(),
                        kind: PropKind::Scalar(t),
                    });
                }
            }
            Some("end_header") => {
                let format = format.ok_or_else(|| bad("missing format line"))?;
                return Ok(Header {
                    format,
                    elements,
                    body: pos,
                });
            }
            _ => {} // comment, obj_info, unknown keywords
        }
    }
}

/// Parse one ASCII token as the declared scalar type. Going through f64
/// keeps integer types exact up to 2^53 while allowing float spellings.
fn num(tok: &str, t: Scalar) -> Result<f64, LoadError> {
    let v = match t {
        Scalar::F32 => tok.parse::<f32>().ok().map(f64::from),
        Scalar::F64 => tok.parse::<f64>().ok(),
        Scalar::I8 | Scalar::I16 | Scalar::I32 => tok.parse::<i64>().ok().map(|v| v as f64),
        Scalar::U8 | Scalar::U16 | Scalar::U32 => tok.parse::<u64>().ok().map(|v| v as f64),
    };
    v.ok_or_else(|| bad("bad numeric value"))
}

/// Triangulate a polygon by fanning from vertex 0: (v0, v1, v2),
/// (v0, v2, v3), ... Works for the convex faces PLY files virtually always
/// contain; degenerate 0/1/2-corner lists fall out naturally (no output).
fn fan(out: &mut Vec<u32>, v: &[u32]) {
    for i in 1..v.len().saturating_sub(1) {
        out.extend_from_slice(&[v[0], v[i], v[i + 1]]);
    }
}

/// Parse an ASCII body: a flat token stream, consumed strictly in the
/// header's declared order. `vertex` records pull out the fields we know
/// by name; `face` records read count + that many indices and triangulate;
/// anything else is skipped with the hang-guard below.
fn parse_ascii(bytes: &[u8], header: &Header) -> Result<PlyData, LoadError> {
    let text = std::str::from_utf8(&bytes[header.body..])
        .map_err(|_| bad("invalid UTF-8 in ASCII body"))?;
    let mut toks = text.split_ascii_whitespace();
    let mut data = PlyData::default();
    for elem in &header.elements {
        match elem.name.as_str() {
            "vertex" => {
                for _ in 0..elem.count {
                    let (mut x, mut y, mut z) = (None, None, None);
                    let (mut nx, mut ny, mut nz) = (None, None, None);
                    let (mut r, mut g, mut b) = (None, None, None);
                    for prop in &elem.props {
                        let tok = toks.next().ok_or_else(|| bad("truncated vertex data"))?;
                        let PropKind::Scalar(t) = prop.kind else {
                            return Err(bad("list property in vertex element"));
                        };
                        match prop.name.as_str() {
                            "x" => x = Some(num(tok, t)?),
                            "y" => y = Some(num(tok, t)?),
                            "z" => z = Some(num(tok, t)?),
                            "nx" => nx = Some(num(tok, t)?),
                            "ny" => ny = Some(num(tok, t)?),
                            "nz" => nz = Some(num(tok, t)?),
                            "red" => r = Some(num(tok, t)? as u8),
                            "green" => g = Some(num(tok, t)? as u8),
                            "blue" => b = Some(num(tok, t)? as u8),
                            _ => {}
                        }
                    }
                    let (Some(x), Some(y), Some(z)) = (x, y, z) else {
                        return Err(bad("vertex missing x/y/z"));
                    };
                    data.positions.push([x as f32, y as f32, z as f32]);
                    if let (Some(nx), Some(ny), Some(nz)) = (nx, ny, nz) {
                        data.normals.push([nx as f32, ny as f32, nz as f32]);
                    }
                    if let (Some(r), Some(g), Some(b)) = (r, g, b) {
                        data.colors.push([
                            f32::from(r) / 255.0,
                            f32::from(g) / 255.0,
                            f32::from(b) / 255.0,
                        ]);
                    }
                }
            }
            "face" => {
                let list = elem
                    .props
                    .iter()
                    .find(|p| matches!(p.kind, PropKind::List { .. }))
                    .ok_or_else(|| bad("face element missing list property"))?;
                let PropKind::List { count, index } = list.kind else {
                    return Err(bad("face element missing list property"));
                };
                for _ in 0..elem.count {
                    let ctok = toks.next().ok_or_else(|| bad("truncated face data"))?;
                    let n = num(ctok, count)? as usize;
                    let mut verts = Vec::new();
                    for _ in 0..n {
                        let itok = toks.next().ok_or_else(|| bad("truncated face data"))?;
                        let i = num(itok, index)? as i64;
                        if i < 0 || i as usize >= data.positions.len() {
                            return Err(bad("face index out of range"));
                        }
                        verts.push(i as u32);
                    }
                    fan(&mut data.indices, &verts);
                }
            }
            _ => {
                // unknown element: skip count records in property order.
                // Ruling (Task 16 review): a zero-property element consumes
                // nothing per record, so a huge declared count used to spin
                // here ~forever. Every record of a real element consumes at
                // least one token (≥ 1 byte), so a count beyond the file
                // size can never fit — error instead of looping.
                if elem.count > bytes.len() {
                    return Err(bad("element count exceeds the file size"));
                }
                for _ in 0..elem.count {
                    for prop in &elem.props {
                        match prop.kind {
                            PropKind::Scalar(_) => {
                                toks.next().ok_or_else(|| bad("truncated data"))?;
                            }
                            PropKind::List { count, .. } => {
                                let tok = toks.next().ok_or_else(|| bad("truncated data"))?;
                                let n = num(tok, count)? as usize;
                                for _ in 0..n {
                                    toks.next().ok_or_else(|| bad("truncated data"))?;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(data)
}

/// Read one little-endian scalar at `off` as f64, bounds-checked.
fn read_num(bytes: &[u8], off: usize, t: Scalar) -> Result<f64, LoadError> {
    let n = t.size();
    if off.saturating_add(n) > bytes.len() {
        return Err(bad("truncated binary data"));
    }
    let b = &bytes[off..off + n];
    Ok(match t {
        Scalar::I8 => f64::from(i8::from_le_bytes([b[0]])),
        Scalar::U8 => f64::from(b[0]),
        Scalar::I16 => f64::from(i16::from_le_bytes([b[0], b[1]])),
        Scalar::U16 => f64::from(u16::from_le_bytes([b[0], b[1]])),
        Scalar::I32 => f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        Scalar::U32 => f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        Scalar::F32 => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        Scalar::F64 => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
    })
}

/// Parse a binary body, mirroring `parse_ascii` structure for structure.
/// Vertex records are a fixed stride (sum of scalar sizes), so the cursor
/// advances by the stride even over properties we do not consume; face
/// records are count-prefixed and walked one index at a time.
fn parse_binary(bytes: &[u8], header: &Header) -> Result<PlyData, LoadError> {
    let mut off = header.body;
    let mut data = PlyData::default();
    for elem in &header.elements {
        match elem.name.as_str() {
            "vertex" => {
                let mut stride = 0usize;
                for prop in &elem.props {
                    match prop.kind {
                        PropKind::Scalar(t) => stride += t.size(),
                        PropKind::List { .. } => {
                            return Err(bad("list property in vertex element"));
                        }
                    }
                }
                for _ in 0..elem.count {
                    let mut vo = off;
                    let (mut x, mut y, mut z) = (None, None, None);
                    let (mut nx, mut ny, mut nz) = (None, None, None);
                    let (mut r, mut g, mut b) = (None, None, None);
                    for prop in &elem.props {
                        let PropKind::Scalar(t) = prop.kind else {
                            return Err(bad("list property in vertex element"));
                        };
                        match prop.name.as_str() {
                            "x" => x = Some(read_num(bytes, vo, t)?),
                            "y" => y = Some(read_num(bytes, vo, t)?),
                            "z" => z = Some(read_num(bytes, vo, t)?),
                            "nx" => nx = Some(read_num(bytes, vo, t)?),
                            "ny" => ny = Some(read_num(bytes, vo, t)?),
                            "nz" => nz = Some(read_num(bytes, vo, t)?),
                            "red" => r = Some(read_num(bytes, vo, t)? as u8),
                            "green" => g = Some(read_num(bytes, vo, t)? as u8),
                            "blue" => b = Some(read_num(bytes, vo, t)? as u8),
                            _ => {}
                        }
                        vo += t.size();
                    }
                    let (Some(x), Some(y), Some(z)) = (x, y, z) else {
                        return Err(bad("vertex missing x/y/z"));
                    };
                    data.positions.push([x as f32, y as f32, z as f32]);
                    if let (Some(nx), Some(ny), Some(nz)) = (nx, ny, nz) {
                        data.normals.push([nx as f32, ny as f32, nz as f32]);
                    }
                    if let (Some(r), Some(g), Some(b)) = (r, g, b) {
                        data.colors.push([
                            f32::from(r) / 255.0,
                            f32::from(g) / 255.0,
                            f32::from(b) / 255.0,
                        ]);
                    }
                    off += stride;
                }
            }
            "face" => {
                let list = elem
                    .props
                    .iter()
                    .find(|p| matches!(p.kind, PropKind::List { .. }))
                    .ok_or_else(|| bad("face element missing list property"))?;
                let PropKind::List { count, index } = list.kind else {
                    return Err(bad("face element missing list property"));
                };
                for _ in 0..elem.count {
                    let n = read_num(bytes, off, count)? as usize;
                    off += count.size();
                    let mut verts = Vec::new();
                    for _ in 0..n {
                        let i = read_num(bytes, off, index)? as i64;
                        off += index.size();
                        if i < 0 || i as usize >= data.positions.len() {
                            return Err(bad("face index out of range"));
                        }
                        verts.push(i as u32);
                    }
                    fan(&mut data.indices, &verts);
                }
            }
            _ => {
                // unknown element: skip count records in property order.
                // Ruling (Task 16 review): a zero-property element never
                // advances `off`, so a huge declared count used to spin
                // here ~forever. Every record of a real element consumes at
                // least one byte, so a count beyond the file size can never
                // fit — error instead of looping.
                if elem.count > bytes.len() {
                    return Err(bad("element count exceeds the file size"));
                }
                for _ in 0..elem.count {
                    for prop in &elem.props {
                        match prop.kind {
                            PropKind::Scalar(t) => off = off.saturating_add(t.size()),
                            PropKind::List { count, index } => {
                                let n = read_num(bytes, off, count)? as usize;
                                // garbage counts can be ~usize::MAX; saturate
                                // instead of overflowing the size sum
                                off = off
                                    .saturating_add(count.size())
                                    .saturating_add(n.saturating_mul(index.size()));
                            }
                        }
                    }
                    if off > bytes.len() {
                        return Err(bad("truncated binary data"));
                    }
                }
            }
        }
    }
    Ok(data)
}
