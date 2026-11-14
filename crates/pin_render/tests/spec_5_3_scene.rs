//! Independent spec tests for Subphase 5.3 — the display scene format (GPU-free).
//!
//! Written from the brief (`docs/briefs/wave5/5.3.md`) and the published contract (DECISIONS
//! "5.3 — Displays public API" and "5.3 — Display scene format") without reading the
//! implementation. The scene parser reads untrusted bundle content, so it gets a bounded
//! random-bytes run (gate 5): random bytes, random strings, mutated placeholder scenes and
//! random documents built from the format's own keys — and whatever parses must also compose
//! on every display without panicking.

use pin_assetgen::fonts::{FONT_5X7, FONT_8X16, blit_text};
use pin_core::schema::backglass::DmdColor;
use pin_render::displays::{
    DmdFrame, DmdSpec, ImageSet, LcdDrawList, LcdItem, LcdSpec, MAX_SCENE_BYTES,
    PLACEHOLDER_SCENES, RgbaImage, Scene, ScoreState, format_score,
};
use proptest::prelude::*;

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

fn scene(text: &str) -> Scene {
    Scene::from_toml_str(text).unwrap_or_else(|e| panic!("scene did not parse: {e}\n{text}"))
}

fn spec(w: u32, h: u32, color: DmdColor) -> DmdSpec {
    DmdSpec::new(w, h, color).unwrap()
}

fn compose(spec: DmdSpec, scene: &Scene, t: u64, scores: &ScoreState) -> DmdFrame {
    DmdFrame::compose(&spec, scene, t, scores, &ImageSet::new())
}

fn orange(scene_text: &str, t: u64) -> DmdFrame {
    compose(
        spec(128, 32, DmdColor::Orange),
        &scene(scene_text),
        t,
        &ScoreState::default(),
    )
}

fn lcd(scene_text: &str, t: u64, scores: &ScoreState) -> LcdDrawList {
    LcdDrawList::compose(
        &LcdSpec::new(1920, 1080).unwrap(),
        &scene(scene_text),
        t,
        scores,
        &ImageSet::new(),
    )
}

/// Bounding box `(x0, y0, x1, y1)` (exclusive end) of the lit dots.
fn bbox(f: &DmdFrame) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..f.height {
        for x in 0..f.width {
            if f.get(x, y) != [0, 0, 0] {
                b = Some(match b {
                    None => (x, y, x + 1, y + 1),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
                });
            }
        }
    }
    b
}

/// Finds `text` in `frame` as an exact glyph pattern (lit where the glyphs are lit, unlit in
/// the rest of the text box) in either kit font at scales 1..=4: `(8x16?, scale, x, y)`.
fn find_text(frame: &DmdFrame, text: &str) -> Option<(bool, u32, u32, u32)> {
    let (w, h) = (frame.width, frame.height);
    let lit = |x: u32, y: u32| frame.get(x, y) != [0, 0, 0];
    for big in [true, false] {
        let font = if big { FONT_8X16 } else { FONT_5X7 };
        for scale in (1..=4u32).rev() {
            let tw = font.text_width(text) * scale;
            let th = font.height() * scale;
            if tw > w || th > h {
                continue;
            }
            let mut pat = vec![0u8; (tw * th) as usize];
            blit_text(font, text, &mut pat, tw, th, 0, 0, scale, 1);
            for oy in 0..=(h - th) {
                'pos: for ox in 0..=(w - tw) {
                    for y in 0..th {
                        for x in 0..tw {
                            if (pat[(y * tw + x) as usize] != 0) != lit(ox + x, oy + y) {
                                continue 'pos;
                            }
                        }
                    }
                    return Some((big, scale, ox, oy));
                }
            }
        }
    }
    None
}

fn text_scene(extra: &str) -> String {
    format!("schema_version = 1\n[[layers]]\nkind = \"text\"\n{extra}\n")
}

// ---------------------------------------------------------------------------------------
// Parsing: accepted documents
// ---------------------------------------------------------------------------------------

#[test]
fn parses_a_full_document() {
    let s = scene(
        r##"
schema_version = 1
duration_ms = 2500
repeat = true
background = "#102030"

[[layers]]
kind = "rect"
x = 0.0
y = 0.0
w = 1.0
h = 0.25
color = "#ff8000"
alpha = 0.5

[[layers]]
kind = "text"
text = "BALL {ball}"
x = 0.5
y = 0.5
size = 0.5
align = "center"
valign = "middle"
font = "auto"
start_ms = 100
end_ms = 2000
blink_ms = 250
fade_in_ms = 100
fade_out_ms = 100
from = [0.0, 0.5]
move_ms = 500

[[layers]]
kind = "image"
file = "display/logo.png"
w = 0.5
h = 0.0

[[layers]]
kind = "score"
"##,
    );
    assert_eq!(s.duration_ms, 2500);
    assert!(s.repeat);
    assert_eq!(s.background, [0x10, 0x20, 0x30]);
    assert_eq!(s.layers.len(), 4, "layers keep their declaration order");
}

#[test]
fn minimal_documents_and_defaults() {
    let s = scene("schema_version = 1\n");
    assert_eq!(s.duration_ms, 0, "default: until replaced");
    assert!(!s.repeat);
    assert_eq!(s.background, [0, 0, 0]);
    assert!(s.layers.is_empty());
    // An empty scene composes to an empty display.
    let f = compose(
        spec(128, 32, DmdColor::Orange),
        &s,
        0,
        &ScoreState::default(),
    );
    assert_eq!(f.lit_count(), 0);
    let l = LcdDrawList::compose(
        &LcdSpec::new(1920, 1080).unwrap(),
        &s,
        0,
        &ScoreState::default(),
        &ImageSet::new(),
    );
    assert!(l.items.is_empty());
    assert_eq!(l.background, [0, 0, 0]);
    assert_eq!((l.width, l.height), (1920, 1080));

    assert_eq!(
        scene("schema_version = 1\nbackground = \"#FfA000\"\n").background,
        [255, 160, 0]
    );
    let s = scene("schema_version = 1\nduration_ms = 3600000\nrepeat = true\n");
    assert_eq!(
        s.duration_ms, 3_600_000,
        "one hour is the documented maximum"
    );
    // from_bytes = from_toml_str on UTF-8.
    let text = "schema_version = 1\nduration_ms = 7\n";
    assert_eq!(Scene::from_bytes(text.as_bytes()).unwrap(), scene(text));
}

#[test]
fn documented_limits_are_inclusive() {
    // 64 layers.
    let mut doc = String::from("schema_version = 1\n");
    for _ in 0..64 {
        doc.push_str("[[layers]]\nkind = \"rect\"\nw = 0.1\nh = 0.1\n");
    }
    assert_eq!(scene(&doc).layers.len(), 64);
    doc.push_str("[[layers]]\nkind = \"rect\"\n");
    assert!(
        Scene::from_toml_str(&doc).is_err(),
        "65 layers must be refused"
    );
    // 256 characters of text.
    let ok = text_scene(&format!("text = \"{}\"", "A".repeat(256)));
    assert_eq!(scene(&ok).layers.len(), 1);
    let bad = text_scene(&format!("text = \"{}\"", "A".repeat(257)));
    assert!(
        Scene::from_toml_str(&bad).is_err(),
        "257 characters must be refused"
    );
    // Characters, not bytes: 256 two-byte characters are fine.
    let ok = text_scene(&format!("text = \"{}\"", "é".repeat(256)));
    assert!(
        Scene::from_toml_str(&ok).is_ok(),
        "the text limit counts characters"
    );
    // Anchors ±4, boxes 0..=4.
    for (key, v, ok) in [
        ("x", "4.0", true),
        ("x", "-4.0", true),
        ("y", "4.0", true),
        ("x", "4.01", false),
        ("y", "-4.01", false),
        ("w", "4.0", true),
        ("h", "4.0", true),
        ("w", "0.0", true),
        ("w", "4.01", false),
        ("h", "-0.01", false),
        ("alpha", "0.0", true),
        ("alpha", "1.0", true),
        ("alpha", "1.01", false),
        ("alpha", "-0.01", false),
    ] {
        let doc = format!("schema_version = 1\n[[layers]]\nkind = \"rect\"\n{key} = {v}\n");
        assert_eq!(Scene::from_toml_str(&doc).is_ok(), ok, "{key} = {v}");
    }
    // Size limit of the file.
    let base = "schema_version = 1\n# ";
    let at = format!("{base}{}", "x".repeat(MAX_SCENE_BYTES - base.len()));
    assert_eq!(at.len(), MAX_SCENE_BYTES);
    assert_eq!(MAX_SCENE_BYTES, 64 * 1024);
    assert!(
        Scene::from_bytes(at.as_bytes()).is_ok(),
        "exactly 64 KiB is accepted"
    );
    let over = format!("{at}x");
    assert!(
        Scene::from_bytes(over.as_bytes()).is_err(),
        "64 KiB + 1 is refused"
    );
    assert!(Scene::from_bytes(&vec![b' '; 1 << 20]).is_err());
}

// ---------------------------------------------------------------------------------------
// Parsing: rejected documents (never a panic, always an error with a message)
// ---------------------------------------------------------------------------------------

#[test]
fn malformed_documents_are_errors() {
    let layer = |body: &str| format!("schema_version = 1\n[[layers]]\n{body}\n");
    let cases: Vec<(String, &str)> = vec![
        (String::new(), "empty file (schema_version is required)"),
        ("duration_ms = 5\n".into(), "missing schema_version"),
        ("schema_version = 2\n".into(), "unknown schema version"),
        ("schema_version = 0\n".into(), "schema version 0"),
        (
            "schema_version = \"1\"\n".into(),
            "schema version of the wrong type",
        ),
        (
            "schema_version = 1\nbogus = 1\n".into(),
            "unknown top-level key",
        ),
        (
            "schema_version = 1\nduration_ms = -1\n".into(),
            "negative duration",
        ),
        (
            "schema_version = 1\nduration_ms = 3600001\n".into(),
            "duration over one hour",
        ),
        (
            "schema_version = 1\nduration_ms = 1.5\n".into(),
            "fractional duration",
        ),
        (
            "schema_version = 1\nrepeat = true\n".into(),
            "repeat without a duration",
        ),
        (
            "schema_version = 1\nrepeat = 1\nduration_ms = 5\n".into(),
            "repeat of the wrong type",
        ),
        (
            "schema_version = 1\nbackground = \"red\"\n".into(),
            "colour name",
        ),
        (
            "schema_version = 1\nbackground = \"#12345\"\n".into(),
            "short colour",
        ),
        (
            "schema_version = 1\nbackground = \"#1234567\"\n".into(),
            "long colour",
        ),
        (
            "schema_version = 1\nbackground = \"#gg0000\"\n".into(),
            "non-hex colour",
        ),
        (
            "schema_version = 1\nbackground = \"#ééé\"\n".into(),
            "multi-byte colour",
        ),
        (
            "schema_version = 1\nbackground = 5\n".into(),
            "colour of the wrong type",
        ),
        (
            "schema_version = 1\nlayers = 3\n".into(),
            "layers of the wrong type",
        ),
        (
            "schema_version = 1\n[layers]\nkind = \"rect\"\n".into(),
            "layers as a table",
        ),
        (layer("x = 0.5"), "layer without a kind"),
        (layer("kind = \"video\""), "unknown layer kind"),
        (layer("kind = 3"), "kind of the wrong type"),
        (layer("kind = \"rect\"\nbogus = 1"), "unknown layer key"),
        (layer("kind = \"rect\"\nx = nan"), "NaN coordinate"),
        (layer("kind = \"rect\"\ny = inf"), "infinite coordinate"),
        (layer("kind = \"rect\"\nw = -inf"), "negative infinite size"),
        (layer("kind = \"rect\"\nw = nan"), "NaN size"),
        (layer("kind = \"rect\"\nalpha = nan"), "NaN alpha"),
        (
            layer("kind = \"rect\"\ncolor = \"#12\""),
            "bad layer colour",
        ),
        (layer("kind = \"rect\"\nalign = \"middle\""), "bad align"),
        (layer("kind = \"rect\"\nvalign = \"center\""), "bad valign"),
        (
            layer("kind = \"text\"\ntext = \"A\"\nfont = \"comic\""),
            "bad font",
        ),
        (
            layer("kind = \"text\"\ntext = \"A\"\nsize = nan"),
            "NaN text size",
        ),
        (layer("kind = \"text\"\ntext = 5"), "text of the wrong type"),
        (layer("kind = \"rect\"\nstart_ms = -5"), "negative start"),
        (
            layer("kind = \"rect\"\nfrom = [0.0]"),
            "from with one element",
        ),
        (layer("kind = \"rect\"\nfrom = [nan, 0.0]"), "NaN in from"),
        (
            layer("kind = \"rect\"\nfrom = \"left\""),
            "from of the wrong type",
        ),
        (
            layer("kind = \"rect\"\nmove_ms = 1e99"),
            "move_ms of the wrong type",
        ),
        (
            "schema_version = 1\nschema_version = 1\n".into(),
            "duplicate key",
        ),
        (
            "schema_version = 1\n[[layers]\nkind = \"rect\"\n".into(),
            "broken table header",
        ),
        (
            "schema_version = 1\nbackground = \"#000000\n".into(),
            "unterminated string",
        ),
        ("\u{feff}\u{0}\u{0}".into(), "NUL bytes"),
    ];
    let mut accepted = Vec::new();
    for (doc, what) in &cases {
        match Scene::from_toml_str(doc) {
            Ok(_) => accepted.push(*what),
            Err(e) => {
                assert!(!e.to_string().is_empty(), "{what}: empty error message");
                assert!(
                    Scene::from_bytes(doc.as_bytes()).is_err(),
                    "{what} via from_bytes"
                );
            }
        }
    }
    assert!(
        accepted.is_empty(),
        "malformed documents were accepted: {accepted:?}"
    );
    // Not UTF-8.
    for bytes in [
        &[0xff, 0xfe, 0xfd][..],
        &[0xc3][..],
        b"schema_version = 1\n# \xff\n",
        &[0x80; 100],
    ] {
        assert!(Scene::from_bytes(bytes).is_err(), "invalid UTF-8 {bytes:?}");
    }
}

#[test]
fn from_takes_exactly_two_numbers() {
    // "5.3 — Display scene format": `from = [x, y]`.
    let doc = "schema_version = 1\n[[layers]]\nkind = \"rect\"\nfrom = [0.0, 0.0, 0.0]\n";
    assert!(
        Scene::from_toml_str(doc).is_err(),
        "`from` with three elements was accepted"
    );
    let doc = "schema_version = 1\n[[layers]]\nkind = \"rect\"\nfrom = [0.0, 0.0]\nmove_ms = 5\n";
    assert!(Scene::from_toml_str(doc).is_ok());
}

#[test]
fn kind_specific_keys_are_checked() {
    // `text` is for text layers only, `file` for image layers only.
    let doc = "schema_version = 1\n[[layers]]\nkind = \"rect\"\ntext = \"A\"\n";
    assert!(Scene::from_toml_str(doc).is_err(), "text on a rect layer");
    let doc = "schema_version = 1\n[[layers]]\nkind = \"text\"\ntext = \"A\"\nfile = \"a.png\"\n";
    assert!(Scene::from_toml_str(doc).is_err(), "file on a text layer");
}

// ---------------------------------------------------------------------------------------
// Random-bytes robustness (gate 5)
// ---------------------------------------------------------------------------------------

/// Whatever parses must compose everywhere without panicking and keep the frame invariants.
fn compose_everywhere(s: &Scene, t: u64) {
    let scores = ScoreState {
        scores: [i64::MIN, i64::MAX, -1, 1_234_567],
        players: 4,
        current_player: 3,
        ball: 255,
        credits: u32::MAX,
    };
    let mut images = ImageSet::new();
    images.insert(
        "a.png",
        RgbaImage::from_rgba(1, 1, vec![255, 255, 255, 255]).unwrap(),
    );
    for (w, h) in [(128u32, 32u32), (192, 64), (16, 8)] {
        for color in [DmdColor::Orange, DmdColor::Rgb] {
            let f = DmdFrame::compose(&spec(w, h, color), s, t, &scores, &images);
            assert_eq!((f.width, f.height), (w, h));
            assert_eq!(f.dots.len(), (w * h) as usize);
            if color == DmdColor::Orange {
                for d in &f.dots {
                    assert!(
                        d[0] == d[1] && d[1] == d[2] && d[0] % 17 == 0,
                        "mono dot {d:?}"
                    );
                }
            }
        }
    }
    for (w, h) in [(1920u32, 1080u32), (16, 16)] {
        let l = LcdDrawList::compose(
            &LcdSpec::new(w, h).unwrap(),
            s,
            t,
            &scores,
            &ImageSet::new(),
        );
        assert_eq!((l.width, l.height), (w, h));
        assert_eq!(l.background, s.background);
        for item in &l.items {
            match item {
                LcdItem::Rect { rect, .. } | LcdItem::Image { rect, .. } => {
                    assert!(
                        rect.iter().all(|v| v.is_finite()),
                        "non-finite rect {rect:?}"
                    );
                }
                LcdItem::Text { origin, cell, .. } => {
                    assert!(origin.iter().all(|v| v.is_finite()) && cell.is_finite());
                }
            }
        }
    }
}

fn parse_and_compose(bytes: &[u8], t: u64) {
    if let Ok(s) = Scene::from_bytes(bytes) {
        assert!(s.layers.len() <= 64);
        assert!(s.duration_ms <= 3_600_000);
        compose_everywhere(&s, t);
        compose_everywhere(&s, 0);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        let a = Scene::from_toml_str(text);
        if bytes.len() <= MAX_SCENE_BYTES {
            assert_eq!(
                a.is_ok(),
                Scene::from_bytes(bytes).is_ok(),
                "from_bytes ≠ from_toml_str"
            );
        }
    }
}

fn toml_value() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<i64>().prop_map(|v| v.to_string()),
        (-10i64..5000).prop_map(|v| v.to_string()),
        (-6.0f64..6.0).prop_map(|v| format!("{v:?}")),
        (0.0f64..1.0).prop_map(|v| format!("{v:?}")),
        any::<f64>().prop_map(|v| if v.is_finite() {
            format!("{v:e}")
        } else {
            "nan".into()
        }),
        Just("nan".to_string()),
        Just("-nan".to_string()),
        Just("inf".to_string()),
        Just("-inf".to_string()),
        Just("1e308".to_string()),
        Just("-1e-320".to_string()),
        Just("true".to_string()),
        Just("false".to_string()),
        Just("0".to_string()),
        Just("9223372036854775807".to_string()),
        Just("-9223372036854775808".to_string()),
        Just("[0.0, 0.5]".to_string()),
        Just("[-4.0, 4.0]".to_string()),
        Just("[1e30, -1e30]".to_string()),
        Just("[]".to_string()),
        Just("[[1]]".to_string()),
        Just("{ a = 1 }".to_string()),
        Just("1979-05-27T07:32:00Z".to_string()),
        "#[0-9a-fA-F]{6}".prop_map(|s| format!("\"{s}\"")),
        "[ -~]{0,12}".prop_map(|s| format!("{s:?}")),
        "\\PC{0,8}".prop_map(|s| format!("{s:?}")),
        prop_oneof![
            Just("text"),
            Just("rect"),
            Just("image"),
            Just("score"),
            Just("left"),
            Just("center"),
            Just("right"),
            Just("top"),
            Just("middle"),
            Just("bottom"),
            Just("auto"),
            Just("5x7"),
            Just("8x16"),
            Just("a.png"),
            Just("{score}"),
            Just("{score1} {score4} {player} {ball} {credits}"),
            Just("{score5}"),
            Just("{"),
            Just("{}"),
            Just("{{score}}"),
            Just("}{"),
            Just("{score"),
            Just("\\n\\t\\u0000"),
        ]
        .prop_map(|s| format!("\"{s}\"")),
    ]
}

const TOP_KEYS: [&str; 6] = [
    "schema_version",
    "duration_ms",
    "repeat",
    "background",
    "layers",
    "bogus",
];
const LAYER_KEYS: [&str; 22] = [
    "kind",
    "text",
    "file",
    "x",
    "y",
    "w",
    "h",
    "align",
    "valign",
    "size",
    "font",
    "color",
    "alpha",
    "start_ms",
    "end_ms",
    "blink_ms",
    "fade_in_ms",
    "fade_out_ms",
    "from",
    "move_ms",
    "bogus",
    "kind",
];

/// A document built from the format's own keys with mostly-plausible values, so that a good
/// share parses and reaches the compositors with extreme numbers.
fn scene_document() -> impl Strategy<Value = String> {
    let top = proptest::collection::vec((0usize..TOP_KEYS.len(), toml_value()), 0..4);
    let layer = (
        prop_oneof![
            4 => prop_oneof![Just("text"), Just("rect"), Just("image"), Just("score")]
                .prop_map(|k| format!("kind = \"{k}\"\n")),
            1 => Just(String::new()),
        ],
        proptest::collection::vec((0usize..LAYER_KEYS.len(), toml_value()), 0..7),
    );
    let layers = proptest::collection::vec(layer, 0..5);
    (any::<bool>(), top, layers, 0u64..4000, any::<bool>()).prop_map(
        |(versioned, top, layers, duration, repeat)| {
            let mut doc = String::new();
            if versioned {
                doc.push_str("schema_version = 1\n");
                doc.push_str(&format!("duration_ms = {duration}\n"));
                if repeat && duration > 0 {
                    doc.push_str("repeat = true\n");
                }
            }
            for (k, v) in top {
                let key = TOP_KEYS[k];
                if versioned && matches!(key, "schema_version" | "duration_ms" | "repeat") {
                    continue;
                }
                doc.push_str(&format!("{key} = {v}\n"));
            }
            for (kind, keys) in layers {
                doc.push_str("[[layers]]\n");
                doc.push_str(&kind);
                let mut seen: Vec<&str> = if kind.is_empty() {
                    vec![]
                } else {
                    vec!["kind"]
                };
                for (k, v) in keys {
                    let key = LAYER_KEYS[k];
                    if seen.contains(&key) {
                        continue;
                    }
                    seen.push(key);
                    doc.push_str(&format!("{key} = {v}\n"));
                }
            }
            doc
        },
    )
}

/// Well-typed documents (every value in its documented range): these must parse, and the
/// compositors must cope with every combination.
fn valid_document() -> impl Strategy<Value = String> {
    let layer = (
        0usize..4,
        (-4.0f32..=4.0, -4.0f32..=4.0, 0.0f32..=4.0, 0.0f32..=4.0),
        (0usize..3, 0usize..3, 0usize..3),
        (0.0f32..=4.0, 0.0f32..=1.0, "#[0-9a-f]{6}"),
        (0u32..5000, 0u32..5000, 0u32..600, 0u32..2000, 0u32..2000),
        proptest::option::of((-4.0f32..=4.0, -4.0f32..=4.0, 0u32..3000)),
        "[ -~]{0,40}",
    )
        .prop_map(|(kind, (x, y, w, h), (al, val, font), (size, alpha, color), tl, mv, text)| {
            let kinds = ["text", "rect", "image", "score"];
            let mut s = format!(
                "[[layers]]\nkind = \"{}\"\nx = {x:?}\ny = {y:?}\nw = {w:?}\nh = {h:?}\n",
                kinds[kind]
            );
            s.push_str(&format!(
                "align = \"{}\"\nvalign = \"{}\"\ncolor = \"{color}\"\nalpha = {alpha:?}\n",
                ["left", "center", "right"][al],
                ["top", "middle", "bottom"][val]
            ));
            if kind == 0 {
                s.push_str(&format!(
                    "text = {text:?}\nsize = {size:?}\nfont = \"{}\"\n",
                    ["auto", "5x7", "8x16"][font]
                ));
            }
            if kind == 2 {
                s.push_str("file = \"a.png\"\n");
            }
            let (start, end, blink, fi, fo) = tl;
            s.push_str(&format!(
                "start_ms = {start}\nend_ms = {end}\nblink_ms = {blink}\nfade_in_ms = {fi}\nfade_out_ms = {fo}\n"
            ));
            if let Some((fx, fy, ms)) = mv {
                s.push_str(&format!("from = [{fx:?}, {fy:?}]\nmove_ms = {ms}\n"));
            }
            s
        });
    (
        0u64..=5000,
        any::<bool>(),
        "#[0-9a-f]{6}",
        proptest::collection::vec(layer, 0..6),
    )
        .prop_map(|(duration, repeat, bg, layers)| {
            let mut doc =
                format!("schema_version = 1\nduration_ms = {duration}\nbackground = \"{bg}\"\n");
            if repeat && duration > 0 {
                doc.push_str("repeat = true\n");
            }
            for l in layers {
                doc.push_str(&l);
            }
            doc
        })
}

fn mutation() -> impl Strategy<Value = (usize, usize, u8, u8, usize)> {
    (
        0usize..PLACEHOLDER_SCENES.len(),
        any::<usize>(),
        0u8..6,
        any::<u8>(),
        1usize..40,
    )
}

fn mutate(src: &str, pos: usize, op: u8, byte: u8, n: usize) -> Vec<u8> {
    let mut b = src.as_bytes().to_vec();
    let p = pos % (b.len() + 1);
    match op {
        0 => b.truncate(p),
        1 => {
            if p < b.len() {
                b[p] = byte;
            }
        }
        2 => b.insert(p, byte),
        3 => {
            let e = (p + n).min(b.len());
            b.drain(p..e);
        }
        4 => {
            let e = (p + n).min(b.len());
            let chunk = b[p..e].to_vec();
            for _ in 0..3 {
                b.splice(p..p, chunk.iter().copied());
            }
        }
        _ => {
            if p < b.len() {
                b[p] ^= 1 << (byte % 8);
            }
        }
    }
    b
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1500))]

    #[test]
    fn prop_random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048), t in any::<u64>()) {
        parse_and_compose(&bytes, t);
    }

    #[test]
    fn prop_random_strings_never_panic(text in "\\PC{0,400}", t in any::<u64>()) {
        let _ = Scene::from_toml_str(&text);
        parse_and_compose(text.as_bytes(), t);
    }

    #[test]
    fn prop_toml_like_noise_never_panics(text in "[a-z_ =\\[\\]\"#0-9.,\\n{}+-]{0,300}", t in any::<u64>()) {
        parse_and_compose(text.as_bytes(), t);
    }

    #[test]
    fn prop_mutated_placeholders_never_panic((which, pos, op, byte, n) in mutation(), t in 0u64..10_000) {
        let bytes = mutate(PLACEHOLDER_SCENES[which].1, pos, op, byte, n);
        parse_and_compose(&bytes, t);
    }

    #[test]
    fn prop_scene_shaped_documents_never_panic(doc in scene_document(), t in prop_oneof![0u64..6000, any::<u64>()]) {
        parse_and_compose(doc.as_bytes(), t);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    #[test]
    fn prop_valid_documents_parse_and_compose(doc in valid_document(), t in prop_oneof![0u64..6000, any::<u64>()]) {
        let s = Scene::from_toml_str(&doc);
        prop_assert!(s.is_ok(), "a document within the documented ranges was refused: {:?}\n{}", s.err().map(|e| e.to_string()), doc);
        let s = s.unwrap();
        compose_everywhere(&s, t);
        compose_everywhere(&s, 0);
        // Deterministic.
        let a = compose(spec(128, 32, DmdColor::Rgb), &s, t, &ScoreState::default());
        let b = compose(spec(128, 32, DmdColor::Rgb), &s, t, &ScoreState::default());
        prop_assert_eq!(a, b);
    }
}

#[test]
fn pathological_documents_do_not_hang_or_panic() {
    let deep_array = format!(
        "schema_version = 1\nx = {}{}\n",
        "[".repeat(20_000),
        "]".repeat(20_000)
    );
    let deep_table = format!(
        "schema_version = 1\nx = {}1{}\n",
        "{a=".repeat(5_000),
        "}".repeat(5_000)
    );
    let many_keys: String = (0..5_000).map(|i| format!("k{i} = {i}\n")).collect();
    let long_key = format!("{} = 1\n", "k".repeat(60_000));
    let dotted = format!("{} = 1\n", vec!["a"; 20_000].join("."));
    let big_text = text_scene(&format!("text = \"{}\"", "{score}".repeat(8_000)));
    for doc in [
        deep_array, deep_table, many_keys, long_key, dotted, big_text,
    ] {
        let _ = Scene::from_toml_str(&doc);
        let _ = Scene::from_bytes(doc.as_bytes());
    }
    // The longest legal expansion: 256 characters of placeholders with huge scores.
    let doc = text_scene(&format!(
        "text = \"{}\"\nsize = 4.0\nw = 4.0",
        "{score1}{score2}".repeat(16)
    ));
    let s = scene(&doc);
    compose_everywhere(&s, 0);
}

// ---------------------------------------------------------------------------------------
// Semantics on the DMD: text placement, fonts, placeholders
// ---------------------------------------------------------------------------------------

#[test]
fn text_layer_is_centred_by_default_in_the_kit_font() {
    // size = 7/32 of the height → 7 dots: the 5×7 font at scale 1.
    let f = orange(
        &text_scene("text = \"HI\"\nsize = 0.21875\nfont = \"5x7\""),
        0,
    );
    let (big, scale, x, y) = find_text(&f, "HI").expect("HI as 5x7 dots");
    assert!(!big);
    assert_eq!(scale, 1);
    // "HI" is 11 × 7 dots; centred in 128 × 32 → x = 58 or 59, y = 12 or 13.
    assert!((58..=59).contains(&x), "x = {x}");
    assert!((12..=13).contains(&y), "y = {y}");
    assert_eq!(f.lit_count(), {
        let mut pat = vec![0u8; 11 * 7];
        blit_text(FONT_5X7, "HI", &mut pat, 11, 7, 0, 0, 1, 1);
        pat.iter().filter(|b| **b != 0).count()
    });
    // Full drive by default (white, alpha 1).
    let (x0, y0, _, _) = bbox(&f).unwrap();
    assert_eq!((x0, y0), (x, y));
    assert!(
        f.dots
            .iter()
            .all(|d| *d == [0, 0, 0] || *d == [255, 255, 255])
    );
}

#[test]
fn text_size_selects_integer_scale_and_font() {
    // Forced 5x7: scale = floor(size · H / 7).
    for (size, want_scale) in [
        (7.0f32, 1u32),
        (13.0, 1),
        (14.0, 2),
        (21.0, 3),
        (28.0, 4),
        (3.0, 1),
    ] {
        let doc = text_scene(&format!(
            "text = \"A1\"\nsize = {:?}\nfont = \"5x7\"",
            size / 32.0
        ));
        let (big, scale, _, _) = find_text(&orange(&doc, 0), "A1").expect("A1");
        assert!(!big);
        assert_eq!(scale, want_scale, "5x7 at {size} dots");
    }
    // Forced 8x16 on the 192 × 64 display: scale = floor(size · H / 16).
    for (size, want_scale) in [(16.0f32, 1u32), (31.0, 1), (32.0, 2), (48.0, 3)] {
        let doc = text_scene(&format!(
            "text = \"A1\"\nsize = {:?}\nfont = \"8x16\"",
            size / 64.0
        ));
        let f = compose(
            spec(192, 64, DmdColor::Red),
            &scene(&doc),
            0,
            &ScoreState::default(),
        );
        let (big, scale, _, _) = find_text(&f, "A1").expect("A1");
        assert!(big);
        assert_eq!(scale, want_scale, "8x16 at {size} dots");
    }
    // Auto: 8×16 when the height allows it and the text fits the width …
    let doc = text_scene("text = \"SCORE\"\nsize = 0.5");
    let (big, scale, _, _) = find_text(&orange(&doc, 0), "SCORE").unwrap();
    assert_eq!((big, scale), (true, 1), "16 dots high → 8x16 at scale 1");
    // … 5×7 when the layer is lower than 16 dots …
    let doc = text_scene("text = \"SCORE\"\nsize = 0.45");
    let (big, scale, _, _) = find_text(&orange(&doc, 0), "SCORE").unwrap();
    assert_eq!((big, scale), (false, 2), "14 dots high → 5x7 at scale 2");
    // … and when 8×16 would not fit the width: 15 glyphs are 134 dots in 8×16; 5×7 at
    // scale 2 is 178 dots, so it shrinks to scale 1 (89 dots).
    let doc = text_scene("text = \"ABCDEFGHIJKLMNO\"\nsize = 0.5");
    let (big, scale, x, _) = find_text(&orange(&doc, 0), "ABCDEFGHIJKLMNO").unwrap();
    assert_eq!((big, scale), (false, 1));
    assert!((19..=20).contains(&x), "still centred: x = {x}");
    // Both sizes of display show the same scene.
    let doc = text_scene("text = \"SCORE\"\nsize = 0.5");
    let f = compose(
        spec(192, 64, DmdColor::Orange),
        &scene(&doc),
        0,
        &ScoreState::default(),
    );
    let (big, scale, x, y) = find_text(&f, "SCORE").unwrap();
    assert_eq!(
        (big, scale),
        (true, 2),
        "32 dots high on 192 × 64 → 8x16 at scale 2"
    );
    // 5 glyphs: (5·9 − 1)·2 = 88 dots wide, 32 high → centred at (52, 16).
    assert!(
        (51..=52).contains(&x) && (15..=16).contains(&y),
        "({x}, {y})"
    );
}

#[test]
fn text_alignment_anchors() {
    let tw = 11u32; // "HI" in 5x7
    let th = 7u32;
    for (align, ax, want_x) in [
        ("left", 0.0f32, 0u32),
        ("right", 1.0, 128 - tw),
        ("left", 0.25, 32),
        ("right", 0.75, 96 - tw),
        ("center", 0.25, 32 - tw / 2 - 1),
    ] {
        for (valign, ay, want_y) in [
            ("top", 0.0f32, 0u32),
            ("bottom", 1.0, 32 - th),
            ("top", 0.5, 16),
            ("bottom", 0.5, 16 - th),
        ] {
            let doc = text_scene(&format!(
                "text = \"HI\"\nsize = 0.21875\nfont = \"5x7\"\nx = {ax:?}\ny = {ay:?}\nalign = \"{align}\"\nvalign = \"{valign}\""
            ));
            let (_, _, x, y) = find_text(&orange(&doc, 0), "HI")
                .unwrap_or_else(|| panic!("{align}/{valign} at ({ax}, {ay}): text missing"));
            let tol = u32::from(align == "center");
            assert!(
                x.abs_diff(want_x) <= tol,
                "{align} at x = {ax}: text starts at {x}, want {want_x}"
            );
            assert_eq!(y, want_y, "{valign} at y = {ay}");
        }
    }
    // Text partly or wholly off the display is clipped, not a panic.
    for (x, y) in [
        (-4.0f32, -4.0f32),
        (4.0, 4.0),
        (0.0, 0.5),
        (1.0, 0.5),
        (0.5, 0.0),
        (0.5, 1.0),
    ] {
        let doc = text_scene(&format!(
            "text = \"CLIPPED TEXT\"\nsize = 1.0\nx = {x:?}\ny = {y:?}"
        ));
        let f = orange(&doc, 0);
        if x.abs() > 2.0 {
            assert_eq!(f.lit_count(), 0, "text at ({x}, {y}) is off the display");
        }
    }
}

#[test]
fn text_placeholders_expand_from_the_score_state() {
    let scores = ScoreState {
        scores: [1_234_567, 20, -3_000, 44_000],
        players: 4,
        current_player: 1,
        ball: 3,
        credits: 7,
    };
    let sp = spec(192, 64, DmdColor::Orange);
    let show = |text: &str| {
        let doc = text_scene(&format!("text = \"{text}\"\nsize = 0.125\nfont = \"5x7\""));
        compose(sp, &scene(&doc), 0, &scores)
    };
    let cases = [
        ("{score1}", "1,234,567".to_string()),
        ("{score2}", "20".to_string()),
        ("{score3}", "-3,000".to_string()),
        ("{score4}", "44,000".to_string()),
        ("{score}", format_score(20)), // the current player's (player 2)
        ("P{player}", "P2".to_string()),
        ("BALL {ball}", "BALL 3".to_string()),
        ("CR {credits}", "CR 7".to_string()),
        ("{ball}-{ball}", "3-3".to_string()),
        ("NO PLACEHOLDER", "NO PLACEHOLDER".to_string()),
    ];
    for (template, want) in cases {
        let f = show(template);
        assert!(
            find_text(&f, &want).is_some(),
            "{template:?} should show {want:?}"
        );
    }
    // The LCD gets the same expansion.
    let doc = text_scene("text = \"{score1} / {score} / P{player} B{ball} C{credits}\"");
    let l = LcdDrawList::compose(
        &LcdSpec::new(1920, 1080).unwrap(),
        &scene(&doc),
        0,
        &scores,
        &ImageSet::new(),
    );
    let texts: Vec<&str> = l
        .items
        .iter()
        .filter_map(|i| match i {
            LcdItem::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["1,234,567 / 20 / P2 B3 C7"]);
}

// ---------------------------------------------------------------------------------------
// Semantics on the DMD: rect, colour, alpha, timeline
// ---------------------------------------------------------------------------------------

fn rect_scene(extra: &str) -> String {
    format!("schema_version = 1\n[[layers]]\nkind = \"rect\"\n{extra}\n")
}

#[test]
fn rect_layer_covers_exactly_its_box() {
    let f = orange(&rect_scene("x = 0.25\ny = 0.5\nw = 0.5\nh = 0.5"), 0);
    assert_eq!(bbox(&f), Some((32, 16, 96, 32)));
    assert_eq!(f.lit_count(), 64 * 16, "solid");
    // Default box: the whole display.
    let f = orange(&rect_scene(""), 0);
    assert_eq!(f.lit_count(), 128 * 32);
    // Same fractions on the bigger display.
    let s = scene(&rect_scene("x = 0.25\ny = 0.5\nw = 0.5\nh = 0.5"));
    let f = compose(spec(192, 64, DmdColor::Red), &s, 0, &ScoreState::default());
    assert_eq!(bbox(&f), Some((48, 32, 144, 64)));
    // Alignment moves the box relative to its anchor.
    let f = orange(
        &rect_scene("x = 0.5\ny = 0.5\nw = 0.25\nh = 0.5\nalign = \"center\"\nvalign = \"middle\""),
        0,
    );
    assert_eq!(bbox(&f), Some((48, 8, 80, 24)));
    let f = orange(
        &rect_scene("x = 1.0\ny = 1.0\nw = 0.25\nh = 0.25\nalign = \"right\"\nvalign = \"bottom\""),
        0,
    );
    assert_eq!(bbox(&f), Some((96, 24, 128, 32)));
    // Boxes partly off the display are clipped; empty boxes draw nothing.
    let f = orange(&rect_scene("x = -0.5\ny = -0.5\nw = 1.0\nh = 1.0"), 0);
    assert_eq!(bbox(&f), Some((0, 0, 64, 16)));
    assert_eq!(orange(&rect_scene("w = 0.0"), 0).lit_count(), 0);
    assert_eq!(orange(&rect_scene("x = 2.0"), 0).lit_count(), 0);
    // Layers composite back to front: a black rect over a white one cuts a hole.
    let doc = "schema_version = 1\n[[layers]]\nkind = \"rect\"\n[[layers]]\nkind = \"rect\"\nx = 0.5\nw = 0.5\ncolor = \"#000000\"\n";
    let f = orange(doc, 0);
    assert_eq!(
        bbox(&f),
        Some((0, 0, 64, 32)),
        "the later (black) layer covers the right half"
    );
}

#[test]
fn colour_and_alpha_map_to_drive_levels() {
    // Mono: brightest channel, 16 levels (v = round(b · 15) · 17).
    for (color, want) in [
        ("#ffffff", 255u8),
        ("#ff0000", 255),
        ("#0000ff", 255),
        ("#808080", 136),
        ("#004000", 68),
        ("#111111", 17),
        ("#000000", 0),
    ] {
        let f = orange(&rect_scene(&format!("color = \"{color}\"")), 0);
        assert_eq!(f.get(5, 5), [want; 3], "mono drive for {color}");
    }
    for (alpha, want) in [(1.0f32, 255u8), (0.8, 204), (0.4, 102), (0.2, 51), (0.0, 0)] {
        let f = orange(&rect_scene(&format!("alpha = {alpha:?}")), 0);
        assert_eq!(f.get(100, 20), [want; 3], "mono drive for alpha {alpha}");
    }
    // RGB: the dot takes the layer colour.
    let s = scene(&rect_scene("color = \"#ff8000\""));
    let f = compose(spec(128, 32, DmdColor::Rgb), &s, 0, &ScoreState::default());
    assert_eq!(f.get(0, 0), [255, 128, 0]);
    let s = scene(&rect_scene("color = \"#00ff00\"\nalpha = 0.4"));
    let f = compose(spec(128, 32, DmdColor::Rgb), &s, 0, &ScoreState::default());
    let d = f.get(64, 16);
    assert!(
        d[0] == 0 && d[2] == 0 && (100..=104).contains(&d[1]),
        "green at 40 %: {d:?}"
    );
}

#[test]
fn timeline_start_end_blink_and_scene_duration() {
    let lit = |doc: &str, t: u64| orange(doc, t).lit_count() > 0;
    // start_ms / end_ms window.
    let doc = format!(
        "duration_ms = 1000\n{}",
        rect_scene("start_ms = 100\nend_ms = 200")
    );
    assert!(!lit(&doc, 0));
    assert!(!lit(&doc, 99));
    assert!(lit(&doc, 100));
    assert!(lit(&doc, 150));
    assert!(lit(&doc, 199));
    assert!(!lit(&doc, 201));
    assert!(!lit(&doc, 999));
    // end_ms = 0 means "until the scene ends".
    let doc = format!("duration_ms = 1000\n{}", rect_scene("start_ms = 100"));
    assert!(!lit(&doc, 50));
    assert!(lit(&doc, 100));
    assert!(lit(&doc, 999));
    // Blink: on and off for blink_ms each, period 2 · blink_ms.
    let doc = rect_scene("blink_ms = 100");
    let states: Vec<bool> = (0..8).map(|i| lit(&doc, 50 + 100 * i)).collect();
    for i in 0..7 {
        assert_ne!(
            states[i],
            states[i + 1],
            "blink toggles every 100 ms: {states:?}"
        );
    }
    assert!(states[0], "a blinking layer starts in its on phase");
    assert_eq!(
        lit(&doc, 10),
        lit(&doc, 90),
        "constant within a half period"
    );
}

#[test]
fn timeline_fades_are_monotone() {
    let level = |doc: &str, t: u64| orange(doc, t).get(10, 10)[0];
    let doc = format!("duration_ms = 2000\n{}", rect_scene("fade_in_ms = 1000"));
    let levels: Vec<u8> = (0..=12).map(|i| level(&doc, i * 100)).collect();
    assert!(levels[0] <= 17, "a fade-in starts dark: {levels:?}");
    assert!(
        levels.windows(2).all(|w| w[0] <= w[1]),
        "fade-in is monotone: {levels:?}"
    );
    assert_eq!(levels[10], 255, "full after fade_in_ms: {levels:?}");
    assert_eq!(levels[12], 255);
    let mid = levels[5];
    assert!(
        (102..=153).contains(&mid),
        "about half way at 500 ms: {mid}"
    );
    assert!(levels.iter().all(|l| l % 17 == 0));

    let doc = format!("duration_ms = 2000\n{}", rect_scene("fade_out_ms = 1000"));
    let levels: Vec<u8> = (0..20).map(|i| level(&doc, i * 100)).collect();
    assert_eq!(levels[0], 255);
    assert_eq!(
        levels[10], 255,
        "the fade-out covers the last fade_out_ms: {levels:?}"
    );
    assert!(
        levels.windows(2).all(|w| w[0] >= w[1]),
        "fade-out is monotone: {levels:?}"
    );
    assert!(levels[19] <= 34, "nearly dark at the end: {levels:?}");
    let mid = levels[15];
    assert!((102..=153).contains(&mid), "about half way: {mid}");
}

#[test]
fn timeline_moves_linearly_from_the_start_position() {
    // A 16 × 8 dot box sliding from the left edge to the right edge in 1 s.
    let doc = format!(
        "duration_ms = 3000\n{}",
        rect_scene("x = 0.875\ny = 0.0\nw = 0.125\nh = 0.25\nfrom = [0.0, 0.75]\nmove_ms = 1000")
    );
    let pos = |t: u64| {
        let b = bbox(&orange(&doc, t)).unwrap_or_else(|| panic!("box missing at {t} ms"));
        assert_eq!(
            (b.2 - b.0, b.3 - b.1),
            (16, 8),
            "the box keeps its size at {t} ms"
        );
        (b.0, b.1)
    };
    assert_eq!(pos(0), (0, 24), "starts at `from`");
    assert_eq!(pos(1000), (112, 0), "arrives at (x, y) after move_ms");
    assert_eq!(pos(2500), (112, 0), "and stays there");
    let (x, y) = pos(500);
    assert!(
        (55..=57).contains(&x) && (11..=13).contains(&y),
        "half way at 500 ms: ({x}, {y})"
    );
    let mut prev = pos(0);
    for t in (50..=1000).step_by(50) {
        let p = pos(t);
        assert!(
            p.0 >= prev.0 && p.1 <= prev.1,
            "monotone move at {t} ms: {prev:?} → {p:?}"
        );
        // Linear: 112 dots per second → 5.6 dots per 50 ms.
        assert!(
            (4..=7).contains(&(p.0 - prev.0)),
            "x step {} at {t} ms",
            p.0 - prev.0
        );
        prev = p;
    }
}

// ---------------------------------------------------------------------------------------
// Image and score layers
// ---------------------------------------------------------------------------------------

#[test]
fn image_layers_draw_the_named_image_or_nothing() {
    let doc = "schema_version = 1\n[[layers]]\nkind = \"image\"\nfile = \"display/logo.png\"\nx = 0.5\nw = 0.5\nh = 1.0\n";
    let s = scene(doc);
    let sp = spec(128, 32, DmdColor::Orange);
    // Missing image: nothing drawn, no panic.
    let f = DmdFrame::compose(&sp, &s, 0, &ScoreState::default(), &ImageSet::new());
    assert_eq!(f.lit_count(), 0);
    // A white image fills the box; a transparent one draws nothing.
    let mut images = ImageSet::new();
    images.insert(
        "display/logo.png",
        RgbaImage::from_gray(4, 4, vec![255; 16]).unwrap(),
    );
    assert!(images.get("display/logo.png").is_some());
    assert!(images.get("display/other.png").is_none());
    let f = DmdFrame::compose(&sp, &s, 0, &ScoreState::default(), &images);
    assert_eq!(bbox(&f), Some((64, 0, 128, 32)));
    assert_eq!(f.lit_count(), 64 * 32);
    let mut images = ImageSet::new();
    images.insert("display/logo.png", RgbaImage::new(4, 4));
    let f = DmdFrame::compose(&sp, &s, 0, &ScoreState::default(), &images);
    assert_eq!(f.lit_count(), 0, "a fully transparent image lights nothing");
    // A left-white / right-black image keeps its orientation.
    let mut px = Vec::new();
    for _ in 0..4 {
        px.extend_from_slice(&[255, 255, 0, 0]);
    }
    let mut images = ImageSet::new();
    images.insert("display/logo.png", RgbaImage::from_gray(4, 4, px).unwrap());
    let f = DmdFrame::compose(&sp, &s, 0, &ScoreState::default(), &images);
    assert_eq!(
        bbox(&f),
        Some((64, 0, 96, 32)),
        "left half of the image box"
    );
    // h = 0 keeps the image's aspect: a 2:1 image 0.25 wide (32 dots) is 16 dots high.
    let doc =
        "schema_version = 1\n[[layers]]\nkind = \"image\"\nfile = \"a.png\"\nw = 0.25\nh = 0.0\n";
    let mut images = ImageSet::new();
    images.insert("a.png", RgbaImage::from_gray(8, 4, vec![255; 32]).unwrap());
    let f = DmdFrame::compose(&sp, &scene(doc), 0, &ScoreState::default(), &images);
    assert_eq!(bbox(&f), Some((0, 0, 32, 16)));
    // On the LCD the layer becomes an Image item naming the file, in canvas pixels.
    let l = LcdDrawList::compose(
        &LcdSpec::new(1920, 1080).unwrap(),
        &s,
        0,
        &ScoreState::default(),
        &ImageSet::new(),
    );
    assert_eq!(l.items.len(), 1);
    match &l.items[0] {
        LcdItem::Image { file, rect, color } => {
            assert_eq!(file, "display/logo.png");
            assert_eq!(*rect, [960.0, 0.0, 1920.0, 1080.0]);
            assert_eq!(color[3], 255);
        }
        other => panic!("expected an image item, got {other:?}"),
    }
}

#[test]
fn score_layer_draws_the_score_view() {
    let scores = ScoreState {
        scores: [1_234_560, 0, 0, 0],
        players: 1,
        current_player: 0,
        ball: 2,
        credits: 1,
    };
    let doc = "schema_version = 1\n[[layers]]\nkind = \"score\"\n";
    for (w, h) in [(128u32, 32u32), (192, 64)] {
        let f = compose(spec(w, h, DmdColor::Orange), &scene(doc), 0, &scores);
        assert!(
            find_text(&f, "1,234,560").is_some(),
            "{w}x{h}: the score view shows the formatted score"
        );
        // Changing the score changes the frame; other players' scores do not show for a
        // one-player game.
        let mut other = scores.clone();
        other.scores[0] = 1_234_561;
        assert_ne!(
            compose(spec(w, h, DmdColor::Orange), &scene(doc), 0, &other),
            f
        );
        let mut hidden = scores.clone();
        hidden.scores[3] = 999;
        assert_eq!(
            compose(spec(w, h, DmdColor::Orange), &scene(doc), 0, &hidden),
            f
        );
    }
    // On the 192 × 64 display the status line fits too.
    let f = compose(spec(192, 64, DmdColor::Orange), &scene(doc), 0, &scores);
    assert!(
        find_text(&f, "BALL 2  CREDITS 1").is_some(),
        "192x64: the status line is shown"
    );
    // The LCD list carries the same strings.
    let l = lcd(doc, 0, &scores);
    let texts: Vec<String> = l
        .items
        .iter()
        .filter_map(|i| match i {
            LcdItem::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t == "1,234,560"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "BALL 2  CREDITS 1"), "{texts:?}");
    // A score layer in a box stays inside the box (short status line: every line fits the
    // 96-dot box at scale 1).
    let idle = ScoreState {
        ball: 0,
        ..scores.clone()
    };
    let doc =
        "schema_version = 1\n[[layers]]\nkind = \"score\"\nx = 0.5\ny = 0.0\nw = 0.5\nh = 1.0\n";
    let f = compose(spec(192, 64, DmdColor::Orange), &scene(doc), 0, &idle);
    let b = bbox(&f).expect("score view drawn");
    assert!(b.0 >= 96, "the score view left its box: {b:?}");
    assert!(find_text(&f, "1,234,560").is_some());
}

// ---------------------------------------------------------------------------------------
// LCD draw lists from scenes
// ---------------------------------------------------------------------------------------

#[test]
fn lcd_draw_list_follows_the_scene_layers_in_order() {
    let doc = r##"
schema_version = 1
background = "#203040"
[[layers]]
kind = "rect"
x = 0.25
y = 0.5
w = 0.5
h = 0.25
color = "#ff8000"
alpha = 0.4
[[layers]]
kind = "text"
text = "HELLO"
x = 0.0
y = 0.0
align = "left"
valign = "top"
size = 0.1
color = "#00ff00"
[[layers]]
kind = "rect"
w = 0.1
h = 0.1
"##;
    let l = lcd(doc, 0, &ScoreState::default());
    assert_eq!((l.width, l.height), (1920, 1080));
    assert_eq!(l.background, [0x20, 0x30, 0x40]);
    assert_eq!(l.items.len(), 3, "one item per layer, back to front");
    match &l.items[0] {
        LcdItem::Rect { rect, color } => {
            assert_eq!(*rect, [480.0, 540.0, 1440.0, 810.0]);
            assert_eq!(&color[..3], &[255, 128, 0]);
            assert_eq!(color[3], 102, "alpha 0.4");
        }
        other => panic!("expected a rect, got {other:?}"),
    }
    match &l.items[1] {
        LcdItem::Text {
            text,
            origin,
            cell,
            color,
        } => {
            assert_eq!(text, "HELLO");
            assert!((cell - 108.0).abs() < 0.01, "size 0.1 of 1080 px: {cell}");
            assert!(
                origin[0].abs() < 0.01 && origin[1].abs() < 0.01,
                "{origin:?}"
            );
            assert_eq!(*color, [0, 255, 0, 255]);
        }
        other => panic!("expected text, got {other:?}"),
    }
    match &l.items[2] {
        LcdItem::Rect { rect, color } => {
            assert_eq!(*rect, [0.0, 0.0, 192.0, 108.0]);
            assert_eq!(*color, [255, 255, 255, 255]);
        }
        other => panic!("expected a rect, got {other:?}"),
    }
    // Centred text: the origin is the top-left of the text box (advance 6 units per glyph,
    // glyph 5 units wide, unit = cell / 7).
    let doc = text_scene("text = \"HELLO\"\nsize = 0.1");
    let l = lcd(&doc, 0, &ScoreState::default());
    let LcdItem::Text { origin, cell, .. } = &l.items[0] else {
        panic!("expected text");
    };
    let unit = cell / 7.0;
    let width = (5.0 * 6.0 - 1.0) * unit;
    assert!(
        (origin[0] + width * 0.5 - 960.0).abs() <= unit,
        "centred in x: {origin:?}"
    );
    assert!(
        (origin[1] + cell * 0.5 - 540.0).abs() <= 1.0,
        "centred in y: {origin:?}"
    );
    // The timeline applies to the LCD as well.
    let doc = format!(
        "duration_ms = 1000\n{}",
        rect_scene("start_ms = 100\nend_ms = 200")
    );
    assert!(lcd(&doc, 50, &ScoreState::default()).items.is_empty());
    assert_eq!(lcd(&doc, 150, &ScoreState::default()).items.len(), 1);
    assert!(lcd(&doc, 250, &ScoreState::default()).items.is_empty());
    let doc = format!("duration_ms = 2000\n{}", rect_scene("fade_in_ms = 1000"));
    let alpha_at = |t: u64| match lcd(&doc, t, &ScoreState::default()).items.first() {
        Some(LcdItem::Rect { color, .. }) => color[3],
        _ => 0,
    };
    assert!(alpha_at(0) <= 8);
    assert!((118..=138).contains(&alpha_at(500)), "{}", alpha_at(500));
    assert_eq!(alpha_at(1500), 255);
}
