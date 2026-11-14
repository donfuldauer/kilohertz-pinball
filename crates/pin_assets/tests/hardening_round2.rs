//! Regressions for the 4.1 review round 2 hardening (fix task 4.1-H, D25):
//! N-a Zip64 entry count and the in-memory archive cap, N-b the
//! archive-level ratio rule, N-c the per-image decode budget, N-d glTF
//! buffers charged and capped by the bytes actually read.

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::zip::{ARCHIVE_PATH, ARCHIVE_RATIO_FLOOR};
use pin_assets::{AssetError, GltfLimits, Vfs, ZipLimits, load_gltf, load_gltf_with_limits};
use serde_json::json;
use support::*;

fn open(bytes: Vec<u8>) -> Result<Vfs, AssetError> {
    Vfs::from_zip_bytes(bytes, ZipLimits::default())
}

// ------------------------------------------------------------------- N-a

/// Turns a plain archive (no comment) into a Zip64 one: a Zip64 end record
/// declaring `count64` entries, its locator (pointing at `locator_offset`,
/// or at the record itself), and a 16-bit count of `count16` with the
/// directory size saturated, so that readers use the Zip64 record.
fn zip64(
    plain: &[u8],
    count16: u16,
    count64: u64,
    junk: usize,
    locator_offset: Option<u64>,
) -> Vec<u8> {
    zip64_ext(plain, count16, count64, junk, locator_offset, 0)
}

/// [`zip64`] with an extensible data sector of `ext` bytes.
fn zip64_ext(
    plain: &[u8],
    count16: u16,
    count64: u64,
    junk: usize,
    locator_offset: Option<u64>,
    ext: usize,
) -> Vec<u8> {
    let eocd = plain.len() - 22;
    assert_eq!(plain[eocd..eocd + 4], 0x0605_4b50u32.to_le_bytes());
    let cd_size = u64::from(u32::from_le_bytes(
        plain[eocd + 12..eocd + 16].try_into().unwrap(),
    ));
    let cd_off = u64::from(u32::from_le_bytes(
        plain[eocd + 16..eocd + 20].try_into().unwrap(),
    ));
    let mut out = vec![0u8; junk];
    out.extend(&plain[..eocd]);
    let record = out.len() as u64;
    out.extend(0x0606_4b50u32.to_le_bytes());
    out.extend((44 + ext as u64).to_le_bytes());
    out.extend(45u16.to_le_bytes());
    out.extend(45u16.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(count64.to_le_bytes());
    out.extend(count64.to_le_bytes());
    out.extend(cd_size.to_le_bytes());
    out.extend(cd_off.to_le_bytes());
    out.extend(vec![0u8; ext]);
    out.extend(0x0706_4b50u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(locator_offset.unwrap_or(record).to_le_bytes());
    out.extend(1u32.to_le_bytes());
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(count16.to_le_bytes());
    out.extend(count16.to_le_bytes());
    out.extend(u32::MAX.to_le_bytes());
    out.extend((cd_off as u32).to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

fn three_files() -> Vec<u8> {
    stored_zip(&[("a.txt", b"a"), ("b.txt", b"bb"), ("c.txt", b"ccc")])
}

#[test]
fn na_zip64_archive_is_read_through_its_zip64_record() {
    // An honest Zip64 archive (16-bit count left at 3) opens.
    let vfs = open(zip64(&three_files(), 3, 3, 0, None)).unwrap();
    assert_eq!(vfs.list(), ["a.txt", "b.txt", "c.txt"]);
    assert_eq!(vfs.read("c.txt").unwrap(), b"ccc");
}

#[test]
fn na_zip64_count_is_checked_before_parsing_whatever_the_16_bit_count() {
    // The reviewer's case: a small 16-bit count, millions in the Zip64 record.
    // Before the fix the 16-bit count passed and the reader was left to walk
    // (or here, to reject with a generic error) the directory.
    for count16 in [0u16, 3, 0xFFFF] {
        let r = open(zip64(&three_files(), count16, 2_000_000, 0, None));
        assert_eq!(
            r.err(),
            Some(AssetError::TooManyEntries(2_000_000)),
            "{count16}"
        );
    }
    // Prepended data (the locator's offset is stale): the reader would search
    // the archive for the record, so the archive is refused outright.
    let r = open(zip64(&three_files(), 3, 2_000_000, 100, Some(0)));
    assert!(
        matches!(&r, Err(AssetError::Zip(m)) if m.contains("not where its locator says")),
        "{r:?}"
    );
    // A record that cannot hold its count before it is refused too.
    let r = open(zip64(&three_files(), 3, 5_000, 0, None));
    assert!(
        matches!(&r, Err(AssetError::Zip(m)) if m.contains("more records than fit")),
        "{r:?}"
    );
    // Over max_entries by one.
    let limits = ZipLimits {
        max_entries: 2,
        ..ZipLimits::default()
    };
    let r = Vfs::from_zip_bytes(zip64(&three_files(), 1, 3, 0, None), limits);
    assert_eq!(r.err(), Some(AssetError::TooManyEntries(3)));
}

/// `zip` with its archive comment replaced by `comment`.
fn with_comment(zip: &[u8], comment: &[u8]) -> Vec<u8> {
    let mut out = zip.to_vec();
    let at = out.len() - 2;
    out[at..].copy_from_slice(&(comment.len() as u16).to_le_bytes());
    out.extend(comment);
    out
}

fn is_zip_error(r: &Result<Vfs, AssetError>, needle: &str) -> bool {
    matches!(r, Err(AssetError::Zip(m)) if m.contains(needle))
}

#[test]
fn na_the_end_record_must_end_the_archive() {
    // Review round 2, B1: a maximal comment plus one trailing byte put the
    // end record one byte outside the old pre-check's window, where the
    // reader still found it. Now the record's comment must end the archive.
    let claim = zip64(&three_files(), 3, 2_000_000, 0, None);
    let mut padded = with_comment(&claim, &vec![0u8; 0xFFFF]);
    let r = open(padded.clone());
    assert_eq!(r.err(), Some(AssetError::TooManyEntries(2_000_000)));
    padded.push(0);
    let r = open(padded);
    assert!(is_zip_error(&r, "comment ends the archive"), "{r:?}");
    // Trailing bytes after an honest archive are refused too.
    let mut trailing = three_files();
    trailing.extend(b"junk");
    assert!(is_zip_error(&open(trailing), "comment ends the archive"));
}

#[test]
fn na_signatures_inside_a_comment_do_not_confuse_the_reader() {
    // A valid archive whose comment contains end-record and locator
    // signatures (the reviewer's false TooManyEntries).
    let mut comment = b"see PK".to_vec();
    comment.extend(0x0605_4b50u32.to_le_bytes());
    comment.extend([0x6F, 0x6D]);
    comment.extend(0x0706_4b50u32.to_le_bytes());
    comment.extend(b" for details");
    for zip in [three_files(), zip64(&three_files(), 3, 3, 0, None)] {
        let vfs = open(with_comment(&zip, &comment)).unwrap();
        assert_eq!(vfs.list(), ["a.txt", "b.txt", "c.txt"]);
    }
}

#[test]
fn na_the_reader_is_never_left_to_search() {
    // Review round 2, B2(a): a flood of central-header signatures behind an
    // end record whose directory does not end at it (the reader used to scan
    // the flood for a directory start).
    let mut flood: Vec<u8> = b"PK\x01\x02".repeat(1 << 18);
    flood.extend(0x0605_4b50u32.to_le_bytes());
    flood.extend([0, 0, 0, 0, 1, 0, 1, 0]);
    flood.extend([0u8; 10]);
    assert!(is_zip_error(&open(flood), "does not end at the end record"));
    // B2(b): many end-record candidates, each with its own directory chain.
    let mut many = three_files();
    let eocd = many[many.len() - 22..].to_vec();
    for _ in 0..40 {
        many.extend(&eocd);
    }
    assert!(is_zip_error(&open(many), "does not end at the end record"));
    // Prepended data (a self-extractor) is refused, zip32 or Zip64.
    let mut prepended = vec![0u8; 100];
    prepended.extend(three_files());
    assert!(is_zip_error(
        &open(prepended),
        "does not end at the end record"
    ));
    let r = open(zip64(&three_files(), 3, 3, 100, Some(0)));
    assert!(is_zip_error(&r, "not where its locator says"), "{r:?}");
    // The directory must start where the end record says.
    let mut moved = three_files();
    let n = moved.len();
    let cd = u32::from_le_bytes(moved[n - 6..n - 2].try_into().unwrap());
    moved[cd as usize] = b'X';
    assert!(is_zip_error(&open(moved), "no central directory header"));
    // Multi-disk records.
    let mut disks = three_files();
    let n = disks.len();
    disks[n - 18] = 1; // disk number
    assert!(is_zip_error(&open(disks), "multi-disk"));
    let mut z64 = zip64(&three_files(), 3, 3, 0, None);
    let n = z64.len();
    z64[n - 22 - 4] = 2; // locator: total disks
    assert!(is_zip_error(&open(z64), "more than one disk"));
}

#[test]
fn na_zip64_extensible_sector_is_capped() {
    use pin_assets::zip::MAX_ZIP64_EXTENSIBLE;
    let cap = MAX_ZIP64_EXTENSIBLE as usize;
    let ok = open(zip64_ext(&three_files(), 3, 3, 0, None, cap)).unwrap();
    assert_eq!(ok.list().len(), 3);
    let r = open(zip64_ext(&three_files(), 3, 3, 0, None, cap + 1));
    assert!(is_zip_error(&r, "extensible data sector"), "{r:?}");
}

#[test]
fn na_from_zip_bytes_has_the_archive_size_cap() {
    let limits = ZipLimits {
        max_total_uncompressed: 1024,
        ..ZipLimits::default()
    };
    let cap = 1024 + (1 << 20);
    let r = Vfs::from_zip_bytes(vec![0u8; cap + 1], limits);
    assert_eq!(r.err(), Some(AssetError::TotalTooLarge(cap as u64 + 1)));
    // At the cap the bytes are parsed (and are not a zip).
    let r = Vfs::from_zip_bytes(vec![0u8; cap], limits);
    assert!(matches!(r, Err(AssetError::Zip(_))), "{r:?}");
}

// ------------------------------------------------------------------- N-b

#[test]
fn nb_small_entries_cannot_add_up_past_the_archive_ratio() {
    // 1 MiB of zeros per entry: each is at the per-entry ratio floor, so
    // only the archive-level rule sees them.
    let mib = vec![0u8; 1 << 20];
    let names: Vec<String> = (0..9).map(|i| format!("z{i}.bin")).collect();
    let files = |n: usize| -> Vec<(&str, &[u8])> {
        names[..n]
            .iter()
            .map(|s| (s.as_str(), mib.as_slice()))
            .collect()
    };
    // At the 8 MiB floor: a tiny archive of compressible data still loads.
    let floor_entries = (ARCHIVE_RATIO_FLOOR >> 20) as usize;
    let vfs = open(deflated_zip(&files(floor_entries))).unwrap();
    assert_eq!(vfs.read("z0.bin").unwrap(), mib);
    // One more MiB: refused at open, blaming the whole archive.
    let zip = deflated_zip(&files(floor_entries + 1));
    let len = zip.len() as u64;
    match open(zip) {
        Err(AssetError::CompressionRatio { path, ratio }) => {
            assert_eq!(path, ARCHIVE_PATH);
            assert_eq!(ratio, ((floor_entries as u64 + 1) << 20) / len);
            assert!(ratio > 100);
        }
        other => panic!("{other:?}"),
    }
    // A lax ratio cap lets it through.
    let lax = ZipLimits {
        max_ratio: 1_000_000,
        ..ZipLimits::default()
    };
    assert!(Vfs::from_zip_bytes(deflated_zip(&files(floor_entries + 1)), lax).is_ok());
}

#[test]
fn nb_incompressible_bundles_are_unaffected() {
    // 12 MiB of pseudo-random bytes (ratio ~1) in 1 MiB entries.
    let mut x = 0x2545_F491u32;
    let data: Vec<u8> = (0..12 << 20)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect();
    let names: Vec<String> = (0..12).map(|i| format!("r{i}.bin")).collect();
    let files: Vec<(&str, &[u8])> = names
        .iter()
        .zip(data.chunks(1 << 20))
        .map(|(n, d)| (n.as_str(), d))
        .collect();
    let vfs = open(deflated_zip(&files)).unwrap();
    assert_eq!(vfs.read("r11.bin").unwrap(), &data[11 << 20..]);
}

// ------------------------------------------------------------------- N-c

/// A PNG whose IHDR claims `w` x `h` RGBA at `depth` bits (no pixel data).
fn png_header(w: u32, h: u32, depth: u8) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut c = b"IHDR".to_vec();
    c.extend(w.to_be_bytes());
    c.extend(h.to_be_bytes());
    c.extend([depth, 6, 0, 0, 0]);
    out.extend(13u32.to_be_bytes());
    out.extend(&c);
    out.extend(crc32(&c).to_be_bytes());
    for ty in [&b"IDAT"[..], b"IEND"] {
        out.extend(0u32.to_be_bytes());
        out.extend(ty);
        out.extend(crc32(ty).to_be_bytes());
    }
    out
}

fn textured_gltf(uri: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "asset": {"version": "2.0"},
        "images": [{"uri": uri}],
        "textures": [{"source": 0}],
        "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}]
    }))
    .unwrap()
}

fn load_image(png: &[u8], limits: &GltfLimits) -> Result<pin_assets::GltfScene, AssetError> {
    let gltf = textured_gltf("t.png");
    let vfs = Vfs::from_zip_bytes(
        stored_zip(&[("t.gltf", &gltf), ("t.png", png)]),
        ZipLimits::default(),
    )
    .unwrap();
    load_gltf_with_limits(&vfs, "t.gltf", limits)
}

fn decode_budget_error(r: &Result<pin_assets::GltfScene, AssetError>) -> bool {
    matches!(r, Err(AssetError::Gltf(m)) if m.contains("decode bytes") && m.contains("budget"))
}

#[test]
fn nc_sixteen_bit_png_is_refused_from_its_header() {
    // The reviewer's case: one 8192² 16-bit RGBA PNG (512 MiB decoder buffer
    // plus the 256 MiB RGBA8 copy). Refused before any pixel is allocated.
    let r = load_image(&png_header(8192, 8192, 16), &GltfLimits::default());
    assert!(decode_budget_error(&r), "{r:?}");
    // An RGBA8 image needs no copy: 64² 8-bit fits a 64²·4-byte budget and
    // gets as far as the (missing) pixel data; 64² 16-bit does not fit.
    let limits = GltfLimits {
        max_image_decode_bytes: 64 * 64 * 4,
        ..GltfLimits::default()
    };
    let r = load_image(&png_header(64, 64, 8), &limits);
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if !m.contains("budget")),
        "{r:?}"
    );
    let r = load_image(&png_header(64, 64, 16), &limits);
    assert!(decode_budget_error(&r), "{r:?}");
}

fn encode_png(image: image::DynamicImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn nc_decode_budget_counts_native_buffer_and_rgba8_copy() {
    // 4x2 16-bit RGBA: native 64 bytes + RGBA8 copy 32 bytes = 96.
    let px = image::Rgba([0xFFFFu16, 0x8000, 0, 0xFFFF]);
    let png16 = encode_png(image::DynamicImage::ImageRgba16(
        image::ImageBuffer::from_pixel(4, 2, px),
    ));
    let at = |budget: u64| GltfLimits {
        max_image_decode_bytes: budget,
        ..GltfLimits::default()
    };
    let scene = load_image(&png16, &at(96)).unwrap();
    assert_eq!((scene.images[0].width, scene.images[0].height), (4, 2));
    assert_eq!(&scene.images[0].rgba8[..4], &[255, 128, 0, 255]);
    assert!(decode_budget_error(&load_image(&png16, &at(95))));
    // RGBA8 needs no copy: 4x2x4 = 32.
    let png8 = encode_png(image::DynamicImage::ImageRgba8(
        image::ImageBuffer::from_pixel(4, 2, image::Rgba([1u8, 2, 3, 4])),
    ));
    assert!(load_image(&png8, &at(32)).is_ok());
    assert!(decode_budget_error(&load_image(&png8, &at(31))));
    // RGB8: 24 + 32.
    let rgb = encode_png(image::DynamicImage::ImageRgb8(
        image::ImageBuffer::from_pixel(4, 2, image::Rgb([1u8, 2, 3])),
    ));
    assert!(load_image(&rgb, &at(56)).is_ok());
    assert!(decode_budget_error(&load_image(&rgb, &at(55))));
    // An encoded file larger than the decode budget is not even read.
    let mut padded = png8.clone();
    padded.resize(png8.len() + (1 << 20), 0);
    let r = load_image(&padded, &at(png8.len() as u64 - 1));
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("encoded file bytes")),
        "{r:?}"
    );
}

/// Inserts a chunk right after the IHDR of `png`.
fn with_chunk(png: &[u8], ty: &[u8], data: &[u8]) -> Vec<u8> {
    let at = 8 + 25;
    let mut c = ty.to_vec();
    c.extend(data);
    let mut out = png[..at].to_vec();
    out.extend((data.len() as u32).to_be_bytes());
    out.extend(&c);
    out.extend(crc32(&c).to_be_bytes());
    out.extend(&png[at..]);
    out
}

#[test]
fn nc_icc_profile_bomb_is_bounded_in_the_header_phase() {
    // A 2x1 PNG with an iCCP chunk inflating to 8 MiB (the reviewer's was a
    // 250 MiB profile, inflated in full and uncharged). The decoder's own
    // buffers now get IMAGE_HEADER_ALLOC (4 MiB) while the header is read, so
    // the profile stops inflating at 4 MiB and is dropped (the PNG decoder
    // treats an ancillary chunk over its limit as absent); the pixels load.
    // (Peak memory is measured outside the test suite, see DECISIONS.md.)
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut z, &vec![0u8; 8 << 20]).unwrap();
    let mut iccp = b"icc\0\0".to_vec();
    iccp.extend(z.finish().unwrap());
    let bomb = with_chunk(&tiny_png(), b"iCCP", &iccp);
    let r = load_image(&bomb, &GltfLimits::default());
    match &r {
        Ok(scene) => assert_eq!(scene.images[0].rgba8.len(), 8),
        Err(AssetError::Gltf(m)) => assert!(m.contains("header allocation budget"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(pin_assets::gltf::IMAGE_HEADER_ALLOC, 4 << 20);
    // A small honest profile is fine.
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut z, &[1u8; 3000]).unwrap();
    let mut iccp = b"icc\0\0".to_vec();
    iccp.extend(z.finish().unwrap());
    let scene = load_image(
        &with_chunk(&tiny_png(), b"iCCP", &iccp),
        &GltfLimits::default(),
    )
    .unwrap();
    assert_eq!(scene.images[0].rgba8.len(), 8);
}

#[test]
fn nc_image_files_are_sized_from_their_header_before_being_read() {
    // Ancillary data in front of the pixels must fit in the first 1 MiB.
    let text = vec![b'a'; (1 << 20) + 10];
    let mut chunk = b"Comment\0".to_vec();
    chunk.extend(&text);
    let r = load_image(
        &with_chunk(&tiny_png(), b"tEXt", &chunk),
        &GltfLimits::default(),
    );
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("first 1048576 bytes") && m.contains("strip embedded metadata")),
        "{r:?}"
    );
}

#[test]
fn nc_truncated_images_do_not_allocate_their_pixels() {
    // A 57-byte PNG claiming 2048² RGBA8 (16 MiB of pixels): deflate cannot
    // reach that ratio, so it is refused before the pixel buffer exists.
    let r = load_image(&png_header(2048, 2048, 8), &GltfLimits::default());
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("too few for a 2048x2048 image")),
        "{r:?}"
    );
    // A JPEG start-of-image with a 2048² frame header and nothing else.
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8];
    jpeg.extend(2048u16.to_be_bytes());
    jpeg.extend(2048u16.to_be_bytes());
    jpeg.extend([1, 1, 0x11, 0]);
    jpeg.extend([0xFF, 0xD9]);
    let r = load_image(&jpeg, &GltfLimits::default());
    assert!(matches!(&r, Err(AssetError::Gltf(_))), "{r:?}");
    // A real, highly compressible image still loads (1024² of zeros).
    let zeros = encode_png(image::DynamicImage::ImageLuma8(image::ImageBuffer::new(
        1024, 1024,
    )));
    assert!(zeros.len() < 10_000, "{}", zeros.len());
    let scene = load_image(&zeros, &GltfLimits::default()).unwrap();
    assert_eq!(scene.images[0].rgba8.len(), 1024 * 1024 * 4);
}

#[test]
fn b3_gltf_json_is_capped_before_reading() {
    assert_eq!(GltfLimits::default().max_json_bytes, 16 << 20);
    let tight = GltfLimits {
        max_json_bytes: 100,
        ..GltfLimits::default()
    };
    // A .gltf (all JSON, data: URIs included).
    let payload = b64(&vec![0u8; 300]);
    let gltf = buffer_gltf(json!({"byteLength": 36, "uri": format!("data:,{payload}")}));
    let vfs = open(stored_zip(&[("t.gltf", &gltf)])).unwrap();
    let r = load_gltf_with_limits(&vfs, "t.gltf", &tight);
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("glTF JSON bytes") && m.contains("budget") && m.contains("export a binary .glb")),
        "{r:?}"
    );
    // A GLB: the JSON chunk length from the header; the BIN chunk is not JSON.
    let json = json!({"asset": {"version": "2.0"}, "buffers": [{"byteLength": 4096}]});
    let file = glb(&json, &vec![0u8; 4096]);
    let vfs = open(stored_zip(&[("t.glb", &file)])).unwrap();
    let small = GltfLimits {
        max_json_bytes: 20,
        ..GltfLimits::default()
    };
    let r = load_gltf_with_limits(&vfs, "t.glb", &small);
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("GLB JSON chunk bytes")),
        "{r:?}"
    );
    let r = load_gltf_with_limits(
        &vfs,
        "t.glb",
        &GltfLimits {
            max_json_bytes: 1000,
            max_buffer_bytes: 1000,
            ..GltfLimits::default()
        },
    );
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("GLB file bytes")),
        "{r:?}"
    );
    assert!(load_gltf(&vfs, "t.glb").is_ok());
}

// ------------------------------------------------------------------- N-d

fn buffer_gltf(buffer: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "asset": {"version": "2.0"},
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
                        "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]}],
        "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 36}],
        "buffers": [buffer]
    }))
    .unwrap()
}

fn triangle() -> Vec<u8> {
    f32_bytes(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0])
}

#[test]
fn nd_only_the_declared_bytes_of_a_buffer_file_are_read() {
    // A 36-byte buffer naming a 2 MiB bundle file whose CRC is wrong: only
    // the first 36 bytes are inflated, so the whole file is never read (a
    // whole read would fail its CRC check).
    let mut big = triangle();
    let mut x = 0x2545_F491u32;
    big.extend((36..2 << 20).map(|_| {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x as u8
    }));
    let mut zip = deflated_zip(&[
        (
            "t.gltf",
            &buffer_gltf(json!({"byteLength": 36, "uri": "big.bin"})),
        ),
        ("big.bin", &big),
    ]);
    let cd = zip
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x01, 0x02])
        .unwrap();
    zip[cd + 16] ^= 0xFF; // central CRC of big.bin (the last entry)
    let vfs = open(zip).unwrap();
    assert!(matches!(vfs.read("big.bin"), Err(AssetError::Zip(m)) if m.contains("CRC")));
    assert_eq!(vfs.read_prefix("big.bin", 36).unwrap(), triangle());
    assert_eq!(vfs.file_size("big.bin").unwrap(), 2 << 20);
    let limits = GltfLimits {
        max_buffer_bytes: 36,
        ..GltfLimits::default()
    };
    let scene = load_gltf_with_limits(&vfs, "t.gltf", &limits).unwrap();
    assert_eq!(scene.meshes[0].positions[1], [1.0, 0.0, 0.0]);
}

#[test]
fn nd_short_buffer_files_are_refused_before_reading() {
    let gltf = buffer_gltf(json!({"byteLength": 36, "uri": "b.bin"}));
    let short = &triangle()[..20];
    let vfs = open(stored_zip(&[("t.gltf", &gltf), ("b.bin", short)])).unwrap();
    let r = load_gltf(&vfs, "t.gltf");
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("has 20 bytes but declares 36")),
        "{r:?}"
    );
}

#[test]
fn nd_data_uris_are_charged_their_decoded_length() {
    // 36 bytes declared, 48 decoded: the budget sees 48.
    let mut payload = triangle();
    payload.extend([0u8; 12]);
    let uri = format!("data:application/octet-stream;base64,{}", b64(&payload));
    let gltf = buffer_gltf(json!({"byteLength": 36, "uri": uri}));
    let vfs = open(stored_zip(&[("t.gltf", &gltf)])).unwrap();
    let at = |budget: u64| GltfLimits {
        max_buffer_bytes: budget,
        ..GltfLimits::default()
    };
    assert!(load_gltf_with_limits(&vfs, "t.gltf", &at(48)).is_ok());
    let r = load_gltf_with_limits(&vfs, "t.gltf", &at(47));
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("buffers hold 48 bytes") && m.contains("budget")),
        "{r:?}"
    );
}

#[test]
fn nd_folder_bundles_read_prefixes_too() {
    let dir = temp_dir("nd_prefix");
    std::fs::write(dir.join("big.bin"), vec![9u8; 4096]).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    assert_eq!(vfs.file_size("big.bin").unwrap(), 4096);
    assert_eq!(vfs.read_prefix("big.bin", 10).unwrap(), vec![9u8; 10]);
    assert_eq!(vfs.read_prefix("big.bin", 1 << 20).unwrap().len(), 4096);
    assert_eq!(
        vfs.file_size("nope.bin").err(),
        Some(AssetError::NotFound("nope.bin".into()))
    );
    assert_eq!(
        vfs.read_prefix("../x", 1).err(),
        Some(AssetError::PathTraversal("../x".into()))
    );
    let tight = Vfs::open_with_limits(
        &dir,
        ZipLimits {
            max_file: 100,
            ..ZipLimits::default()
        },
    )
    .unwrap();
    assert_eq!(tight.read_prefix("big.bin", 100).unwrap().len(), 100);
    assert!(matches!(
        tight.read_prefix("big.bin", 101),
        Err(AssetError::TooLarge { .. })
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

fn b64(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

mod props {
    use super::*;
    use proptest::prelude::*;

    fn config() -> ProptestConfig {
        ProptestConfig {
            cases: 128,
            failure_persistence: None,
            ..ProptestConfig::default()
        }
    }

    proptest! {
        #![proptest_config(config())]

        /// Mutated Zip64 archives never panic; a Zip64 count over the cap is
        /// always refused before parsing, whatever the 16-bit count.
        #[test]
        fn zip64_mutations(
            count16 in any::<u16>(),
            count64 in any::<u64>(),
            junk in 0usize..64,
            stale in any::<bool>(),
            edits in proptest::collection::vec((any::<usize>(), any::<u8>()), 0..8),
        ) {
            let base = zip64(&three_files(), count16, count64, junk, stale.then_some(0));
            let r = open(base.clone());
            if count64 > 10_000 && !stale {
                prop_assert_eq!(r.err(), Some(AssetError::TooManyEntries(count64 as usize)));
            }
            let mut bytes = base;
            for (i, b) in edits {
                let n = bytes.len();
                bytes[i % n] = b;
            }
            if let Ok(vfs) = open(bytes) {
                for name in vfs.list() {
                    let _ = vfs.read(&name);
                    let _ = vfs.read_prefix(&name, 2);
                }
            }
        }

        /// Random image bytes never panic the decoder path.
        #[test]
        fn random_image_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
            png.extend(&bytes);
            let _ = load_image(&png, &GltfLimits::default());
            let _ = load_image(&bytes, &GltfLimits::default());
        }
    }
}
