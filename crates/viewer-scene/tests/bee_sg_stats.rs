// Diagnostic: decode Bee.glb's spec-gloss texture (the second embedded
// PNG) directly and print specular rgb + glossiness alpha statistics,
// plus the diffuse texture stats — establishing what the SG bake inputs
// really contain.
#[test]
#[ignore = "texture diagnostic; --ignored --nocapture"]
fn bee_sg_texture_stats() {
    let path = r"C:\Users\Steven\Desktop\Bee.glb";
    let bytes = std::fs::read(path).expect("read glb");
    eprintln!("file size: {}", bytes.len());
    // Sanity: the first PNG signature PowerShell found.
    eprintln!(
        "byte at 6239792: {:02x} {:02x} {:02x} {:02x}",
        bytes[6239792], bytes[6239793], bytes[6239794], bytes[6239795]
    );

    // Find embedded PNGs by signature + IEND chunk.
    let sig = [0x89u8, 0x50, 0x4E, 0x47];
    let mut starts: Vec<usize> = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == sig)
        .map(|(i, _)| i)
        .collect();
    eprintln!("signature hits: {}", starts.len());
    let mut pngs: Vec<(usize, usize)> = Vec::new(); // (start, end-exclusive)
    while let Some(i) = starts.pop() {
        let mut off = i + 8;
        let mut end = None;
        while off + 8 <= bytes.len() {
            // PNG chunk lengths are BIG-endian.
            let len = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
            let ctype = [
                bytes[off + 4],
                bytes[off + 5],
                bytes[off + 6],
                bytes[off + 7],
            ];
            off += 8 + len + 4; // len + type + data + crc
            if ctype == *b"IEND" {
                end = Some(off);
                break;
            }
        }
        if let Some(end) = end {
            pngs.push((i, end));
        }
    }
    pngs.sort();
    eprintln!("embedded PNGs found: {}", pngs.len());
    let sg = image::load_from_memory(&bytes[pngs[1].0..pngs[1].1])
        .expect("sg decodes")
        .to_rgba8();
    let diff = image::load_from_memory(&bytes[pngs[0].0..pngs[0].1])
        .expect("diff decodes")
        .to_rgba8();
    eprintln!(
        "SG texture {}x{}, diffuse {}x{}",
        sg.width(),
        sg.height(),
        diff.width(),
        diff.height()
    );

    // SG: rgb = specular (sRGB), a = glossiness (linear).
    let (mut sr, mut sgn, mut sb, mut sa) = (0f32, 0f32, 0f32, 0f32);
    let (mut dr, mut dg, mut db) = (0f32, 0f32, 0f32);
    let (mut smin, mut smax) = (1f32, 0f32);
    let mut n = 0usize;
    for (_, _, px) in sg.enumerate_pixels() {
        sr += px[0] as f32 / 255.0;
        sgn += px[1] as f32 / 255.0;
        sb += px[2] as f32 / 255.0;
        sa += px[3] as f32 / 255.0;
        let lum = (px[0] as f32 + px[1] as f32 + px[2] as f32) / 765.0;
        smin = smin.min(lum);
        smax = smax.max(lum);
        n += 1;
    }
    eprintln!(
        "SG mean: specular rgb=({:.3},{:.3},{:.3}) sRGB, glossiness alpha={:.3}; lum {smin:.3}..{smax:.3}",
        sr / n as f32,
        sgn / n as f32,
        sb / n as f32,
        sa / n as f32
    );
    for (_, _, px) in diff.enumerate_pixels() {
        dr += px[0] as f32 / 255.0;
        dg += px[1] as f32 / 255.0;
        db += px[2] as f32 / 255.0;
    }
    eprintln!(
        "diffuse mean rgb=({:.3},{:.3},{:.3}) sRGB",
        dr / n as f32,
        dg / n as f32,
        db / n as f32
    );
}
