//! 5.3 Implementer tests, GPU-free: score formatting, scenes, DMD frames,
//! LCD draw lists, routing, change detection and shader validation.

use pin_core::schema::backglass::{
    BackglassManifest, DmdColor, DmdConfig, LampMask, LcdConfig, ScoreDisplay, ScoreDisplayMode,
    Translite, TransliteLayer,
};
use pin_core::schema::table::{LampKind, SceneDef};
use pin_core::script::HostCommandQueue;
use pin_core::{LampId, SceneId};
use pin_render::displays::*;
use pin_render::topology::{
    DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP, DisplayInfo, Rect, SurfaceRole, TopologyMode,
    plan_topology,
};
use proptest::prelude::*;

// ---------- score formatting ----------

#[test]
fn format_score_examples() {
    assert_eq!(format_score(0), "0");
    assert_eq!(format_score(7), "7");
    assert_eq!(format_score(999), "999");
    assert_eq!(format_score(1000), "1,000");
    assert_eq!(format_score(123_456), "123,456");
    assert_eq!(format_score(1_234_567), "1,234,567");
    assert_eq!(format_score(-1_234_567), "-1,234,567");
    assert_eq!(format_score(-999), "-999");
    assert_eq!(format_score(i64::MAX), "9,223,372,036,854,775,807");
    assert_eq!(format_score(i64::MIN), "-9,223,372,036,854,775,808");
    assert_eq!(format_score_with(1_234_567, None), "1234567");
    assert_eq!(format_score_with(1_234_567, Some('.')), "1.234.567");
}

proptest! {
    #[test]
    fn format_score_roundtrips_and_groups(score in any::<i64>()) {
        let s = format_score(score);
        let digits: String = s.chars().filter(|c| *c != ',').collect();
        prop_assert_eq!(digits.parse::<i64>().unwrap(), score);
        let body = s.strip_prefix('-').unwrap_or(&s);
        prop_assert_eq!(s.starts_with('-'), score < 0);
        let groups: Vec<&str> = body.split(',').collect();
        prop_assert!(!groups[0].is_empty() && groups[0].len() <= 3);
        prop_assert!(!groups[0].starts_with('0') || body == "0");
        for g in &groups[1..] {
            prop_assert_eq!(g.len(), 3);
        }
        prop_assert_eq!(format_score_with(score, None), score.to_string());
    }

    #[test]
    fn score_lines_cover_players(scores in prop::array::uniform4(any::<i64>()),
                                 players in 0u8..8, current in 0u8..8, ball in 0u8..6, credits in 0u32..100) {
        let st = ScoreState { scores, players, current_player: current, ball, credits };
        let l = score_lines(&st);
        let n = usize::from(players).clamp(1, 4);
        prop_assert_eq!(l.rows.len(), n);
        prop_assert_eq!(l.rows.iter().filter(|r| r.current).count(), 1);
        for (i, r) in l.rows.iter().enumerate() {
            prop_assert_eq!(r.player as usize, i + 1);
            prop_assert_eq!(&r.text, &format_score(scores[i]));
        }
        prop_assert!(!l.status.is_empty());
        let layout = score_view_layout(&st);
        prop_assert_eq!(layout.len(), n + 1);
        for t in &layout {
            prop_assert!(t.x >= 0.0 && t.x <= 1.0 && t.y >= 0.0 && t.y <= 1.0 && t.size > 0.0);
        }
    }

    #[test]
    fn scene_parser_survives_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = Scene::from_bytes(&bytes);
    }

    #[test]
    fn scene_parser_survives_mutated_scenes(idx in 0usize..6, pos in 0usize..4000, byte in any::<u8>()) {
        let mut text = PLACEHOLDER_SCENES[idx].1.as_bytes().to_vec();
        let p = pos % text.len();
        text[p] = byte;
        let _ = Scene::from_bytes(&text);
    }

    #[test]
    fn color_parser_never_panics(s in ".{0,10}") {
        if let Some(c) = parse_color(&s) {
            prop_assert_eq!(format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]), s.to_ascii_lowercase());
        }
    }

    #[test]
    fn fit_aspect_is_inside_and_maximal(x in -500i32..500, y in -500i32..500, w in 1u32..4000, h in 1u32..4000,
                                        aw in 1u32..2000, ah in 1u32..2000) {
        let vp = Rect::new(x, y, w, h);
        let r = fit_aspect(vp, aw, ah);
        prop_assert!(vp.contains(&r));
        // One side fills the viewport.
        prop_assert!(r.w == w || r.h == h);
        // Aspect within rounding.
        let err = (f64::from(r.w) * f64::from(ah) - f64::from(r.h) * f64::from(aw)).abs();
        prop_assert!(err <= f64::from(aw.max(ah)), "{r:?} vs {aw}:{ah}");
    }

    #[test]
    fn layer_state_opacity_bounded(t in 0u64..10_000, start in 0u64..3000, blink in 0u64..500,
                                   fade_in in 0u64..1000, fade_out in 0u64..1000, alpha in 0.0f32..=1.0) {
        let text = format!(
            "schema_version = 1\nduration_ms = 5000\n[[layers]]\nkind = \"rect\"\nalpha = {alpha}\nstart_ms = {start}\nblink_ms = {blink}\nfade_in_ms = {fade_in}\nfade_out_ms = {fade_out}\nfrom = [0.0, 0.0]\nmove_ms = 500\nx = 1.0\ny = 1.0\n");
        let s = Scene::from_toml_str(&text).unwrap();
        if let Some(st) = s.layers[0].state_at(t, s.duration_ms) {
            prop_assert!(st.opacity > 0.0 && st.opacity <= alpha + 1e-6);
            prop_assert!((0.0..=1.0).contains(&st.x) && (0.0..=1.0).contains(&st.y));
            prop_assert!(t >= start && t < 5000);
        }
    }
}

#[test]
fn score_state_applies_commands() {
    let mut st = ScoreState::default();
    let mut q = HostCommandQueue::default();
    assert!(!st.apply_commands(&q));
    q.set_score(0, 1500).unwrap();
    q.set_score(2, 99).unwrap();
    q.set_score(9, 5).unwrap();
    assert!(st.apply_commands(&q));
    assert_eq!(st.scores, [1500, 0, 99, 0]);
    assert_eq!(st.players, 3);
    assert!(!st.apply_commands(&q));
}

#[test]
fn score_status_lines() {
    let mut st = ScoreState {
        credits: 3,
        ..ScoreState::default()
    };
    assert_eq!(score_lines(&st).status, "CREDITS 3");
    st.ball = 2;
    assert_eq!(score_lines(&st).status, "BALL 2  CREDITS 3");
    st.players = 3;
    st.current_player = 1;
    assert_eq!(score_lines(&st).status, "PLAYER 2  BALL 2");
}

#[test]
fn expand_text_placeholders() {
    let st = ScoreState {
        scores: [1234, 5, 6, 7],
        players: 2,
        current_player: 1,
        ball: 3,
        credits: 9,
    };
    assert_eq!(
        expand_text(
            "{score} {score1} {score4} P{player} B{ball} C{credits} {x} {",
            &st
        ),
        "5 1,234 7 P2 B3 C9 {x} {"
    );
    assert_eq!(expand_text("plain", &st), "plain");
}

// ---------- scenes ----------

#[test]
fn placeholder_scenes_parse_and_cover_brief() {
    for (id, text) in PLACEHOLDER_SCENES {
        let s = Scene::from_toml_str(text).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(!s.layers.is_empty(), "{id}");
    }
    for needed in [
        "scene_powerball_mania",
        "scene_attract",
        "scene_score",
        "scene_game_over",
    ] {
        assert!(placeholder_scene(needed).is_some(), "{needed}");
    }
    assert!(placeholder_scene("nope").is_none());
    let lib = SceneLibrary::placeholders();
    assert_eq!(lib.len(), PLACEHOLDER_SCENES.len());
    assert_eq!(lib.id_of("scene_score"), Some(SceneId(1)));
    assert_eq!(lib.name(SceneId(3)), Some("scene_powerball_mania"));
}

#[test]
fn scene_validation_errors() {
    let bad = [
        ("schema_version = 2", "SchemaVersion"),
        ("schema_version = 1\nfoo = 1", "Parse"),
        ("schema_version = 1\nbackground = \"red\"", "Invalid"),
        ("schema_version = 1\nrepeat = true", "Invalid"),
        ("schema_version = 1\nduration_ms = 99999999", "Invalid"),
        ("schema_version = 1\n[[layers]]\nkind = \"text\"", "Invalid"),
        (
            "schema_version = 1\n[[layers]]\nkind = \"image\"",
            "Invalid",
        ),
        (
            "schema_version = 1\n[[layers]]\nkind = \"rect\"\ntext = \"x\"",
            "Invalid",
        ),
        (
            "schema_version = 1\n[[layers]]\nkind = \"text\"\ntext = \"x\"\nfile = \"a\"",
            "Invalid",
        ),
        (
            "schema_version = 1\n[[layers]]\nkind = \"rect\"\nx = 9.0",
            "Invalid",
        ),
        (
            "schema_version = 1\n[[layers]]\nkind = \"rect\"\nw = -1.0",
            "Invalid",
        ),
        (
            "schema_version = 1\n[[layers]]\nkind = \"text\"\ntext = \"a\"\nsize = 0.0",
            "Invalid",
        ),
        (
            "schema_version = 1\n[[layers]]\nkind = \"rect\"\nalpha = 2.0",
            "Invalid",
        ),
        (
            "schema_version = 1\n[[layers]]\nkind = \"rect\"\nblink_ms = 99999999",
            "Invalid",
        ),
        (
            "schema_version = 1\n[[layers]]\nkind = \"rect\"\ncolor = \"#12345\"",
            "Invalid",
        ),
        (
            "schema_version = 1\n[[layers]]\nkind = \"rect\"\nfrom = [9.0, 0.0]",
            "Invalid",
        ),
        ("schema_version = 1\n[[layers]]\nkind = \"blob\"", "Parse"),
    ];
    for (text, kind) in bad {
        let e = Scene::from_toml_str(text).unwrap_err();
        assert!(format!("{e:?}").starts_with(kind), "{text}: {e:?}");
        assert!(!e.to_string().is_empty());
    }
    let long = format!(
        "schema_version = 1\n[[layers]]\nkind = \"text\"\ntext = \"{}\"",
        "a".repeat(MAX_TEXT_CHARS + 1)
    );
    assert!(Scene::from_toml_str(&long).is_err());
    let many = format!(
        "schema_version = 1\n{}",
        "[[layers]]\nkind = \"rect\"\n".repeat(MAX_SCENE_LAYERS + 1)
    );
    assert!(Scene::from_toml_str(&many).is_err());
    assert_eq!(
        Scene::from_bytes(&vec![b' '; MAX_SCENE_BYTES + 1]),
        Err(SceneError::TooLarge(MAX_SCENE_BYTES + 1))
    );
    assert_eq!(Scene::from_bytes(&[0xff, 0xfe]), Err(SceneError::NotUtf8));
}

#[test]
fn scene_defaults_and_images() {
    let s = Scene::from_toml_str(
        "schema_version = 1\n[[layers]]\nkind = \"text\"\ntext = \"HI\"\n[[layers]]\nkind = \"image\"\nfile = \"a.png\"\nfont = \"5x7\"\n[[layers]]\nkind = \"score\"",
    )
    .unwrap();
    let t = &s.layers[0];
    assert_eq!(
        (t.x, t.y, t.align, t.valign),
        (0.5, 0.5, Align::Center, VAlign::Middle)
    );
    assert_eq!(t.color, [255, 255, 255]);
    let i = &s.layers[1];
    assert_eq!((i.x, i.y, i.w, i.h), (0.0, 0.0, 1.0, 0.0));
    assert_eq!(i.font, FontChoice::Dmd5x7);
    assert_eq!(s.image_files().collect::<Vec<_>>(), ["a.png"]);
    assert_eq!(s.layers[2].kind, LayerKind::Score);
}

#[test]
fn layer_animation_timeline() {
    let s = Scene::from_toml_str(
        "schema_version = 1\nduration_ms = 1000\n[[layers]]\nkind = \"rect\"\nx = 0.5\ny = 0.5\nstart_ms = 100\nblink_ms = 50\nfade_in_ms = 100\nfade_out_ms = 200\nfrom = [0.0, 0.5]\nmove_ms = 100",
    )
    .unwrap();
    let l = &s.layers[0];
    assert!(l.state_at(99, 1000).is_none());
    assert!(
        l.state_at(100, 1000).is_none(),
        "fade-in starts at 0 opacity"
    );
    let a = l.state_at(125, 1000).unwrap();
    assert!((a.opacity - 0.25).abs() < 1e-6 && (a.x - 0.125).abs() < 1e-6);
    assert!(l.state_at(150, 1000).is_none(), "blink off phase");
    let b = l.state_at(300, 1000).unwrap();
    assert_eq!((b.x, b.y, b.opacity), (0.5, 0.5, 1.0));
    let c = l.state_at(900, 1000).unwrap();
    assert!((c.opacity - 0.5).abs() < 1e-6);
    assert!(l.state_at(1000, 1000).is_none());
    // Open-ended scene: no fade-out, never ends.
    assert!(l.state_at(1_000_000, 0).is_some());
}

#[test]
fn scene_library_load_with_fallback() {
    let defs = vec![
        SceneDef {
            id: "scene_custom".into(),
            file: "display/custom.toml".into(),
        },
        SceneDef {
            id: "scene_powerball_mania".into(),
            file: "display/powerball_mania.toml".into(),
        },
    ];
    let lib = SceneLibrary::load(&defs, |path| {
        if path == "display/custom.toml" {
            Ok(b"schema_version = 1\n[[layers]]\nkind = \"rect\"".to_vec())
        } else {
            Err("not found".into())
        }
    })
    .unwrap();
    assert_eq!(lib.id_of("scene_custom"), Some(SceneId(0)));
    assert_eq!(lib.id_of("scene_powerball_mania"), Some(SceneId(1)));
    assert_eq!(
        lib.get(SceneId(1)),
        placeholder_scene("scene_powerball_mania").as_ref()
    );
    // Missing without placeholder → Read error; bad content → InFile.
    let e = SceneLibrary::load(&defs[..1], |_| Err("gone".into())).unwrap_err();
    assert!(matches!(e, SceneError::Read { .. }), "{e:?}");
    let e = SceneLibrary::load(&defs[..1], |_| Ok(b"nonsense".to_vec())).unwrap_err();
    assert!(matches!(e, SceneError::InFile { .. }), "{e}");
    // Re-inserting a name moves it.
    let mut lib = SceneLibrary::new();
    assert!(lib.is_empty());
    let s = placeholder_scene("scene_score").unwrap();
    lib.insert(SceneId(2), "a", s.clone());
    lib.insert(SceneId(2), "b", s);
    assert_eq!(lib.id_of("a"), None);
    assert_eq!(lib.id_of("b"), Some(SceneId(2)));
    assert!(lib.get(SceneId(0)).is_none());
}

#[test]
fn scene_player_timing() {
    let lib = SceneLibrary::placeholders();
    let attract = lib.id_of("scene_attract").unwrap();
    let pm = lib.id_of("scene_powerball_mania").unwrap();
    let mut p = ScenePlayer::new();
    assert!(p.active(&lib, 0).is_none());
    let mut q = HostCommandQueue::default();
    q.show_scene(attract).unwrap();
    q.show_scene(pm).unwrap();
    p.apply_commands(&q, 1000);
    assert_eq!(p.current(), Some((pm, 1000)));
    let (_, e) = p.active(&lib, 3999).unwrap();
    assert_eq!(e, 2999);
    assert!(p.active(&lib, 4000).is_none(), "3 s scene ended");
    p.show(attract, 0);
    let (_, e) = p.active(&lib, 4500).unwrap();
    assert_eq!(e, 500, "repeat wraps");
    p.show(SceneId(999), 0);
    assert!(p.active(&lib, 1).is_none());
    p.clear();
    assert!(p.current().is_none());
    p.apply_commands(&HostCommandQueue::default(), 5);
    assert!(p.current().is_none());
}

// ---------- DMD frames ----------

fn text_scene(text: &str, size: f32, font: &str) -> Scene {
    Scene::from_toml_str(&format!(
        "schema_version = 1\n[[layers]]\nkind = \"text\"\ntext = \"{text}\"\nx = 0.0\ny = 0.0\nalign = \"left\"\nvalign = \"top\"\nsize = {size}\nfont = \"{font}\""
    ))
    .unwrap()
}

#[test]
fn dmd_text_matches_font_bitmap() {
    use pin_assetgen::fonts::{FONT_5X7, FONT_8X16};
    let st = ScoreState::default();
    let images = ImageSet::new();
    for (spec, font, size, bmp) in [
        (DMD_128X32, "5x7", 0.25, FONT_5X7),
        (DMD_128X32, "auto", 0.5, FONT_8X16),
        (DMD_192X64, "8x16", 0.25, FONT_8X16),
    ] {
        let f = DmdFrame::compose(&spec, &text_scene("A1", size, font), 0, &st, &images);
        let mut lit = 0;
        for (i, c) in "A1".chars().enumerate() {
            let g = bmp.glyph(c).unwrap();
            let x0 = i as u32 * (bmp.width() + 1);
            for y in 0..bmp.height() {
                for x in 0..bmp.width() {
                    let expect = if g.pixel(x, y) { [255; 3] } else { [0; 3] };
                    assert_eq!(f.get(x0 + x, y), expect, "{font} ({x0}+{x},{y})");
                    lit += usize::from(g.pixel(x, y));
                }
            }
        }
        assert_eq!(f.lit_count(), lit, "{font}: nothing else lit");
    }
}

#[test]
fn dmd_font_choice_and_scaling() {
    assert_eq!(dmd_text_dots("AB", DmdFont::Font5x7, 1), (11, 7));
    assert_eq!(dmd_text_dots("AB", DmdFont::Font8x16, 2), (34, 32));
    assert_eq!(dmd_text_dots("", DmdFont::Font5x7, 3), (0, 21));
    let st = ScoreState::default();
    let im = ImageSet::new();
    // size 1.0 on 64 dots → 8×16 at scale 4: "A" is 32×64 dots.
    let f = DmdFrame::compose(&DMD_192X64, &text_scene("A", 1.0, "auto"), 0, &st, &im);
    let (mut maxx, mut maxy) = (0, 0);
    for y in 0..64 {
        for x in 0..192 {
            if f.get(x, y)[0] > 0 {
                maxx = maxx.max(x);
                maxy = maxy.max(y);
            }
        }
    }
    assert_eq!(maxy, 59, "8x16 row 14 doubled ×4 ends at 59");
    assert!(maxx < 32);
    // Too wide for 8×16 → falls back to 5×7.
    let long = "ABCDEFGHIJKLMNOPQRST";
    let f = DmdFrame::compose(&DMD_128X32, &text_scene(long, 0.5, "auto"), 0, &st, &im);
    let rows = (0..32)
        .filter(|&y| (0..128).any(|x| f.get(x, y)[0] > 0))
        .count();
    assert!(rows <= 7, "5x7 font used ({rows} rows)");
}

#[test]
fn dmd_mono_quantises_and_rgb_keeps_colour() {
    let scene = Scene::from_toml_str(
        "schema_version = 1\nbackground = \"#000000\"\n[[layers]]\nkind = \"rect\"\nw = 0.5\ncolor = \"#804020\"\n[[layers]]\nkind = \"rect\"\nx = 0.5\nw = 0.5\ncolor = \"#ffffff\"\nalpha = 0.5",
    )
    .unwrap();
    let st = ScoreState::default();
    let im = ImageSet::new();
    let mono = DmdFrame::compose(&DMD_128X32, &scene, 0, &st, &im);
    // 0x80/255 = 0.502 → level 8 → 136.
    assert_eq!(mono.get(0, 0), [136; 3]);
    assert_eq!(mono.get(127, 31), [136; 3]);
    let rgb = DmdSpec::new(128, 32, DmdColor::Rgb).unwrap();
    let f = DmdFrame::compose(&rgb, &scene, 0, &st, &im);
    assert_eq!(f.get(10, 10), [0x80, 0x40, 0x20]);
    assert_eq!(f.get(100, 10), [128, 128, 128]);
    for d in &mono.dots {
        assert!(d[0] % 17 == 0 && d[0] == d[1] && d[1] == d[2]);
    }
}

#[test]
fn dmd_image_and_score_layers() {
    let mut im = ImageSet::new();
    // 2×1 image: left opaque white, right transparent.
    im.insert(
        "a.png",
        RgbaImage::from_rgba(2, 1, vec![255, 255, 255, 255, 255, 255, 255, 0]).unwrap(),
    );
    let scene = Scene::from_toml_str(
        "schema_version = 1\n[[layers]]\nkind = \"image\"\nfile = \"a.png\"\nw = 0.5\nh = 1.0\n[[layers]]\nkind = \"image\"\nfile = \"missing.png\"",
    )
    .unwrap();
    let st = ScoreState::default();
    let f = DmdFrame::compose(&DMD_128X32, &scene, 0, &st, &im);
    assert_eq!(f.get(10, 5), [255; 3]);
    assert_eq!(f.get(40, 5), [0; 3]);
    assert_eq!(f.get(100, 5), [0; 3]);
    // Image keeping aspect: 2×1 at w = 0.25 (32 dots) → 16 rows.
    let s2 = Scene::from_toml_str(
        "schema_version = 1\n[[layers]]\nkind = \"image\"\nfile = \"a.png\"\nw = 0.25",
    )
    .unwrap();
    let f = DmdFrame::compose(&DMD_128X32, &s2, 0, &st, &im);
    assert_eq!(f.lit_count(), 16 * 16);
    // Score view: single player shows the formatted score.
    let mut model = ScoreDisplayModel::new(
        ScoreDisplayConfig::Dmd(DMD_128X32),
        SceneLibrary::placeholders(),
    );
    model.scores_mut().scores[0] = 1_234_567;
    let DisplayContent::Dmd(f) = model.compose(0) else {
        panic!()
    };
    assert!(f.lit_count() > 100);
    // Multi-player: the player up is brighter than the others.
    model.scores_mut().players = 2;
    model.scores_mut().current_player = 1;
    model.scores_mut().scores[1] = 8_888;
    let DisplayContent::Dmd(f) = model.compose(0) else {
        panic!()
    };
    let max_in = |x0: u32, x1: u32| {
        (0..12)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| f.get(x, y)[0])
            .max()
            .unwrap()
    };
    assert_eq!(max_in(64, 128), 255, "player 2 up");
    assert_eq!(max_in(0, 64), 136, "player 1 at half brightness");
}

#[test]
fn dmd_spec_and_geometry() {
    assert!(DmdSpec::new(8, 32, DmdColor::Red).is_err());
    assert!(DmdSpec::new(128, 300, DmdColor::Red).is_err());
    let g = dmd_geometry(&DMD_128X32, Rect::new(10, 20, 1024, 512));
    assert_eq!(g.pitch, 8.0);
    assert_eq!(g.origin, [10.0, 20.0 + (512.0 - 256.0) / 2.0]);
    assert_eq!(g.dot_center(0, 0), [14.0, 152.0]);
    assert!((g.radius - 3.2).abs() < 1e-6);
    assert_eq!(dmd_dot_color(DmdColor::Rgb, [255, 0, 0]), [1.0, 0.1, 0.1]);
    let o = dmd_dot_color(DmdColor::Orange, [0; 3]);
    assert_eq!(o, dmd_tint(DmdColor::Orange).map(|c| c * DMD_UNLIT_LEVEL));
    let mut f = DmdFrame::new(4, 2);
    f.set(3, 1, [1, 2, 3]);
    f.set(9, 9, [9; 3]);
    assert_eq!(f.get(3, 1), [1, 2, 3]);
    assert_eq!(f.get(9, 9), [0; 3]);
    assert_eq!(f.lit_count(), 1);
}

// ---------- LCD ----------

#[test]
fn lcd_compose_items() {
    let spec = LcdSpec::default();
    assert!(LcdSpec::new(8, 1080).is_err());
    let mut im = ImageSet::new();
    im.insert("logo.png", RgbaImage::new(200, 100));
    let scene = Scene::from_toml_str(
        "schema_version = 1\nbackground = \"#102030\"\n[[layers]]\nkind = \"rect\"\nw = 0.5\nh = 0.5\ncolor = \"#ff0000\"\nalpha = 0.5\n[[layers]]\nkind = \"image\"\nfile = \"logo.png\"\nx = 0.5\nw = 0.25\n[[layers]]\nkind = \"text\"\ntext = \"HELLO\"\nsize = 0.2\n[[layers]]\nkind = \"image\"\nfile = \"none.png\"",
    )
    .unwrap();
    let l = LcdDrawList::compose(&spec, &scene, 0, &ScoreState::default(), &im);
    assert_eq!(l.background, [0x10, 0x20, 0x30]);
    assert_eq!(l.items.len(), 3, "missing image without size skipped");
    assert_eq!(
        l.items[0],
        LcdItem::Rect {
            rect: [0.0, 0.0, 960.0, 540.0],
            color: [255, 0, 0, 128]
        }
    );
    assert_eq!(
        l.items[1],
        LcdItem::Image {
            file: "logo.png".into(),
            rect: [960.0, 0.0, 1440.0, 240.0],
            color: [255, 255, 255, 255]
        }
    );
    let LcdItem::Text { origin, cell, .. } = &l.items[2] else {
        panic!()
    };
    assert!((cell - 216.0).abs() < 1e-3);
    let w = vector_text_width("HELLO", 216.0);
    assert!((w - 29.0 * 216.0 / 7.0).abs() < 1e-3);
    assert!((origin[0] - (960.0 - w / 2.0)).abs() < 1e-3);
    assert!((origin[1] - (540.0 - 108.0)).abs() < 1e-3);
    // Long text shrinks to fit the canvas.
    let s = text_scene(&"W".repeat(60), 1.0, "auto");
    let l = LcdDrawList::compose(&spec, &s, 0, &ScoreState::default(), &im);
    let LcdItem::Text { text, cell, .. } = &l.items[0] else {
        panic!()
    };
    assert!(vector_text_width(text, *cell) <= 1920.0 + 1e-3);
    assert_eq!(vector_text_width("", 10.0), 0.0);
}

// ---------- routing / config / change detection ----------

fn display(i: usize, x: i32) -> DisplayInfo {
    DisplayInfo {
        index: i,
        bounds: Rect::new(x, 0, 1920, 1080),
        refresh_hz: 60.0,
        content_scale: 1.0,
    }
}

#[test]
fn routes_pip_and_dedicated() {
    let displays = [display(0, 0), display(1, 1920), display(2, 3840)];
    let single = plan_topology(
        &displays,
        &TopologyMode::SingleMonitor {
            display: 0,
            backglass: DEFAULT_BACKGLASS_PIP,
            score: DEFAULT_SCORE_PIP,
        },
    )
    .unwrap();
    let r = display_routes(&single);
    assert_eq!(r.len(), 2);
    assert!(r.iter().all(|r| r.window == 0 && !r.dedicated));
    assert_eq!(r[0].role, SurfaceRole::Backglass);
    assert_eq!(r[1].role, SurfaceRole::Score);
    assert_eq!(
        Some(r[0].viewport),
        single.windows[0].viewport(SurfaceRole::Backglass)
    );
    let cab = plan_topology(
        &displays,
        &TopologyMode::Cabinet {
            playfield: 0,
            backglass: Some(1),
            score: None,
        },
    )
    .unwrap();
    let r = display_routes(&cab);
    assert_eq!(r[0].role, SurfaceRole::Backglass);
    assert!(r[0].dedicated && r[0].window == 1);
    assert_eq!(r[0].viewport, Rect::new(0, 0, 1920, 1080));
    assert!(!r[1].dedicated && r[1].window == 0);
}

#[test]
fn fit_aspect_examples() {
    assert_eq!(
        fit_aspect(Rect::new(0, 0, 1920, 1080), 4, 3),
        Rect::new(240, 0, 1440, 1080)
    );
    assert_eq!(
        fit_aspect(Rect::new(10, 10, 400, 400), 4, 1),
        Rect::new(10, 160, 400, 100)
    );
    assert_eq!(
        fit_aspect(Rect::new(1, 2, 3, 4), 0, 1),
        Rect::new(1, 2, 3, 4)
    );
}

#[test]
fn score_display_config_from_manifest() {
    let sd = ScoreDisplay {
        mode: ScoreDisplayMode::Dmd,
        dmd: Some(DmdConfig {
            width: 192,
            height: 64,
            color: DmdColor::Rgb,
        }),
        lcd: None,
    };
    let c = ScoreDisplayConfig::from_manifest(&sd).unwrap();
    assert_eq!(
        c,
        ScoreDisplayConfig::Dmd(DmdSpec::new(192, 64, DmdColor::Rgb).unwrap())
    );
    assert_eq!(c.aspect(), (192, 64));
    let lcd = ScoreDisplay {
        mode: ScoreDisplayMode::Lcd,
        dmd: None,
        lcd: None,
    };
    assert_eq!(
        ScoreDisplayConfig::from_manifest(&lcd).unwrap(),
        ScoreDisplayConfig::Lcd(LcdSpec::default())
    );
    let bad = ScoreDisplay {
        mode: ScoreDisplayMode::Lcd,
        dmd: None,
        lcd: Some(LcdConfig {
            width: 0,
            height: 10,
        }),
    };
    assert!(ScoreDisplayConfig::from_manifest(&bad).is_err());
    let dflt = ScoreDisplay {
        mode: ScoreDisplayMode::Dmd,
        dmd: None,
        lcd: None,
    };
    assert_eq!(
        ScoreDisplayConfig::from_manifest(&dflt).unwrap(),
        ScoreDisplayConfig::Dmd(DMD_128X32)
    );
}

#[test]
fn change_tracker_detects_lamps_scores_and_scenes() {
    let mut model = ScoreDisplayModel::new(
        ScoreDisplayConfig::Dmd(DMD_128X32),
        SceneLibrary::placeholders(),
    );
    let mut tr = ChangeTracker::new(vec![LampId(3)]);
    let mut lamps = [0.0f32; 8];
    let d = tr.observe(&lamps, &model.compose(0));
    assert!(d.backglass && d.score);
    assert_eq!(
        tr.observe(&lamps, &model.compose(1)),
        DisplayDirty::default()
    );
    lamps[5] = 1.0; // unwatched lamp
    lamps[3] = 0.0005; // below epsilon
    assert_eq!(
        tr.observe(&lamps, &model.compose(2)),
        DisplayDirty::default()
    );
    lamps[3] = 0.5;
    assert!(tr.observe(&lamps, &model.compose(3)).backglass);
    lamps[3] = 0.5004;
    assert!(!tr.observe(&lamps, &model.compose(3)).backglass);
    lamps[3] = 0.0;
    assert!(
        tr.observe(&lamps, &model.compose(3)).backglass,
        "reaching 0 counts"
    );
    // A score command changes the content.
    let mut q = HostCommandQueue::default();
    q.set_score(0, 100).unwrap();
    model.apply_commands(&q, 10);
    assert!(tr.observe(&lamps, &model.compose(10)).score);
    assert!(!tr.observe(&lamps, &model.compose(11)).score);
    // A blinking scene changes over time, then stays static between toggles.
    let mut q = HostCommandQueue::default();
    q.show_scene(model.library().id_of("scene_ball_saved").unwrap())
        .unwrap();
    model.apply_commands(&q, 100);
    assert!(tr.observe(&lamps, &model.compose(100)).score);
    assert!(!tr.observe(&lamps, &model.compose(200)).score);
    assert!(tr.observe(&lamps, &model.compose(400)).score, "blink off");
    // After the scene the score view returns.
    let a = model.compose(100 + 2000);
    let DisplayContent::Dmd(f) = &a else { panic!() };
    assert!(f.lit_count() > 0);
    assert_eq!(model.config(), ScoreDisplayConfig::Dmd(DMD_128X32));
    assert!(model.scenes().current().is_some());
    model.scenes_mut().clear();
    assert_eq!(model.scores().scores[0], 100);
}

#[test]
fn lcd_model_composes_draw_lists() {
    let mut model = ScoreDisplayModel::new(
        ScoreDisplayConfig::Lcd(LcdSpec::default()),
        SceneLibrary::new(),
    );
    model.images_mut().insert("x.png", RgbaImage::new(1, 1));
    let DisplayContent::Lcd(l) = model.compose(0) else {
        panic!()
    };
    // Single player: score + status.
    assert_eq!(l.items.len(), 2);
    let LcdItem::Text { text, .. } = &l.items[0] else {
        panic!()
    };
    assert_eq!(text, "0");
}

// ---------- backglass description ----------

#[test]
fn backglass_from_manifest() {
    let m = BackglassManifest {
        schema_version: 1,
        translite: Translite {
            layers: vec![
                TransliteLayer {
                    file: "flash.png".into(),
                    depth: 2,
                    lamp_mask: Some("bg_flash".into()),
                },
                TransliteLayer {
                    file: "base.png".into(),
                    depth: 0,
                    lamp_mask: None,
                },
                TransliteLayer {
                    file: "logo.png".into(),
                    depth: 1,
                    lamp_mask: Some("bg_logo".into()),
                },
            ],
        },
        lamp_masks: vec![
            LampMask {
                id: "bg_logo".into(),
                lamp: "backglass_logo".into(),
                file: "logo_mask.png".into(),
            },
            LampMask {
                id: "bg_flash".into(),
                lamp: "flasher_1".into(),
                file: "flash_mask.png".into(),
            },
        ],
        score_display: ScoreDisplay {
            mode: ScoreDisplayMode::Dmd,
            dmd: None,
            lcd: None,
        },
        backbox: None,
    };
    let resolve = |name: &str| match name {
        "backglass_logo" => Some((LampId(4), LampKind::Insert)),
        "flasher_1" => Some((LampId(9), LampKind::Flasher)),
        _ => None,
    };
    let mut loaded = Vec::new();
    let d = BackglassDesc::from_manifest(&m, resolve, |p| {
        loaded.push(p.to_owned());
        Ok(RgbaImage::new(8, 6))
    })
    .unwrap();
    assert_eq!(d.aspect(), (8, 6));
    assert!(d.layers[0].lamp.is_none());
    let l1 = d.layers[1].lamp.as_ref().unwrap();
    assert_eq!((l1.lamp, l1.blend), (LampId(4), LayerBlend::Over));
    let l2 = d.layers[2].lamp.as_ref().unwrap();
    assert_eq!((l2.lamp, l2.blend), (LampId(9), LayerBlend::Additive));
    assert_eq!(
        loaded,
        [
            "base.png",
            "logo.png",
            "logo_mask.png",
            "flash.png",
            "flash_mask.png"
        ]
    );
    // Errors.
    let e = BackglassDesc::from_manifest(&m, |_| None, |_| Ok(RgbaImage::new(1, 1))).unwrap_err();
    assert!(matches!(e, DisplayError::Reference(_)), "{e}");
    let e = BackglassDesc::from_manifest(&m, resolve, |_| Err("io".into())).unwrap_err();
    assert!(matches!(e, DisplayError::Image(..)), "{e}");
    let mut m2 = m.clone();
    m2.lamp_masks.clear();
    assert!(BackglassDesc::from_manifest(&m2, resolve, |_| Ok(RgbaImage::new(1, 1))).is_err());
    m2.translite.layers.clear();
    assert!(BackglassDesc::from_manifest(&m2, resolve, |_| Ok(RgbaImage::new(1, 1))).is_err());
    assert_eq!(BackglassDesc { layers: vec![] }.aspect(), (4, 3));
}

#[test]
fn rgba_image_constructors() {
    assert!(RgbaImage::from_gray(2, 2, vec![0; 3]).is_none());
    assert!(RgbaImage::from_rgb(0, 2, vec![]).is_none());
    assert!(RgbaImage::from_rgba(1, 1, vec![0; 3]).is_none());
    let g = RgbaImage::from_gray(1, 1, vec![7]).unwrap();
    assert_eq!(g.pixel(0, 0), [7, 7, 7, 255]);
    assert_eq!(g.pixel(1, 0), [0; 4]);
    let c = RgbaImage::from_rgb(1, 1, vec![1, 2, 3]).unwrap();
    assert_eq!(c.pixel(0, 0), [1, 2, 3, 255]);
    let mut set = ImageSet::new();
    set.insert("a", c.clone());
    assert_eq!(set.get("a"), Some(&c));
    assert_eq!(set.iter().count(), 1);
    assert!(DisplayError::BadSize("x".into()).to_string().contains("x"));
}

// ---------- shaders ----------

#[test]
fn display_shaders_validate_and_translate() {
    for (name, src) in shader_sources() {
        let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| {
            panic!("{name}: {}", e.emit_to_string(&src));
        });
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        naga::back::spv::write_vec(&module, &info, &Default::default(), None)
            .unwrap_or_else(|e| panic!("{name} spv: {e:?}"));
    }
}

// ---------- review/tester follow-ups (D1, N2, N3) ----------

#[test]
fn from_needs_exactly_two_numbers() {
    for bad in ["[0.0]", "[0.0, 0.0, 0.0]", "[]"] {
        let t = format!("schema_version = 1\n[[layers]]\nkind = \"rect\"\nfrom = {bad}\n");
        assert!(Scene::from_toml_str(&t).is_err(), "{bad}");
    }
    let ok = "schema_version = 1\n[[layers]]\nkind = \"rect\"\nfrom = [0.25, 0.5]\n";
    assert_eq!(
        Scene::from_toml_str(ok).unwrap().layers[0].from,
        Some([0.25, 0.5])
    );
}

#[test]
fn fit_aspect_extremes_do_not_panic() {
    let r = fit_aspect(
        Rect::new(i32::MAX, i32::MAX, u32::MAX, u32::MAX),
        u32::MAX,
        1,
    );
    assert_eq!((r.w, r.x), (u32::MAX, i32::MAX));
    let r = fit_aspect(Rect::new(i32::MIN, 0, u32::MAX, 1), 1, u32::MAX);
    assert_eq!(r.h, 1);
}

proptest! {
    #[test]
    fn fit_aspect_never_panics(x in any::<i32>(), y in any::<i32>(), w in any::<u32>(), h in any::<u32>(),
                               aw in any::<u32>(), ah in any::<u32>()) {
        let r = fit_aspect(Rect::new(x, y, w, h), aw, ah);
        prop_assert!(r.w <= w && r.h <= h);
    }
}

#[test]
fn score_layer_stays_inside_its_box() {
    let spec = DmdSpec::new(192, 64, DmdColor::Orange).unwrap();
    let scene = Scene::from_toml_str(
        "schema_version = 1\n[[layers]]\nkind = \"score\"\nx = 0.5\nw = 0.5\n",
    )
    .unwrap();
    let st = ScoreState {
        scores: [123_456_789, 0, 0, 0],
        ball: 2,
        credits: 1,
        ..ScoreState::default()
    };
    let f = DmdFrame::compose(&spec, &scene, 0, &st, &ImageSet::new());
    assert!(f.lit_count() > 0);
    for y in 0..64 {
        for x in 0..96 {
            assert_eq!(f.get(x, y), [0; 3], "dot ({x},{y}) outside the box");
        }
    }
}

// ---------- fix round 1 (B1, N1, N2, N8) ----------

#[test]
fn invalid_images_are_skipped_by_composition() {
    let mut im = ImageSet::new();
    im.insert("zero.png", RgbaImage::new(0, 0));
    im.insert(
        "short.png",
        RgbaImage {
            width: 8,
            height: 8,
            pixels: vec![255; 10],
        },
    );
    let scene = Scene::from_toml_str(
        "schema_version = 1\n[[layers]]\nkind = \"image\"\nfile = \"zero.png\"\n[[layers]]\nkind = \"image\"\nfile = \"short.png\"\nh = 1.0\n[[layers]]\nkind = \"image\"\nfile = \"short.png\"\n",
    )
    .unwrap();
    let st = ScoreState::default();
    let f = DmdFrame::compose(&DMD_128X32, &scene, 0, &st, &im);
    assert_eq!(f.lit_count(), 0);
    let l = LcdDrawList::compose(&LcdSpec::default(), &scene, 0, &st, &im);
    // Only the explicit-size layer stays in the list; the renderer has no
    // texture for it and skips it.
    assert_eq!(l.items.len(), 1);
    assert!(!RgbaImage::new(0, 5).is_valid());
    assert!(RgbaImage::new(3, 2).is_valid());
    // Byte count overflowing usize: empty buffer, invalid, no panic.
    if usize::BITS == 32 {
        assert!(!RgbaImage::new(u32::MAX, u32::MAX).is_valid());
    }
    let bad = RgbaImage {
        width: 4,
        height: 4,
        pixels: vec![1; 7],
    };
    assert_eq!(bad.pixel(3, 3), [0; 4]);
}

/// The worst parseable scenes: 64 layers of each heavy kind with boxes 4 × 4
/// displays large and the longest texts. Composition work is bounded by the
/// display area per layer. Times are printed for the record only (D8: tests
/// never assert on wall-clock time).
#[test]
fn hostile_scenes_compose_with_bounded_work() {
    let long = "W".repeat(MAX_TEXT_CHARS);
    let mut texts = String::from("schema_version = 1\n");
    let mut scores = String::from("schema_version = 1\n");
    let mut images = String::from("schema_version = 1\n");
    let mut huge = String::from("schema_version = 1\n");
    for i in 0..MAX_SCENE_LAYERS {
        texts += &format!(
            "[[layers]]\nkind = \"text\"\ntext = \"{}\"\nsize = 4.0\nx = 0.{i}\n",
            if i % 2 == 0 { long.as_str() } else { "W" }
        );
        scores += "[[layers]]\nkind = \"score\"\nx = -1.0\ny = -1.0\nw = 4.0\nh = 4.0\n";
        images += "[[layers]]\nkind = \"image\"\nfile = \"big.png\"\nx = -1.0\ny = -1.0\nw = 4.0\nh = 4.0\nalpha = 0.5\n";
        huge += "[[layers]]\nkind = \"text\"\ntext = \"{score}{score}{score}{score}\"\nsize = 4.0\nfont = \"8x16\"\n";
    }
    let mut im = ImageSet::new();
    im.insert(
        "big.png",
        RgbaImage::from_rgba(1024, 1024, vec![200; 1024 * 1024 * 4]).unwrap(),
    );
    let st = ScoreState {
        scores: [i64::MIN, i64::MAX, -1, 1],
        players: 4,
        ball: 255,
        credits: u32::MAX,
        ..ScoreState::default()
    };
    for spec in [DMD_192X64, DmdSpec::new(512, 256, DmdColor::Rgb).unwrap()] {
        for (name, text) in [
            ("text", &texts),
            ("score", &scores),
            ("image", &images),
            ("8x16", &huge),
        ] {
            let scene = Scene::from_toml_str(text).unwrap();
            let t = std::time::Instant::now();
            let f = DmdFrame::compose(&spec, &scene, 0, &st, &im);
            println!(
                "hostile {name} {}x{}: {:?} ({} lit)",
                spec.width,
                spec.height,
                t.elapsed(),
                f.lit_count()
            );
            assert_eq!(f.dots.len(), (spec.width * spec.height) as usize);
            let l = LcdDrawList::compose(&LcdSpec::default(), &scene, 0, &st, &im);
            assert!(l.items.len() <= MAX_SCENE_LAYERS * 5);
        }
    }
}
