//! Subphase 4.1 spec tests (Independent Tester): bundle VFS and zip hardening.
//! Brief: deliverable 1, FIXED API `Vfs` / `ZipLimits` / `AssetError`,
//! verification criteria 1 (byte equality) and 2 (malicious archives).

#[path = "spec_fixtures/mod.rs"]
mod fx;

use fx::{RawEntry, TempDir, raw_zip, roomy, stored_zip};
use pin_assets::{AssetError, Vfs, ZipLimits};

fn open(bytes: Vec<u8>) -> Result<Vfs, AssetError> {
    Vfs::from_zip_bytes(bytes, ZipLimits::default())
}

fn open_err(bytes: Vec<u8>, limits: ZipLimits) -> AssetError {
    match Vfs::from_zip_bytes(bytes, limits) {
        Ok(v) => panic!("hostile archive was accepted; files: {:?}", v.list()),
        Err(e) => e,
    }
}

/// A hostile entry next to harmless ones, in every position.
fn with_hostile(entry: RawEntry) -> Vec<Vec<u8>> {
    let a = RawEntry::file("table.toml", b"x = 1\n");
    let b = RawEntry::file("audio/a.ogg", b"OggS");
    vec![
        raw_zip(std::slice::from_ref(&entry)),
        raw_zip(&[entry.clone(), a.clone(), b.clone()]),
        raw_zip(&[a.clone(), entry.clone(), b.clone()]),
        raw_zip(&[a, b, entry]),
    ]
}

// --------------------------- defaults ---------------------------------------

#[test]
fn zip_limits_default_values() {
    let d = ZipLimits::default();
    assert_eq!(d.max_total_uncompressed, 512 * 1024 * 1024);
    assert_eq!(d.max_file, 256 * 1024 * 1024);
    assert_eq!(d.max_ratio, 100);
    assert_eq!(d.max_entries, 10_000);
}

#[test]
fn asset_error_is_a_std_error_with_display_and_debug() {
    fn assert_error<E: std::error::Error + std::fmt::Debug + std::fmt::Display>(_: &E) {}
    let all = [
        AssetError::AbsolutePath("/etc/x".into()),
        AssetError::PathTraversal("../x".into()),
        AssetError::Symlink("l".into()),
        AssetError::TooLarge {
            path: "big".into(),
            size: 9,
        },
        AssetError::TotalTooLarge(10),
        AssetError::CompressionRatio {
            path: "bomb".into(),
            ratio: 1000,
        },
        AssetError::TooManyEntries(11),
        AssetError::DuplicateEntry("a/b".into()),
        AssetError::Encrypted("e".into()),
        AssetError::BadName,
        AssetError::NotFound("nope".into()),
        AssetError::Io("io".into()),
        AssetError::Zip("zip".into()),
        AssetError::Gltf("gltf".into()),
        AssetError::Invalid(vec![pin_assets::ValidationError {
            file: "table.toml".into(),
            path: "flippers[0].coil".into(),
            message: "unknown coil".into(),
        }]),
    ];
    for e in &all {
        assert_error(e);
        assert!(!e.to_string().is_empty(), "{e:?} has an empty Display");
        assert!(!format!("{e:?}").is_empty());
    }
    // Messages are actionable: they carry the offending path.
    assert!(all[0].to_string().contains("/etc/x"));
    assert!(all[1].to_string().contains("../x"));
    assert!(all[3].to_string().contains("big"));
    assert!(all[7].to_string().contains("a/b"));
    assert!(all[10].to_string().contains("nope"));
    assert!(all[14].to_string().contains("flippers[0].coil"));
}

#[test]
fn validation_error_display_format() {
    let e = pin_assets::ValidationError {
        file: "table.toml".into(),
        path: "flippers[0].coil".into(),
        message: "unknown coil \"x\"".into(),
    };
    assert_eq!(
        e.to_string(),
        "table.toml: flippers[0].coil: unknown coil \"x\""
    );
    assert_eq!(e.clone(), e);
}

// --------------------------- benign archives --------------------------------

#[test]
fn zip_reads_lists_and_exists() {
    let v = open(stored_zip(&[
        ("table.toml", b"a = 1\n"),
        ("sub/dir/z.bin", &[0u8, 1, 2, 255]),
        ("audio/a.ogg", b"OggS"),
    ]))
    .unwrap();
    assert_eq!(
        v.list(),
        vec![
            "audio/a.ogg".to_owned(),
            "sub/dir/z.bin".to_owned(),
            "table.toml".to_owned()
        ]
    );
    assert_eq!(v.read("table.toml").unwrap(), b"a = 1\n");
    assert_eq!(v.read("sub/dir/z.bin").unwrap(), vec![0u8, 1, 2, 255]);
    assert!(v.exists("audio/a.ogg"));
    assert!(!v.exists("audio/b.ogg"));
    assert!(
        !v.exists("audio"),
        "exists() is about files, list() has no dirs"
    );
    assert!(
        matches!(v.read("missing.txt"), Err(AssetError::NotFound(p)) if p.contains("missing.txt"))
    );
}

#[test]
fn zip_deflated_roundtrip_and_empty_file() {
    let big: Vec<u8> = (0..50_000u32).map(|i| (i * 31 % 251) as u8).collect();
    let bytes = fx::deflated_zip([
        ("table.toml", b"a = 1\n".as_slice()),
        ("data/big.bin", big.as_slice()),
        ("empty.txt", b"".as_slice()),
    ]);
    let v = open(bytes).unwrap();
    assert_eq!(v.read("data/big.bin").unwrap(), big);
    assert_eq!(v.read("empty.txt").unwrap(), Vec::<u8>::new());
    assert_eq!(v.list().len(), 3);
}

#[test]
fn zip_directory_entries_are_not_listed_as_files() {
    let mut dir = RawEntry::file("audio/", b"");
    dir.external_attrs = (0o040755 << 16) | 0x10;
    let v = open(raw_zip(&[
        dir,
        RawEntry::file("audio/a.ogg", b"OggS"),
        RawEntry::file("table.toml", b"a=1"),
    ]))
    .unwrap();
    assert_eq!(
        v.list(),
        vec!["audio/a.ogg".to_owned(), "table.toml".to_owned()]
    );
}

#[test]
fn zip_single_top_level_folder_is_stripped() {
    let v = open(stored_zip(&[
        ("apex_circuit/table.toml", b"a = 1\n"),
        ("apex_circuit/backglass.toml", b"b = 2\n"),
        ("apex_circuit/audio/a.ogg", b"OggS"),
    ]))
    .unwrap();
    assert_eq!(
        v.list(),
        vec![
            "audio/a.ogg".to_owned(),
            "backglass.toml".to_owned(),
            "table.toml".to_owned()
        ]
    );
    assert_eq!(v.read("table.toml").unwrap(), b"a = 1\n");
    assert_eq!(v.read("audio/a.ogg").unwrap(), b"OggS");
    assert!(v.exists("backglass.toml"));
}

#[test]
fn zip_root_table_toml_is_not_stripped() {
    let v = open(stored_zip(&[
        ("table.toml", b"root"),
        ("inner/table.toml", b"inner"),
    ]))
    .unwrap();
    assert_eq!(v.read("table.toml").unwrap(), b"root");
    assert_eq!(v.read("inner/table.toml").unwrap(), b"inner");
}

#[test]
fn garbage_is_a_zip_error_not_a_panic() {
    for bytes in [
        Vec::new(),
        b"PK".to_vec(),
        b"this is not a zip archive at all, just text".to_vec(),
        vec![0u8; 4096],
    ] {
        match Vfs::from_zip_bytes(bytes, ZipLimits::default()) {
            Err(AssetError::Zip(_)) | Err(AssetError::Io(_)) => {}
            other => panic!("expected Zip/Io error, got {:?}", other.map(|v| v.list())),
        }
    }
    // Truncated real archive.
    let good = stored_zip(&[("table.toml", b"a = 1\n")]);
    for cut in [good.len() - 1, good.len() / 2, 10] {
        assert!(Vfs::from_zip_bytes(good[..cut].to_vec(), ZipLimits::default()).is_err());
    }
}

// --------------------------- criterion 2: malicious archives ----------------

#[test]
fn rejects_path_traversal() {
    for name in [
        "../x",
        "a/../../x",
        "..\\x",
        "..",
        "a/..",
        "a/b/../../../x",
        "a\\..\\..\\x",
        "audio/../../etc/passwd",
    ] {
        for bytes in with_hostile(RawEntry::file(name, b"evil")) {
            let e = open_err(bytes, ZipLimits::default());
            assert!(
                matches!(e, AssetError::PathTraversal(_)),
                "{name:?}: expected PathTraversal, got {e:?}"
            );
        }
    }
}

#[test]
fn rejects_absolute_paths() {
    for name in [
        "/etc/x",
        "C:\\x",
        "C:/x",
        "\\\\server\\x",
        "/x",
        "c:\\windows\\system32\\x",
        "\\x",
        "//server/share/x",
    ] {
        for bytes in with_hostile(RawEntry::file(name, b"evil")) {
            let e = open_err(bytes, ZipLimits::default());
            assert!(
                matches!(e, AssetError::AbsolutePath(_)),
                "{name:?}: expected AbsolutePath, got {e:?}"
            );
        }
    }
}

#[test]
fn rejects_symlink_entries() {
    for (name, target) in [
        ("link", "/etc/passwd"),
        ("audio/link.ogg", "../../secret"),
        ("harmless", "table.toml"),
    ] {
        for bytes in with_hostile(RawEntry::symlink(name, target)) {
            let e = open_err(bytes, ZipLimits::default());
            assert!(
                matches!(e, AssetError::Symlink(_)),
                "{name:?}: expected Symlink, got {e:?}"
            );
        }
    }
}

#[test]
fn rejects_symlink_written_by_zip_crate() {
    use std::io::Write;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    w.start_file("table.toml", opts).unwrap();
    w.write_all(b"a = 1\n").unwrap();
    w.add_symlink("passwd", "/etc/passwd", opts).unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let e = open_err(bytes, ZipLimits::default());
    assert!(
        matches!(e, AssetError::Symlink(ref p) if p.contains("passwd")),
        "{e:?}"
    );
}

#[test]
fn rejects_encrypted_entries() {
    for bytes in with_hostile(RawEntry::encrypted("secret.bin", b"0123456789abcdef")) {
        let e = open_err(bytes, ZipLimits::default());
        assert!(
            matches!(e, AssetError::Encrypted(ref p) if p.contains("secret.bin")),
            "expected Encrypted, got {e:?}"
        );
    }
}

fn non_utf8_names(flags: u16) {
    for raw in [
        vec![b'a', 0xFF, 0xFE, b'.', b'x'],
        vec![0xC3, 0x28],
        vec![b'd', b'/', 0x80, b'/', b'f'],
    ] {
        let mut e = RawEntry::file("placeholder", b"data");
        e.name = raw.clone();
        e.flags = flags;
        for bytes in with_hostile(e.clone()) {
            let err = open_err(bytes, ZipLimits::default());
            assert_eq!(
                err,
                AssetError::BadName,
                "name bytes {raw:?} flags {flags:#x}"
            );
        }
    }
}

/// Entry flagged "name is UTF-8" (bit 11) whose name is not valid UTF-8.
#[test]
fn rejects_non_utf8_names_flagged_utf8() {
    non_utf8_names(0x0800);
}

/// Same bytes without the UTF-8 flag (a legacy code-page name): still not
/// UTF-8, so the brief's "non-UTF-8 names" cap applies.
#[test]
fn rejects_non_utf8_names_legacy_codepage() {
    non_utf8_names(0);
}

#[test]
fn rejects_duplicates_after_normalisation() {
    let cases: &[&[&str]] = &[
        &["a/b", "a//b"],
        &["a/b", "./a/b"],
        &["a//b", "./a/b"],
        &["a/b", "a/b"],
        &["a/b", "a\\b"],
        &["a/b", "a/./b"],
        &["table.toml", "x", "./table.toml"],
    ];
    for names in cases {
        let entries: Vec<RawEntry> = names.iter().map(|n| RawEntry::file(n, b"data")).collect();
        let e = open_err(raw_zip(&entries), ZipLimits::default());
        assert!(
            matches!(e, AssetError::DuplicateEntry(_)),
            "{names:?}: expected DuplicateEntry, got {e:?}"
        );
    }
}

#[test]
fn normalised_names_are_served_when_unique() {
    let v = open(raw_zip(&[
        RawEntry::file("./table.toml", b"t"),
        RawEntry::file("a//b.txt", b"b"),
        RawEntry::file("c\\d.txt", b"d"),
    ]))
    .unwrap();
    assert_eq!(
        v.list(),
        vec![
            "a/b.txt".to_owned(),
            "c/d.txt".to_owned(),
            "table.toml".to_owned()
        ]
    );
    assert_eq!(v.read("a/b.txt").unwrap(), b"b");
    assert_eq!(v.read("c/d.txt").unwrap(), b"d");
}

#[test]
fn rejects_oversize_file() {
    let data = vec![7u8; 1025];
    let limits = ZipLimits {
        max_file: 1024,
        ..roomy()
    };
    let e = open_err(
        stored_zip(&[("table.toml", b"ok"), ("blob/big.bin", &data)]),
        limits,
    );
    assert_eq!(
        e,
        AssetError::TooLarge {
            path: "blob/big.bin".into(),
            size: 1025
        }
    );
    // At the cap is fine.
    let v = Vfs::from_zip_bytes(
        stored_zip(&[("table.toml", b"ok"), ("blob/big.bin", &data[..1024])]),
        ZipLimits {
            max_file: 1024,
            ..roomy()
        },
    )
    .expect("a file of exactly max_file bytes is allowed");
    assert_eq!(v.read("blob/big.bin").unwrap().len(), 1024);
}

#[test]
fn rejects_total_oversize() {
    let chunk = vec![1u8; 400];
    let files: Vec<(String, &[u8])> = (0..3).map(|i| (format!("f{i}.bin"), &chunk[..])).collect();
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), *d)).collect();
    let limits = ZipLimits {
        max_total_uncompressed: 1199,
        max_file: 400,
        ..roomy()
    };
    let e = open_err(stored_zip(&refs), limits);
    assert!(
        matches!(e, AssetError::TotalTooLarge(n) if n > 1199),
        "expected TotalTooLarge(>1199), got {e:?}"
    );
    let ok = Vfs::from_zip_bytes(
        stored_zip(&refs),
        ZipLimits {
            max_total_uncompressed: 1200,
            max_file: 400,
            ..roomy()
        },
    );
    assert!(
        ok.is_ok(),
        "a total of exactly max_total_uncompressed is allowed"
    );
}

#[test]
fn rejects_zip_bomb_by_ratio() {
    // 4 MiB of zeros deflates to a few kB: ratio about 1000:1.
    let zeros = vec![0u8; 4 * 1024 * 1024];
    let bytes = fx::deflated_zip([
        ("table.toml", b"a = 1\n".as_slice()),
        ("bomb.bin", zeros.as_slice()),
    ]);
    assert!(bytes.len() < zeros.len() / 200, "fixture is not a bomb");
    let e = open_err(bytes.clone(), ZipLimits::default());
    match e {
        AssetError::CompressionRatio { ref path, ratio } => {
            assert_eq!(path, "bomb.bin");
            assert!(ratio > 100, "reported ratio {ratio} must exceed the cap");
        }
        other => panic!("expected CompressionRatio, got {other:?}"),
    }
    // The same archive passes when the ratio cap is lifted: the cap is what tripped.
    let v = Vfs::from_zip_bytes(bytes, roomy()).unwrap();
    assert_eq!(v.read("bomb.bin").unwrap().len(), zeros.len());
}

#[test]
fn ordinary_compression_is_not_a_bomb() {
    // Text that compresses ~3-10x must load under the default 100:1 cap.
    let mut text = String::new();
    for i in 0..4000 {
        text.push_str(&format!("[[switches]]\nid = \"sw_{i}\"\nnumber = {i}\n"));
    }
    let v = open(fx::deflated_zip([("table.toml", text.as_bytes())])).unwrap();
    assert_eq!(v.read("table.toml").unwrap(), text.as_bytes());
}

#[test]
fn rejects_too_many_entries() {
    let names: Vec<String> = (0..6).map(|i| format!("f{i}.txt")).collect();
    let refs: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    let e = open_err(
        stored_zip(&refs),
        ZipLimits {
            max_entries: 5,
            ..roomy()
        },
    );
    assert_eq!(e, AssetError::TooManyEntries(6));
    assert!(
        Vfs::from_zip_bytes(
            stored_zip(&refs),
            ZipLimits {
                max_entries: 6,
                ..roomy()
            }
        )
        .is_ok(),
        "exactly max_entries entries are allowed"
    );
}

#[test]
fn default_entry_cap_is_enforced() {
    let names: Vec<String> = (0..10_001).map(|i| format!("f{i}")).collect();
    let refs: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
    let e = open_err(stored_zip(&refs), ZipLimits::default());
    assert_eq!(e, AssetError::TooManyEntries(10_001));
}

/// The header's uncompressed size is attacker-controlled: an entry that
/// declares 16 bytes but inflates to 2 MiB must never be handed out in full.
#[test]
fn lying_uncompressed_size_never_yields_more_than_declared() {
    let zeros = vec![0u8; 2 * 1024 * 1024];
    let mut bytes = fx::deflated_zip([("bomb.bin", zeros.as_slice())]);
    let real = (zeros.len() as u32).to_le_bytes();
    let mut patched = 0;
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4] == real {
            bytes[i..i + 4].copy_from_slice(&16u32.to_le_bytes());
            patched += 1;
            i += 4;
        } else {
            i += 1;
        }
    }
    assert!(patched >= 1, "size field not found in the archive");
    let limits = ZipLimits {
        max_file: 1024,
        max_total_uncompressed: 4096,
        ..ZipLimits::default()
    };
    if let Ok(v) = Vfs::from_zip_bytes(bytes, limits) {
        match v.read("bomb.bin") {
            Err(_) => {}
            Ok(data) => assert!(
                data.len() <= 16,
                "read() returned {} bytes for an entry declared as 16 bytes",
                data.len()
            ),
        }
    }
}

// --------------------------- read() path hygiene ----------------------------

#[test]
fn zip_read_refuses_escaping_requests() {
    let v = open(stored_zip(&[("table.toml", b"t"), ("a/b.txt", b"b")])).unwrap();
    for p in ["../table.toml", "a/../../x", "..", "a/../../table.toml"] {
        assert!(
            matches!(v.read(p), Err(AssetError::PathTraversal(_))),
            "read({p:?}) = {:?}",
            v.read(p)
        );
        assert!(!v.exists(p));
    }
    for p in [
        "/table.toml",
        "/etc/passwd",
        "C:\\x",
        "C:/x",
        "\\\\server\\x",
    ] {
        assert!(
            matches!(v.read(p), Err(AssetError::AbsolutePath(_))),
            "read({p:?}) = {:?}",
            v.read(p)
        );
        assert!(!v.exists(p));
    }
}

// --------------------------- directory VFS ----------------------------------

fn sample_tree() -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut m = std::collections::BTreeMap::new();
    m.insert("table.toml".to_owned(), b"a = 1\n".to_vec());
    m.insert("audio/a.ogg".to_owned(), b"OggS".to_vec());
    m.insert("deep/er/still/x.bin".to_owned(), vec![0u8, 255, 3]);
    m
}

#[test]
fn directory_reads_lists_and_exists() {
    let outer = TempDir::new("dir");
    let root = outer.path().join("bundle");
    fx::write_tree(&root, &sample_tree());
    let v = Vfs::open(&root).unwrap();
    assert_eq!(
        v.list(),
        vec![
            "audio/a.ogg".to_owned(),
            "deep/er/still/x.bin".to_owned(),
            "table.toml".to_owned()
        ]
    );
    assert_eq!(v.read("deep/er/still/x.bin").unwrap(), vec![0u8, 255, 3]);
    assert!(v.exists("audio/a.ogg"));
    assert!(!v.exists("audio/zzz.ogg"));
    assert!(matches!(v.read("nope.txt"), Err(AssetError::NotFound(p)) if p.contains("nope.txt")));
}

#[test]
fn directory_refuses_paths_escaping_the_root() {
    let outer = TempDir::new("escape");
    let root = outer.path().join("bundle");
    fx::write_tree(&root, &sample_tree());
    std::fs::write(outer.path().join("secret.txt"), b"TOP SECRET").unwrap();
    let v = Vfs::open(&root).unwrap();

    for p in [
        "../secret.txt",
        "audio/../../secret.txt",
        "deep/er/../../../secret.txt",
        "..\\secret.txt",
    ] {
        let r = v.read(p);
        assert!(
            matches!(r, Err(AssetError::PathTraversal(_))),
            "read({p:?}) = {r:?}"
        );
        assert!(!v.exists(p), "exists({p:?}) must be false");
    }
    let abs = outer.path().join("secret.txt");
    let abs = abs.to_str().unwrap();
    let r = v.read(abs);
    assert!(
        matches!(r, Err(AssetError::AbsolutePath(_))),
        "read({abs:?}) = {r:?}"
    );
    assert!(!v.exists(abs));
    for p in [
        "/etc/passwd",
        "C:\\Windows\\win.ini",
        "C:/Windows/win.ini",
        "\\\\server\\share\\x",
    ] {
        let r = v.read(p);
        assert!(
            matches!(r, Err(AssetError::AbsolutePath(_))),
            "read({p:?}) = {r:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn directory_refuses_symlinks_resolving_outside_the_root() {
    use std::os::unix::fs::symlink;
    let outer = TempDir::new("symlink");
    let root = outer.path().join("bundle");
    fx::write_tree(&root, &sample_tree());
    std::fs::write(outer.path().join("secret.txt"), b"TOP SECRET").unwrap();
    std::fs::create_dir_all(outer.path().join("outside_dir")).unwrap();
    std::fs::write(outer.path().join("outside_dir/inner.txt"), b"TOP SECRET 2").unwrap();
    symlink(outer.path().join("secret.txt"), root.join("link.txt")).unwrap();
    symlink("../../secret.txt", root.join("audio/rel_link.txt")).unwrap();
    symlink(outer.path().join("outside_dir"), root.join("linked_dir")).unwrap();

    // Either the bundle is refused outright or every escaping read is refused.
    match Vfs::open(&root) {
        Err(e) => assert!(
            matches!(e, AssetError::Symlink(_) | AssetError::PathTraversal(_)),
            "{e:?}"
        ),
        Ok(v) => {
            for p in ["link.txt", "audio/rel_link.txt", "linked_dir/inner.txt"] {
                let r = v.read(p);
                assert!(
                    matches!(
                        r,
                        Err(AssetError::Symlink(_)) | Err(AssetError::PathTraversal(_))
                    ),
                    "read({p:?}) through an escaping symlink = {r:?}"
                );
                assert!(!v.exists(p), "exists({p:?}) must be false");
                assert!(
                    !v.list().iter().any(|f| f == p),
                    "list() must not offer the escaping symlink {p:?}"
                );
            }
            // The honest files still work.
            assert_eq!(v.read("table.toml").unwrap(), b"a = 1\n");
        }
    }
}

// --------------------------- open() dispatch --------------------------------

#[test]
fn open_dispatches_on_directory_zip_and_pinbundle() {
    let outer = TempDir::new("open");
    let tree = sample_tree();
    let root = outer.path().join("bundle");
    fx::write_tree(&root, &tree);
    let bytes = fx::zip_of(&tree);
    let zip_path = outer.path().join("bundle.zip");
    let pb_path = outer.path().join("bundle.pinbundle");
    std::fs::write(&zip_path, &bytes).unwrap();
    std::fs::write(&pb_path, &bytes).unwrap();

    let dir = Vfs::open(&root).unwrap();
    let z = Vfs::open(&zip_path).unwrap();
    let pb = Vfs::open(&pb_path).unwrap();
    let mem = Vfs::from_zip_bytes(bytes, ZipLimits::default()).unwrap();
    let names: Vec<String> = tree.keys().cloned().collect();
    for v in [&dir, &z, &pb, &mem] {
        assert_eq!(v.list(), names);
        for (name, data) in &tree {
            assert_eq!(&v.read(name).unwrap(), data, "{name}");
            assert!(v.exists(name));
        }
    }
}

#[test]
fn open_with_limits_applies_to_zip_files() {
    let outer = TempDir::new("limits");
    let p = outer.path().join("b.pinbundle");
    std::fs::write(&p, stored_zip(&[("a", b"1"), ("b", b"2"), ("c", b"3")])).unwrap();
    let limits = ZipLimits {
        max_entries: 2,
        ..roomy()
    };
    let Err(e) = Vfs::open_with_limits(&p, limits) else {
        panic!("limit must apply");
    };
    assert_eq!(e, AssetError::TooManyEntries(3));
    assert!(Vfs::open_with_limits(&p, roomy()).is_ok());
    assert!(Vfs::open(&p).is_ok());
}

#[test]
fn open_reports_hostile_zip_files_and_missing_paths() {
    let outer = TempDir::new("hostile");
    let p = outer.path().join("evil.zip");
    std::fs::write(&p, raw_zip(&[RawEntry::file("../evil.txt", b"x")])).unwrap();
    assert!(matches!(Vfs::open(&p), Err(AssetError::PathTraversal(_))));
    assert!(
        !outer.path().join("evil.txt").exists()
            && !outer.path().parent().unwrap().join("evil.txt").exists(),
        "nothing may be extracted to disk"
    );
    let missing = outer.path().join("does_not_exist.zip");
    assert!(matches!(
        Vfs::open(&missing),
        Err(AssetError::Io(_)) | Err(AssetError::NotFound(_))
    ));
    let missing_dir = outer.path().join("no_such_dir");
    assert!(matches!(
        Vfs::open(&missing_dir),
        Err(AssetError::Io(_)) | Err(AssetError::NotFound(_))
    ));
}
