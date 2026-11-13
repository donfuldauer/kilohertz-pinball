//! Implementer tests for 4.6: the generated kit loads through 4.1 / 3.3, is
//! byte-stable, and resolves the Appendix A example's sounds.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use pin_assetgen::sounds::{MOTIF_IDS, MUSIC_ID, SFX_IDS};
use pin_assetgen::{AssetKind, KitManifest, TableMeta, generate_kit, sha256_hex};

fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pin_assetgen_kit_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn apex_meta(seed: u64) -> TableMeta {
    TableMeta {
        title: "Apex Circuit".into(),
        palette: vec![[0x0b, 0x1d, 0x3a], [0xf2, 0xa9, 0x00], [0xe6, 0x39, 0x46]],
        seed,
    }
}

fn file(dir: &Path, rel: &str) -> Vec<u8> {
    std::fs::read(dir.join(rel)).unwrap()
}

fn apex_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex")
}

#[test]
fn two_runs_are_byte_identical_and_hashes_match() {
    let (a, b) = (temp_dir("a"), temp_dir("b"));
    let ma = generate_kit(&apex_meta(7), &a).unwrap();
    let mb = generate_kit(&apex_meta(7), &b).unwrap();
    assert_eq!(ma, mb);
    assert_eq!(file(&a, "assets.toml"), file(&b, "assets.toml"));
    assert_eq!(KitManifest::read(&a).unwrap(), ma);
    assert_eq!(ma.entries.len(), pin_assetgen::stable_ids().len());
    for e in &ma.entries {
        let bytes = file(&a, &e.path);
        assert_eq!(bytes, file(&b, &e.path), "{}", e.id);
        assert_eq!(sha256_hex(&bytes), e.sha256_or_hash, "{}", e.id);
    }
    // Regenerating into the same directory is idempotent.
    let again = generate_kit(&apex_meta(7), &a).unwrap();
    assert_eq!(again, ma);
}

#[test]
fn metadata_changes_music_and_art_but_not_the_fixed_set() {
    let base = generate_kit(&apex_meta(1), &temp_dir("m1")).unwrap();
    let seed = generate_kit(&apex_meta(2), &temp_dir("m2")).unwrap();
    let mut t = apex_meta(1);
    t.title = "Other Table".into();
    let title = generate_kit(&t, &temp_dir("m3")).unwrap();
    let mut p = apex_meta(1);
    p.palette = vec![[0xff, 0xff, 0xff]];
    let pal = generate_kit(&p, &temp_dir("m4")).unwrap();
    for other in [&seed, &title, &pal] {
        for (x, y) in base.entries.iter().zip(&other.entries) {
            assert_eq!(x.id, y.id);
            match x.kind {
                AssetKind::Sound | AssetKind::Motif | AssetKind::Font => {
                    assert_eq!(x.sha256_or_hash, y.sha256_or_hash, "{}", x.id)
                }
                AssetKind::Music => assert_ne!(x.sha256_or_hash, y.sha256_or_hash, "{}", x.id),
                AssetKind::Image => {}
            }
        }
        let h = |m: &KitManifest, id: &str| m.get(id).unwrap().sha256_or_hash.clone();
        assert_ne!(h(&base, "img_backglass_base"), h(other, "img_backglass_base"));
        assert_ne!(h(&base, "img_playfield"), h(other, "img_playfield"));
    }
}

#[test]
fn every_file_decodes_through_the_engine_loaders() {
    let dir = temp_dir("decode");
    let m = generate_kit(&apex_meta(3), &dir).unwrap();
    let mut sounds = 0;
    let mut images = 0;
    for e in &m.entries {
        let bytes = file(&dir, &e.path);
        match e.kind {
            AssetKind::Sound | AssetKind::Music | AssetKind::Motif => {
                let s = pin_audio::mixer::decode(&bytes, 48_000)
                    .unwrap_or_else(|err| panic!("{}: {err}", e.id));
                let data = s.data();
                assert!(!data.is_empty(), "{}", e.id);
                let rms = (data.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>()
                    / data.len() as f64)
                    .sqrt();
                let peak = data.iter().fold(0.0f64, |m, &v| m.max(f64::from(v).abs()));
                assert!(20.0 * rms.log10() > -40.0, "{}: rms {rms}", e.id);
                assert!(peak <= 0.989, "{}: peak {peak}", e.id);
                sounds += 1;
            }
            AssetKind::Font | AssetKind::Image => {
                assert!(e.path.ends_with(".png"));
                let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                    .unwrap_or_else(|err| panic!("{}: {err}", e.id));
                assert!(img.width() > 0 && img.height() > 0);
                images += 1;
            }
        }
    }
    assert_eq!(sounds, SFX_IDS.len() + 1 + MOTIF_IDS.len());
    assert!(images >= 6);
}

/// Copies the Appendix A example bundle next to the kit, pointing its sounds
/// at kit files.
fn apex_bundle_with_kit(tag: &str, all_sounds: bool) -> (PathBuf, KitManifest) {
    let dir = temp_dir(tag);
    let m = generate_kit(&apex_meta(5), &dir).unwrap();
    let fx = apex_fixture();
    let mut table = std::fs::read_to_string(fx.join("table.toml")).unwrap();
    let old = "file = \"audio/bumper.ogg\"";
    assert!(table.contains(old));
    table = table.replace(old, &format!("file = {:?}", m.get("sfx_bumper").unwrap().path));
    if all_sounds {
        // Every other kit sound as an extra [[sounds]] entry.
        for d in m.sound_defs().into_iter().filter(|d| d.id != "sfx_bumper") {
            let bus = match d.bus {
                pin_core::schema::table::SoundBus::Music => "music",
                pin_core::schema::table::SoundBus::Callout => "callout",
                pin_core::schema::table::SoundBus::Sfx => "sfx",
            };
            table.push_str(&format!(
                "\n[[sounds]]\nid = {:?}\nfile = {:?}\nbus = {bus:?}\n",
                d.id, d.file
            ));
        }
    }
    std::fs::write(dir.join("table.toml"), table).unwrap();
    // backglass.toml's layer paths are the kit's backglass paths (A.2 layout).
    std::fs::copy(fx.join("backglass.toml"), dir.join("backglass.toml")).unwrap();
    std::fs::copy(fx.join("rules.lua"), dir.join("rules.lua")).unwrap();
    std::fs::create_dir_all(dir.join("display")).unwrap();
    std::fs::copy(
        fx.join("display/powerball_mania.toml"),
        dir.join("display/powerball_mania.toml"),
    )
    .unwrap();
    std::fs::write(dir.join("playfield.glb"), b"placeholder").unwrap();
    std::fs::write(dir.join("backbox.glb"), b"placeholder").unwrap();
    (dir, m)
}

#[test]
fn apex_bundle_with_kit_assets_passes_load_table() {
    for all in [false, true] {
        let (dir, m) = apex_bundle_with_kit(if all { "apex_all" } else { "apex" }, all);
        let vfs = pin_assets::Vfs::open(&dir).unwrap();
        let loaded = pin_assets::load_table(&vfs).unwrap_or_else(|e| panic!("{e}"));
        // Every [[sounds]] id the kit provides resolves to an existing kit file.
        for s in &loaded.manifest.sounds {
            let e = m.get(&s.id).unwrap_or_else(|| panic!("{} not in kit", s.id));
            assert_eq!(e.path, s.file);
            assert!(vfs.exists(&e.path));
            assert!(loaded.ids.get(pin_assets::IdKind::Sound, &s.id).is_some());
        }
        // Every translite layer and lamp mask is a kit image.
        let bg = &loaded.backglass;
        for layer in &bg.translite.layers {
            assert!(m.entries.iter().any(|e| e.path == layer.file), "{}", layer.file);
        }
        for mask in &bg.lamp_masks {
            assert!(m.entries.iter().any(|e| e.path == mask.file), "{}", mask.file);
        }
        if all {
            assert_eq!(loaded.manifest.sounds.len(), m.sound_defs().len());
        }
    }
}

#[test]
fn manifest_ids_cover_the_stable_lists() {
    let dir = temp_dir("ids");
    let m = generate_kit(&apex_meta(0), &dir).unwrap();
    for id in SFX_IDS.iter().chain(MOTIF_IDS).chain([&MUSIC_ID]) {
        let e = m.get(id).unwrap();
        assert!(dir.join(&e.path).is_file());
    }
    for e in &m.entries {
        assert!(!e.path.contains('\\') && !e.path.starts_with('/'));
        assert_eq!(e.sha256_or_hash.len(), 64);
    }
}

#[test]
fn unwritable_output_is_an_error() {
    let dir = temp_dir("blocked");
    let blocker = dir.join("file");
    std::fs::write(&blocker, b"x").unwrap();
    assert!(generate_kit(&apex_meta(0), &blocker).is_err());
    // A kit sub-directory name taken by a file.
    let dir2 = temp_dir("blocked2");
    std::fs::write(dir2.join("audio"), b"x").unwrap();
    assert!(generate_kit(&apex_meta(0), &dir2).is_err());
    assert!(KitManifest::read(&dir2).is_err());
}

#[test]
fn cli_writes_a_kit() {
    let dir = temp_dir("cli");
    let args = [
        "--title",
        "Apex Circuit",
        "--palette",
        "#0b1d3a,#f2a900,#e63946",
        "--seed",
        "7",
        "--out",
        dir.to_str().unwrap(),
    ];
    let m = pin_assetgen::cli::run(args.iter().map(|s| s.to_string())).unwrap();
    let lib = generate_kit(&apex_meta(7), &temp_dir("cli_lib")).unwrap();
    assert_eq!(m, lib);
    assert!(pin_assetgen::cli::run(["--title".to_string(), "x".into(), "--out".into(), dir.join("assets.toml").to_string_lossy().into_owned()]).is_err());

    // The binary itself.
    let exe = env!("CARGO_BIN_EXE_pin_assetgen");
    let out = std::process::Command::new(exe)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bad = std::process::Command::new(exe).arg("--nope").output().unwrap();
    assert_eq!(bad.status.code(), Some(2));
}
