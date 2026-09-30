//! Minimal PNG writer for screenshots (8-bit RGB).

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, tag: &[u8], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut c = tag.to_vec();
    c.extend_from_slice(data);
    out.extend_from_slice(&c);
    out.extend_from_slice(&crc32(&c).to_be_bytes());
}

/// Encode RGB triples (width x height) as PNG, enlarged by `scale`.
pub fn encode(width: usize, height: usize, rgb: &[u8], scale: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity((width * scale * 3 + 1) * height * scale);
    for y in 0..height {
        let line = &rgb[y * width * 3..(y + 1) * width * 3];
        let mut wide = Vec::with_capacity(width * scale * 3 + 1);
        wide.push(0);
        for px in line.chunks_exact(3) {
            for _ in 0..scale {
                wide.extend_from_slice(px);
            }
        }
        for _ in 0..scale {
            raw.extend_from_slice(&wide);
        }
    }
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&((width * scale) as u32).to_be_bytes());
    ihdr.extend_from_slice(&((height * scale) as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn write(path: &str, width: usize, height: usize, rgb: &[u8], scale: usize) -> std::io::Result<()> {
    std::fs::write(path, encode(width, height, rgb, scale))
}
