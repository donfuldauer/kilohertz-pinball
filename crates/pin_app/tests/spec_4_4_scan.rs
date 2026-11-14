//! Subphase 4.4 spec tests (Independent Tester): table directory scanning.
//!
//! Black-box, written from the brief and the published API
//! (`docs/DECISIONS.md` "4.4 — Menu UI public API"). The oracle for "valid
//! bundle" is `pin_assets::load_table` over `pin_assets::Vfs::open`.
#![allow(clippy::field_reassign_with_default, clippy::unnecessary_cast)]

#[path = "fixtures/spec_4_4/support.rs"]
mod support;

use pin_app::ui::{TableEntry, TableFeature, TableStatus, inspect_bundle, scan_tables};
use proptest::prelude::*;
use std::path::Path;
use support::*;

fn oracle_ok(path: &Path) -> bool {
    pin_assets::Vfs::open(path)
        .and_then(|vfs| pin_assets::load_table(&vfs))
        .is_ok()
}

fn names(entries: &[TableEntry]) -> Vec<String> {
    entries.iter().map(|e| e.file_name.clone()).collect()
}

fn find<'a>(entries: &'a [TableEntry], file_name: &str) -> &'a TableEntry {
    entries
        .iter()
        .find(|e| e.file_name == file_name)
        .unwrap_or_else(|| panic!("no entry {file_name:?} in {:?}", names(entries)))
}

/// Checks every invariant the brief gives for a scan result.
fn check_invariants(root: &Path, entries: &[TableEntry]) {
    for e in entries {
        assert!(
            e.path.starts_with(root),
            "{:?} is outside the scanned root",
            e.path
        );
        assert_eq!(e.is_ready(), e.info().is_some());
        assert_eq!(e.is_ready(), e.error().is_none());
        match &e.status {
            TableStatus::Ready(info) => {
                assert!(e.is_ready());
                assert!(!info.name.is_empty());
                assert!(
                    oracle_ok(&e.path),
                    "{:?} listed Ready but pin_assets rejects it",
                    e.path
                );
            }
            TableStatus::Broken(error) => {
                assert!(!e.is_ready());
                assert!(!error.trim().is_empty(), "broken entry without an error");
            }
        }
    }
    for pair in entries.windows(2) {
        assert!(
            pair[0].sort_key() <= pair[1].sort_key(),
            "entries not sorted by sort_key: {:?} then {:?}",
            pair[0].file_name,
            pair[1].file_name
        );
    }
    let mut paths: Vec<_> = entries.iter().map(|e| e.path.clone()).collect();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), entries.len(), "duplicate entries");
}

#[test]
fn base_fixture_is_a_valid_bundle() {
    assert!(oracle_ok(&base_bundle()));
}

#[test]
fn missing_root_is_empty() {
    let tmp = TempDir::new("missing");
    assert!(scan_tables(&tmp.join("does_not_exist")).is_empty());
}

#[test]
fn root_that_is_a_file_is_empty() {
    let tmp = TempDir::new("rootfile");
    let f = tmp.join("file.txt");
    std::fs::write(&f, b"x").unwrap();
    assert!(scan_tables(&f).is_empty());
}

#[test]
fn empty_root_is_empty() {
    let tmp = TempDir::new("empty");
    assert!(scan_tables(tmp.path()).is_empty());
}

#[test]
fn valid_folder_bundle_is_listed_with_its_metadata() {
    let tmp = TempDir::new("valid");
    write_valid_bundle(&tmp.join("neon"), "neon_run", "Neon Run", "Ada Tester");
    let entries = scan_tables(tmp.path());
    check_invariants(tmp.path(), &entries);
    assert_eq!(entries.len(), 1, "{:?}", names(&entries));
    let e = &entries[0];
    assert_eq!(e.file_name, "neon");
    assert_eq!(e.path, tmp.join("neon"));
    assert!(e.is_ready(), "error: {:?}", e.error());
    let info = e.info().unwrap();
    assert_eq!(info.id, "neon_run");
    assert_eq!(info.name, "Neon Run");
    assert_eq!(info.author, "Ada Tester");
    assert_eq!(info.version, "1.0.0");
    assert_eq!(info.players_max, 4);
    assert_eq!(e.title().to_string(), "Neon Run");
    // rules.lua → the Lua badge, never the Wasm one.
    assert!(info.features.contains(&TableFeature::LuaRules));
    assert!(!info.features.contains(&TableFeature::WasmRules));
    // Badges are sorted and unique, each with a visible label.
    let mut sorted = info.features.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, info.features);
    for f in &info.features {
        assert!(!f.label().is_empty());
    }
}

#[test]
fn features_match_table_feature_of() {
    let vfs = pin_assets::Vfs::open(&base_bundle()).unwrap();
    let loaded = pin_assets::load_table(&vfs).unwrap();
    let expected = TableFeature::of(&loaded);
    match inspect_bundle(&base_bundle()) {
        TableStatus::Ready(info) => assert_eq!(info.features, expected),
        TableStatus::Broken(e) => panic!("base bundle reported broken: {e}"),
    }
}

#[test]
fn archive_bundles_are_listed() {
    let tmp = TempDir::new("zip");
    let src = TempDir::new("zipsrc");
    write_valid_bundle(&src.join("b"), "zipped", "Zipped Table", "Z");
    zip_dir(&src.join("b"), &tmp.join("one.pinbundle"));
    zip_dir(&src.join("b"), &tmp.join("two.zip"));
    assert!(oracle_ok(&tmp.join("one.pinbundle")), "fixture zip invalid");
    let entries = scan_tables(tmp.path());
    check_invariants(tmp.path(), &entries);
    assert_eq!(entries.len(), 2, "{:?}", names(&entries));
    for name in ["one.pinbundle", "two.zip"] {
        let e = find(&entries, name);
        assert!(e.is_ready(), "{name}: {:?}", e.error());
        assert_eq!(e.info().unwrap().name, "Zipped Table");
    }
}

#[test]
fn broken_bundles_are_reported_with_their_error() {
    let tmp = TempDir::new("broken");
    write_valid_bundle(&tmp.join("good"), "good", "Good", "G");
    write_garbage_manifest_bundle(&tmp.join("garbage"));
    write_bad_schema_bundle(&tmp.join("future"));
    write_missing_backglass_bundle(&tmp.join("no_backglass"));
    write_missing_script_bundle(&tmp.join("no_script"));
    write_fake_archive(&tmp.join("fake.pinbundle"));
    write_fake_archive(&tmp.join("fake.zip"));
    std::fs::write(tmp.join("empty.zip"), b"").unwrap();

    let entries = scan_tables(tmp.path());
    check_invariants(tmp.path(), &entries);
    assert!(find(&entries, "good").is_ready());
    for name in [
        "garbage",
        "future",
        "no_backglass",
        "fake.pinbundle",
        "fake.zip",
        "empty.zip",
    ] {
        let e = find(&entries, name);
        assert!(!e.is_ready(), "{name} must be broken");
        assert!(!e.error().unwrap().trim().is_empty());
        assert!(e.info().is_none());
    }
    // Whatever pin_assets says about the bundle without its script is what
    // the browser shows.
    let e = find(&entries, "no_script");
    assert_eq!(e.is_ready(), oracle_ok(&tmp.join("no_script")));
    assert_eq!(
        entries.iter().filter(|e| e.is_ready()).count(),
        1 + usize::from(oracle_ok(&tmp.join("no_script")))
    );
}

#[test]
fn broken_error_names_the_problem() {
    let tmp = TempDir::new("errtext");
    write_bad_schema_bundle(&tmp.join("future"));
    write_missing_backglass_bundle(&tmp.join("no_backglass"));
    let entries = scan_tables(tmp.path());
    let e = find(&entries, "future").error().unwrap().to_string();
    assert!(
        e.contains("9999") || e.to_lowercase().contains("version"),
        "error does not mention the schema version: {e}"
    );
    let e = find(&entries, "no_backglass").error().unwrap().to_string();
    assert!(
        e.contains("backglass"),
        "error does not mention backglass.toml: {e}"
    );
}

#[test]
fn inspect_bundle_agrees_with_scan() {
    let tmp = TempDir::new("inspect");
    write_valid_bundle(&tmp.join("good"), "good", "Good", "G");
    write_garbage_manifest_bundle(&tmp.join("garbage"));
    write_fake_archive(&tmp.join("fake.zip"));
    for e in scan_tables(tmp.path()) {
        assert_eq!(inspect_bundle(&e.path), e.status, "{}", e.file_name);
    }
    // A path that does not exist is broken, not a panic.
    assert!(matches!(
        inspect_bundle(&tmp.join("nope")),
        TableStatus::Broken(_)
    ));
    assert!(matches!(
        inspect_bundle(&tmp.join("nope.zip")),
        TableStatus::Broken(_)
    ));
}

#[test]
fn order_is_deterministic_and_independent_of_creation_order() {
    let ids = ["delta", "alpha", "Charlie", "bravo", "echo_2", "echo_10"];
    let a = TempDir::new("order_a");
    let b = TempDir::new("order_b");
    for id in ids {
        write_valid_bundle(&a.join(id), id, &format!("Table {id}"), "x");
    }
    for id in ids.iter().rev() {
        write_valid_bundle(&b.join(id), id, &format!("Table {id}"), "x");
    }
    write_garbage_manifest_bundle(&a.join("broken_one"));
    write_garbage_manifest_bundle(&b.join("broken_one"));

    let first = scan_tables(a.path());
    check_invariants(a.path(), &first);
    assert_eq!(first.len(), ids.len() + 1);
    for _ in 0..5 {
        assert_eq!(scan_tables(a.path()), first, "scan is not repeatable");
    }
    let other = scan_tables(b.path());
    assert_eq!(names(&first), names(&other));
}

#[test]
fn empty_and_junk_entries_are_never_ready() {
    let tmp = TempDir::new("junk");
    std::fs::create_dir_all(tmp.join("empty_dir")).unwrap();
    std::fs::create_dir_all(tmp.join("only_subdirs/a/b/c")).unwrap();
    std::fs::write(tmp.join("readme.txt"), b"not a table").unwrap();
    std::fs::write(tmp.join("config.toml"), b"wireframe = true\n").unwrap();
    std::fs::write(tmp.join(".DS_Store"), b"\0\0").unwrap();
    // table.toml that is a directory.
    std::fs::create_dir_all(tmp.join("dir_manifest/table.toml")).unwrap();
    let entries = scan_tables(tmp.path());
    check_invariants(tmp.path(), &entries);
    assert!(
        entries.iter().all(|e| !e.is_ready()),
        "{:?}",
        names(&entries)
    );
    // Plain files that are not archives are not tables at all.
    for name in ["readme.txt", "config.toml", ".DS_Store"] {
        assert!(
            entries.iter().all(|e| e.file_name != name),
            "{name} listed as a table"
        );
    }
}

#[test]
fn nested_directories_do_not_break_the_scan() {
    let tmp = TempDir::new("nested");
    write_valid_bundle(&tmp.join("top"), "top", "Top Level", "x");
    write_valid_bundle(&tmp.join("group/inner_a"), "inner_a", "Inner A", "x");
    write_valid_bundle(&tmp.join("group/inner_b"), "inner_b", "Inner B", "x");
    write_valid_bundle(&tmp.join("deep/1/2/3/inner_c"), "inner_c", "Inner C", "x");
    std::fs::create_dir_all(tmp.join("deep/1/empty")).unwrap();

    let entries = scan_tables(tmp.path());
    check_invariants(tmp.path(), &entries);
    let top = find(&entries, "top");
    assert!(top.is_ready());
    assert_eq!(top.info().unwrap().name, "Top Level");
    // No table is listed twice under different entries, and the two scans agree.
    let ready: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.info().map(|i| i.id.as_str()))
        .collect();
    let mut unique = ready.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ready.len(), "a table is listed twice");
    assert_eq!(scan_tables(tmp.path()), entries);
}

#[cfg(unix)]
#[test]
fn hostile_directory_contents_do_not_panic() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = TempDir::new("hostile");
    write_valid_bundle(&tmp.join("good"), "good", "Good", "G");
    // A symlink loop, a dangling symlink and a symlink to the root itself.
    std::os::unix::fs::symlink(tmp.join("loop_b"), tmp.join("loop_a")).unwrap();
    std::os::unix::fs::symlink(tmp.join("loop_a"), tmp.join("loop_b")).unwrap();
    std::os::unix::fs::symlink(tmp.join("gone"), tmp.join("dangling")).unwrap();
    std::os::unix::fs::symlink(tmp.path(), tmp.join("self")).unwrap();
    std::os::unix::fs::symlink(tmp.join("gone.zip"), tmp.join("dangling.zip")).unwrap();
    // A name that is not UTF-8.
    let bad = std::ffi::OsStr::from_bytes(b"bad_\xff\xfe_name");
    let _ = std::fs::create_dir(tmp.path().join(bad));
    // A bundle whose manifest is a symlink out of the bundle: an otherwise
    // complete bundle (Ready as written) whose `table.toml` is then replaced
    // by a symlink to a valid manifest outside it, so only the symlink
    // check can reject it.
    write_valid_bundle(&tmp.join("escape"), "escape", "Escape", "E");
    assert!(
        find(&scan_tables(tmp.path()), "escape").is_ready(),
        "the escape bundle must be valid before its manifest is swapped"
    );
    std::fs::remove_file(tmp.join("escape/table.toml")).unwrap();
    std::os::unix::fs::symlink(
        base_bundle().join("table.toml"),
        tmp.join("escape/table.toml"),
    )
    .unwrap();

    let entries = scan_tables(tmp.path());
    assert!(find(&entries, "good").is_ready());
    for e in &entries {
        if let TableStatus::Broken(err) = &e.status {
            assert!(!err.trim().is_empty());
        }
    }
    assert!(
        entries
            .iter()
            .filter(|e| e.file_name == "escape")
            .all(|e| !e.is_ready()),
        "a manifest symlinked from outside the bundle was accepted"
    );
    assert_eq!(scan_tables(tmp.path()), entries);
}

#[test]
fn truncated_archives_are_broken_not_panics() {
    let src = TempDir::new("truncsrc");
    write_valid_bundle(&src.join("b"), "t", "T", "x");
    zip_dir(&src.join("b"), &src.join("full.zip"));
    let bytes = std::fs::read(src.join("full.zip")).unwrap();
    let tmp = TempDir::new("trunc");
    for (i, cut) in [1usize, 4, 22, 30, bytes.len() / 2, bytes.len() - 1]
        .into_iter()
        .enumerate()
    {
        std::fs::write(tmp.join(&format!("cut_{i}.pinbundle")), &bytes[..cut]).unwrap();
    }
    let entries = scan_tables(tmp.path());
    check_invariants(tmp.path(), &entries);
    assert_eq!(entries.len(), 6);
    assert!(entries.iter().all(|e| !e.is_ready()));
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Valid,
    Garbage,
    BadSchema,
    NoBackglass,
    FakeZip,
    EmptyDir,
}

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![
        3 => Just(Kind::Valid),
        1 => Just(Kind::Garbage),
        1 => Just(Kind::BadSchema),
        1 => Just(Kind::NoBackglass),
        1 => Just(Kind::FakeZip),
        1 => Just(Kind::EmptyDir),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, failure_persistence: None, ..ProptestConfig::default() })]

    /// Any mix of valid and broken bundles: every valid one is Ready, every
    /// broken one is reported, the order is sorted and repeatable.
    #[test]
    fn random_directories_scan_deterministically(
        items in proptest::collection::btree_map("[a-z][a-z0-9_]{0,7}", kind(), 0..7)
    ) {
        let tmp = TempDir::new("prop");
        let mut valid = 0usize;
        for (name, kind) in &items {
            match kind {
                Kind::Valid => {
                    valid += 1;
                    write_valid_bundle(&tmp.join(name), name, &format!("T {name}"), "p");
                }
                Kind::Garbage => write_garbage_manifest_bundle(&tmp.join(name)),
                Kind::BadSchema => write_bad_schema_bundle(&tmp.join(name)),
                Kind::NoBackglass => write_missing_backglass_bundle(&tmp.join(name)),
                Kind::FakeZip => write_fake_archive(&tmp.join(&format!("{name}.zip"))),
                Kind::EmptyDir => std::fs::create_dir_all(tmp.join(name)).unwrap(),
            }
        }
        let entries = scan_tables(tmp.path());
        check_invariants(tmp.path(), &entries);
        prop_assert_eq!(entries.iter().filter(|e| e.is_ready()).count(), valid);
        for (name, kind) in &items {
            match kind {
                Kind::Valid => {
                    let e = find(&entries, name);
                    prop_assert_eq!(&e.info().unwrap().id, name);
                }
                Kind::Garbage | Kind::BadSchema | Kind::NoBackglass => {
                    prop_assert!(find(&entries, name).error().is_some());
                }
                Kind::FakeZip => {
                    let zip_name = format!("{name}.zip");
                    prop_assert!(find(&entries, &zip_name).error().is_some());
                }
                Kind::EmptyDir => {}
            }
        }
        prop_assert_eq!(scan_tables(tmp.path()), entries);
    }
}
