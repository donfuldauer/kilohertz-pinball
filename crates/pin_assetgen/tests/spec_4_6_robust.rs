//! Spec tests, Subphase 4.6 — no panic on arbitrary TableMeta (verification 6)
//! and the invariants that must survive it.

mod spec_4_6_support;

use pin_assetgen::TableMeta;
use pin_assetgen::sounds::SFX_IDS;
use proptest::prelude::*;
use spec_4_6_support::*;

/// Generates a kit and checks what must hold for any metadata: Ok, a manifest
/// whose files exist inside `out_dir`, the stable ids, decodable outputs.
fn check(m: &TableMeta, name: &str, deep: bool) -> Result<(), String> {
    let dir = tmp(name);
    let man = pin_assetgen::generate_kit(m, &dir).map_err(|e| format!("generate_kit: {e}"))?;
    let tree = read_tree(&dir);
    if !tree.contains_key("assets.toml") {
        return Err("assets.toml missing".into());
    }
    if tree["assets.toml"].is_empty() {
        return Err("assets.toml is empty".into());
    }
    let text = String::from_utf8(tree["assets.toml"].clone()).map_err(|e| e.to_string())?;
    text.parse::<toml::Table>()
        .map_err(|e| format!("assets.toml is not valid TOML: {e}"))?;
    let idx = index(&man);
    for id in SFX_IDS {
        if !idx.contains_key(*id) {
            return Err(format!("{id} missing"));
        }
    }
    for e in &man.entries {
        if e.path.contains("..") || e.path.starts_with('/') || e.path.contains('\\') {
            return Err(format!("unsafe path {:?}", e.path));
        }
        let Some(bytes) = tree.get(&e.path) else {
            return Err(format!("{}: {:?} not written under out_dir", e.id, e.path));
        };
        if deep && is_audio(&e.kind) {
            let s = pin_audio::mixer::decode(bytes, 48_000)
                .map_err(|err| format!("{}: {err:?}", e.id))?;
            let (r, p) = (rms(s.data()), peak(s.data()));
            if r <= RMS_FLOOR || p > PEAK_CEIL {
                return Err(format!("{}: RMS {r}, peak {p}", e.id));
            }
        }
        if deep && matches!(e.kind, pin_assetgen::AssetKind::Image) {
            let img = load_png_via_assets(bytes, "robust_png");
            if img.width == 0 || img.height == 0 {
                return Err(format!("{}: empty image", e.id));
            }
        }
    }
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

#[test]
fn edge_case_metadata_generates_a_loadable_kit() {
    let long = "x".repeat(4096);
    let cases: Vec<TableMeta> = vec![
        meta("", &[], 0),
        meta("", &[], u64::MAX),
        meta("Apex Circuit", &[], 1),
        meta("", &APEX_PALETTE, 1),
        meta(
            "\u{30d4}\u{30f3}\u{30dc}\u{30fc}\u{30eb} \u{1f3b1} caf\u{e9}",
            &APEX_PALETTE,
            u64::MAX,
        ),
        meta("../../etc/passwd", &[[0, 0, 0]], 7),
        meta("a/b\\c:d*e?\"<>|\0\n\r\t", &[[255, 255, 255]], 7),
        meta(&long, &[[1, 2, 3]; 64], 0x8000_0000_0000_0000),
        meta("black on black", &[[0, 0, 0], [0, 0, 0]], 3),
        meta("   ", &[[255, 255, 255], [255, 255, 255]], 3),
        meta("\u{202e}rtl \u{0301}\u{fffd}", &[[12, 34, 56]], 1 << 63),
    ];
    for (i, m) in cases.iter().enumerate() {
        check(m, &format!("edge{i}"), true)
            .unwrap_or_else(|e| panic!("case {i} (title {:?}): {e}", m.title));
    }
}

#[test]
fn hostile_titles_do_not_write_outside_out_dir() {
    let parent = tmp("jail");
    let out = parent.join("kit");
    std::fs::create_dir_all(&out).unwrap();
    for title in ["../escape", "..\\escape", "/abs", "a/../../b", "..", "."] {
        pin_assetgen::generate_kit(&meta(title, &APEX_PALETTE, 1), &out)
            .unwrap_or_else(|e| panic!("{title:?}: {e}"));
        let names: Vec<_> = std::fs::read_dir(&parent)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("kit")],
            "{title:?} escaped out_dir"
        );
    }
    let _ = std::fs::remove_dir_all(parent);
}

#[test]
fn same_edge_meta_is_still_deterministic() {
    for (i, m) in [
        meta("", &[], u64::MAX),
        meta("\u{e9}\u{1f3b1}", &[[9, 9, 9]], 0),
    ]
    .iter()
    .enumerate()
    {
        let (a, _) = generate(m, &format!("edge_det_a{i}"));
        let (b, _) = generate(m, &format!("edge_det_b{i}"));
        assert!(read_tree(&a) == read_tree(&b), "case {i}: runs differ");
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, max_shrink_iters: 8, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_meta_never_panics(
        title in prop_oneof![Just(String::new()), ".{0,40}", "\\PC{0,24}", "[ -~]{0,64}"],
        palette in proptest::collection::vec(any::<[u8; 3]>(), 0..9),
        seed in prop_oneof![Just(0u64), Just(u64::MAX), any::<u64>()],
    ) {
        let m = TableMeta { title, palette, seed };
        if let Err(e) = check(&m, "prop", false) {
            return Err(TestCaseError::fail(e));
        }
    }
}
