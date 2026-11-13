//! Bounds-checked buffer, accessor and URI handling for glTF.

use super::gerr;
use crate::zip::normalize_path;
use crate::{AssetError, Vfs};
use ::gltf::Document;
use ::gltf::accessor::{Accessor, DataType, Dimensions};
use ::gltf::buffer::{Source, View};

/// Every buffer's bytes (truncated to the declared `byteLength`). Buffers
/// naming the same resolved source share one copy.
pub(super) struct Buffers {
    /// Index into `store` for each glTF buffer.
    slot: Vec<usize>,
    store: Vec<Vec<u8>>,
}

impl Buffers {
    /// Loads every buffer. The sum of the declared lengths of the distinct
    /// sources is checked against `budget` before anything is read; then
    /// every source is charged the bytes actually held for it (a bundle file
    /// is sized first and only its first `byteLength` bytes are read; a
    /// `data:` URI or the GLB `BIN` chunk is charged its whole decoded
    /// length), so the budget bounds the memory really used (N-d).
    pub(super) fn load(
        document: &Document,
        mut bin: Option<Vec<u8>>,
        vfs: &Vfs,
        base: &str,
        budget: u64,
    ) -> Result<Buffers, AssetError> {
        let over = |what: &str, n: u64| {
            gerr(format!(
                "{what} {n} bytes, over the {budget}-byte buffer budget"
            ))
        };
        let mut keys: Vec<Option<String>> = Vec::with_capacity(document.buffers().len());
        let mut seen = std::collections::HashMap::new();
        let mut planned: u64 = 0;
        for buffer in document.buffers() {
            let key = match buffer.source() {
                Source::Bin => {
                    planned = planned.saturating_add(buffer.length() as u64);
                    None
                }
                Source::Uri(uri) => Some(source_key(uri, base)?),
            };
            if let Some(k) = &key {
                let len = seen.entry(k.clone()).or_insert(0usize);
                *len = (*len).max(buffer.length());
            }
            keys.push(key);
        }
        planned = seen
            .values()
            .fold(planned, |acc, &l| acc.saturating_add(l as u64));
        if planned > budget {
            return Err(over("buffers declare", planned));
        }
        let mut spent: u64 = 0;
        let mut slot = Vec::with_capacity(keys.len());
        let mut store: Vec<Vec<u8>> = Vec::new();
        let mut by_key: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (buffer, key) in document.buffers().zip(keys) {
            let index = match &key {
                Some(k) if by_key.contains_key(k) => by_key[k],
                _ => {
                    let keep = match &key {
                        Some(k) => seen[k],
                        None => buffer.length(),
                    };
                    let mut bytes = match &key {
                        None => bin.take().ok_or_else(|| {
                            gerr(format!(
                                "buffer {} refers to a missing GLB BIN chunk",
                                buffer.index()
                            ))
                        })?,
                        Some(k) if k.starts_with("data:") => load_source(k, vfs)?,
                        Some(k) => {
                            let size = vfs.file_size(k)?;
                            if size < keep as u64 {
                                return Err(gerr(format!(
                                    "buffer {} has {size} bytes but declares {keep}",
                                    buffer.index()
                                )));
                            }
                            vfs.read_prefix(k, keep as u64)?
                        }
                    };
                    spent = spent.saturating_add(bytes.len() as u64);
                    if spent > budget {
                        return Err(over("buffers hold", spent));
                    }
                    bytes.truncate(keep);
                    bytes.shrink_to_fit();
                    store.push(bytes);
                    if let Some(k) = key {
                        by_key.insert(k, store.len() - 1);
                    }
                    store.len() - 1
                }
            };
            if store[index].len() < buffer.length() {
                return Err(gerr(format!(
                    "buffer {} has {} bytes but declares {}",
                    buffer.index(),
                    store[index].len(),
                    buffer.length()
                )));
            }
            slot.push(index);
        }
        Ok(Buffers { slot, store })
    }

    /// The bytes of a buffer view.
    pub(super) fn view(&self, view: &View<'_>) -> Result<&[u8], AssetError> {
        let buffer = self
            .slot
            .get(view.buffer().index())
            .map(|&i| &self.store[i])
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

/// The bytes of `accessor` from its first element, with stride and count.
fn elements<'b>(
    buffers: &'b Buffers,
    accessor: &Accessor<'_>,
    what: &str,
) -> Result<(&'b [u8], usize, usize), AssetError> {
    if accessor.sparse().is_some() {
        return Err(gerr(format!(
            "{what}: sparse accessor {} is not supported",
            accessor.index()
        )));
    }
    let elem = component_size(accessor.data_type()) * dimension_count(accessor.dimensions());
    let count = accessor.count();
    // Validation only admits a view-less accessor when it is sparse.
    let view = accessor.view().ok_or_else(|| {
        gerr(format!(
            "{what}: accessor {} has no buffer view",
            accessor.index()
        ))
    })?;
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
        Some(n) if n <= bytes.len() => {
            Ok((&bytes[accessor.offset().min(bytes.len())..], stride, count))
        }
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
    let (bytes, stride, count) = elements(buffers, accessor, what)?;
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
    let (bytes, stride, count) = elements(buffers, accessor, what)?;
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

/// The resolved source of a buffer or image URI: the `data:` URI itself, or
/// the normalised bundle path relative to the glTF file. Two URIs with the
/// same key name the same bytes. Any other scheme (`http:`, `file:`, ...) is
/// refused.
pub(super) fn source_key(uri: &str, base: &str) -> Result<String, AssetError> {
    if uri.starts_with("data:") {
        return Ok(uri.to_owned());
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
    join_relative(base, &path, uri)
}

/// Loads the bytes of a [`source_key`]: `data:` decoded inline, else the
/// bundle file.
pub(super) fn load_source(key: &str, vfs: &Vfs) -> Result<Vec<u8>, AssetError> {
    if let Some(rest) = key.strip_prefix("data:") {
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
    vfs.read(key)
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
        for t in [
            DataType::I8,
            DataType::U8,
            DataType::I16,
            DataType::U16,
            DataType::U32,
            DataType::F32,
        ] {
            assert!(component_size(t) >= 1);
        }
        for d in [
            Dimensions::Scalar,
            Dimensions::Vec2,
            Dimensions::Vec3,
            Dimensions::Vec4,
            Dimensions::Mat2,
            Dimensions::Mat3,
            Dimensions::Mat4,
        ] {
            assert!(dimension_count(d) >= 1);
        }
    }
}
