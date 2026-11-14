//! A small, fully deterministic PNG encoder: 8-bit greyscale, RGB or RGBA,
//! no interlace, every row filtered with the same fixed filter (Sub), zlib
//! level 6 through `flate2` (zlib-rs backend), one IDAT chunk, no ancillary
//! chunks (no time stamp, no gamma, no text). The same pixels always give the
//! same bytes.

use std::io::Write as _;

use flate2::Compression;
use flate2::write::ZlibEncoder;

/// zlib compression level used for every image.
pub const ZLIB_LEVEL: u32 = 6;

/// Pixel layout of an [`Image`](super::Image).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorType {
    /// One byte per pixel (luminance).
    Gray,
    /// Three bytes per pixel.
    Rgb,
    /// Four bytes per pixel (straight alpha).
    Rgba,
}

impl ColorType {
    /// Bytes per pixel.
    pub fn channels(self) -> usize {
        match self {
            ColorType::Gray => 1,
            ColorType::Rgb => 3,
            ColorType::Rgba => 4,
        }
    }

    fn png_code(self) -> u8 {
        match self {
            ColorType::Gray => 0,
            ColorType::Rgb => 2,
            ColorType::Rgba => 6,
        }
    }
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

/// Encodes `data` (row-major, `color.channels()` bytes per pixel) as a PNG.
///
/// Returns `None` when a dimension is zero or above 2^31 − 1 (the PNG limit)
/// or `data` has the wrong length.
pub fn encode_png(width: u32, height: u32, color: ColorType, data: &[u8]) -> Option<Vec<u8>> {
    const MAX: u32 = 0x7fff_ffff;
    if width == 0 || height == 0 || width > MAX || height > MAX {
        return None;
    }
    let bpp = color.channels();
    let stride = (width as usize).checked_mul(bpp)?;
    if data.len() != stride.checked_mul(height as usize)? {
        return None;
    }
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, color.png_code(), 0, 0, 0]);

    let mut z = ZlibEncoder::new(Vec::new(), Compression::new(ZLIB_LEVEL));
    let mut line = vec![0u8; stride + 1];
    for row in data.chunks_exact(stride) {
        line[0] = 1; // Sub
        for i in 0..stride {
            let left = if i >= bpp { row[i - bpp] } else { 0 };
            line[i + 1] = row[i].wrapping_sub(left);
        }
        // Writing to a Vec cannot fail.
        z.write_all(&line).ok()?;
    }
    let idat = z.finish().ok()?;

    let mut out = Vec::with_capacity(idat.len() + 64);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_shapes() {
        assert!(encode_png(0, 1, ColorType::Gray, &[]).is_none());
        assert!(encode_png(1, 0, ColorType::Gray, &[]).is_none());
        assert!(encode_png(2, 2, ColorType::Rgb, &[0; 11]).is_none());
        assert!(encode_png(u32::MAX, 1, ColorType::Gray, &[]).is_none());
    }

    #[test]
    fn round_trips_through_image() {
        for color in [ColorType::Gray, ColorType::Rgb, ColorType::Rgba] {
            let (w, h) = (7u32, 5u32);
            let n = (w * h) as usize * color.channels();
            let data: Vec<u8> = (0..n).map(|i| (i * 37 % 251) as u8).collect();
            let png = encode_png(w, h, color, &data).unwrap();
            let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
            assert_eq!((img.width(), img.height()), (w, h));
            let bytes = match color {
                ColorType::Gray => img.to_luma8().into_raw(),
                ColorType::Rgb => img.to_rgb8().into_raw(),
                ColorType::Rgba => img.to_rgba8().into_raw(),
            };
            assert_eq!(bytes, data);
            assert_eq!(png, encode_png(w, h, color, &data).unwrap());
        }
    }
}
