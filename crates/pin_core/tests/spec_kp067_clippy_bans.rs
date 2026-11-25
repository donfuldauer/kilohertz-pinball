//! KP-067 black-box checks of the eight sim crates' `clippy.toml` bans.
//! REQ-WEB-002 prerequisite / D9 (SPEC §2.4): `f32/f64::min/max` go through `pin_core::fmath`,
//! no SIMD-backed glam types, libm bans remain.

use std::collections::BTreeSet;
use std::path::PathBuf;

const CRATES: [&str; 8] = [
    "pin_core",
    "pin_physics",
    "pin_input",
    "pin_sim",
    "pin_script",
    "pin_assets",
    "pin_tablegen",
    "pin_assetgen",
];
const GLAM_CRATES: [&str; 4] = ["pin_assets", "pin_physics", "pin_sim", "pin_tablegen"];

struct Entry {
    path: String,
    reason: String,
}

struct Cfg {
    methods: Vec<Entry>,
    types: Vec<Entry>,
}

fn entries(v: &toml::Value, key: &str, name: &str) -> Vec<Entry> {
    let Some(arr) = v.get(key) else {
        return Vec::new();
    };
    arr.as_array()
        .unwrap_or_else(|| panic!("{name}: {key} is not an array"))
        .iter()
        .map(|e| Entry {
            path: e
                .get("path")
                .and_then(|p| p.as_str())
                .unwrap_or_else(|| panic!("{name}: {key} entry without path"))
                .to_string(),
            reason: e
                .get("reason")
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect()
}

fn load(name: &str) -> Cfg {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(name)
        .join("clippy.toml");
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let v: toml::Value = toml::from_str(&text).unwrap_or_else(|e| panic!("{name}: bad TOML: {e}"));
    Cfg {
        methods: entries(&v, "disallowed-methods", name),
        types: entries(&v, "disallowed-types", name),
    }
}

fn has(list: &[Entry], path: &str) -> bool {
    list.iter().any(|e| e.path == path)
}

/// REQ-WEB-002 / D9: std float min/max are banned with a reason in all 8 crates.
#[test]
fn minmax_banned_in_all_crates() {
    let mut missing = Vec::new();
    for c in CRATES {
        let cfg = load(c);
        for p in ["f32::min", "f32::max", "f64::min", "f64::max"] {
            match cfg.methods.iter().find(|e| e.path == p) {
                Some(e) if !e.reason.trim().is_empty() => {}
                _ => missing.push(format!("{c}: {p}")),
            }
        }
    }
    assert!(missing.is_empty(), "min/max bans missing: {missing:?}");
}

/// D9 (SPEC §2.4): the pre-existing libm bans remain; clamp is not banned.
#[test]
fn libm_bans_preserved_and_clamp_allowed() {
    for c in CRATES {
        let cfg = load(c);
        for p in [
            "f32::sin",
            "f32::atan2",
            "f32::powf",
            "f64::sin",
            "f64::atan2",
            "f64::powf",
            "f32::hypot",
            "f64::cbrt",
        ] {
            assert!(has(&cfg.methods, p), "{c}: {p} not banned");
        }
        // origin/main has 25 libm names per float type = 50 (the brief said 26 each; the real
        // pre-existing count read from origin/main is 50).
        let n = cfg
            .methods
            .iter()
            .filter(|e| {
                (e.path.starts_with("f32::") || e.path.starts_with("f64::"))
                    && !e.path.ends_with("::min")
                    && !e.path.ends_with("::max")
            })
            .count();
        assert_eq!(n, 50, "{c}: libm ban count");
        assert!(
            !has(&cfg.methods, "f32::clamp") && !has(&cfg.methods, "f64::clamp"),
            "{c}: clamp banned"
        );
    }
}

/// REQ-WEB-002 / D9: glam-using crates ban SIMD-backed types and quaternion lerp/slerp.
#[test]
fn glam_crates_ban_simd_types_and_quat_lerp() {
    for c in GLAM_CRATES {
        let cfg = load(c);
        for p in ["glam::Vec3A", "glam::Vec4", "glam::Affine3A"] {
            assert!(has(&cfg.types, p), "{c}: disallowed-types lacks {p}");
        }
        for p in ["glam::Quat::lerp", "glam::Quat::slerp"] {
            assert!(has(&cfg.methods, p), "{c}: disallowed-methods lacks {p}");
        }
    }
}

/// D9: crates without glam must not name glam paths (unresolvable path is a clippy warning).
#[test]
fn non_glam_crates_have_no_glam_paths() {
    for c in CRATES.iter().filter(|c| !GLAM_CRATES.contains(c)) {
        let cfg = load(c);
        for e in cfg.methods.iter().chain(cfg.types.iter()) {
            assert!(!e.path.starts_with("glam::"), "{c}: unexpected {}", e.path);
        }
    }
}

/// D9: every entry has a reason and no path is duplicated within a file.
#[test]
fn every_entry_has_reason_and_no_duplicates() {
    for c in CRATES {
        let cfg = load(c);
        for (kind, list) in [("methods", &cfg.methods), ("types", &cfg.types)] {
            let mut seen = BTreeSet::new();
            for e in list {
                assert!(
                    !e.reason.trim().is_empty(),
                    "{c} {kind}: {} has no reason",
                    e.path
                );
                assert!(
                    seen.insert(e.path.clone()),
                    "{c} {kind}: duplicate {}",
                    e.path
                );
            }
        }
    }
}
