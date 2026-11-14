//! Subphase 4.1 spec tests (Independent Tester): bounded random-input runs.
//! Brief: verification criterion 4 / gate step 5 - the zip, TOML and glTF
//! (extras) parsers never panic on hostile bytes - plus a few invariants that
//! must hold for every input.

#[path = "spec_fixtures/mod.rs"]
mod fx;

use std::collections::BTreeMap;

use fx::{RawEntry, raw_zip};
use pin_assets::{AssetError, Vfs, ZipLimits, load_gltf, load_table, validate_gltf_bindings};
use proptest::prelude::*;

fn small_limits() -> ZipLimits {
    // Keeps a mutated archive from legitimately inflating to hundreds of MiB.
    ZipLimits {
        max_total_uncompressed: 8 << 20,
        max_file: 4 << 20,
        max_ratio: 100,
        max_entries: 256,
    }
}

/// Exercises every accessor of whatever opened.
fn poke(v: &Vfs) {
    let names = v.list();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "list() must be sorted");
    for n in &names {
        assert!(
            !n.starts_with('/') && !n.contains('\\') && !n.ends_with('/'),
            "{n:?}"
        );
        assert!(
            n.split('/').all(|c| !c.is_empty() && c != "." && c != ".."),
            "{n:?}"
        );
        assert!(v.exists(n));
        let _ = v.read(n);
    }
    let _ = load_table(v);
    for n in names.iter().take(4) {
        let _ = load_gltf(v, n);
    }
}

fn zip_with(name: &str, data: Vec<u8>) -> Vfs {
    let mut m = BTreeMap::new();
    m.insert(name.to_owned(), data);
    Vfs::from_zip_bytes(fx::zip_of(&m), ZipLimits::default()).unwrap()
}

fn mutate(mut bytes: Vec<u8>, edits: &[(usize, u8)]) -> Vec<u8> {
    for (pos, val) in edits {
        let i = pos % bytes.len();
        bytes[i] = *val;
    }
    bytes
}

fn edits() -> impl Strategy<Value = Vec<(usize, u8)>> {
    prop::collection::vec((any::<usize>(), any::<u8>()), 1..8)
}

fn name_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z]{1,6}(/[a-z]{1,6}){0,3}",
        "[a-z./\\\\:]{1,12}",
        Just("../x".to_owned()),
        Just("/abs".to_owned()),
        Just("C:\\x".to_owned()),
        "\\PC{1,8}",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, .. ProptestConfig::default() })]

    // ---- zip ---------------------------------------------------------------

    #[test]
    fn zip_random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(v) = Vfs::from_zip_bytes(bytes, small_limits()) {
            poke(&v);
        }
    }

    #[test]
    fn zip_random_bytes_after_a_zip_signature_never_panic(
        tail in prop::collection::vec(any::<u8>(), 0..512),
        eocd_at_end in any::<bool>(),
    ) {
        let mut bytes = b"PK\x03\x04".to_vec();
        bytes.extend_from_slice(&tail);
        if eocd_at_end {
            bytes.extend_from_slice(b"PK\x05\x06");
            bytes.extend_from_slice(&tail[..tail.len().min(18)]);
            bytes.resize(bytes.len() + 18usize.saturating_sub(tail.len().min(18)), 0);
        }
        if let Ok(v) = Vfs::from_zip_bytes(bytes, small_limits()) {
            poke(&v);
        }
    }

    #[test]
    fn mutated_stored_zip_never_panics(e in edits()) {
        let base = fx::stored_zip(&[
            ("table.toml", fx::TABLE_TOML.as_bytes()),
            ("backglass.toml", fx::BACKGLASS_TOML.as_bytes()),
            ("audio/a.ogg", b"OggS"),
        ]);
        if let Ok(v) = Vfs::from_zip_bytes(mutate(base, &e), small_limits()) {
            poke(&v);
        }
    }

    #[test]
    fn mutated_deflated_bundle_never_panics(e in edits(), tail_only in any::<bool>()) {
        let base = fx::zip_of(&fx::bundle());
        let n = base.len();
        // Half the cases aim at the central directory / EOCD at the end.
        let e: Vec<(usize, u8)> = if tail_only {
            e.iter().map(|(p, v)| (n - 1 - (p % 600.min(n)), *v)).collect()
        } else {
            e
        };
        if let Ok(v) = Vfs::from_zip_bytes(mutate(base, &e), small_limits()) {
            poke(&v);
        }
    }

    #[test]
    fn truncated_bundle_never_panics(cut in 0usize..4096) {
        let base = fx::zip_of(&fx::bundle());
        let keep = base.len().saturating_sub(cut);
        if let Ok(v) = Vfs::from_zip_bytes(base[..keep].to_vec(), small_limits()) {
            poke(&v);
        }
    }

    /// Whatever names an archive carries, an accepted archive only ever lists
    /// clean relative paths, and hostile names are refused with the right kind.
    #[test]
    fn arbitrary_entry_names_are_rejected_or_normalised(
        names in prop::collection::vec(name_strategy(), 1..5),
    ) {
        let entries: Vec<RawEntry> = names.iter().map(|n| RawEntry::file(n, b"x")).collect();
        match Vfs::from_zip_bytes(raw_zip(&entries), small_limits()) {
            Ok(v) => {
                poke(&v);
                for n in &names {
                    let parts: Vec<&str> = n.split(['/', '\\']).collect();
                    prop_assert!(!parts.contains(&".."), "accepted a `..` component: {:?}", n);
                    prop_assert!(!n.starts_with('/') && !n.starts_with('\\'), "accepted absolute {:?}", n);
                }
            }
            Err(e) => {
                prop_assert!(!e.to_string().is_empty());
            }
        }
    }

    #[test]
    fn dotdot_component_anywhere_is_path_traversal(
        before in prop::collection::vec("[a-z]{1,5}", 0..3),
        after in prop::collection::vec("[a-z]{1,5}", 0..3),
        backslash in any::<bool>(),
    ) {
        let mut parts: Vec<String> = before;
        parts.push("..".to_owned());
        parts.extend(after);
        let name = parts.join(if backslash { "\\" } else { "/" });
        let r = Vfs::from_zip_bytes(raw_zip(&[RawEntry::file("table.toml", b"x"), RawEntry::file(&name, b"x")]), small_limits());
        prop_assert!(matches!(r, Err(AssetError::PathTraversal(_))), "{:?} -> {:?}", name, r.err());
    }

    #[test]
    fn read_of_arbitrary_request_paths_never_panics_or_escapes(req in name_strategy()) {
        let v = Vfs::from_zip_bytes(fx::stored_zip(&[("table.toml", b"t"), ("a/b.txt", b"b")]), small_limits()).unwrap();
        match v.read(&req) {
            Ok(data) => {
                // `read` normalises (`a\\b.txt`, `./a//b.txt`); `list` holds canonical names.
                let canonical = pin_assets::zip::normalize_path(&req).unwrap();
                prop_assert!(v.list().contains(&canonical), "read({:?}) succeeded for a name not in list()", req);
                prop_assert!(data == b"t" || data == b"b");
            }
            Err(_) => prop_assert!(!v.exists(&req) || v.list().contains(&req)),
        }
    }

    /// Any stored archive of clean, distinct names round-trips byte for byte.
    #[test]
    fn clean_archives_roundtrip(
        files in prop::collection::btree_map("[a-z]{1,6}(/[a-z]{1,6}){0,2}\\.[a-z]{1,3}", prop::collection::vec(any::<u8>(), 0..200), 1..6),
        deflate in any::<bool>(),
    ) {
        // Drop names that would be both a file and a directory of another entry.
        let names: Vec<String> = files.keys().cloned().collect();
        prop_assume!(!names.iter().any(|a| names.iter().any(|b| b.starts_with(&format!("{a}/")))));
        // A lone `<dir>/table.toml` triggers the documented wrapper stripping.
        prop_assume!(!names.iter().any(|n| n.ends_with("table.toml")));
        let bytes = if deflate {
            fx::zip_of(&files)
        } else {
            let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
            fx::stored_zip(&refs)
        };
        // Random 200-byte payloads cannot exceed the default ratio cap.
        let v = Vfs::from_zip_bytes(bytes, ZipLimits::default()).unwrap();
        prop_assert_eq!(v.list(), names);
        for (n, d) in &files {
            prop_assert_eq!(&v.read(n).unwrap(), d);
        }
    }

    // ---- TOML manifests ----------------------------------------------------

    #[test]
    fn random_bytes_as_manifests_never_panic(
        table in prop::collection::vec(any::<u8>(), 0..512),
        backglass in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        let mut files = fx::bundle();
        files.insert("table.toml".to_owned(), table);
        files.insert("backglass.toml".to_owned(), backglass);
        let v = fx::vfs_from(&files);
        prop_assert!(load_table(&v).is_err());
    }

    #[test]
    fn random_text_as_manifests_never_panics(table in "\\PC{0,300}", backglass in "[ -~\\n]{0,200}") {
        let v = fx::vfs_from(&fx::bundle_with(&table, &backglass));
        let _ = load_table(&v);
    }

    #[test]
    fn mutated_table_toml_never_panics(e in edits()) {
        let bytes = mutate(fx::TABLE_TOML.as_bytes().to_vec(), &e);
        let mut files = fx::bundle();
        files.insert("table.toml".to_owned(), bytes);
        let v = fx::vfs_from(&files);
        match load_table(&v) {
            Ok(t) => prop_assert_eq!(t.flippers.len(), t.flipper_stages.len()),
            Err(AssetError::Invalid(errs)) => {
                prop_assert!(!errs.is_empty());
                let keys: Vec<(&str, &str)> = errs.iter().map(|x| (x.file.as_str(), x.path.as_str())).collect();
                let mut sorted = keys.clone();
                sorted.sort();
                // Single-digit indices only in this fixture, so string order is index order.
                prop_assert_eq!(keys, sorted, "errors must be sorted by (file, path)");
            }
            Err(other) => prop_assert!(false, "unexpected error kind {:?}", other),
        }
    }

    #[test]
    fn mutated_backglass_toml_never_panics(e in edits()) {
        let bytes = mutate(fx::BACKGLASS_TOML.as_bytes().to_vec(), &e);
        let mut files = fx::bundle();
        files.insert("backglass.toml".to_owned(), bytes);
        let v = fx::vfs_from(&files);
        match load_table(&v) {
            Ok(_) | Err(AssetError::Invalid(_)) => {}
            Err(other) => prop_assert!(false, "unexpected error kind {:?}", other),
        }
    }

    /// Structurally valid tables with random numbers and layers: never a panic,
    /// and the outcome follows the stated rules.
    #[test]
    fn random_numbers_and_layers_follow_the_rules(
        sw in 0u32..600, lamp in 0u32..600, layer in 0u8..=255, to_layer in 0u8..=255,
    ) {
        let mut t = fx::sub(fx::TABLE_TOML, "id = \"ramp_made_opto\"\nnumber = 28", &format!("id = \"ramp_made_opto\"\nnumber = {sw}"));
        t = fx::sub(&t, "id = \"backglass_logo\"\nnumber = 41", &format!("id = \"backglass_logo\"\nnumber = {lamp}"));
        t = fx::sub(&t, "kind = \"opto\"\nlayer = 0", &format!("kind = \"opto\"\nlayer = {layer}"));
        t = fx::sub(&t, "to_layer = 2", &format!("to_layer = {to_layer}"));
        let v = fx::vfs_from(&fx::bundle_with(&t, fx::BACKGLASS_TOML));
        let must_fail = sw > 255 || lamp > 255 || layer > 7 || to_layer > 7
            || [24, 25, 26, 27].contains(&sw) || lamp == 40;
        // Cabinet numbers 1..=8 colliding with a declared switch is not pinned by the brief.
        let unclear = (1..=8).contains(&sw);
        match load_table(&v) {
            Ok(tab) => {
                prop_assert!(!must_fail, "sw {} lamp {} layer {} to_layer {} must be rejected", sw, lamp, layer, to_layer);
                prop_assert_eq!(tab.ids.switch("ramp_made_opto").map(|s| u32::from(s.0)), Some(sw));
                prop_assert_eq!(tab.ids.lamp("backglass_logo").map(|s| u32::from(s.0)), Some(lamp));
            }
            Err(AssetError::Invalid(errs)) => {
                prop_assert!(must_fail || unclear, "valid table rejected: {:?}", errs);
                if sw > 255 { prop_assert!(errs.iter().any(|e| e.path == "switches[4].number")); }
                if lamp > 255 { prop_assert!(errs.iter().any(|e| e.path == "lamps[1].number")); }
                if layer > 7 { prop_assert!(errs.iter().any(|e| e.path == "switches[0].layer")); }
                if to_layer > 7 { prop_assert!(errs.iter().any(|e| e.path == "layer_transitions[0].to_layer")); }
            }
            Err(other) => prop_assert!(false, "unexpected error kind {:?}", other),
        }
    }

    /// Edit-distance suggestion: any single-character typo of a coil name is
    /// reported with the intended name; a name far from every coil is not.
    #[test]
    fn single_typo_always_gets_a_suggestion(pos in 0usize..17, kind in 0u8..3, c in "[a-z_]") {
        let good = "left_flipper_coil";
        let mut chars: Vec<char> = good.chars().collect();
        let ch = c.chars().next().unwrap();
        match kind {
            0 => { chars.remove(pos); }
            1 => { chars.insert(pos, ch); }
            _ => { chars[pos] = ch; }
        }
        let typo: String = chars.into_iter().collect();
        prop_assume!(typo != good);
        let t = fx::sub(fx::TABLE_TOML, "coil = \"left_flipper_coil\"\neos_switch", &format!("coil = \"{typo}\"\neos_switch"));
        let errs = fx::table_errors(&t);
        let e = fx::expect_at(&errs, "table.toml", &["flippers[0].coil"]);
        prop_assert!(e.message.contains(&format!("\"{typo}\"")), "{}", e);
        prop_assert!(e.message.contains("did you mean \"left_flipper_coil\"?"), "{}", e);
    }

    #[test]
    fn distant_names_get_no_suggestion(name in "[q-z]{20,28}") {
        let t = fx::sub(fx::TABLE_TOML, "coil = \"left_flipper_coil\"\neos_switch", &format!("coil = \"{name}\"\neos_switch"));
        let errs = fx::table_errors(&t);
        let e = fx::expect_at(&errs, "table.toml", &["flippers[0].coil"]);
        prop_assert!(e.message.contains(&name), "{}", e);
        prop_assert!(!e.message.contains("did you mean"), "{}", e);
    }

    // ---- glTF --------------------------------------------------------------

    #[test]
    fn gltf_random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..1024), as_glb in any::<bool>()) {
        let name = if as_glb { "x.glb" } else { "x.gltf" };
        let v = zip_with(name, bytes);
        prop_assert!(load_gltf(&v, name).is_err());
    }

    #[test]
    fn gltf_random_bytes_after_glb_header_never_panic(tail in prop::collection::vec(any::<u8>(), 0..512), len in any::<u32>(), exact in any::<bool>()) {
        let mut bytes = b"glTF\x02\0\0\0".to_vec();
        let total = if exact { (12 + tail.len()) as u32 } else { len };
        bytes.extend_from_slice(&total.to_le_bytes());
        bytes.extend_from_slice(&tail);
        let v = zip_with("x.glb", bytes);
        let _ = load_gltf(&v, "x.glb");
    }

    #[test]
    fn mutated_glb_never_panics(e in edits(), json_only in any::<bool>()) {
        let base = fx::playfield_glb();
        let json_len = u32::from_le_bytes([base[12], base[13], base[14], base[15]]) as usize;
        let e: Vec<(usize, u8)> = if json_only {
            e.iter().map(|(p, v)| (20 + p % json_len, *v)).collect()
        } else {
            e
        };
        let v = zip_with("x.glb", mutate(base, &e));
        if let Ok(s) = load_gltf(&v, "x.glb") {
            for m in &s.meshes {
                // Whatever loads must be internally consistent for the renderer.
                prop_assert!(m.indices.iter().all(|i| (*i as usize) < m.positions.len()),
                    "index out of range in mesh {:?}", m.name);
                if let Some(mat) = m.material { prop_assert!(mat < s.materials.len()); }
                if let Some(l) = m.extras.collision_layer { prop_assert!(l <= 7); }
            }
            for mat in &s.materials {
                for img in [mat.base_color_image, mat.metallic_roughness_image, mat.normal_image, mat.emissive_image].into_iter().flatten() {
                    prop_assert!(img < s.images.len());
                }
            }
        }
    }

    #[test]
    fn truncated_glb_never_panics(cut in 1usize..1200) {
        let base = fx::playfield_glb();
        let keep = base.len().saturating_sub(cut);
        let v = zip_with("x.glb", base[..keep].to_vec());
        let _ = load_gltf(&v, "x.glb");
    }

    #[test]
    fn mutated_gltf_json_with_external_buffer_never_panics(e in edits()) {
        let json = fx::scene_json(fx::BufferRef::Uri("pf.bin"), fx::DECK_EXTRAS, fx::INSERT_EXTRAS).into_bytes();
        let mut m = BTreeMap::new();
        m.insert("pf.gltf".to_owned(), mutate(json, &e));
        m.insert("pf.bin".to_owned(), fx::scene_bin().0);
        let v = Vfs::from_zip_bytes(fx::zip_of(&m), ZipLimits::default()).unwrap();
        let _ = load_gltf(&v, "pf.gltf");
    }

    /// Random JSON in the extras slots of a valid scene.
    #[test]
    fn random_extras_json_never_panics(deck in json_value(3), insert in json_value(3)) {
        let v = zip_with("x.glb", fx::glb_with_extras(&deck, &insert));
        if let Ok(s) = load_gltf(&v, "x.glb") {
            for m in &s.meshes {
                if let Some(l) = m.extras.collision_layer { prop_assert!(l <= 7, "layer {} from extras {} / {}", l, deck, insert); }
            }
        }
    }

    /// Random values under the four known keys.
    #[test]
    fn random_values_for_known_extras_keys(
        layer in json_value(1), switch in json_value(1), lamp in json_value(1), collider in json_value(1),
    ) {
        let extras = format!(r#"{{"collision_layer":{layer},"switch":{switch},"lamp":{lamp},"collider":{collider}}}"#);
        let v = zip_with("x.glb", fx::glb_with_extras(&extras, "{}"));
        if let Ok(s) = load_gltf(&v, "x.glb") {
            for m in &s.meshes {
                if let Some(l) = m.extras.collision_layer { prop_assert!(l <= 7); }
            }
            // Binding validation of arbitrary names must not panic either.
            let ids = load_table(&fx::vfs_from(&fx::bundle())).unwrap().ids;
            let _ = validate_gltf_bindings(&s, &ids);
        }
    }

    /// Well-typed extras always round-trip exactly.
    #[test]
    fn well_typed_extras_roundtrip(layer in 0u8..=7, switch in "[a-z_0-9]{1,16}", lamp in "[a-z_0-9]{1,16}", collider in any::<bool>()) {
        let extras = format!(r#"{{"collision_layer":{layer},"switch":"{switch}","lamp":"{lamp}","collider":{collider}}}"#);
        let v = zip_with("x.glb", fx::glb_with_extras(&extras, "{}"));
        let s = load_gltf(&v, "x.glb").unwrap();
        let deck = s.meshes.iter().find(|m| m.name == "deck").unwrap();
        prop_assert_eq!(deck.extras.collision_layer, Some(layer));
        prop_assert_eq!(deck.extras.switch.as_deref(), Some(switch.as_str()));
        prop_assert_eq!(deck.extras.lamp.as_deref(), Some(lamp.as_str()));
        prop_assert_eq!(deck.extras.collider, collider);
    }
}

/// A small recursive JSON text generator.
fn json_value(depth: u32) -> BoxedStrategy<String> {
    let leaf = prop_oneof![
        Just("null".to_owned()),
        Just("true".to_owned()),
        Just("false".to_owned()),
        any::<i64>().prop_map(|n| n.to_string()),
        (-10i32..300).prop_map(|n| n.to_string()),
        (-1.0e12f64..1.0e12).prop_map(|f| format!("{f:?}")),
        Just("1e400".to_owned()),
        Just("-0.0".to_owned()),
        "[a-z_]{0,12}".prop_map(|s| format!("\"{s}\"")),
        Just("\"\\u0000\\ud83d\\ude00\\n\"".to_owned()),
    ];
    if depth == 0 {
        return leaf.boxed();
    }
    let key = prop_oneof![
        Just("collision_layer".to_owned()),
        Just("switch".to_owned()),
        Just("lamp".to_owned()),
        Just("collider".to_owned()),
        "[a-z]{1,6}",
    ];
    prop_oneof![
        3 => leaf,
        1 => prop::collection::vec(json_value(depth - 1), 0..4).prop_map(|v| format!("[{}]", v.join(","))),
        3 => prop::collection::btree_map(key, json_value(depth - 1), 0..5).prop_map(|m| {
            let items: Vec<String> = m.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
            format!("{{{}}}", items.join(","))
        }),
    ]
    .boxed()
}

/// Alias spellings resolve to the listed canonical name: the branch the
/// property above checks with `normalize_path`, made deterministic
/// (CodeRabbit PR 4).
#[test]
fn alias_request_paths_resolve_to_listed_names() {
    let v = Vfs::from_zip_bytes(
        fx::stored_zip(&[("table.toml", b"t"), ("a/b.txt", b"b")]),
        small_limits(),
    )
    .unwrap();
    for alias in [
        "a\\b.txt",
        "./a/b.txt",
        "a//b.txt",
        "a/./b.txt",
        ".\\a\\b.txt",
    ] {
        assert_eq!(v.read(alias).unwrap(), b"b", "{alias}");
        assert!(v.exists(alias), "{alias}");
        let canonical = pin_assets::zip::normalize_path(alias).unwrap();
        assert_eq!(canonical, "a/b.txt", "{alias}");
        assert!(v.list().contains(&canonical), "{alias}");
    }
}
