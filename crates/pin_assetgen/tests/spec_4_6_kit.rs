//! Spec tests, Subphase 4.6 — the generated kit (brief: deliverables 1, 2, 4,
//! 5, 6; verification 1, 2, 3, 5).

#![allow(clippy::needless_range_loop)]

mod spec_4_6_support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pin_assetgen::sounds::{SFX_IDS, render_sfx};
use pin_assetgen::{AssetKind, KitManifest};
use spec_4_6_support::*;

/// The Appendix A kit, generated once per test binary.
fn apex() -> &'static (PathBuf, KitManifest) {
    static KIT: OnceLock<(PathBuf, KitManifest)> = OnceLock::new();
    KIT.get_or_init(|| generate(&apex_meta(), "apex"))
}

fn apex_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex")
}

fn of_kind<'a>(man: &'a KitManifest, kind: &str) -> Vec<&'a pin_assetgen::KitEntry> {
    man.entries
        .iter()
        .filter(|e| kind_name(&e.kind) == kind)
        .collect()
}

// ------------------------------------------------------------ manifest shape

#[test]
fn manifest_ids_and_paths_are_unique_relative_and_exist() {
    let (dir, man) = apex();
    assert!(!man.entries.is_empty());
    let _ = index(man); // unique ids
    let mut paths = BTreeSet::new();
    for e in &man.entries {
        assert!(!e.id.is_empty(), "empty id");
        assert!(
            e.id.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "id {:?} is not a stable snake_case identifier",
            e.id
        );
        assert!(!e.path.is_empty(), "{}: empty path", e.id);
        assert!(
            !e.path.contains('\\'),
            "{}: path {:?} has a backslash",
            e.id,
            e.path
        );
        assert!(
            !e.path.starts_with('/'),
            "{}: path {:?} is absolute",
            e.id,
            e.path
        );
        assert!(!e.path.contains(':'), "{}: path {:?}", e.id, e.path);
        assert!(
            e.path
                .split('/')
                .all(|c| !c.is_empty() && c != "." && c != ".."),
            "{}: path {:?} is not a clean bundle-relative path",
            e.id,
            e.path
        );
        assert!(paths.insert(e.path.clone()), "path {:?} used twice", e.path);
        let md = std::fs::metadata(dir.join(&e.path))
            .unwrap_or_else(|err| panic!("{}: {:?}: {err}", e.id, e.path));
        assert!(
            md.is_file() && md.len() > 0,
            "{}: {:?} is empty",
            e.id,
            e.path
        );
        assert!(!e.sha256_or_hash.is_empty(), "{}: empty hash", e.id);
    }
}

#[test]
fn manifest_has_every_deliverable_kind() {
    let (_, man) = apex();
    let idx = index(man);
    for id in SFX_IDS {
        let (path, kind, _) = idx
            .get(*id)
            .unwrap_or_else(|| panic!("stable id {id} missing from the manifest"));
        assert_eq!(*kind, "Sound", "{id}");
        assert!(path.to_ascii_lowercase().ends_with(".wav"), "{id}: {path}");
    }
    assert!(!of_kind(man, "Music").is_empty(), "no music loop");
    assert!(
        of_kind(man, "Motif").len() >= 5,
        "a fixed set of callout motifs is required, found {}",
        of_kind(man, "Motif").len()
    );
    assert!(
        of_kind(man, "Image").len() >= 3,
        "translite, playfield art and insert labels are required, found {} images",
        of_kind(man, "Image").len()
    );
    for e in of_kind(man, "Image") {
        assert!(
            e.path.to_ascii_lowercase().ends_with(".png"),
            "{}: {}",
            e.id,
            e.path
        );
    }
    for e in man.entries.iter().filter(|e| is_audio(&e.kind)) {
        assert!(
            e.path.to_ascii_lowercase().ends_with(".wav"),
            "{}: {}",
            e.id,
            e.path
        );
    }
}

fn strings(v: &toml::Value, out: &mut BTreeSet<String>) {
    match v {
        toml::Value::String(s) => {
            out.insert(s.clone());
        }
        toml::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        toml::Value::Table(t) => {
            for (k, x) in t {
                out.insert(k.clone());
                strings(x, out);
            }
        }
        _ => {}
    }
}

#[test]
fn assets_toml_is_written_and_lists_every_entry() {
    let (dir, man) = apex();
    let text = std::fs::read_to_string(dir.join("assets.toml"))
        .expect("<out_dir>/assets.toml must be written");
    let table: toml::Table = text.parse().expect("assets.toml must be valid TOML");
    let mut all = BTreeSet::new();
    strings(&toml::Value::Table(table), &mut all);
    for e in &man.entries {
        assert!(all.contains(&e.id), "assets.toml lacks id {:?}", e.id);
        assert!(all.contains(&e.path), "assets.toml lacks path {:?}", e.path);
        assert!(
            all.contains(&e.sha256_or_hash),
            "assets.toml lacks the hash of {:?}",
            e.id
        );
    }
}

// ------------------------------------------------------------------- hashes

#[test]
fn manifest_hashes_match_the_files() {
    let (dir, man) = apex();
    // Equal bytes <=> equal hash, over every pair of entries.
    let files: Vec<Vec<u8>> = man
        .entries
        .iter()
        .map(|e| file_bytes(dir, &e.path))
        .collect();
    for i in 0..files.len() {
        for j in i + 1..files.len() {
            assert_eq!(
                files[i] == files[j],
                man.entries[i].sha256_or_hash == man.entries[j].sha256_or_hash,
                "hash/content mismatch between {} and {}",
                man.entries[i].id,
                man.entries[j].id
            );
        }
    }
    // When the field is a SHA-256 (64 hex digits, optional "sha256:" prefix),
    // it must be the SHA-256 of the file.
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    for (e, bytes) in man.entries.iter().zip(&files) {
        let h = e.sha256_or_hash.to_ascii_lowercase();
        let h = h.strip_prefix("sha256:").unwrap_or(&h);
        if h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()) {
            assert_eq!(
                h,
                sha256_hex(bytes),
                "{}: SHA-256 does not match {}",
                e.id,
                e.path
            );
        }
    }
}

#[test]
fn a_changed_file_gets_a_changed_hash() {
    // Same id, different content (other seed) => different hash; same content
    // => same hash.
    let (_, a) = apex();
    let (dir_a, _) = apex();
    let (dir_b, b) = generate(&meta("Apex Circuit", &APEX_PALETTE, 2), "hash_seed2");
    let ib = index(&b);
    for e in &a.entries {
        let (pb, _, hb) = &ib[&e.id];
        let same = file_bytes(dir_a, &e.path) == file_bytes(&dir_b, pb);
        assert_eq!(same, &e.sha256_or_hash == hb, "{}", e.id);
    }
    let _ = std::fs::remove_dir_all(dir_b);
}

// -------------------------------------------------------------- determinism

#[test]
fn same_meta_gives_byte_identical_kits() {
    let (dir_a, man_a) = apex();
    let (dir_b, man_b) = generate(&apex_meta(), "apex_again");
    let (ta, tb) = (read_tree(dir_a), read_tree(&dir_b));
    assert_eq!(
        ta.keys().collect::<Vec<_>>(),
        tb.keys().collect::<Vec<_>>(),
        "file sets differ"
    );
    for (name, bytes) in &ta {
        assert!(
            bytes == &tb[name],
            "{name} differs between two identical runs"
        );
    }
    assert!(ta.contains_key("assets.toml"));
    assert_eq!(man_a.entries.len(), man_b.entries.len());
    for (x, y) in man_a.entries.iter().zip(&man_b.entries) {
        assert_eq!(
            (&x.id, &x.path, kind_name(&x.kind), &x.sha256_or_hash),
            (&y.id, &y.path, kind_name(&y.kind), &y.sha256_or_hash)
        );
    }
    let _ = std::fs::remove_dir_all(dir_b);
}

/// (changed ids by kind name) between the apex kit and another meta, after
/// asserting the stable id → path/kind mapping is unchanged.
fn changed_vs_apex(
    other: &pin_assetgen::TableMeta,
    name: &str,
) -> BTreeMap<&'static str, Vec<String>> {
    let (dir_a, a) = apex();
    let (dir_b, b) = generate(other, name);
    let (ia, ib) = (index(a), index(&b));
    assert_eq!(
        ia.iter().map(|(k, v)| (k, &v.0, v.1)).collect::<Vec<_>>(),
        ib.iter().map(|(k, v)| (k, &v.0, v.1)).collect::<Vec<_>>(),
        "stable ids/paths/kinds must not depend on the table metadata"
    );
    let mut out: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for (id, (path, kind, _)) in &ia {
        if file_bytes(dir_a, path) != file_bytes(&dir_b, path) {
            out.entry(kind).or_default().push(id.clone());
        }
    }
    let _ = std::fs::remove_dir_all(dir_b);
    out
}

fn assert_music_and_art_change_but_not_sfx(
    changed: &BTreeMap<&'static str, Vec<String>>,
    what: &str,
) {
    assert!(
        !changed.contains_key("Sound"),
        "{what} changed SFX: {:?}",
        changed["Sound"]
    );
    assert!(
        changed.contains_key("Music"),
        "{what} did not change the music loop"
    );
    assert!(
        changed.contains_key("Image"),
        "{what} did not change any art"
    );
    assert!(
        !changed.contains_key("Font"),
        "{what} changed a font: {:?}",
        changed["Font"]
    );
}

#[test]
fn a_different_seed_changes_music_and_art_but_not_the_sfx() {
    let c = changed_vs_apex(&meta("Apex Circuit", &APEX_PALETTE, 2), "seed2");
    assert_music_and_art_change_but_not_sfx(&c, "seed");
    let c = changed_vs_apex(&meta("Apex Circuit", &APEX_PALETTE, u64::MAX), "seedmax");
    assert_music_and_art_change_but_not_sfx(&c, "seed");
}

#[test]
fn a_different_title_changes_music_and_art_but_not_the_sfx() {
    let c = changed_vs_apex(&meta("Neon Harbour", &APEX_PALETTE, 1), "title");
    assert_music_and_art_change_but_not_sfx(&c, "title");
    // One-character difference is still a different table.
    let c = changed_vs_apex(&meta("Apex Circuit!", &APEX_PALETTE, 1), "title1");
    assert_music_and_art_change_but_not_sfx(&c, "title");
}

#[test]
fn a_different_palette_changes_the_art_but_not_the_sfx() {
    let c = changed_vs_apex(
        &meta(
            "Apex Circuit",
            &[[0x10, 0x80, 0x40], [0xff, 0xff, 0xff], [0x00, 0x00, 0x00]],
            1,
        ),
        "palette",
    );
    assert!(
        !c.contains_key("Sound"),
        "palette changed SFX: {:?}",
        c["Sound"]
    );
    assert!(
        c.contains_key("Image"),
        "art is not derived from the palette"
    );
    assert!(
        c.contains_key("Music"),
        "music is seeded by title + palette + seed"
    );
}

// -------------------------------------------------------------------- audio

#[test]
fn every_wav_is_48k_pcm16_or_float_and_decodes_with_pin_audio() {
    let (dir, man) = apex();
    for e in man.entries.iter().filter(|e| is_audio(&e.kind)) {
        let bytes = file_bytes(dir, &e.path);
        let (tag, channels, rate, bits) = wav_info(&bytes);
        assert_eq!(rate, 48_000, "{}: sample rate", e.id);
        assert!((1..=2).contains(&channels), "{}: {channels} channels", e.id);
        assert!(
            matches!((tag, bits), (1, 16) | (3, 32) | (0xfffe, 16) | (0xfffe, 32)),
            "{}: format tag {tag:#x} with {bits} bits is neither 16-bit PCM nor float",
            e.id
        );
        let (l, r) = decode48(&bytes);
        assert!(!l.is_empty(), "{}: no frames", e.id);
        assert!(l.iter().chain(&r).all(|v| v.is_finite()), "{}", e.id);
        let block = usize::from(channels) * usize::from(bits / 8);
        let data_frames = (bytes.len() - 44) / block;
        assert!(
            l.len().abs_diff(data_frames) <= 64,
            "{}: decoded {} frames from a file holding about {data_frames}",
            e.id,
            l.len()
        );
    }
}

#[test]
fn every_sound_file_is_non_silent_and_below_clipping() {
    let (dir, man) = apex();
    for e in man.entries.iter().filter(|e| is_audio(&e.kind)) {
        let (l, r) = decode48(&file_bytes(dir, &e.path));
        for (side, s) in [("left", &l), ("right", &r)] {
            let (rm, pk) = (rms(s), peak(s));
            assert!(
                rm > RMS_FLOOR,
                "{} ({side}): RMS {rm} is not above -40 dBFS",
                e.id
            );
            assert!(
                pk <= PEAK_CEIL,
                "{} ({side}): peak {pk} is above -0.1 dBFS",
                e.id
            );
        }
    }
}

#[test]
fn sfx_files_hold_what_render_sfx_renders() {
    let (dir, man) = apex();
    let idx = index(man);
    for id in SFX_IDS {
        let (l, _) = decode48(&file_bytes(dir, &idx[*id].0));
        let want = render_sfx(id, 48_000).unwrap();
        assert_eq!(l.len(), want.len(), "{id}: frame count");
        let worst = l
            .iter()
            .zip(&want)
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(
            worst <= 1.0e-3,
            "{id}: file differs from render_sfx by {worst}"
        );
    }
}

#[test]
fn ball_roll_file_loops_seamlessly() {
    let (dir, man) = apex();
    let (l, _) = decode48(&file_bytes(dir, &index(man)["sfx_ball_roll"].0));
    let (j, m) = (seam(&l), max_step(&l));
    assert!(
        j <= m + 1.0e-4,
        "wrap jump {j} exceeds largest interior step {m}"
    );
    assert!(j < 0.1, "wrap jump {j}");
}

#[test]
fn music_is_a_short_loop() {
    let (dir, man) = apex();
    for e in of_kind(man, "Music") {
        let (l, _) = decode48(&file_bytes(dir, &e.path));
        let secs = l.len() as f64 / 48_000.0;
        assert!(
            (1.0..=120.0).contains(&secs),
            "{}: {secs} s is not a short loop",
            e.id
        );
        // A loop must not click at the wrap point.
        assert!(seam(&l) < 0.1, "{}: wrap jump {}", e.id, seam(&l));
    }
}

#[test]
fn motifs_are_distinct_from_each_other() {
    let (dir, man) = apex();
    let motifs = of_kind(man, "Motif");
    for i in 0..motifs.len() {
        for j in i + 1..motifs.len() {
            assert!(
                file_bytes(dir, &motifs[i].path) != file_bytes(dir, &motifs[j].path),
                "motifs {} and {} are the same sound",
                motifs[i].id,
                motifs[j].id
            );
        }
    }
}

// ------------------------------------------------------------------- images

#[test]
fn every_png_loads_through_pin_assets_and_is_not_a_flat_fill() {
    let (dir, man) = apex();
    for e in of_kind(man, "Image") {
        let bytes = file_bytes(dir, &e.path);
        assert!(bytes.starts_with(&PNG_SIG), "{}: not a PNG", e.id);
        let img = load_png_via_assets(&bytes, "png_probe");
        assert!(img.width > 0 && img.height > 0, "{}", e.id);
        assert_eq!(
            img.rgba8.len(),
            img.width as usize * img.height as usize * 4
        );
        let first = &img.rgba8[0..4];
        assert!(
            img.rgba8.chunks_exact(4).any(|p| p != first),
            "{}: {}x{} image is one flat colour",
            e.id,
            img.width,
            img.height
        );
    }
}

#[test]
fn art_uses_the_table_palette() {
    // Art "derived from table metadata (title, palette)": a saturated
    // single-hue palette must tint the art. Compare the mean colour of every
    // image under an all-red and an all-blue palette.
    let (dir_r, man_r) = generate(&meta("Tint", &[[255, 0, 0]], 5), "tint_red");
    let (dir_b, man_b) = generate(&meta("Tint", &[[0, 0, 255]], 5), "tint_blue");
    let mean = |dir: &Path, path: &str| {
        let img = load_png_via_assets(&file_bytes(dir, path), "tint_probe");
        let n = (img.rgba8.len() / 4) as f64;
        let mut sum = [0.0f64; 3];
        for p in img.rgba8.chunks_exact(4) {
            let a = f64::from(p[3]) / 255.0;
            for c in 0..3 {
                sum[c] += f64::from(p[c]) * a;
            }
        }
        [sum[0] / n, sum[1] / n, sum[2] / n]
    };
    let ib = index(&man_b);
    let mut tinted = 0;
    for e in of_kind(&man_r, "Image") {
        let r = mean(&dir_r, &e.path);
        let b = mean(&dir_b, &ib[&e.id].0);
        if r[0] - r[2] > b[0] - b[2] + 1.0 {
            tinted += 1;
        }
    }
    assert!(
        tinted > 0,
        "no image is redder under a red palette than under a blue one"
    );
    let _ = std::fs::remove_dir_all(dir_r);
    let _ = std::fs::remove_dir_all(dir_b);
}

// --------------------------------------------------------------- load_table

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let dest = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy_dir(&p, &dest);
        } else {
            std::fs::copy(&p, &dest).unwrap();
        }
    }
}

/// Appendix A bundle whose sounds and translite come from the generated kit,
/// referenced by the manifest's ids and paths.
#[test]
fn a_bundle_referencing_the_kit_passes_load_table() {
    let dir = tmp("bundle");
    copy_dir(&apex_fixture(), &dir);
    std::fs::write(dir.join("playfield.glb"), b"glTF").unwrap();
    let man = pin_assetgen::generate_kit(&apex_meta(), &dir).expect("generate into a bundle dir");
    let idx = index(&man);

    // table.toml: the Appendix A sound now points at the kit's file, and every
    // other audio entry of the manifest is declared under its stable id.
    let mut table = std::fs::read_to_string(apex_fixture().join("table.toml")).unwrap();
    let old = "file = \"audio/bumper.ogg\"";
    assert!(table.contains(old));
    table = table.replace(old, &format!("file = \"{}\"", idx["sfx_bumper"].0));
    let mut declared = vec!["sfx_bumper".to_owned()];
    for e in man
        .entries
        .iter()
        .filter(|e| is_audio(&e.kind) && e.id != "sfx_bumper")
    {
        let bus = match e.kind {
            AssetKind::Music => "music",
            AssetKind::Motif => "callout",
            _ => "sfx",
        };
        table.push_str(&format!(
            "\n[[sounds]]\nid = \"{}\"\nfile = \"{}\"\nbus = \"{bus}\"\n",
            e.id, e.path
        ));
        declared.push(e.id.clone());
    }
    std::fs::write(dir.join("table.toml"), table).unwrap();

    // backglass.toml: every generated image is a translite layer.
    let layers: Vec<String> = man
        .entries
        .iter()
        .filter(|e| matches!(e.kind, AssetKind::Image))
        .enumerate()
        .map(|(i, e)| format!("  {{ file = \"{}\", depth = {i} }},", e.path))
        .collect();
    assert!(!layers.is_empty());
    let backglass = format!(
        "schema_version = 1\n\n[translite]\nlayers = [\n{}\n]\n\n[score_display]\nmode = \"dmd\"\n\
         dmd = {{ width = 128, height = 32, color = \"orange\" }}\nlcd = {{ width = 1920, height = 1080 }}\n",
        layers.join("\n")
    );
    std::fs::write(dir.join("backglass.toml"), backglass).unwrap();

    let vfs = pin_assets::Vfs::open(&dir).unwrap();
    let loaded = pin_assets::load_table(&vfs).unwrap_or_else(|e| panic!("load_table: {e}"));
    assert_eq!(loaded.manifest.sounds.len(), declared.len());
    for s in &loaded.manifest.sounds {
        assert_eq!(s.file, idx[&s.id].0, "{}", s.id);
        // Resolves through the VFS to a file pin_audio decodes.
        let bytes = vfs
            .read(&s.file)
            .unwrap_or_else(|e| panic!("{}: {e}", s.file));
        assert!(pin_audio::mixer::decode(&bytes, 48_000).is_ok(), "{}", s.id);
    }
    assert_eq!(loaded.backglass.translite.layers.len(), layers.len());
    let _ = std::fs::remove_dir_all(dir);
}

/// Verification 5: every [[sounds]] id of the Appendix A example resolves to a
/// manifest entry with an existing file, and so does the stable id list.
#[test]
fn appendix_a_sound_ids_and_the_stable_list_resolve() {
    let (dir, man) = apex();
    let idx = index(man);
    let text = std::fs::read_to_string(apex_fixture().join("table.toml")).unwrap();
    let table: toml::Table = text.parse().unwrap();
    let sounds = table["sounds"].as_array().unwrap();
    assert!(!sounds.is_empty());
    let mut ids: Vec<String> = sounds
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    ids.extend(SFX_IDS.iter().map(|s| (*s).to_owned()));
    for id in ids {
        let (path, kind, _) = idx
            .get(&id)
            .unwrap_or_else(|| panic!("sound id {id} is not in the manifest"));
        assert!(
            matches!(*kind, "Sound" | "Music" | "Motif"),
            "{id}: kind {kind}"
        );
        assert!(dir.join(path).is_file(), "{id}: {path} missing");
    }
}

#[test]
fn generating_twice_into_the_same_directory_is_idempotent() {
    let (dir, first) = generate(&apex_meta(), "twice");
    let before = read_tree(&dir);
    let second = pin_assetgen::generate_kit(&apex_meta(), &dir).expect("regenerate in place");
    let after = read_tree(&dir);
    assert!(before == after, "regenerating changed the bundle");
    assert_eq!(index(&first), index(&second));
    let _ = std::fs::remove_dir_all(dir);
}
