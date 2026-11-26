//! KP-010 (#90) acceptance G: `[theme]` (REQ-BUILD-003, REQ-BUILD-005).

mod kp010_common;

use kp010_common::{PARTS_TABLE, invalid_table, ok_table, table};
use pin_core::schema::SchemaError;
use pin_core::schema::parts::{DEFAULT_MOTIF, ThemeDef};

/// REQ-BUILD-005: without `[theme]` the effective theme is `meta.name`, the valid
/// `meta.palette` entries (unvalidated as before), the default motif and no fonts.
#[test]
fn req_build_005_theme_falls_back_to_meta() {
    let t = ok_table("");
    assert!(t.theme.is_none());
    let th = t.effective_theme();
    assert_eq!(th.title, "T");
    assert_eq!(
        th.palette,
        vec![[1, 2, 3]],
        "`nonsense` in meta.palette is skipped, not an error"
    );
    assert_eq!(th.motif, DEFAULT_MOTIF);
    assert!(th.fonts.is_empty());
    // An empty [theme] falls back the same way and round-trips.
    let e = ok_table("[theme]\n");
    assert_eq!(e.theme, Some(ThemeDef::default()));
    assert_eq!(e.effective_theme(), th);
    assert_eq!(table(&e.to_toml_string().unwrap()).unwrap(), e);
}

/// REQ-BUILD-003: `theme.palette` replaces `meta.palette`; the other fields are taken as given.
#[test]
fn req_build_003_theme_fields_win() {
    let t = table(PARTS_TABLE).unwrap();
    let th = t.effective_theme();
    assert_eq!(th.title, "Parts Demo");
    assert_eq!(
        th.palette,
        vec![[0x10, 0x18, 0x20], [0xf2, 0xaa, 0x4c], [0xe6, 0x39, 0x46]]
    );
    assert_eq!(th.motif, "circuit");
    assert_eq!(
        th.fonts.get("title").map(String::as_str),
        Some("Kilohertz Display")
    );
    assert_eq!(
        th.fonts.keys().collect::<Vec<_>>(),
        ["label", "title"],
        "sorted by role"
    );
    let t = ok_table("[theme]\ntitle = \"Other\"\n");
    assert_eq!(t.effective_theme().title, "Other");
    assert_eq!(t.effective_theme().palette, vec![[1, 2, 3]]);
}

/// REQ-BUILD-003: bounds and errors naming the key.
#[test]
fn req_build_003_theme_validation_names_the_key() {
    let sixteen: Vec<String> = (0..16).map(|i| format!("\"#0000{i:02x}\"")).collect();
    ok_table(&format!("[theme]\npalette = [{}]\n", sixteen.join(", ")));
    for (src, want) in [
        (
            "[theme]\npalette = [\"#000000\", \"#12345g\"]\n",
            "theme.palette[1]",
        ),
        ("[theme]\npalette = []\n", "theme.palette"),
        (
            &format!("[theme]\npalette = [{}, \"#000000\"]\n", sixteen.join(", ")),
            "theme.palette",
        ),
        ("[theme]\ntitle = \"\"\n", "theme.title"),
        ("[theme]\ntitle = \"a\\nb\"\n", "theme.title"),
        (
            &format!("[theme]\ntitle = \"{}\"\n", "x".repeat(129)),
            "theme.title",
        ),
        ("[theme]\nmotif = \"Neon\"\n", "theme.motif"),
        (
            &format!("[theme]\nmotif = \"{}\"\n", "m".repeat(33)),
            "theme.motif",
        ),
        (
            "[theme]\nfonts = { title = \"../fonts/evil.ttf\" }\n",
            "theme.fonts.title",
        ),
        (
            "[theme]\nfonts = { title = \"C:\\\\x\" }\n",
            "theme.fonts.title",
        ),
        (
            "[theme]\nfonts = { title = \".hidden\" }\n",
            "theme.fonts.title",
        ),
        ("[theme]\nfonts = { title = \"\" }\n", "theme.fonts.title"),
        (
            "[theme]\nfonts = { \"Title Role\" = \"Sans\" }\n",
            "theme.fonts",
        ),
        (
            "[theme]\nfonts = { a = \"S\", b = \"S\", c = \"S\", d = \"S\", e = \"S\", f = \"S\", g = \"S\", h = \"S\", i = \"S\" }\n",
            "theme.fonts",
        ),
    ] {
        let m = invalid_table(src);
        assert!(m.contains(want), "{want}: {m}");
    }
    ok_table(
        "[theme]\nfonts = { a = \"S\", b = \"S\", c = \"S\", d = \"S\", e = \"S\", f = \"S\", g = \"S\", h = \"S\" }\n",
    );
    ok_table("[theme]\nmotif = \"art-deco_2\"\ntitle = \"Space & Time!\"\n");
}

/// REQ-BUILD-005: unknown theme keys are parse errors.
#[test]
fn req_build_005_theme_unknown_key() {
    assert!(matches!(
        table(&kp010_common::minimal_table(
            "[theme]\ncolour = \"#000000\"\n"
        )),
        Err(SchemaError::Parse(_))
    ));
}
