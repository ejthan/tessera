// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask icons`: the placeholder app icon, drawn from code.

use std::path::Path;

use anyhow::Context as _;

const COLORS: [[u8; 3]; 5] = [
    [0x0f, 0x76, 0x6e],
    [0x25, 0x63, 0xeb],
    [0xf5, 0x9e, 0x0b],
    [0xdc, 0x26, 0x26],
    [0x33, 0x41, 0x55],
];
/// Which color each tile has, by row.
const TILES: [[usize; 4]; 4] = [[1, 0, 2, 4], [0, 4, 1, 3], [2, 1, 3, 0], [4, 3, 0, 1]];
// Geometry on a 1024-unit canvas: 4 tiles and 3 gaps leave the same margin on each side.
const MARGIN: u32 = 68;
const TILE: u32 = 204;
const PITCH: u32 = 228;
const RADIUS: u32 = 40;
/// Samples per pixel along each axis.
const SAMPLES: u32 = 4;

/// The tile at the canvas position (`u`, `v`), if a tile covers it.
fn color_at(u: u32, v: u32) -> Option<[u8; 3]> {
    let (u, v) = (u.checked_sub(MARGIN)?, v.checked_sub(MARGIN)?);
    let (col, row) = ((u / PITCH) as usize, (v / PITCH) as usize);
    let (x, y) = (u % PITCH, v % PITCH);
    if col > 3 || row > 3 || x >= TILE || y >= TILE {
        return None;
    }
    // How far past the straight edges a corner sample is.
    let past = |at: u32| RADIUS.saturating_sub(at.min(TILE - 1 - at));
    let in_corner = past(x).pow(2) + past(y).pow(2) <= RADIUS.pow(2);
    in_corner.then_some(COLORS[TILES[row][col]])
}

/// RGBA pixels of a 4×4 mosaic of rounded squares; the art is a placeholder (D109).
pub fn mosaic(size: u32) -> Vec<u8> {
    let grid = size * SAMPLES;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut sum, mut hits) = ([0u32; 3], 0);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let at = |pixel: u32, sample: u32| {
                        (2 * (pixel * SAMPLES + sample) + 1) * 1024 / (2 * grid)
                    };
                    if let Some(color) = color_at(at(x, sx), at(y, sy)) {
                        hits += 1;
                        for (total, channel) in sum.iter_mut().zip(color) {
                            *total += u32::from(channel);
                        }
                    }
                }
            }
            let [r, g, b] = sum.map(|total| (total / hits.max(1)) as u8);
            pixels.extend([r, g, b, (hits * 255 / (SAMPLES * SAMPLES)) as u8]);
        }
    }
    pixels
}

/// An `.icns`: the magic, the total length, then one `type`, length, PNG per entry.
pub fn icns(pngs: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let length = 8 + pngs.iter().map(|(_, png)| 8 + png.len()).sum::<usize>();
    let mut file = Vec::with_capacity(length);
    file.extend(b"icns");
    file.extend((length as u32).to_be_bytes());
    for (kind, png) in pngs {
        file.extend(*kind);
        file.extend((8 + png.len() as u32).to_be_bytes());
        file.extend(png);
    }
    file
}

/// An `.ico`: a 6-byte header, a 16-byte entry per size, then the PNGs.
pub fn ico(pngs: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut file = vec![0, 0, 1, 0];
    file.extend((pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len();
    for (size, png) in pngs {
        // The byte holds 1 to 255, and 0 means 256.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        file.extend([side, side, 0, 0, 1, 0, 32, 0]);
        file.extend((png.len() as u32).to_le_bytes());
        file.extend((offset as u32).to_le_bytes());
        offset += png.len();
    }
    for (_, png) in pngs {
        file.extend(png);
    }
    file
}

fn png(size: u32) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut encoder = ::png::Encoder::new(&mut bytes, size, size);
    encoder.set_color(::png::ColorType::Rgba);
    encoder.set_depth(::png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&mosaic(size))?;
    writer.finish()?;
    Ok(bytes)
}

/// Writes the PNGs and both containers to `assets/icons/app` under `root`.
pub fn run(root: &Path) -> anyhow::Result<()> {
    let dir = root.join("assets/icons/app");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.join(name);
        std::fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))
    };
    let [p32, p128, p256, p512, p1024] = [32, 128, 256, 512, 1024].map(png);
    let (p32, p128, p256, p512, p1024) = (p32?, p128?, p256?, p512?, p1024?);
    write("32.png", &p32)?;
    write("128.png", &p128)?;
    write("256.png", &p256)?;
    write("512.png", &p512)?;
    write("1024.png", &p1024)?;
    write(
        "tessera.icns",
        &icns(&[
            (b"ic07", p128),
            (b"ic08", p256.clone()),
            (b"ic09", p512),
            (b"ic10", p1024),
        ]),
    )?;
    write("tessera.ico", &ico(&[(32, p32), (256, p256)]))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{icns, ico, mosaic, png};

    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

    fn fnv(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    #[test]
    fn a_png_decodes_back_to_its_size() {
        let reader = ::png::Decoder::new(Cursor::new(png(32).expect("png")))
            .read_info()
            .expect("decode");
        assert_eq!((reader.info().width, reader.info().height), (32, 32));
    }

    #[test]
    fn icns_has_the_magic_and_its_own_length() {
        let file = icns(&[(b"ic07", vec![1, 2, 3]), (b"ic08", vec![4])]);
        assert_eq!(&file[..4], b"icns");
        let length = u32::from_be_bytes(file[4..8].try_into().expect("four bytes"));
        assert_eq!(length as usize, file.len());
        assert_eq!(&file[8..12], b"ic07");
        assert_eq!(file[12..16], 11u32.to_be_bytes());
        assert_eq!(&file[19..23], b"ic08");
    }

    #[test]
    fn ico_entries_point_at_png_signatures() {
        let body = || [SIGNATURE, b"rest"].concat();
        let file = ico(&[(32, body()), (256, body())]);
        assert_eq!(file[..6], [0, 0, 1, 0, 2, 0]);
        assert_eq!(file[6], 32);
        assert_eq!(file[22], 0, "256 is written as 0");
        for entry in [6, 22] {
            let at = |i: usize| u32::from_le_bytes(file[i..i + 4].try_into().expect("four bytes"));
            let (size, offset) = (at(entry + 8) as usize, at(entry + 12) as usize);
            assert_eq!(size, body().len());
            assert_eq!(&file[offset..offset + 8], SIGNATURE);
        }
    }

    #[test]
    fn the_mosaic_is_stable() {
        assert_eq!(mosaic(32).len(), 32 * 32 * 4);
        assert_eq!(fnv(&mosaic(32)), 4553148994320112010);
        assert_eq!(fnv(&mosaic(64)), 16756294113680683381);
    }

    #[test]
    fn the_mosaic_leaves_its_margin_clear_and_fills_a_tile() {
        let pixels = mosaic(64);
        let alpha = |x: usize, y: usize| pixels[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(10, 10), 255);
    }
}
