//! Headless directory-scan tests (4.4 verification: "directory scanning").

#[path = "ui_fixtures.rs"]
mod fixtures;

use fixtures::*;
use pin_app::ui::{
    MenuInput, MenuState, TableFeature, TableStatus, decode_preview, inspect_bundle, scan_tables,
};

/// A tables folder with valid, broken and ignored candidates.
fn populated_root() -> std::path::PathBuf {
    let root = temp_dir("scan");
    let files = bundle_files();
    // Valid folder with a preview (red/green 2x1).
    let with_preview = with_file(
        &files,
        "preview.png",
        &png(2, 1, |x, _| {
            if x == 0 {
                [255, 0, 0, 255]
            } else {
                [0, 255, 0, 255]
            }
        }),
    );
    write_dir(&root.join("b_apex"), &with_preview);
    // Valid folder, renamed table, no preview; upper-case name sorts by
    // lower-cased key.
    write_dir(
        &root.join("A_second"),
        &edit_table(&files, "name = \"Apex Circuit\"", "name = \"Second Table\""),
    );
    // Broken: manifest is not TOML.
    write_dir(
        &root.join("c_broken"),
        &with_file(&files, "table.toml", b"this is [not toml"),
    );
    // Broken archive.
    std::fs::write(root.join("d_garbage.zip"), b"PK not really a zip").unwrap();
    // Valid stored archive.
    std::fs::write(root.join("e_packed.pinbundle"), stored_zip(&files)).unwrap();
    // Ignored: hidden folder, plain file, other extension.
    write_dir(&root.join(".hidden"), &files);
    std::fs::write(root.join("notes.txt"), b"hello").unwrap();
    std::fs::write(root.join("f.tar"), b"x").unwrap();
    // Valid folder with a broken preview.
    write_dir(
        &root.join("g_badpreview"),
        &with_file(&files, "preview.png", b"\x89PNG broken"),
    );
    root
}

#[test]
fn scan_lists_candidates_sorted_with_status() {
    let root = populated_root();
    let entries = scan_tables(&root);
    let names: Vec<&str> = entries.iter().map(|e| e.file_name.as_str()).collect();
    assert_eq!(
        names,
        [
            "A_second",
            "b_apex",
            "c_broken",
            "d_garbage.zip",
            "e_packed.pinbundle",
            "g_badpreview"
        ]
    );
    let ready: Vec<bool> = entries.iter().map(|e| e.is_ready()).collect();
    assert_eq!(ready, [true, true, false, false, true, true]);
    assert_eq!(entries[0].title(), "Second Table");
    assert_eq!(entries[1].title(), "Apex Circuit");
    // Broken entries keep their file name as title and expose the error.
    assert_eq!(entries[2].title(), "c_broken");
    assert!(
        entries[2].error().is_some_and(|e| e.contains("table.toml")),
        "{:?}",
        entries[2].error()
    );
    assert!(entries[3].error().is_some());
    for e in &entries {
        assert_eq!(e.path, root.join(&e.file_name));
        assert_eq!(e.sort_key().0, e.file_name.to_ascii_lowercase());
    }
    // Deterministic: a second scan is identical.
    assert_eq!(scan_tables(&root), entries);
}

#[test]
fn metadata_badges_and_preview() {
    let root = populated_root();
    let entries = scan_tables(&root);
    let info = entries[1].info().unwrap();
    assert_eq!(info.id, "apex_circuit");
    assert_eq!(info.author, "Reference");
    assert_eq!(info.version, "1.0.0");
    assert_eq!(info.players_max, 4);
    for f in [
        TableFeature::LuaRules,
        TableFeature::Ramps,
        TableFeature::Magnets,
        TableFeature::DmdDisplay,
        TableFeature::Dof,
    ] {
        assert!(
            info.features.contains(&f),
            "{f:?} missing in {:?}",
            info.features
        );
    }
    assert!(!info.features.contains(&TableFeature::LcdDisplay));
    let mut sorted = info.features.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, info.features);
    for f in &info.features {
        assert!(!f.label().is_empty());
    }
    let art = info.preview.as_ref().expect("preview");
    assert_eq!(
        (art.file.as_str(), art.width, art.height),
        ("preview.png", 2, 1)
    );
    assert_eq!(art.rgba, [255, 0, 0, 255, 0, 255, 0, 255]);
    assert!(info.preview_error.is_none());
    // No preview file: none, no error.
    let second = entries[0].info().unwrap();
    assert!(second.preview.is_none() && second.preview_error.is_none());
    // Broken preview: the table still loads, the error is noted.
    let bad = entries[5].info().unwrap();
    assert!(bad.preview.is_none());
    assert!(
        bad.preview_error
            .as_deref()
            .is_some_and(|e| e.contains("preview.png"))
    );
    // The packed archive has the same metadata as the folder.
    assert_eq!(entries[4].info().unwrap().features, info.features);
}

#[test]
fn feature_badges_follow_the_manifest() {
    let root = temp_dir("lcd");
    let files = bundle_files();
    let bg = files
        .iter()
        .find(|(n, _)| n == "backglass.toml")
        .map(|(_, d)| String::from_utf8(d.clone()).unwrap())
        .unwrap();
    let lcd = with_file(
        &files,
        "backglass.toml",
        bg.replacen("mode = \"dmd\"", "mode = \"lcd\"", 1)
            .as_bytes(),
    );
    // The reference backglass already carries a valid `lcd` block, so the
    // LCD variant is a known-valid bundle and must load.
    assert!(bg.contains("lcd = { width = 1920, height = 1080 }"));
    write_dir(&root, &lcd);
    match inspect_bundle(&root) {
        TableStatus::Ready(info) => {
            assert!(
                info.features.contains(&TableFeature::LcdDisplay),
                "{info:?}"
            );
            assert!(!info.features.contains(&TableFeature::DmdDisplay));
        }
        other => panic!("valid LCD bundle not ready: {other:?}"),
    }
}

#[test]
fn missing_or_file_root_is_empty() {
    let root = temp_dir("missing");
    assert!(scan_tables(&root.join("nope")).is_empty());
    std::fs::write(root.join("file"), b"x").unwrap();
    assert!(scan_tables(&root.join("file")).is_empty());
    assert!(scan_tables(&root).is_empty());
}

#[test]
fn preview_is_downscaled_and_bounded() {
    let big = png(600, 300, |x, y| [(x % 256) as u8, (y % 256) as u8, 7, 255]);
    let art = decode_preview(&big).unwrap();
    assert_eq!((art.width, art.height), (256, 128));
    assert_eq!(art.rgba.len(), 256 * 128 * 4);
    let tall = png(10, 1000, |_, _| [9, 9, 9, 255]);
    let art = decode_preview(&tall).unwrap();
    assert_eq!((art.width, art.height), (2, 256));
    assert!(art.rgba.chunks_exact(4).all(|p| p == [9, 9, 9, 255]));
    // Oversized images are refused before decoding.
    let huge = png(4097, 1, |_, _| [0, 0, 0, 255]);
    assert!(decode_preview(&huge).is_err());
    assert!(decode_preview(b"").is_err());
    assert!(decode_preview(b"GIF89a....").is_err());
}

#[test]
fn scanned_tables_drive_the_browser() {
    let root = populated_root();
    let mut m = MenuState::default();
    m.set_tables(scan_tables(&root));
    m.apply(MenuInput::Toggle);
    assert_eq!(m.row_count(), 1 + 6);
    m.apply(MenuInput::SetFilter("APEX".into()));
    // "Apex Circuit" (b_apex, e_packed, g_badpreview titles).
    assert_eq!(m.visible_tables(), &[1, 4, 5]);
    m.apply(MenuInput::SetFilter("garbage".into()));
    assert_eq!(m.visible_tables(), &[3]);
    m.apply(MenuInput::SetFilter("reference".into()));
    assert_eq!(m.visible_tables(), &[0, 1, 4, 5]);
    m.apply(MenuInput::SetFilter(String::new()));
    assert_eq!(m.visible_tables().len(), 6);
    // Focus is kept by path across a rescan.
    m.apply(MenuInput::FocusTable(4));
    assert_eq!(m.focused_table(), Some(4));
    std::fs::remove_dir_all(root.join("A_second")).unwrap();
    m.set_tables(scan_tables(&root));
    assert_eq!(m.focused_table(), Some(3));
    assert_eq!(m.tables()[3].file_name, "e_packed.pinbundle");
}
