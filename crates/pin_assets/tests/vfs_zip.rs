//! VFS and zip hardening (verification criterion 2).

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{AssetError, Vfs, ZipLimits};
use support::*;

fn open(bytes: Vec<u8>) -> Result<Vfs, AssetError> {
    Vfs::from_zip_bytes(bytes, ZipLimits::default())
}

#[test]
fn traversal_entries_are_rejected() {
    for name in ["../x", "a/../../x", "..\\x", "a/..", "ok/../x"] {
        let r = open(stored_zip(&[(name, b"x")]));
        assert_eq!(r.err(), Some(AssetError::PathTraversal(name.into())), "{name}");
    }
}

#[test]
fn absolute_entries_are_rejected() {
    for name in ["/etc/x", "C:\\x", "C:/x", "\\\\server\\x", "\\x", "//server/x"] {
        let r = open(stored_zip(&[(name, b"x")]));
        assert_eq!(r.err(), Some(AssetError::AbsolutePath(name.into())), "{name}");
    }
}

#[test]
fn symlink_entries_are_rejected() {
    let r = open(raw_zip(&[
        RawEntry::file("table.toml", b"x"),
        RawEntry::symlink("link", "/etc/passwd"),
    ]));
    assert_eq!(r.err(), Some(AssetError::Symlink("link".into())));
}

#[test]
fn encrypted_entries_are_rejected() {
    let mut e = RawEntry::file("secret.lua", b"x");
    e.flags |= 1;
    assert_eq!(
        open(raw_zip(&[e])).err(),
        Some(AssetError::Encrypted("secret.lua".into()))
    );
}

#[test]
fn bad_names_are_rejected() {
    let mut e = RawEntry::file("x", b"x");
    e.name = vec![b'a', 0xff, 0xfe];
    assert_eq!(open(raw_zip(&[e])).err(), Some(AssetError::BadName));
    let e = RawEntry::file("a\0b", b"x");
    assert_eq!(open(raw_zip(&[e])).err(), Some(AssetError::BadName));
    let e = RawEntry::file(".", b"x");
    assert_eq!(open(raw_zip(&[e])).err(), Some(AssetError::BadName));
}

#[test]
fn duplicates_after_normalisation_are_rejected() {
    for pair in [["a/b", "a//b"], ["a/b", "./a/b"], ["a//b", "./a/b"], ["a/b", "a\\b"]] {
        let r = open(stored_zip(&[(pair[0], b"1"), (pair[1], b"2")]));
        assert_eq!(r.err(), Some(AssetError::DuplicateEntry("a/b".into())), "{pair:?}");
    }
    let r = open(stored_zip(&[("a", b"1"), ("a", b"2")]));
    assert!(matches!(r, Err(AssetError::DuplicateEntry(_))), "{r:?}");
}

#[test]
fn oversize_file_is_rejected() {
    let limits = ZipLimits {
        max_file: 10,
        ..ZipLimits::default()
    };
    let r = Vfs::from_zip_bytes(stored_zip(&[("big", &[7u8; 11])]), limits);
    assert_eq!(
        r.err(),
        Some(AssetError::TooLarge {
            path: "big".into(),
            size: 11
        })
    );
    // The declared size is checked, without decompressing.
    let mut e = RawEntry::file("liar", b"x");
    e.declared_size = Some(u32::MAX);
    e.declared_compressed = Some(u32::MAX / 50);
    let r = open(raw_zip(&[e]));
    assert!(matches!(r, Err(AssetError::TooLarge { size: 4_294_967_295, .. })), "{r:?}");
}

#[test]
fn total_oversize_is_rejected() {
    let limits = ZipLimits {
        max_total_uncompressed: 25,
        ..ZipLimits::default()
    };
    let d = [1u8; 10];
    let r = Vfs::from_zip_bytes(stored_zip(&[("a", &d), ("b", &d), ("c", &d)]), limits);
    assert_eq!(r.err(), Some(AssetError::TotalTooLarge(30)));
    assert!(Vfs::from_zip_bytes(stored_zip(&[("a", &d), ("b", &d)]), limits).is_ok());
}

#[test]
fn zip_bomb_ratio_is_rejected() {
    let zeros = vec![0u8; 1 << 20];
    let r = open(deflated_zip(&[("bomb.bin", &zeros)]));
    match r {
        Err(AssetError::CompressionRatio { path, ratio }) => {
            assert_eq!(path, "bomb.bin");
            assert!(ratio > 100);
        }
        other => panic!("{other:?}"),
    }
    // A lax limit lets it through, and it reads back exactly.
    let limits = ZipLimits {
        max_ratio: 100_000,
        ..ZipLimits::default()
    };
    let vfs = Vfs::from_zip_bytes(deflated_zip(&[("bomb.bin", &zeros)]), limits).unwrap();
    assert_eq!(vfs.read("bomb.bin").unwrap(), zeros);
}

#[test]
fn too_many_entries_is_rejected() {
    let names: Vec<String> = (0..11).map(|i| format!("f{i}")).collect();
    let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
    let limits = ZipLimits {
        max_entries: 10,
        ..ZipLimits::default()
    };
    assert_eq!(
        Vfs::from_zip_bytes(stored_zip(&files), limits).err(),
        Some(AssetError::TooManyEntries(11))
    );
    assert!(Vfs::from_zip_bytes(stored_zip(&files[..10]), limits).is_ok());
}

#[test]
fn understated_size_is_caught_on_read() {
    let mut e = RawEntry::file("short", b"hello world");
    e.declared_size = Some(5);
    e.declared_compressed = Some(11);
    // Stored entries are read by compressed size; the mismatch is an error.
    let vfs = open(raw_zip(&[e])).unwrap();
    assert!(matches!(vfs.read("short"), Err(AssetError::Zip(_))));
}

#[test]
fn garbage_and_unsupported_methods() {
    assert!(matches!(open(b"PK\x03\x04garbage".to_vec()), Err(AssetError::Zip(_))));
    assert!(matches!(open(Vec::new()), Err(AssetError::Zip(_))));
}

#[test]
fn zip_read_list_exists() {
    let vfs = open(deflated_zip(&[
        ("table.toml", b"t"),
        ("dir/", b""),
        ("audio/a.ogg", b"ogg"),
        ("./b.txt", b"b"),
    ]))
    .unwrap();
    assert_eq!(vfs.list(), vec!["audio/a.ogg", "b.txt", "table.toml"]);
    assert_eq!(vfs.read("audio/a.ogg").unwrap(), b"ogg");
    assert_eq!(vfs.read("audio//a.ogg").unwrap(), b"ogg");
    assert_eq!(vfs.read("./audio\\a.ogg").unwrap(), b"ogg");
    assert!(vfs.exists("b.txt"));
    assert!(!vfs.exists("dir"));
    assert!(!vfs.exists("../b.txt"));
    assert!(!vfs.exists(""));
    assert_eq!(vfs.read("nope").err(), Some(AssetError::NotFound("nope".into())));
    assert_eq!(vfs.read("").err(), Some(AssetError::NotFound("".into())));
    assert_eq!(
        vfs.read("../table.toml").err(),
        Some(AssetError::PathTraversal("../table.toml".into()))
    );
    assert_eq!(
        vfs.read("/table.toml").err(),
        Some(AssetError::AbsolutePath("/table.toml".into()))
    );
}

#[test]
fn single_wrapping_folder_is_stripped() {
    let vfs = open(stored_zip(&[("pkg/table.toml", b"t"), ("pkg/x/y", b"y")])).unwrap();
    assert_eq!(vfs.list(), vec!["table.toml", "x/y"]);
    // Not stripped when table.toml is at the root or content sits outside.
    let vfs = open(stored_zip(&[("table.toml", b"t"), ("pkg/table.toml", b"y")])).unwrap();
    assert_eq!(vfs.list(), vec!["pkg/table.toml", "table.toml"]);
    let vfs = open(stored_zip(&[("pkg/table.toml", b"t"), ("other", b"y")])).unwrap();
    assert_eq!(vfs.list(), vec!["other", "pkg/table.toml"]);
}

#[test]
fn open_paths() {
    let dir = temp_dir("open");
    let zip_path = dir.join("t.PINBUNDLE");
    std::fs::write(&zip_path, stored_zip(&[("table.toml", b"t")])).unwrap();
    assert_eq!(Vfs::open(&zip_path).unwrap().list(), vec!["table.toml"]);
    let other = dir.join("t.rar");
    std::fs::write(&other, b"x").unwrap();
    assert!(matches!(Vfs::open(&other), Err(AssetError::Io(_))));
    assert!(matches!(Vfs::open(&dir.join("missing.zip")), Err(AssetError::Io(_))));
    let big = dir.join("big.zip");
    std::fs::write(&big, vec![0u8; 4 << 20]).unwrap();
    let tiny = ZipLimits {
        max_total_uncompressed: 1,
        ..ZipLimits::default()
    };
    assert_eq!(
        Vfs::open_with_limits(&big, tiny).err(),
        Some(AssetError::TotalTooLarge(4 << 20))
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn directory_vfs() {
    let dir = temp_dir("dirvfs");
    write_dir(
        &dir,
        &[
            ("table.toml".into(), b"t".to_vec()),
            ("audio/a.ogg".into(), b"ogg".to_vec()),
            ("big.bin".into(), vec![1u8; 100]),
        ],
    );
    let vfs = Vfs::open(&dir).unwrap();
    assert_eq!(vfs.list(), vec!["audio/a.ogg", "big.bin", "table.toml"]);
    assert_eq!(vfs.read("audio/a.ogg").unwrap(), b"ogg");
    assert_eq!(vfs.read(".\\audio//a.ogg").unwrap(), b"ogg");
    assert!(vfs.exists("table.toml"));
    assert!(!vfs.exists("audio"));
    assert!(!vfs.exists("missing"));
    assert_eq!(vfs.read("missing").err(), Some(AssetError::NotFound("missing".into())));
    assert_eq!(vfs.read("audio").err(), Some(AssetError::NotFound("audio".into())));
    assert_eq!(
        vfs.read("../outside").err(),
        Some(AssetError::PathTraversal("../outside".into()))
    );
    assert_eq!(
        vfs.read("C:/x").err(),
        Some(AssetError::AbsolutePath("C:/x".into()))
    );
    let small = ZipLimits {
        max_file: 10,
        ..ZipLimits::default()
    };
    let vfs = Vfs::open_with_limits(&dir, small).unwrap();
    assert_eq!(
        vfs.read("big.bin").err(),
        Some(AssetError::TooLarge {
            path: "big.bin".into(),
            size: 100
        })
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn directory_wrapper_is_stripped() {
    let dir = temp_dir("dirwrap");
    write_dir(&dir, &[("pkg/table.toml".into(), b"t".to_vec())]);
    let vfs = Vfs::open(&dir).unwrap();
    assert_eq!(vfs.list(), vec!["table.toml"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn directory_symlinks_cannot_escape() {
    use std::os::unix::fs::symlink;
    let outside = temp_dir("outside");
    std::fs::write(outside.join("secret"), b"s").unwrap();
    let dir = temp_dir("dirlink");
    write_dir(&dir, &[("table.toml".into(), b"t".to_vec())]);
    symlink(outside.join("secret"), dir.join("escape")).unwrap();
    symlink(&outside, dir.join("escape_dir")).unwrap();
    symlink(dir.join("table.toml"), dir.join("inside")).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    assert_eq!(vfs.read("escape").err(), Some(AssetError::PathTraversal("escape".into())));
    assert_eq!(
        vfs.read("escape_dir/secret").err(),
        Some(AssetError::PathTraversal("escape_dir/secret".into()))
    );
    assert!(!vfs.exists("escape"));
    assert_eq!(vfs.read("inside").unwrap(), b"t");
    assert_eq!(vfs.list(), vec!["inside", "table.toml"]);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

#[test]
fn error_display_is_informative() {
    let all = [
        AssetError::AbsolutePath("/x".into()),
        AssetError::PathTraversal("../x".into()),
        AssetError::Symlink("l".into()),
        AssetError::TooLarge {
            path: "p".into(),
            size: 1,
        },
        AssetError::TotalTooLarge(1),
        AssetError::CompressionRatio {
            path: "p".into(),
            ratio: 1000,
        },
        AssetError::TooManyEntries(3),
        AssetError::DuplicateEntry("d".into()),
        AssetError::Encrypted("e".into()),
        AssetError::BadName,
        AssetError::NotFound("n".into()),
        AssetError::Io("io".into()),
        AssetError::Zip("z".into()),
        AssetError::Gltf("g".into()),
        AssetError::Invalid(vec![pin_assets::ValidationError::new("t", "p", "m")]),
    ];
    for e in all {
        assert!(!e.to_string().is_empty());
        let _: &dyn std::error::Error = &e;
    }
    let io: AssetError = std::io::Error::other("boom").into();
    assert_eq!(io, AssetError::Io("boom".into()));
}
