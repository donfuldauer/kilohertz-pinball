//! Independent GPU-free spec tests for Subphase 5.4 (hardware ray-query reflections).
//!
//! Written from the 5.4 brief alone, without reading the implementation
//! (`src/raytrace/**`) or the Implementer's tests. Contract under test:
//!
//! * `ReflectionQuality { Off, BallsOnly, BallsAndClearCoat }`, `Default == Off` (D33),
//!   `FromStr` / `Display` as `"off" | "balls" | "balls_and_clearcoat"`;
//! * `RayQueryConfig::default().ray_roughness_cutoff == 0.3`;
//! * the `raytrace` module rustdoc carries the backend survey: it names
//!   `EXPERIMENTAL_RAY_QUERY` and the pinned wgpu version `30.0.1`.
//!
//! The roughness cutoff is not exposed as a pure function by the contract, so its
//! behaviour (ray result below the cutoff, probe result above it) is tested on the GPU
//! in `spec_5_4_gpu.rs`.

#![allow(clippy::needless_update)] // `..RayQueryConfig::default()` stays valid whatever fields the config grows
use pin_render::raytrace::{RayQueryConfig, ReflectionQuality};
use proptest::prelude::*;

const ALL: [(ReflectionQuality, &str); 3] = [
    (ReflectionQuality::Off, "off"),
    (ReflectionQuality::BallsOnly, "balls"),
    (ReflectionQuality::BallsAndClearCoat, "balls_and_clearcoat"),
];

#[test]
fn default_quality_is_off() {
    // D33: ray-query reflections ship off by default.
    assert_eq!(ReflectionQuality::default(), ReflectionQuality::Off);
}

#[test]
fn quality_displays_as_the_setting_strings() {
    for (q, s) in ALL {
        assert_eq!(q.to_string(), s, "{q:?}");
        assert_eq!(format!("{q}"), s, "{q:?}");
    }
}

#[test]
fn quality_parses_the_setting_strings() {
    for (q, s) in ALL {
        let parsed: ReflectionQuality = s
            .parse()
            .unwrap_or_else(|e| panic!("{s:?} does not parse: {e:?}"));
        assert_eq!(parsed, q, "{s:?}");
    }
}

#[test]
fn quality_display_parse_roundtrip() {
    for (q, _) in ALL {
        let back: ReflectionQuality = q
            .to_string()
            .parse()
            .unwrap_or_else(|e| panic!("{q:?} display does not parse back: {e:?}"));
        assert_eq!(back, q);
    }
}

#[test]
fn quality_strings_are_distinct() {
    for (i, (_, a)) in ALL.iter().enumerate() {
        for (_, b) in &ALL[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

#[test]
fn quality_rejects_other_strings() {
    // Neither empty input, near misses, other words, nor the setting strings with
    // extra characters are settings.
    for s in [
        "",
        " ",
        "on",
        "ball",
        "balls_only",
        "balls_and_clear_coat",
        "balls and clearcoat",
        "off ",
        " off",
        "balls\n",
        "offf",
        "balls_and_clearcoatx",
        "\u{0}",
        "ray_query",
        "probes",
    ] {
        assert!(
            s.parse::<ReflectionQuality>().is_err(),
            "{s:?} must not parse as a reflection quality"
        );
    }
}

#[test]
fn ray_roughness_cutoff_defaults_to_0_3() {
    assert_eq!(RayQueryConfig::default().ray_roughness_cutoff, 0.3);
}

#[test]
fn ray_roughness_cutoff_is_a_plain_config_field() {
    // The cutoff can be set through the public config; the default does not move.
    let c = RayQueryConfig {
        ray_roughness_cutoff: 0.5,
        ..RayQueryConfig::default()
    };
    assert_eq!(c.ray_roughness_cutoff, 0.5);
    assert_eq!(RayQueryConfig::default().ray_roughness_cutoff, 0.3);
}

/// The `//!` lines of `src/raytrace/mod.rs` (the module rustdoc).
fn raytrace_module_doc() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/raytrace/mod.rs");
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    src.lines()
        .map(str::trim_start)
        .filter(|l| l.starts_with("//!"))
        .map(|l| l.trim_start_matches("//!"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn raytrace_rustdoc_carries_the_backend_survey() {
    let doc = raytrace_module_doc();
    assert!(
        doc.contains("EXPERIMENTAL_RAY_QUERY"),
        "the raytrace module rustdoc must name Features::EXPERIMENTAL_RAY_QUERY"
    );
    assert!(
        doc.contains("30.0.1"),
        "the raytrace module rustdoc must name the pinned wgpu version 30.0.1"
    );
    // A survey names backends: Vulkan (lavapipe reports the bit, DECISIONS Q3) and at
    // least one other backend.
    assert!(doc.contains("Vulkan"), "survey must cover Vulkan");
    assert!(
        ["GL", "Metal", "DX12"].iter().any(|b| doc.contains(b)),
        "survey must cover a backend other than Vulkan (GL / Metal / DX12)"
    );
}

/// Strings close to the valid settings: random edits (insert, delete, upper-case,
/// replace) of a valid string.
fn near_miss() -> impl Strategy<Value = String> {
    (
        0usize..3,
        prop::collection::vec(
            (0usize..4, any::<prop::sample::Index>(), any::<char>()),
            0..4,
        ),
    )
        .prop_map(|(i, edits)| {
            let mut s: Vec<char> = ALL[i].1.chars().collect();
            for (op, at, c) in edits {
                match op {
                    0 => {
                        let k = at.index(s.len() + 1);
                        s.insert(k, c);
                    }
                    1 if !s.is_empty() => {
                        let k = at.index(s.len());
                        s.remove(k);
                    }
                    2 if !s.is_empty() => {
                        let k = at.index(s.len());
                        s[k] = s[k].to_ascii_uppercase();
                    }
                    3 if !s.is_empty() => {
                        let k = at.index(s.len());
                        s[k] = c;
                    }
                    _ => {}
                }
            }
            s.into_iter().collect()
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

    /// Bounded random-string fuzz: parsing never panics, and whatever parses is one of
    /// the three settings and displays back to a string that parses to the same value.
    #[test]
    fn prop_parse_never_panics(s in ".{0,40}") {
        if let Ok(q) = s.parse::<ReflectionQuality>() {
            let shown = q.to_string();
            prop_assert!(ALL.iter().any(|(_, v)| *v == shown));
            prop_assert_eq!(shown.parse::<ReflectionQuality>().ok(), Some(q));
        }
    }

    /// Random bytes (lossy UTF-8) never panic either.
    #[test]
    fn prop_parse_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
        let s = String::from_utf8_lossy(&bytes);
        let _ = s.parse::<ReflectionQuality>();
    }

    /// Edits of a valid string never panic; the valid strings are always accepted, and
    /// anything accepted displays back to a string that parses to the same value.
    #[test]
    fn prop_near_misses(s in near_miss()) {
        match s.parse::<ReflectionQuality>() {
            Ok(q) => {
                prop_assert_eq!(q.to_string().parse::<ReflectionQuality>().ok(), Some(q));
                if let Some((v, _)) = ALL.iter().find(|(_, t)| *t == s) {
                    prop_assert_eq!(q, *v);
                }
            }
            Err(_) => {
                prop_assert!(
                    !ALL.iter().any(|(_, t)| *t == s),
                    "valid setting {:?} rejected", s
                );
            }
        }
    }
}
