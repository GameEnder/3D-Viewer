//! UserChoice hash computation, reverse-engineered from SetUserFTA v1.8.3
//! (Kolbicz IT). Windows 8+ protects per-user file-type defaults
//! (`FileExts\.ext\UserChoice`) with a hash over the assignment; Windows 11
//! build 24H2+ (feature `AppDefaultHashRotation`, variant `UserChoiceLatest`,
//! stage >= 0x648) computes it as follows:
//!
//! 1. Input string: `ext + SID + ProgID + timestr + magic`
//!    - `timestr` = "%08x%08x" of the UserChoice key's last-write FILETIME
//!      with seconds and milliseconds zeroed (minute granularity, so the
//!      hash survives the value writes that follow).
//!    - `magic` = "user choice set via windows user experience
//!      {d18b6dd5-6124-4341-9318-804003bafa0b}"
//! 2. The input is UTF-16-encoded, lowercased, NUL-terminated.
//! 3. `state = MD5(utf16 bytes)` — standard MD5 including the terminator.
//! 4. Two custom arithmetic passes run over the first `2*n` dwords
//!    (`n` = dword count rounded down to even), each seeded from
//!    `state[0..2]` (pass 1 adds fixed constants).
//! 5. The 8-byte result is the pairwise XOR of the passes' outputs,
//!    standard-base64-encoded (12 chars).
//!
//! Registry write order (matters): delete the UserChoice key if present,
//! recreate it (fresh key = writable DACL), read its last-write, compute
//! the hash, then write `Hash` FIRST and `ProgId` SECOND.

/// The Windows-experience magic string (v1.8.3, stage >= 0x648).
pub const MAGIC: &str =
    "user choice set via windows user experience {d18b6dd5-6124-4341-9318-804003bafa0b}";

/// Round a FILETIME down to a whole minute (the timestamp element:
/// zeroing wSeconds and wMilliseconds, as SetUserFTA does).
pub fn filetime_to_minute(ft: u64) -> u64 {
    const MINUTE: u64 = 60 * 10_000_000;
    ft - ft % MINUTE
}

/// Standard MD5 (used for the initial state over the UTF-16 bytes).
fn md5(data: &[u8]) -> [u32; 4] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xefcd_ab89;
    let mut c0: u32 = 0x98ba_dcfe;
    let mut d0: u32 = 0x1032_5476;

    // Padding: 0x80, zeros, 8-byte little-endian bit length.
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in msg.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = tmp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    [a0, b0, c0, d0]
}

/// Pass 1 over the wide string's dwords (transliteration of SetUserFTA
/// 004024d0). Returns (chain, acc). `n` = dword count rounded down to even.
fn pass1(dwords: &[u32], n: usize, state: &[u32; 4]) -> (u32, u32) {
    let m1 = (state[0] | 1).wrapping_add(0x69fb_0000);
    let m2 = (state[1] | 1).wrapping_add(0x13db_0000);
    let mut acc: u32 = 0;
    let mut h: u32 = 0;
    for i in 0..(n / 2) {
        let x0 = dwords[i * 2];
        let x1 = dwords[i * 2 + 1];
        // Step A: fold in x0.
        let a = h.wrapping_add(x0);
        let u = a
            .wrapping_mul(m1)
            .wrapping_add((a >> 16).wrapping_mul(0xef05_69fb));
        let u = u
            .wrapping_mul(0x79f8_a395)
            .wrapping_add((u >> 16).wrapping_mul(0x689b_6b9f));
        let v = u
            .wrapping_mul(0xea97_0001)
            .wrapping_add((u >> 16).wrapping_mul(0xc3ef_ea97));
        // Step B: fold in x1.
        let b = v.wrapping_add(x1);
        let w = b
            .wrapping_mul(m2)
            .wrapping_add((b >> 16).wrapping_mul(0xc317_13db));
        let w = w
            .wrapping_mul(0x59c3_af2d)
            .wrapping_add((w >> 16).wrapping_mul(0xddcd_1f0f));
        h = w
            .wrapping_mul(0x1ec9_0001)
            .wrapping_add((w >> 16).wrapping_mul(0x35bd_1ec9));
        acc = acc.wrapping_add(v).wrapping_add(h);
    }
    (h, acc)
}

/// Pass 2 (transliteration of SetUserFTA 00402650). Same loop shape,
/// different constants, seeded from state[0..2] directly.
fn pass2(dwords: &[u32], n: usize, state: &[u32; 4]) -> (u32, u32) {
    let u7 = state[0] | 1;
    let u8 = state[1] | 1;
    let mut acc: u32 = 0;
    let mut h: u32 = 0;
    for i in 0..(n / 2) {
        let x0 = dwords[i * 2];
        let x1 = dwords[i * 2 + 1];
        // Step A.
        let a = h.wrapping_add(x0);
        let t1 = a.wrapping_mul(u7) >> 16;
        let t2 = (u7
            .wrapping_mul(0xb111_0000)
            .wrapping_mul(a)
            .wrapping_add(t1.wrapping_mul(0xcf98_b111)))
            >> 16;
        let t3 = (t1
            .wrapping_mul(0x048f_0000)
            .wrapping_add(t2.wrapping_mul(0x8708_5b9f)))
            >> 16;
        let y = ((t3
            .wrapping_mul(0x12ce_b96d)
            .wrapping_add(t2.wrapping_mul(0xe9b3_0000)))
            >> 16)
            .wrapping_mul(0x257e_1d83)
            .wrapping_add(t3.wrapping_mul(0x3bc7_0000));
        // Step B.
        let b = x1.wrapping_add(y);
        let t4 = b.wrapping_mul(u8) >> 16;
        let t5 = (u8
            .wrapping_mul(0x16f5_0000)
            .wrapping_mul(b)
            .wrapping_add(t4.wrapping_mul(0xa274_16f5)))
            >> 16;
        let t6 = (t4
            .wrapping_mul(0x6c0b_0000)
            .wrapping_add(t5.wrapping_mul(0xd383_96ff)))
            >> 16;
        h = t6.wrapping_mul(0xf231_0000).wrapping_add(
            (t6.wrapping_mul(0x7c93_2b89)
                .wrapping_add(t5.wrapping_mul(0xa377_0000))
                >> 16)
                .wrapping_mul(0xbfa4_9f69),
        );
        acc = acc.wrapping_add(y).wrapping_add(h);
    }
    (h, acc)
}

/// Standard base64 (with padding) of 12 output bytes max.
fn base64(data: &[u8]) -> String {
    const TBL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TBL[(n >> 18) as usize & 0x3f] as char);
        out.push(TBL[(n >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 {
            TBL[(n >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TBL[n as usize & 0x3f] as char
        } else {
            '='
        });
    }
    out
}

/// Compute the UserChoice hash for the given inputs (stage >= 0x648
/// construction: ext + sid + progid + timestr + magic).
pub fn user_choice_hash(ext: &str, sid: &str, progid: &str, ft_minute: u64) -> String {
    let timestr = format!("{:08x}{:08x}", ft_minute >> 32, ft_minute as u32);
    let input = format!("{ext}{sid}{progid}{timestr}{MAGIC}");

    // UTF-16LE, lowercased, NUL-terminated.
    let wide: Vec<u16> = input
        .to_lowercase()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut bytes = Vec::with_capacity(wide.len() * 2);
    for w in &wide {
        bytes.extend_from_slice(&w.to_le_bytes());
    }

    let state = md5(&bytes);
    let ndw = bytes.len() >> 2;
    let n = ndw - (ndw & 1);
    let dwords: Vec<u32> = bytes[..n * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();

    let (h1a, h1b) = pass1(&dwords, n, &state);
    let (h2a, h2b) = pass2(&dwords, n, &state);
    let r0 = h2a ^ h1a;
    let r1 = h2b ^ h1b;
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(&r0.to_le_bytes());
    out.extend_from_slice(&r1.to_le_bytes());
    base64(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "S-1-5-21-3708609648-1742982103-4094751306-1001";

    // Vectors captured from SetUserFTA v1.8.3 on this machine (Windows 11
    // build 26200): (ext, progid, key last-write FILETIME, resulting Hash).
    #[test]
    fn reproduces_setuserfta_vectors() {
        let vectors: &[(&str, &str, u64, &str)] = &[
            (
                ".zzzbb1",
                "3DViewer.Rust",
                134366643983915137,
                "6mFS/7Yrku4=",
            ),
            (
                ".zzzbb2",
                "Fake.Prog.Test",
                134366645823398562,
                "ZMwSpNgWcmg=",
            ),
            (
                ".zzzbb1",
                "3DViewer.Rust",
                134366645828909066,
                "WTpNAn20dSs=",
            ),
            (".glb", "3DViewer.Rust", 134366659162539079, "CPrLI/XM7hI="),
        ];
        for (ext, progid, ft, expected) in vectors {
            let got = user_choice_hash(ext, SID, progid, filetime_to_minute(*ft));
            assert_eq!(got, *expected, "mismatch for {ext}/{progid} @ ft={ft}");
        }
    }

    /// Field verification: writes real Windows defaults for all supported
    /// extensions. Run manually with
    /// `cargo test -p viewer-app field -- --ignored`
    /// (mutates the current user's registry — that is its purpose).
    #[cfg(windows)]
    #[test]
    #[ignore = "writes real Windows UserChoice defaults for all 8 extensions"]
    fn field_set_defaults_all() {
        let exts = [
            "glb", "gltf", "obj", "stl", "ply", "3mf", "fbx", "nif", "ma",
        ];
        for ext in exts {
            set_default(ext, "3DViewer.Rust").unwrap_or_else(|e| panic!(".{ext}: {e}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Registry side: make `prog_id` the Windows-validated default for `.{ext}`.
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub use imp::set_default;

#[cfg(windows)]
mod imp {
    use super::{filetime_to_minute, user_choice_hash};
    use winreg::RegKey;
    use winreg::enums::*;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn LocalFree(handle: isize) -> isize;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: isize, desired_access: u32, token: *mut isize) -> i32;
        fn GetTokenInformation(
            token: isize,
            class: u32,
            info: *mut u8,
            info_len: u32,
            return_len: *mut u32,
        ) -> i32;
        fn ConvertSidToStringSidW(sid: *const u8, string_sid: *mut *mut u16) -> i32;
    }

    /// Current user's SID string (e.g. "S-1-5-21-...").
    fn current_sid() -> Result<String, String> {
        const TOKEN_QUERY: u32 = 8;
        const TOKEN_USER: u32 = 1;
        let mut token = 0isize;
        // SAFETY: standard token query on our own process.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err("could not open process token".into());
        }
        let mut buf = [0u8; 256]; // TOKEN_USER + SID always fits
        let mut needed = 0u32;
        // SAFETY: buffer size passed explicitly; API is name-free of shared state.
        if unsafe {
            GetTokenInformation(
                token,
                TOKEN_USER,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut needed,
            )
        } == 0
        {
            return Err(format!("could not query token (needs {needed} bytes)"));
        }
        // TOKEN_USER { User: SID_AND_ATTRIBUTES { Sid: PSID, ... } } — Sid is first.
        let sid = unsafe { (buf.as_ptr() as *const *const u8).read_unaligned() };
        let mut sid_w: *mut u16 = std::ptr::null_mut();
        // SAFETY: sid points into our buffer; sid_w is allocator-owned, freed below.
        if unsafe { ConvertSidToStringSidW(sid, &mut sid_w) } == 0 {
            return Err("could not convert SID to string".into());
        }
        let s = unsafe {
            let len = (0usize..).take_while(|&i| *sid_w.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(sid_w, len))
        };
        unsafe { LocalFree(sid_w as isize) };
        Ok(s)
    }

    /// Make `prog_id` the default handler for `.{ext}` by writing a
    /// Windows-validated UserChoice (ProgId + hash).
    ///
    /// Sequence (as reverse-engineered from SetUserFTA): delete any existing
    /// UserChoice key, recreate it (a fresh key carries a writable DACL),
    /// read its last-write time, compute the hash over that minute, then
    /// write `Hash` FIRST and `ProgId` SECOND — the final last-write falls
    /// in the hashed minute, so Windows' validation accepts the pair.
    pub fn set_default(ext: &str, prog_id: &str) -> Result<(), String> {
        let sid = current_sid()?;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let path = format!(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.{ext}\UserChoice"
        );
        let dotted = format!(".{ext}");

        // Delete any existing UserChoice (fresh start). "Not found" is fine.
        if let Err(e) = hkcu.delete_subkey(&path) {
            if e.kind() == std::io::ErrorKind::NotFound {
                // absent — nothing to delete
            } else {
                return Err(format!("delete UserChoice for {dotted}: {e}"));
            }
        }

        // Create the fresh key (writable), then close like the reference tool.
        let (key, _) = hkcu
            .create_subkey(&path)
            .map_err(|e| format!("create UserChoice for {dotted}: {e}"))?;
        drop(key);

        // Hash + write. Retry covers a minute-boundary crossing between the
        // key's creation and the value writes (Windows validates against the
        // post-write last-write, truncated to the minute).
        for _ in 0..3 {
            let key = hkcu
                .open_subkey_with_flags(&path, KEY_READ | KEY_SET_VALUE)
                .map_err(|e| format!("reopen UserChoice for {dotted}: {e}"))?;
            let meta = key
                .query_info()
                .map_err(|e| format!("read last-write for {dotted}: {e}"))?;
            let ft = (u64::from(meta.last_write_time.dwHighDateTime) << 32)
                | u64::from(meta.last_write_time.dwLowDateTime);
            let minute = filetime_to_minute(ft);
            let hash = user_choice_hash(&dotted, &sid, prog_id, minute);
            key.set_value("Hash", &hash)
                .map_err(|e| format!("write Hash for {dotted}: {e}"))?;
            key.set_value("ProgId", &prog_id)
                .map_err(|e| format!("write ProgId for {dotted}: {e}"))?;

            // Confirm the writes stayed within the hashed minute.
            let meta = key
                .query_info()
                .map_err(|e| format!("re-read last-write for {dotted}: {e}"))?;
            let ft = (u64::from(meta.last_write_time.dwHighDateTime) << 32)
                | u64::from(meta.last_write_time.dwLowDateTime);
            if filetime_to_minute(ft) == minute {
                return Ok(());
            }
        }
        Err(format!(
            "timestamp kept moving for {dotted} (minute boundary) — try again"
        ))
    }
}

#[cfg(not(windows))]
pub fn set_default(_ext: &str, _prog_id: &str) -> Result<(), String> {
    Err("file associations are only supported on Windows".into())
}
