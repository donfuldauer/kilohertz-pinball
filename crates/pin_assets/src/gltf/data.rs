//! Bounds-checked buffer, accessor and URI handling for glTF.

use super::gerr;
use crate::zip::normalize_path;
use crate::{AssetError, Vfs};
use ::gltf::Document;
use ::gltf::accessor::{Accessor, DataType, Dimensions};
use ::gltf::buffer::{Source, View};

/// Largest element count of an accessor without a buffer view (zero-filled).
const MAX_IMPLICIT_COUNT: usize = 1 << 24;

/// Every buffer's bytes, at least as long as the declared `byteLength`.
pub(super) struct Buffers {
    data: Vec<Vec<u8>>,
}

impl Buffers {
    pub(super) fn load(
        document: &Document,
        mut bin: Option<Vec<u8>>,
        vfs: &Vfs,
        base: &str,
    ) -> Result<Buffers, AssetError> {
        let mut data = Vec::with_capacity(document.buffers().len());
        for buffer in document.buffers() {
            let bytes = match buffer.source() {
                Source::Bin => bin
                    .take()
                    .ok_or_else(|| gerr(format!("buffer {} refers to a missing GLB BIN chunk", buffer.index())))?,
                Source::Uri(uri) => resolve_uri(uri, vfs, base)?,
            };
            if bytes.len() < buffer.length() {
                return Err(gerr(format!(
                    "buffer {} has {} bytes but declares {}",
                    buffer.index(),
                    bytes.len(),
                    buffer.length()
                )));
            }
            data.push(bytes);
        }
        Ok(Buffers { data })
    }

    /// The bytes of a buffer view.
    pub(super) fn view(&self, view: &View<'_>) -> Result<&[u8], AssetError> {
        let buffer = self
            .data
            .get(view.buffer().index())
            .ok_or_else(|| gerr("buffer view refers to a missing buffer"))?;
        view.offset()
            .checked_add(view.length())
            .and_then(|end| buffer.get(view.offset()..end))
            .ok_or_else(|| gerr(format!("buffer view {} runs past its buffer", view.index())))
    }
}

fn component_size(t: DataType) -> usize {
    match t {
        DataType::I8 | DataType::U8 => 1,
        DataType::I16 | DataType::U16 => 2,
        DataType::U32 | DataType::F32 => 4,
    }
}

fn dimension_count(d: Dimensions) -> usize {
    match d {
        Dimensions::Scalar => 1,
        Dimensions::Vec2 => 2,
        Dimensions::Vec3 => 3,
        Dimensions::Vec4 | Dimensions::Mat2 => 4,
        Dimensions::Mat3 => 9,
        Dimensions::Mat4 => 16,
    }
}

/// Element byte slices of `accessor` (`None` = zero-filled accessor).
fn elements<'b>(
    buffers: &'b Buffers,
    accessor: &Accessor<'_>,
    what: &str,
) -> Result<Option<(&'b [u8], usize, usize)>, AssetError> {
    if accessor.sparse().is_some() {
        return Err(gerr(format!(
            "{what}: sparse accessor {} is not supported",
            accessor.index()
        )));
    }
    let elem = component_size(accessor.data_type()) * dimension_count(accessor.dimensions());
    let count = accessor.count();
    let Some(view) = accessor.view() else {
        if count > MAX_IMPLICIT_COUNT {
            return Err(gerr(format!("{what}: accessor {} is too large", accessor.index())));
        }
        return Ok(None);
    };
    let bytes = buffers.view(&view)?;
    let stride = view.stride().unwrap_or(elem);
    if stride < elem {
        return Err(gerr(format!(
            "{what}: accessor {} stride {stride} is below its element size {elem}",
            accessor.index()
        )));
    }
    let needed = match count {
        0 => Some(0),
        n => (n - 1)
            .checked_mul(stride)
            .and_then(|v| v.checked_add(elem))
            .and_then(|v| v.checked_add(accessor.offset())),
    };
    match needed {
        Some(n) if n <= bytes.len() => Ok(Some((&bytes[accessor.offset().min(bytes.len())..], stride, count))),
        _ => Err(gerr(format!(
            "{what}: accessor {} runs past its buffer view",
            accessor.index()
        ))),
    }
}

fn component(bytes: &[u8], t: DataType, normalized: bool) -> f32 {
    match t {
        DataType::F32 => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        DataType::U8 if normalized => f32::from(bytes[0]) / 255.0,
        DataType::U16 if normalized => {
            f32::from(u16::from_le_bytes([bytes[0], bytes[1]])) / 65_535.0
        }
        DataType::I8 if normalized => (f32::from(bytes[0] as i8) / 127.0).max(-1.0),
        DataType::I16 if normalized => {
            (f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32_767.0).max(-1.0)
        }
        DataType::U8 => f32::from(bytes[0]),
        DataType::U16 => f32::from(u16::from_le_bytes([bytes[0], bytes[1]])),
        DataType::I8 => f32::from(bytes[0] as i8),
        DataType::I16 => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])),
        DataType::U32 => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32,
    }
}

/// Reads an `N`-component float attribute (float, or normalized integers).
pub(super) fn read_f32s<const N: usize>(
    buffers: &Buffers,
    accessor: &Accessor<'_>,
    what: &str,
) -> Result<Vec<[f32; N]>, AssetError> {
    if dimension_count(accessor.dimensions()) != N || accessor.dimensions() == Dimensions::Mat2 {
        return Err(gerr(format!(
            "{what}: accessor {} must have {N} components",
            accessor.index()
        )));
    }
    let t = accessor.data_type();
    if t != DataType::F32 && !accessor.normalized() {
        return Err(gerr(format!(
            "{what}: accessor {} must be float or normalized",
            accessor.index()
        )));
    }
    let size = component_size(t);
    let Some((bytes, stride, count)) = elements(buffers, accessor, what)? else {
        return Ok(vec![[0.0; N]; accessor.count()]);
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = &bytes[i * stride..];
        let mut v = [0.0f32; N];
        for (k, c) in v.iter_mut().enumerate() {
            *c = component(&e[k * size..], t, accessor.normalized());
        }
        if v.iter().any(|c| !c.is_finite()) {
            return Err(gerr(format!(
                "{what}: accessor {} holds a non-finite value",
                accessor.index()
            )));
        }
        out.push(v);
    }
    Ok(out)
}

/// Reads a scalar unsigned index accessor.
pub(super) fn read_indices(
    buffers: &Buffers,
    accessor: &Accessor<'_>,
    what: &str,
) -> Result<Vec<u32>, AssetError> {
    let t = accessor.data_type();
    if accessor.dimensions() != Dimensions::Scalar
        || !matches!(t, DataType::U8 | DataType::U16 | DataType::U32)
    {
        return Err(gerr(format!(
            "{what}: index accessor {} must be scalar u8/u16/u32",
            accessor.index()
        )));
    }
    let Some((bytes, stride, count)) = elements(buffers, accessor, what)? else {
        return Ok(vec![0; accessor.count()]);
    };
    Ok((0..count)
        .map(|i| {
            let e = &bytes[i * stride..];
            match t {
                DataType::U8 => u32::from(e[0]),
                DataType::U16 => u32::from(u16::from_le_bytes([e[0], e[1]])),
                _ => u32::from_le_bytes([e[0], e[1], e[2], e[3]]),
            }
        })
        .collect())
}

/// Decodes standard or URL-safe base64 (padding optional, whitespace ignored).
pub(super) fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 3);
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut padding = false;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                padding = true;
                continue;
            }
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        if padding {
            return None; // data after padding
        }
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (bits < 6).then_some(out)
}

/// Decodes `%XX` escapes.
pub(super) fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b.get(i + 1..i + 3)?;
            let hex = core::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Joins a relative URI path to `base` lexically; `..` may not climb above
/// the bundle root.
fn join_relative(base: &str, rel: &str, uri: &str) -> Result<String, AssetError> {
    let rel = rel.replace('\\', "/");
    if rel.starts_with('/') {
        return Err(AssetError::AbsolutePath(uri.to_owned()));
    }
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for c in rel.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(AssetError::PathTraversal(uri.to_owned()));
                }
            }
            c => parts.push(c),
        }
    }
    normalize_path(&parts.join("/"))
}

/// Loads a buffer or image URI: `data:` inline, else a bundle file relative to
/// the glTF file. Any other scheme (`http:`, `file:`, ...) is refused.
pub(super) fn resolve_uri(uri: &str, vfs: &Vfs, base: &str) -> Result<Vec<u8>, AssetError> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (meta, payload) = rest
            .split_once(',')
            .ok_or_else(|| gerr("malformed data: URI"))?;
        return if meta.ends_with(";base64") {
            decode_base64(payload).ok_or_else(|| gerr("invalid base64 in data: URI"))
        } else {
            percent_decode(payload)
                .map(String::into_bytes)
                .ok_or_else(|| gerr("invalid escape in data: URI"))
        };
    }
    let path = percent_decode(uri).ok_or_else(|| gerr(format!("invalid URI {uri:?}")))?;
    // A scheme is letters/digits/+-. followed by ':' before any '/'; a
    // single letter is a Windows drive (absolute).
    if let Some((scheme, _)) = path.split_once(':')
        && !scheme.contains('/')
    {
        if scheme.len() == 1 {
            return Err(AssetError::AbsolutePath(uri.to_owned()));
        }
        return Err(gerr(format!("URI scheme of {uri:?} is not allowed")));
    }
    let full = join_relative(base, &path, uri)?;
    vfs.read(&full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVsbG8").unwrap(), b"hello");
        assert_eq!(decode_base64("aGV sbG8=\n").unwrap(), b"hello");
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("-_8=").unwrap(), vec![0xfb, 0xff]);
        assert!(decode_base64("a").is_none());
        assert!(decode_base64("a*bc").is_none());
        assert!(decode_base64("aa==aa").is_none());
    }

    #[test]
    fn percent() {
        assert_eq!(percent_decode("a%20b").unwrap(), "a b");
        assert!(percent_decode("a%2").is_none());
        assert!(percent_decode("a%zz").is_none());
        assert!(percent_decode("%ff").is_none());
    }

    #[test]
    fn joins() {
        assert_eq!(join_relative("m", "t.png", "u").unwrap(), "m/t.png");
        assert_eq!(join_relative("m/x", "../t.png", "u").unwrap(), "m/t.png");
        assert_eq!(join_relative("", "./a//b", "u").unwrap(), "a/b");
        assert_eq!(
            join_relative("", "../t.png", "u"),
            Err(AssetError::PathTraversal("u".into()))
        );
        assert_eq!(
            join_relative("", "/t.png", "u"),
            Err(AssetError::AbsolutePath("u".into()))
        );
    }

    #[test]
    fn components() {
        assert_eq!(component(&[255], DataType::U8, true), 1.0);
        assert_eq!(component(&[0xff, 0xff], DataType::U16, true), 1.0);
        assert_eq!(component(&[0x81], DataType::I8, true), -1.0);
        assert_eq!(component(&[0x01, 0x80], DataType::I16, true), -1.0);
        assert_eq!(component(&[3], DataType::U8, false), 3.0);
        assert_eq!(component(&[3, 0], DataType::U16, false), 3.0);
        assert_eq!(component(&[0xfe], DataType::I8, false), -2.0);
        assert_eq!(component(&[0xfe, 0xff], DataType::I16, false), -2.0);
        assert_eq!(component(&[1, 0, 0, 0], DataType::U32, false), 1.0);
        for t in [DataType::I8, DataType::U8, DataType::I16, DataType::U16, DataType::U32, DataType::F32] {
            assert!(component_size(t) >= 1);
        }
        for d in [Dimensions::Scalar, Dimensions::Vec2, Dimensions::Vec3, Dimensions::Vec4, Dimensions::Mat2, Dimensions::Mat3, Dimensions::Mat4] {
            assert!(dimension_count(d) >= 1);
        }
    }
}
