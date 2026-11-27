//! #113 KP-032 (REQ-CAB-008, SPEC §7 and Appendix A.4): the simulation's
//! gravity follows the playfield attitude derived from the bundle's cabinet
//! (its `cabinet.toml`, or preset `classic_standard` when it has none) and
//! its legs and levelers.
//!
//! The bundles are copies of the replay fixture (`sim_support`) with
//! `cabinet.toml` added; the rolling measurements also replace `rules.lua`
//! with one that launches nothing, so a single ball released from rest on
//! the open deck (right of the posts, clear of the bumper, magnet and
//! lanes) is the only moving body.

#[path = "sim_support/mod.rs"]
mod sim_support;

use glam::Vec3;
use pin_core::schema::{CabinetGeometry, CabinetManifest, playfield_attitude};
use pin_sim::load::{GRAVITY_M_S2, attitude_gravity};
use pin_sim::{SimConfig, SimError, Simulation};
use proptest::prelude::*;
use sim_support::{ScratchDir, fixture, fixture_table_toml, fixture_variant, on_sim_thread};
use std::path::{Path, PathBuf};

/// Standard steel ball radius, m.
const R: f32 = 0.0135;
/// Release point on the open deck, m (table frame).
const RELEASE: [f32; 3] = [0.450, 1.000, R];
/// Rules that launch nothing (no ball but the test's own).
const QUIET_RULES: &str = "function on_init() end\nfunction on_switch(id, closed) end\n";
/// Velocity sampling window, ticks after the release (1 ms each).
const T0: u64 = 50;
const T1: u64 = 750;

fn load(dir: &Path) -> Result<Simulation, SimError> {
    Simulation::load(dir, SimConfig::default())
}

fn gravity_of(dir: PathBuf) -> Vec3 {
    on_sim_thread(move || load(&dir).expect("load").solver().config().gravity)
}

/// A fixture copy with `cabinet` as its `cabinet.toml` (or none) and,
/// optionally, the quiet rules and a `table.toml` with `rolling_resistance = 0`.
fn variant(tag: &str, cabinet: Option<&str>, quiet: bool, no_resistance: bool) -> ScratchDir {
    let table = if no_resistance {
        format!(
            "{}\n[physics]\nrolling_resistance = 0.0\n",
            fixture_table_toml()
        )
    } else {
        fixture_table_toml()
    };
    let mut edits: Vec<(&str, Option<&[u8]>)> = vec![("table.toml", Some(table.as_bytes()))];
    if let Some(c) = cabinet {
        edits.push(("cabinet.toml", Some(c.as_bytes())));
    }
    if quiet {
        edits.push(("rules.lua", Some(QUIET_RULES.as_bytes())));
    }
    fixture_variant(tag, &edits)
}

/// The expected gravity for a cabinet text (REQ-CAB-008, decision D-1),
/// independently of the loader: f64, libm, cast per component.
fn expected_gravity(geom: &CabinetGeometry) -> [f64; 3] {
    let a = playfield_attitude(geom);
    let g = f64::from(GRAVITY_M_S2);
    let (sp, cp) = (libm::sin(a.pitch_rad), libm::cos(a.pitch_rad));
    let (sr, cr) = (libm::sin(a.roll_rad), libm::cos(a.roll_rad));
    [-g * sr, -g * cr * sp, -g * cr * cp]
}

fn geom_of(cabinet: &str) -> CabinetGeometry {
    CabinetManifest::from_toml_str(cabinet)
        .and_then(|m| m.resolve())
        .expect("cabinet resolves")
}

fn assert_vec_close(got: Vec3, want: [f64; 3], rel: f64, label: &str) {
    let norm = libm::sqrt(want.iter().map(|w| w * w).sum::<f64>());
    for (i, w) in want.iter().enumerate() {
        let d = (f64::from(got[i]) - w).abs();
        assert!(
            d <= rel * norm,
            "{label}: component {i} = {} vs {w} (diff {d})",
            got[i]
        );
    }
}

fn assert_norm_981(g: Vec3, label: &str) {
    let n = libm::sqrt(
        f64::from(g.x) * f64::from(g.x)
            + f64::from(g.y) * f64::from(g.y)
            + f64::from(g.z) * f64::from(g.z),
    );
    let rel = (n - 9.81).abs() / 9.81;
    assert!(rel <= 1e-6, "{label}: |g| = {n} (rel {rel})");
}

/// Left-right leg spacing Sx of a geometry, mm (SPEC A.4: distance between
/// the mean left and mean right leg centrelines).
fn leg_spacing_x(g: &CabinetGeometry) -> f64 {
    let half = f64::from(g.body.width) * 0.5;
    let ix = |leg: &pin_core::schema::cabinet::Leg| f64::from(leg.corner_inset[0]);
    let right = ((half - ix(&g.legs.front_right)) + (half - ix(&g.legs.rear_right))) * 0.5;
    let left = ((ix(&g.legs.front_left) - half) + (ix(&g.legs.rear_left) - half)) * 0.5;
    right - left
}

struct Roll {
    /// Measured acceleration over the window, m/s² (x, y).
    ax: f64,
    ay: f64,
    rolling_resistance: f64,
    gravity: Vec3,
}

/// Releases a steel ball from rest at [`RELEASE`] and returns its mean
/// acceleration between ticks [`T0`] and [`T1`] (velocity difference over
/// the window; semi-implicit Euler makes velocity exact in the tick count).
fn roll(dir: PathBuf) -> Roll {
    on_sim_thread(move || {
        let mut s = load(&dir).expect("load");
        let h = s
            .spawn_ball("standard_steel", RELEASE, [0.0; 3])
            .expect("spawn");
        s.advance(T0);
        let v0 = s.ball(h).expect("ball at T0").velocity_m_s;
        s.advance(T1 - T0);
        let b = s.ball(h).expect("ball at T1");
        assert!(
            b.position_m[1] > 0.3,
            "ball left the open deck: {:?}",
            b.position_m
        );
        let dt = (T1 - T0) as f64 * 1.0e-3;
        Roll {
            ax: f64::from(b.velocity_m_s[0] - v0[0]) / dt,
            ay: f64::from(b.velocity_m_s[1] - v0[1]) / dt,
            rolling_resistance: f64::from(s.solver().config().rolling_resistance),
            gravity: s.solver().config().gravity,
        }
    })
}

const RIGHT_PLUS_2MM: &str = "schema_version = 1\npreset = \"classic_standard\"\n\
[legs.front_right.leveler]\nextension = 2.0\n\
[legs.rear_right.leveler]\nextension = 2.0\n";

const REAR_PLUS_20MM: &str = "schema_version = 1\npreset = \"classic_standard\"\n\
[legs.rear_left.leveler]\nextension = 20.0\n\
[legs.rear_right.leveler]\nextension = 20.0\n";

// (a) -------------------------------------------------------------------

#[test]
fn req_cab_008_no_cabinet_toml_uses_classic_standard() {
    assert!(!fixture().join("cabinet.toml").exists());
    let g = gravity_of(fixture());
    let preset = CabinetGeometry::default_preset().expect("preset");
    let a = playfield_attitude(&preset);
    assert!(
        (a.pitch_deg() - 6.5).abs() < 1e-9,
        "pitch {}",
        a.pitch_deg()
    );
    assert_eq!(a.roll_rad, 0.0);
    assert_eq!(g.x, 0.0, "a level preset pulls nothing sideways");
    assert_norm_981(g, "no cabinet.toml");
    assert_vec_close(g, expected_gravity(&preset), 1e-6, "no cabinet.toml");
    // Exactly the same vector as naming the preset explicitly.
    let named = variant(
        "kp032_named_classic",
        Some("schema_version = 1\npreset = \"classic_standard\"\n"),
        false,
        false,
    );
    assert_eq!(gravity_of(named.path().to_path_buf()), g);
    assert_eq!(attitude_gravity(a), g);
}

/// R3 / D-3: for the shipped 6.5° tables without `cabinet.toml` the new
/// gravity is bit-identical to the pre-KP-032 vector built from
/// `meta.pitch_deg` in `f32` (why the replay digest did not move).
#[test]
fn req_cab_008_classic_standard_gravity_is_bit_identical_to_the_meta_pitch_vector() {
    let pitch = pin_core::units::deg_to_rad(6.5);
    let old = Vec3::new(
        0.0,
        -GRAVITY_M_S2 * libm::sinf(pitch),
        -GRAVITY_M_S2 * libm::cosf(pitch),
    );
    let g = gravity_of(fixture());
    assert_eq!(
        [g.x.to_bits(), g.y.to_bits(), g.z.to_bits()],
        [old.x.to_bits(), old.y.to_bits(), old.z.to_bits()],
        "new {g} vs old {old}"
    );
}

// (b) -------------------------------------------------------------------

#[test]
fn req_cab_008_every_preset_is_6_5_degrees_and_level() {
    for id in ["classic_widebody", "modern_standard"] {
        let text = format!("schema_version = 1\npreset = \"{id}\"\n");
        let dir = variant(&format!("kp032_{id}"), Some(&text), false, false);
        let g = gravity_of(dir.path().to_path_buf());
        let a = playfield_attitude(&geom_of(&text));
        assert!(
            (a.pitch_deg() - 6.5).abs() < 1e-9,
            "{id}: {}",
            a.pitch_deg()
        );
        assert_eq!(a.roll_rad, 0.0, "{id}");
        assert_eq!(g.x, 0.0, "{id}");
        assert_norm_981(g, id);
        let pitch = libm::atan2(-f64::from(g.y), -f64::from(g.z));
        assert!(
            (pitch.to_degrees() - 6.5).abs() < 1e-5,
            "{id}: gravity pitch {}",
            pitch.to_degrees()
        );
    }
}

// (c) -------------------------------------------------------------------

#[test]
fn req_cab_008_longer_rear_levelers_steepen_the_game() {
    let dir = variant("kp032_rear20", Some(REAR_PLUS_20MM), false, false);
    let g = gravity_of(dir.path().to_path_buf());
    let geom = geom_of(REAR_PLUS_20MM);
    let a = playfield_attitude(&geom);
    assert!(a.pitch_deg() > 6.5 + 0.5, "pitch {}", a.pitch_deg());
    assert_eq!(a.roll_rad, 0.0);
    assert_vec_close(g, expected_gravity(&geom), 1e-6, "rear +20 mm");
    assert_norm_981(g, "rear +20 mm");
    let base = gravity_of(fixture());
    assert!(
        g.y < base.y,
        "steeper pulls harder toward -y: {g} vs {base}"
    );
    assert!(
        g.z > base.z,
        "steeper leaves less normal load: {g} vs {base}"
    );
}

#[test]
fn req_cab_008_rolling_acceleration_matches_the_derived_pitch() {
    // Default levelers (the fixture has no cabinet.toml): 5/7 g (sin θ − c cos θ)
    // within 0.1 %.
    let dir = variant("kp032_roll_default", None, true, false);
    let r = roll(dir.path().to_path_buf());
    let theta = playfield_attitude(&CabinetGeometry::default_preset().unwrap()).pitch_rad;
    let c = r.rolling_resistance;
    assert!(c > 0.0, "the table's default resistance is used");
    let want = 5.0 / 7.0 * 9.81 * (libm::sin(theta) - c * libm::cos(theta));
    let err = (-r.ay - want) / want;
    println!(
        "roll default: a = {:.6} want {want:.6} err {:+.4} %",
        -r.ay,
        err * 100.0
    );
    assert!(err.abs() < 1e-3, "rolling acceleration error {err}");
    assert!(
        r.ax.abs() < 1e-6,
        "no sideways drift on a level cabinet: {}",
        r.ax
    );

    // Rear levelers +20 mm: the same law at the steeper derived pitch.
    let dir = variant("kp032_roll_rear20", Some(REAR_PLUS_20MM), true, false);
    let r = roll(dir.path().to_path_buf());
    let theta = playfield_attitude(&geom_of(REAR_PLUS_20MM)).pitch_rad;
    let want = 5.0 / 7.0 * 9.81 * (libm::sin(theta) - c * libm::cos(theta));
    let err = (-r.ay - want) / want;
    println!(
        "roll rear+20: a = {:.6} want {want:.6} err {:+.4} %",
        -r.ay,
        err * 100.0
    );
    assert!(err.abs() < 1e-3, "rolling acceleration error {err}");
}

// (d) -------------------------------------------------------------------

#[test]
fn req_cab_008_uneven_side_levelers_pull_the_ball_sideways() {
    let geom = geom_of(RIGHT_PLUS_2MM);
    let preset = CabinetGeometry::default_preset().unwrap();
    assert_eq!(preset.legs.front_left.leveler.extension, 0.0);
    let sx = leg_spacing_x(&preset);
    let a = playfield_attitude(&geom);
    let roll_want = libm::atan2(2.0, sx);
    assert!(
        (a.roll_rad - roll_want).abs() < 1e-12,
        "roll {} vs atan2(2, {sx}) = {roll_want}",
        a.roll_rad
    );
    assert!((a.pitch_deg() - 6.5).abs() < 1e-9, "pitch unchanged");

    let dir = variant("kp032_roll_right2", Some(RIGHT_PLUS_2MM), true, true);
    let r = roll(dir.path().to_path_buf());
    assert_eq!(r.rolling_resistance, 0.0);
    assert!(r.gravity.x < 0.0, "right side up pulls toward -x");
    assert_vec_close(r.gravity, expected_gravity(&geom), 1e-6, "right +2 mm");
    assert_norm_981(r.gravity, "right +2 mm");
    let want_x = -5.0 / 7.0 * 9.81 * libm::sin(roll_want);
    let err = (r.ax - want_x) / want_x;
    println!(
        "lateral: a_x = {:.7} want {want_x:.7} err {:+.4} % (roll {:.4} deg, Sx {sx} mm)",
        r.ax,
        err * 100.0,
        roll_want.to_degrees()
    );
    assert!(err.abs() < 0.01, "lateral drift error {err}");
    // Along the table: 5/7 g cos φ sin θ (c = 0).
    let want_y = -5.0 / 7.0 * 9.81 * libm::cos(roll_want) * libm::sin(a.pitch_rad);
    let err_y = (r.ay - want_y) / want_y;
    assert!(err_y.abs() < 1e-3, "rolling acceleration error {err_y}");

    // Mirrored: left side up pulls toward +x.
    let left = "schema_version = 1\npreset = \"classic_standard\"\n\
[legs.front_left.leveler]\nextension = 2.0\n\
[legs.rear_left.leveler]\nextension = 2.0\n";
    let dir = variant("kp032_left2", Some(left), false, false);
    let g = gravity_of(dir.path().to_path_buf());
    assert!(g.x > 0.0, "left side up pulls toward +x: {g}");
    assert_eq!(g.x, -r.gravity.x);
}

// (e) -------------------------------------------------------------------

fn assert_cabinet_build_error(tag: &str, bytes: &[u8]) {
    let dir = fixture_variant(tag, &[("cabinet.toml", Some(bytes))]);
    let path = dir.path().to_path_buf();
    let res = on_sim_thread(move || load(&path).map(|_| ()));
    match res {
        Err(SimError::Build(m)) => assert!(m.contains("cabinet.toml"), "{tag}: {m}"),
        other => panic!("{tag}: expected a build error naming cabinet.toml, got {other:?}"),
    }
}

#[test]
fn req_cab_008_bad_cabinet_toml_is_a_build_error_naming_the_file() {
    assert_cabinet_build_error("kp032_bad_syntax", b"schema_version = = 1\n[legs\n");
    assert_cabinet_build_error(
        "kp032_bad_key",
        b"schema_version = 1\npreset = \"classic_standard\"\nwheels = 4\n",
    );
    assert_cabinet_build_error(
        "kp032_bad_preset",
        b"schema_version = 1\npreset = \"no_such_cabinet\"\n",
    );
    assert_cabinet_build_error(
        "kp032_bad_version",
        b"schema_version = 2\npreset = \"classic_standard\"\n",
    );
    assert_cabinet_build_error("kp032_bad_utf8", &[0xff, 0xfe, b'x', 0x80]);
    assert_cabinet_build_error(
        "kp032_bad_incomplete",
        b"schema_version = 1\n[legs.default.leveler]\nextension = 1.0\n",
    );
}

/// D-2: the sim does not enforce the `meta.pitch_deg` agreement rule; a
/// cabinet whose derived pitch is far from 6.5° still loads.
#[test]
fn req_cab_008_pitch_disagreeing_with_meta_still_loads() {
    let dir = variant("kp032_disagree", Some(REAR_PLUS_20MM), false, false);
    assert!(fixture_table_toml().contains("pitch_deg = 6.5"));
    let g = gravity_of(dir.path().to_path_buf());
    assert_vec_close(
        g,
        expected_gravity(&geom_of(REAR_PLUS_20MM)),
        1e-6,
        "disagree",
    );
}

// (f) -------------------------------------------------------------------

/// SplitMix64: deterministic bytes for the bounded random run.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// R11: random bytes and mutated valid files as `cabinet.toml` never panic
/// the loader; every failure is a build error naming the file.
#[test]
fn req_cab_008_cabinet_toml_survives_bounded_random_bytes() {
    let seed_text = RIGHT_PLUS_2MM.as_bytes();
    let dir = fixture_variant("kp032_fuzz", &[]);
    let path = dir.path().to_path_buf();
    on_sim_thread(move || {
        let mut st = 0x113_u64;
        let mut loaded = 0u32;
        for case in 0..48u32 {
            let bytes: Vec<u8> = if case % 2 == 0 {
                let n = (splitmix(&mut st) % 512) as usize;
                (0..n).map(|_| splitmix(&mut st) as u8).collect()
            } else {
                let mut b = seed_text.to_vec();
                for _ in 0..1 + splitmix(&mut st) % 4 {
                    let i = (splitmix(&mut st) as usize) % b.len();
                    b[i] = splitmix(&mut st) as u8;
                }
                b
            };
            std::fs::write(path.join("cabinet.toml"), &bytes).expect("write");
            match load(&path) {
                Ok(s) => {
                    loaded += 1;
                    let g = s.solver().config().gravity;
                    assert!(g.is_finite(), "case {case}: {g}");
                }
                Err(SimError::Build(m)) => assert!(m.contains("cabinet.toml"), "case {case}: {m}"),
                Err(e) => panic!("case {case}: unexpected error {e:?}"),
            }
        }
        println!("random cabinet.toml: {loaded} of 48 loaded");
    });
}

// (g) -------------------------------------------------------------------

#[test]
fn req_cab_008_loading_twice_is_bit_identical() {
    let dir = variant("kp032_determinism", Some(RIGHT_PLUS_2MM), false, false);
    let path = dir.path().to_path_buf();
    let run = move || {
        let p = path.clone();
        on_sim_thread(move || {
            let mut s = load(&p).expect("load");
            let g = s.solver().config().gravity;
            s.advance(2000);
            (
                [g.x.to_bits(), g.y.to_bits(), g.z.to_bits()],
                s.state_hash(),
            )
        })
    };
    let a = run();
    let b = run();
    assert_eq!(a, b);
}

// (h) -------------------------------------------------------------------

#[test]
fn req_cab_008_replay_fixture_hash_is_stable_over_10_runs() {
    let hashes: Vec<u64> = (0..10)
        .map(|_| {
            on_sim_thread(|| {
                let mut s = load(&fixture()).expect("load");
                s.advance(2000);
                s.state_hash()
            })
        })
        .collect();
    assert!(hashes.iter().all(|h| *h == hashes[0]), "{hashes:x?}");
}

// Numeric helper ------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// D-1: |g| = 9.81 for every attitude, signs follow pitch and roll, and
    /// zero roll gives exactly zero x.
    #[test]
    fn req_cab_008_attitude_gravity_is_unit_norm_and_signed(
        pitch in -0.5f64..0.5,
        roll in -0.2f64..0.2,
    ) {
        let g = attitude_gravity(pin_core::schema::Attitude { pitch_rad: pitch, roll_rad: roll });
        let n = libm::sqrt(
            f64::from(g.x) * f64::from(g.x)
                + f64::from(g.y) * f64::from(g.y)
                + f64::from(g.z) * f64::from(g.z),
        );
        prop_assert!((n - 9.81).abs() / 9.81 < 1e-6, "|g| = {}", n);
        prop_assert!(g.z < 0.0);
        if pitch > 1e-6 { prop_assert!(g.y < 0.0); }
        if pitch < -1e-6 { prop_assert!(g.y > 0.0); }
        if roll > 1e-6 { prop_assert!(g.x < 0.0); }
        if roll < -1e-6 { prop_assert!(g.x > 0.0); }
        let g0 = attitude_gravity(pin_core::schema::Attitude { pitch_rad: pitch, roll_rad: 0.0 });
        prop_assert_eq!(g0.x, 0.0);
    }
}
