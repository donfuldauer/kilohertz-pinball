//! Regressions for the 4.1 review (fix round 1): zip data ranges and live
//! ratio, entry counts read from the end record, honest small files, OS junk
//! and symlinked folders, manifest size, id overflow and positive parameters.

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{AssetError, ValidationError, Vfs, ZipLimits, load_table};
use std::io::Write;
use support::*;

fn open(bytes: Vec<u8>) -> Result<Vfs, AssetError> {
    Vfs::from_zip_bytes(bytes, ZipLimits::default())
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// One deflated entry whose compressed stream is followed by `padding`
/// bytes that the central directory counts as compressed data.
fn deflated_entry_zip(name: &str, data: &[u8], padding: usize) -> Vec<u8> {
    let stream = deflate(data);
    let csize = (stream.len() + padding) as u32;
    let crc = crc32(data);
    let mut out = Vec::new();
    out.extend(0x0403_4b50u32.to_le_bytes());
    out.extend(20u16.to_le_bytes());
    out.extend(0x0800u16.to_le_bytes());
    out.extend(8u16.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(crc.to_le_bytes());
    out.extend(csize.to_le_bytes());
    out.extend((data.len() as u32).to_le_bytes());
    out.extend((name.len() as u16).to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend(name.as_bytes());
    out.extend(&stream);
    out.extend(vec![0u8; padding]);
    let cd = out.len() as u32;
    let mut central = Vec::new();
    central.extend(0x0201_4b50u32.to_le_bytes());
    central.extend((3u16 << 8 | 20).to_le_bytes());
    central.extend(20u16.to_le_bytes());
    central.extend(0x0800u16.to_le_bytes());
    central.extend(8u16.to_le_bytes());
    central.extend(0u32.to_le_bytes());
    central.extend(crc.to_le_bytes());
    central.extend(csize.to_le_bytes());
    central.extend((data.len() as u32).to_le_bytes());
    central.extend((name.len() as u16).to_le_bytes());
    central.extend([0u8; 8]);
    central.extend((0o100644u32 << 16).to_le_bytes());
    central.extend(0u32.to_le_bytes());
    central.extend(name.as_bytes());
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(cd.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// Offsets of the local and central compressed-size fields of the first entry.
fn size_fields(zip: &[u8]) -> (usize, usize) {
    let cd = zip
        .windows(4)
        .position(|w| w == [0x50, 0x4b, 0x01, 0x02])
        .unwrap();
    (18, cd + 20)
}

#[test]
fn b1_declared_compressed_size_must_be_real_bytes() {
    let zeros = vec![0u8; 64 << 20];
    let honest = deflated_zip(&[("bomb.bin", &zeros)]);
    assert!(matches!(
        open(honest.clone()),
        Err(AssetError::CompressionRatio { .. })
    ));
    // The reviewer's probe: claim 64 MiB of compressed data in both headers.
    let mut lying = honest;
    let (local, central) = size_fields(&lying);
    for at in [local, central] {
        lying[at..at + 4].copy_from_slice(&(64u32 << 20).to_le_bytes());
    }
    let r = open(lying);
    assert!(
        matches!(&r, Err(AssetError::Zip(m)) if m.contains("past the end")),
        "{r:?}"
    );
}

#[test]
fn b1_ratio_is_enforced_on_bytes_actually_inflated() {
    // 4 MiB of zeros inflate from ~4 KiB, but 60 KiB of padding makes the
    // declared ratio ~68:1. The live check stops it on read.
    let zeros = vec![0u8; 4 << 20];
    let zip = deflated_entry_zip("pad.bin", &zeros, 60 * 1024);
    let vfs = open(zip).unwrap();
    let r = vfs.read("pad.bin");
    match r {
        Err(AssetError::CompressionRatio { path, ratio }) => {
            assert_eq!(path, "pad.bin");
            assert!(ratio > 100);
        }
        other => panic!("{other:?}"),
    }
    // The same entry without padding is refused at open, and with a lax cap it
    // reads back exactly.
    let lax = ZipLimits {
        max_ratio: 100_000,
        ..ZipLimits::default()
    };
    let vfs = Vfs::from_zip_bytes(deflated_entry_zip("pad.bin", &zeros, 60 * 1024), lax).unwrap();
    assert_eq!(vfs.read("pad.bin").unwrap(), zeros);
}

#[test]
fn b1_overlapping_entries_are_rejected() {
    // A second central record pointing at the first entry's data.
    let zip = stored_zip(&[("aaaa", b"0123456789")]);
    let cd = zip
        .windows(4)
        .position(|w| w == [0x50, 0x4b, 0x01, 0x02])
        .unwrap();
    let eocd = zip.len() - 22;
    let record = zip[cd..eocd].to_vec();
    let mut twin = record.clone();
    let name_at = 46;
    twin[name_at..name_at + 4].copy_from_slice(b"bbbb");
    let mut out = zip[..eocd].to_vec();
    out.extend(&twin);
    let mut end = zip[eocd..].to_vec();
    end[8..10].copy_from_slice(&2u16.to_le_bytes());
    end[10..12].copy_from_slice(&2u16.to_le_bytes());
    end[12..16].copy_from_slice(&((record.len() * 2) as u32).to_le_bytes());
    out.extend(end);
    let r = open(out);
    assert!(
        matches!(&r, Err(AssetError::Zip(m)) if m.contains("overlaps")),
        "{r:?}"
    );
}

#[test]
fn b1_crc_is_checked() {
    let mut zip = stored_zip(&[("a", b"hello")]);
    let at = 30 + 1; // data of the first entry
    zip[at] = b'j';
    let vfs = open(zip).unwrap();
    assert!(matches!(vfs.read("a"), Err(AssetError::Zip(m)) if m.contains("CRC")));
}

#[test]
fn n1_entry_count_is_read_from_the_end_record() {
    // EOCD claims 65 535 entries; nothing else is parsed.
    let mut zip = stored_zip(&[("a", b"x")]);
    let eocd = zip.len() - 22;
    zip[eocd + 8..eocd + 12].copy_from_slice(&[0xFF; 4]);
    assert_eq!(
        open(zip.clone()).err(),
        Some(AssetError::TooManyEntries(65_535))
    );
    // Zip64 end record claiming two million entries.
    let body = &zip[..eocd];
    let mut out = body.to_vec();
    let z64 = out.len() as u64;
    out.extend(0x0606_4b50u32.to_le_bytes());
    out.extend(44u64.to_le_bytes());
    out.extend([45u8, 0, 45, 0]);
    out.extend([0u8; 8]);
    out.extend(2_000_000u64.to_le_bytes());
    out.extend(2_000_000u64.to_le_bytes());
    out.extend(0u64.to_le_bytes());
    out.extend(0u64.to_le_bytes());
    out.extend(0x0706_4b50u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(z64.to_le_bytes());
    out.extend(1u32.to_le_bytes());
    out.extend(&zip[eocd..]);
    assert_eq!(open(out).err(), Some(AssetError::TooManyEntries(2_000_000)));
}

#[test]
fn n1_archive_file_cap_is_the_uncompressed_cap() {
    let dir = temp_dir("archive_cap");
    let path = dir.join("big.pinbundle");
    std::fs::write(&path, vec![0u8; (1 << 20) + 11]).unwrap();
    let limits = ZipLimits {
        max_total_uncompressed: 10,
        ..ZipLimits::default()
    };
    assert_eq!(
        Vfs::open_with_limits(&path, limits).err(),
        Some(AssetError::TotalTooLarge((1 << 20) + 11))
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn n5_honest_small_files_pass_the_ratio_check() {
    // 1 MiB of silence (~1000:1) is fine; one byte more is checked.
    let silence = vec![0u8; 1 << 20];
    let vfs = open(deflated_zip(&[("silence.wav", &silence)])).unwrap();
    assert_eq!(vfs.read("silence.wav").unwrap(), silence);
    let more = vec![0u8; (1 << 20) + 1];
    assert!(matches!(
        open(deflated_zip(&[("big.wav", &more)])),
        Err(AssetError::CompressionRatio { .. })
    ));
}

#[test]
fn n6_os_junk_is_ignored_and_unlisted() {
    let files: Vec<(&str, &[u8])> = vec![
        (".DS_Store", b"j"),
        ("Thumbs.db", b"j"),
        ("__MACOSX/pkg/._table.toml", b"j"),
        ("pkg/table.toml", b"t"),
        ("pkg/desktop.ini", b"j"),
        ("pkg/a.lua", b"a"),
    ];
    let vfs = open(stored_zip(&files)).unwrap();
    assert_eq!(vfs.list(), vec!["a.lua", "table.toml"]);
    assert_eq!(vfs.read("table.toml").unwrap(), b"t");
    assert!(!vfs.exists("desktop.ini"));

    let dir = temp_dir("junk");
    let owned: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|(n, d)| (n.to_string(), d.to_vec()))
        .collect();
    write_dir(&dir, &owned);
    let vfs = Vfs::open(&dir).unwrap();
    assert_eq!(vfs.list(), vec!["a.lua", "table.toml"]);
    assert!(!vfs.exists("desktop.ini"));
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn n6_symlinked_folders_are_neither_listed_nor_read() {
    let dir = temp_dir("dirlink_inside");
    write_dir(
        &dir,
        &[
            ("table.toml".into(), b"t".to_vec()),
            ("real/a.txt".into(), b"a".to_vec()),
        ],
    );
    std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    assert_eq!(vfs.list(), vec!["real/a.txt", "table.toml"]);
    assert_eq!(
        vfs.read("link/a.txt").err(),
        Some(AssetError::Symlink("link/a.txt".into()))
    );
    assert!(!vfs.exists("link/a.txt"));
    assert_eq!(vfs.read("real/a.txt").unwrap(), b"a");
    let _ = std::fs::remove_dir_all(dir);
}

fn errors_for(table: &str) -> Vec<ValidationError> {
    let files = with_file(&bundle_files(), "table.toml", table.as_bytes());
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let vfs = Vfs::from_zip_bytes(stored_zip(&refs), ZipLimits::default()).unwrap();
    match load_table(&vfs) {
        Err(AssetError::Invalid(e)) => e,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn has(errors: &[ValidationError], path: &str, text: &str) -> bool {
    errors
        .iter()
        .any(|e| e.file == "table.toml" && e.path == path && e.message.contains(text))
}

#[test]
fn n3_manifest_size_is_capped() {
    let mut big = table_text();
    big.push('#');
    big.push_str(&" ".repeat(4 << 20));
    let e = errors_for(&big);
    assert!(has(&e, "", "max 4194304"), "{e:?}");
}

#[test]
fn n4_too_many_entries_of_a_kind() {
    let keys: Vec<String> = (0..65_537).map(|i| format!("\"k{i}\"")).collect();
    let text = table_text().replace(
        "keys = [\"high_score_1\", \"high_score_2\", \"games_played\"]",
        &format!("keys = [{}]", keys.join(",")),
    );
    let e = errors_for(&text);
    assert!(has(&e, "persist.keys", "max 65536"), "{e:?}");

    let modes: Vec<String> = (0..65_537)
        .map(|i| format!("{{id=\"m{i}\",start_scene=\"scene_powerball_mania\"}}"))
        .collect();
    let text = table_text().replace(
        "[[modes]]                              # rule modes the soak must see start at least once\nid = \"powerball_mania\"\nstart_scene = \"scene_powerball_mania\"\n",
        "",
    );
    let text = format!("modes = [{}]\n{text}", modes.join(","));
    let e = errors_for(&text);
    assert!(has(&e, "modes", "too many mode entries: 65537"), "{e:?}");
}

#[test]
fn n3_duplicates_are_still_reported_in_order() {
    let mut text = table_text();
    for i in 0..3 {
        text.push_str(&format!(
            "\n[[sounds]]\nid = \"sfx_bumper\"\nfile = \"audio/bumper.ogg\"\nbus = \"sfx\"  # dup {i}\n"
        ));
    }
    let e = errors_for(&text);
    let dups: Vec<&str> = e
        .iter()
        .filter(|e| e.message.contains("duplicate sound id"))
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(dups.len(), 3, "{e:?}");
    assert!(
        e.iter()
            .all(|e| !e.message.contains("first declared at sounds[1]"))
    );
}

#[test]
fn n9_physical_parameters_must_be_positive() {
    let text = table_text()
        .replacen("radius = 12.0", "radius = 0.0", 1)
        .replace("k_coil = 2.0e-6", "k_coil = -1.0")
        .replace("d_min = 8.0", "d_min = 0.0")
        .replace("range = 60.0", "range = -5.0");
    let e = errors_for(&text);
    for (path, text) in [
        ("switches[0].radius", "radius must be positive"),
        ("magnets[0].k_coil", "must not be negative"),
        ("magnets[0].d_min", "d_min must be positive"),
        ("magnets[0].range", "range must be positive"),
    ] {
        assert!(has(&e, path, text), "{path}: {e:?}");
    }
}
