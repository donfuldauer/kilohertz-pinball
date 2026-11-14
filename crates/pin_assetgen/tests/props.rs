//! Property tests for 4.6: arbitrary metadata never panics, the manifest
//! parser and CLI survive random input, fonts render any text.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use pin_assetgen::fonts::{FONT_5X7, FONT_8X16, render_text};
use pin_assetgen::rng::Pcg32;
use pin_assetgen::{KitManifest, TableMeta, art, generate_kit, meta_seed, sounds};
use proptest::prelude::*;

fn temp_dir() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pin_assetgen_props_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn arb_meta() -> impl Strategy<Value = TableMeta> {
    (
        prop_oneof![
            Just(String::new()),
            ".{0,80}",
            "[A-Za-z ]{1,20}",
            Just("\u{0}\u{1F680}\r\n\"\\'".to_string())
        ],
        prop::collection::vec(any::<[u8; 3]>(), 0..6),
        any::<u64>(),
    )
        .prop_map(|(title, palette, seed)| TableMeta {
            title,
            palette,
            seed,
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, ..ProptestConfig::default() })]

    /// Verification 6: no panic on arbitrary metadata; the kit is complete,
    /// re-readable and its hashes match.
    #[test]
    fn generate_kit_never_panics(meta in arb_meta()) {
        let dir = temp_dir();
        let m = generate_kit(&meta, &dir).unwrap();
        prop_assert_eq!(&KitManifest::read(&dir).unwrap(), &m);
        for e in &m.entries {
            let bytes = std::fs::read(dir.join(&e.path)).unwrap();
            prop_assert_eq!(pin_assetgen::sha256_hex(&bytes), e.sha256_or_hash.clone());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn art_and_music_never_panic(meta in arb_meta()) {
        prop_assert!(art::translite_base(&meta).to_png().is_some());
        prop_assert!(art::translite_logo(&meta).to_png().is_some());
        let mask = art::translite_logo_mask(&meta);
        prop_assert!(mask.data.iter().all(|&v| v == 0 || v == 255));
        let music = sounds::render_music(&meta, 8_000).unwrap();
        prop_assert!(music.iter().all(|v| v.is_finite() && v.abs() <= 0.9));
        let _ = meta_seed(&meta);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn manifest_parser_survives_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let text = String::from_utf8_lossy(&bytes);
        let _ = KitManifest::from_toml(&text);
    }

    #[test]
    fn manifest_parser_survives_mutated_manifests(pos in 0usize..4000, byte in any::<u8>()) {
        let meta = TableMeta { title: "T".into(), palette: vec![[1, 2, 3]], seed: 1 };
        let m = KitManifest {
            entries: vec![pin_assetgen::KitEntry {
                id: "sfx_flipper".into(),
                path: "audio/sfx_flipper.wav".into(),
                kind: pin_assetgen::AssetKind::Sound,
                sha256_or_hash: "00".into(),
            }],
        };
        let mut text = m.to_toml(&meta).into_bytes();
        let p = pos % text.len();
        text[p] = byte;
        let _ = KitManifest::from_toml(&String::from_utf8_lossy(&text));
    }

    #[test]
    fn cli_parser_survives_random_args(args in prop::collection::vec(".{0,12}", 0..8)) {
        let _ = pin_assetgen::cli::parse_args(args);
    }

    #[test]
    fn render_text_shape(text in ".{0,40}") {
        for font in [FONT_5X7, FONT_8X16] {
            let mut out = Vec::new();
            let (w, h) = render_text(font, &text, &mut out);
            let n = text.chars().count() as u32;
            let expect = if n == 0 { 0 } else { n * font.width() + n - 1 };
            prop_assert_eq!(w, expect);
            prop_assert_eq!(h, font.height());
            prop_assert_eq!(out.len(), (w * h) as usize);
            prop_assert!(out.iter().all(|&v| v == 0 || v == 255));
            // Every non-space character lights at least one pixel.
            let lit = out.iter().filter(|&&v| v == 255).count();
            prop_assert_eq!(lit == 0, text.chars().all(|c| c == ' '));
        }
    }

    #[test]
    fn pcg_below_is_in_range(seed in any::<u64>(), stream in any::<u64>(), n in 1u32..) {
        let mut r = Pcg32::new(seed, stream);
        for _ in 0..16 {
            prop_assert!(r.below(n) < n);
        }
    }

    #[test]
    fn sfx_at_any_supported_rate(idx in 0usize..9, rate in 8_000u32..=96_000) {
        let id = sounds::SFX_IDS[idx];
        let v = sounds::render_sfx(id, rate).unwrap();
        prop_assert!(!v.is_empty());
        let peak = v.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        prop_assert!(peak <= 0.9 && peak > 0.5);
    }

    #[test]
    fn png_encoder_round_trips(w in 1u32..24, h in 1u32..24, seed in any::<u64>()) {
        let mut r = Pcg32::new(seed, 1);
        let data: Vec<u8> = (0..w * h * 4).map(|_| r.next_u32() as u8).collect();
        let png = art::encode_png(w, h, art::ColorType::Rgba, &data).unwrap();
        let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        prop_assert_eq!(img.to_rgba8().into_raw(), data);
    }
}
