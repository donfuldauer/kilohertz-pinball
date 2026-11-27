//! KP-032 (#113) independent spec tests, SPEC §7 REQ-CAB-008 and Appendix
//! A.4: the simulation's gravity is the playfield attitude derived from the
//! cabinet geometry, the mount and the four legs (leg length plus leveler
//! extension). Written from the brief alone (black box).
//!
//! Gravity contract (#113 D-1): `g = 9.81·(−sin φ, −cos φ·sin θ, −cos φ·cos θ)`
//! in the playfield frame (+x right, +y up the table, +z up from the deck),
//! θ the pitch and φ the roll of Appendix A.4. Rolling model on the open
//! deck: `a = 5/7·g·(sin θ − c·cos θ)`.

mod sim_support;
mod spec_kp032_support;

use pin_core::schema::physics::PhysicsDefaults;
use pin_sim::SimError;
use proptest::prelude::*;
use proptest::test_runner::{Config as PtConfig, TestCaseError, TestRunner};
use sim_support::on_sim_thread;
use spec_kp032_support::*;

const DEG: f64 = core::f64::consts::PI / 180.0;

/// Loads `bundle` and returns its gravity (panics on a load error).
fn gravity_of(bundle: &std::path::Path) -> [f64; 3] {
    let sim = load(bundle).unwrap_or_else(|e| panic!("load {}: {e:?}", bundle.display()));
    sim_gravity(&sim)
}

/// Asserts `g` is the D-1 formula for `(pitch, roll)` within 1e-5 relative
/// (vector error over |g|) and has norm 9.81 within 1e-6 relative.
fn assert_gravity(g: [f64; 3], pitch: f64, roll: f64, label: &str) {
    let want = gravity_formula(pitch, roll);
    let e = rel_err(g, want);
    assert!(
        e <= 1e-5,
        "{label}: gravity {g:?} vs formula {want:?} (θ = {:.6}°, φ = {:.6}°), rel err {e:e}",
        pitch / DEG,
        roll / DEG
    );
    let n = norm(g);
    assert!(
        ((n - G) / G).abs() <= 1e-6,
        "{label}: |g| = {n} not 9.81 within 1e-6"
    );
}

// ---------------------------------------------------------------------------
// Item 1: default tables and the three presets.
// ---------------------------------------------------------------------------

/// REQ-CAB-008, "Every preset gives 6.5° and 0 roll with its default
/// levelers"; A.4 "when absent the table gets the classic_standard preset".
/// No `cabinet.toml` and each preset written as `cabinet.toml` give
/// g = 9.81·(0, −sin 6.5°, −cos 6.5°) within 1e-5 relative, x exactly 0,
/// norm 9.81 within 1e-6.
#[test]
fn req_cab_008_default_and_preset_gravity_is_6_5_deg_level() {
    on_sim_thread(|| {
        let theta = DEFAULT_PITCH_DEG * DEG;
        // (a) the fixture without cabinet.toml (unmodified, and with quiet rules).
        assert!(!plain_fixture().join("cabinet.toml").exists());
        let g = gravity_of(&plain_fixture());
        assert_gravity(g, theta, 0.0, "no cabinet.toml");
        assert_eq!(g[0], 0.0, "no cabinet.toml: x must be exactly 0");
        let open = open_deck_bundle("item1_open", None, None);
        let g = gravity_of(&open);
        assert_gravity(g, theta, 0.0, "open deck, no cabinet.toml");
        assert_eq!(g[0], 0.0);
        // (b) the presets.
        for p in PRESETS {
            check_preset_data(&p);
            let text = preset_toml(p.id);
            let (op, or) = oracle_attitude(&text);
            assert!((op - theta).abs() < 1e-9 && or == 0.0, "oracle {}", p.id);
            let b = fixture_with_cabinet(&format!("item1_{}", p.id), text.as_bytes());
            let g = gravity_of(&b);
            assert_gravity(g, theta, 0.0, p.id);
            assert_eq!(g[0], 0.0, "{}: x must be exactly 0", p.id);
        }
    });
}

// ---------------------------------------------------------------------------
// Item 2: pitch from the legs.
// ---------------------------------------------------------------------------

/// REQ-CAB-008 "lengthening the rear levelers steepens the game"; A.4 body
/// pitch = atan2(R − F, Sy), playfield pitch = body pitch + mount.pitch_deg.
/// A sweep of rear and front leveler extensions on every preset: the oracle
/// and an independent hand computation agree within 1e-9 rad, the gravity
/// equals the formula for that attitude, and `meta.pitch_deg = 6.5` still
/// loads (the validator, not the sim, polices it).
#[test]
fn req_cab_008_rear_levelers_set_pitch_per_appendix_a4() {
    on_sim_thread(|| {
        for p in PRESETS {
            check_preset_data(&p);
            let [lo, hi] = p.extension_range;
            // Rear up, front up, mixed.
            let cases = [
                (lo, lo + 10.0),
                (lo, lo + 5.0),
                (lo, hi),
                (lo + 10.0, lo),
                (lo + 20.0, lo + 3.5),
                (hi, hi),
            ];
            let mut prev_pitch: Option<(f64, f64)> = None;
            for (front, rear) in cases {
                let e = Extensions {
                    fl: front,
                    fr: front,
                    rl: rear,
                    rr: rear,
                };
                let text = cabinet_toml(p.id, e);
                let (op, or) = oracle_attitude(&text);
                let (hp, hr) = hand_attitude(&p, e);
                assert!(
                    (op - hp).abs() <= 1e-9 && (or - hr).abs() <= 1e-9,
                    "{} {e:?}: oracle ({op}, {or}) vs hand ({hp}, {hr})",
                    p.id
                );
                // Steeper exactly when the rear is longer.
                let expect_sign = (rear - front).signum();
                let d = hp - DEFAULT_PITCH_DEG * DEG;
                if rear == front {
                    assert!(d.abs() < 1e-12);
                } else {
                    assert_eq!(d.signum(), expect_sign, "{} {e:?}", p.id);
                }
                let tag = format!("item2_{}_{}_{}", p.id, front, rear).replace('.', "p");
                let b = fixture_with_cabinet(&tag, text.as_bytes());
                // The bundle still states pitch_deg = 6.5.
                let table = std::fs::read_to_string(b.join("table.toml")).unwrap();
                assert!(table.contains("pitch_deg = 6.5"));
                let g = gravity_of(&b);
                assert_gravity(g, op, or, &format!("{} {e:?}", p.id));
                assert_gravity(g, hp, hr, &format!("{} {e:?} (hand)", p.id));
                assert_eq!(g[0], 0.0, "{} {e:?}: equal sides must not roll", p.id);
                prev_pitch = Some((op, g[1]));
            }
            assert!(prev_pitch.is_some());
        }
    });
}

/// REQ-CAB-008: +10 mm on both rear levelers of classic_standard (the
/// brief's example): θ = 6.5° + atan2(10, 1239) by hand (Sy = 1315 − 2·38),
/// and the down-table gravity component grows.
#[test]
fn req_cab_008_rear_plus_10mm_example() {
    on_sim_thread(|| {
        let p = preset_data("classic_standard");
        check_preset_data(&p);
        let e = Extensions {
            fl: 0.0,
            fr: 0.0,
            rl: 10.0,
            rr: 10.0,
        };
        let text = cabinet_toml(p.id, e);
        let literal = libm::atan2(10.0, 1239.0) + 6.5 * DEG;
        let (hp, hr) = hand_attitude(&p, e);
        let (op, or) = oracle_attitude(&text);
        assert!((hp - literal).abs() <= 1e-12 && hr == 0.0);
        assert!((op - literal).abs() <= 1e-9 && or.abs() <= 1e-9);
        let base = gravity_of(&fixture_with_cabinet(
            "item2_base",
            preset_toml(p.id).as_bytes(),
        ));
        let g = gravity_of(&fixture_with_cabinet("item2_rear10", text.as_bytes()));
        assert_gravity(g, literal, 0.0, "rear +10 mm");
        assert!(g[1] < base[1], "rear +10 mm must pull harder toward −y");
    });
}

// ---------------------------------------------------------------------------
// Item 3: roll from the levelers.
// ---------------------------------------------------------------------------

/// REQ-CAB-008 "an unlevel cabinet pulls the ball to one side"; A.4 body
/// roll = atan2(Rt − L, Sx), positive roll (right side up) pulls toward −x.
/// Right legs +2.0 mm: φ = atan2(2.0, Sx), g.x = −9.81·sin φ < 0, pitch
/// unchanged.
#[test]
fn req_cab_008_right_levelers_roll_toward_left() {
    on_sim_thread(|| {
        for p in PRESETS {
            check_preset_data(&p);
            let lo = p.extension_range[0];
            let e = Extensions {
                fl: lo,
                fr: lo + 2.0,
                rl: lo,
                rr: lo + 2.0,
            };
            let sx = p.body_width - 2.0 * p.corner_inset[0];
            let phi = libm::atan2(2.0, sx);
            let theta = DEFAULT_PITCH_DEG * DEG;
            let text = cabinet_toml(p.id, e);
            let (op, or) = oracle_attitude(&text);
            let (hp, hr) = hand_attitude(&p, e);
            assert!((hr - phi).abs() <= 1e-12 && (hp - theta).abs() <= 1e-12);
            assert!((or - phi).abs() <= 1e-9 && (op - theta).abs() <= 1e-9);
            let b = fixture_with_cabinet(&format!("item3_{}", p.id), text.as_bytes());
            let g = gravity_of(&b);
            assert_gravity(g, theta, phi, &format!("{} roll", p.id));
            assert!(g[0] < 0.0, "{}: roll must pull toward −x, g = {g:?}", p.id);
            let gx = -G * libm::sin(phi);
            assert!(
                ((g[0] - gx) / gx).abs() <= 1e-5,
                "{}: g.x = {} vs −9.81·sin φ = {gx}",
                p.id,
                g[0]
            );
            // Pitch unchanged: atan2(−g.y, −g.z) is θ.
            let pitch = libm::atan2(-g[1], -g[2]);
            assert!((pitch - theta).abs() <= 1e-5, "{}: pitch {pitch}", p.id);
        }
        // Mirror: left legs +2 mm pull toward +x.
        let p = preset_data("classic_standard");
        let e = Extensions {
            fl: 2.0,
            fr: 0.0,
            rl: 2.0,
            rr: 0.0,
        };
        let g = gravity_of(&fixture_with_cabinet(
            "item3_mirror",
            cabinet_toml(p.id, e).as_bytes(),
        ));
        assert!(g[0] > 0.0, "left side up must pull toward +x, g = {g:?}");
    });
}

// ---------------------------------------------------------------------------
// Items 4 and 5: rolling on the open deck.
// ---------------------------------------------------------------------------

/// Spawn point of the rolling runs: centred across, near the top wall.
const START: [f32; 3] = [0.257, 1.045, R];
/// Stop when the ball is this close to the front (m, playfield y).
const STOP_Y: f64 = 0.040;
/// First tick used by the fit (skips any contact-settling transient).
const FIT_FROM: usize = 20;

/// One measured roll: along-table (−y) and lateral (−x) accelerations from
/// a quadratic fit over ticks `FIT_FROM..` until the ball passes `STOP_Y`,
/// the distance travelled, the final lateral offset and the tick count.
#[derive(Debug, Clone, Copy)]
struct Roll {
    a_down: f64,
    a_left: f64,
    travelled: f64,
    drift_x: f64,
    ticks: usize,
}

fn roll(bundle: &std::path::Path) -> Roll {
    let mut sim = load(bundle).unwrap_or_else(|e| panic!("load: {e:?}"));
    let dt = 0.001f64;
    let run = run_ball(&mut sim, START, 2500);
    let end = run
        .pos
        .iter()
        .position(|p| p[1] < STOP_Y)
        .expect("ball reached the front of the open deck within 2.5 s");
    let pos = &run.pos[..end];
    // The ball stays on the deck the whole run (no bounce, no leaving it).
    for (k, p) in pos.iter().enumerate().skip(FIT_FROM) {
        assert!(
            (p[2] - f64::from(R)).abs() < 2e-4,
            "tick {k}: ball left the deck, z = {}",
            p[2]
        );
    }
    let down: Vec<(f64, f64)> = pos
        .iter()
        .enumerate()
        .skip(FIT_FROM)
        .map(|(k, p)| (k as f64 * dt, -p[1]))
        .collect();
    let left: Vec<(f64, f64)> = pos
        .iter()
        .enumerate()
        .skip(FIT_FROM)
        .map(|(k, p)| (k as f64 * dt, -p[0]))
        .collect();
    Roll {
        a_down: fit_accel(&down),
        a_left: fit_accel(&left),
        travelled: pos[0][1] - pos[end - 1][1],
        drift_x: pos[end - 1][0] - pos[0][0],
        ticks: end,
    }
}

/// REQ-CAB-008 acceptance: "Rolling acceleration on the open deck matches
/// 5/7·g·(sin θ − c cos θ) at the derived pitch within 0.1 %", with c = 0
/// (`[physics] rolling_resistance = 0`): the literal 5/7·g·sin θ, θ the
/// derived default pitch, over more than 1 m of flat open deck.
#[test]
fn req_cab_008_rolling_acceleration_literal_with_zero_resistance() {
    on_sim_thread(|| {
        let (theta, phi) = default_attitude();
        assert!((theta - 6.5 * DEG).abs() < 1e-9 && phi == 0.0);
        let b = open_deck_bundle("item4_c0", Some(0.0), None);
        let r = roll(&b);
        let want = 5.0 / 7.0 * G * libm::sin(theta);
        let err = (r.a_down - want) / want;
        println!(
            "c = 0: a = {:.6} m/s² (expected {want:.6}), error {:+.4} %, {:.4} m in {} ticks",
            r.a_down,
            err * 100.0,
            r.travelled,
            r.ticks
        );
        assert!(r.travelled >= 1.0, "open stretch {} m < 1 m", r.travelled);
        assert!(
            err.abs() <= 1e-3,
            "rolling acceleration error {err:e} > 0.1 %"
        );
    });
}

/// REQ-CAB-008 acceptance with the builtin rolling resistance c (the table
/// has no `[physics]`): a = 5/7·g·(sin θ − c·cos θ) within 0.1 %.
#[test]
fn req_cab_008_rolling_acceleration_with_default_resistance() {
    on_sim_thread(|| {
        let (theta, _) = default_attitude();
        let c = f64::from(PhysicsDefaults::builtin().resolve(None).rolling_resistance);
        assert!(c > 0.0);
        let b = open_deck_bundle("item4_cdef", None, None);
        let r = roll(&b);
        let want = 5.0 / 7.0 * G * (libm::sin(theta) - c * libm::cos(theta));
        let err = (r.a_down - want) / want;
        println!(
            "c = {c}: a = {:.6} m/s² (expected {want:.6}), error {:+.4} %, {:.4} m in {} ticks",
            r.a_down,
            err * 100.0,
            r.travelled,
            r.ticks
        );
        assert!(r.travelled >= 1.0, "open stretch {} m < 1 m", r.travelled);
        assert!(
            err.abs() <= 1e-3,
            "rolling acceleration error {err:e} > 0.1 %"
        );
    });
}

/// REQ-CAB-008 acceptance: "with a 2 mm side-to-side leveler difference the
/// lateral drift matches the analytic roll within 1 %": right legs +2 mm,
/// c = 0; the fitted −x acceleration equals 5/7·g·sin φ within 1 % and the
/// ball drifts toward −x. The along-table acceleration is 5/7·g·cos φ·sin θ.
#[test]
fn req_cab_008_lateral_drift_with_2mm_leveler_difference() {
    on_sim_thread(|| {
        let p = preset_data("classic_standard");
        let e = Extensions {
            fl: 0.0,
            fr: 2.0,
            rl: 0.0,
            rr: 2.0,
        };
        let text = cabinet_toml(p.id, e);
        let (theta, phi) = oracle_attitude(&text);
        assert!((phi - libm::atan2(2.0, 484.0)).abs() < 1e-9);
        let b = open_deck_bundle("item5_roll", Some(0.0), Some(&text));
        let r = roll(&b);
        let want = 5.0 / 7.0 * G * libm::sin(phi);
        let err = (r.a_left - want) / want;
        println!(
            "lateral: a = {:.6} m/s² (expected {want:.6}), error {:+.4} %, drift {:.5} m",
            r.a_left,
            err * 100.0,
            r.drift_x
        );
        assert!(
            r.drift_x < 0.0,
            "drift must be toward −x, got {}",
            r.drift_x
        );
        assert!(
            err.abs() <= 1e-2,
            "lateral acceleration error {err:e} > 1 %"
        );
        let want_down = 5.0 / 7.0 * G * libm::cos(phi) * libm::sin(theta);
        let err_down = (r.a_down - want_down) / want_down;
        assert!(err_down.abs() <= 1e-3, "down-table error {err_down:e}");
    });
}

/// REQ-CAB-008 "Every preset is level side to side": with the default
/// cabinet the same run drifts sideways by less than 1e-4 m.
#[test]
fn req_cab_008_default_cabinet_has_no_sideways_drift() {
    on_sim_thread(|| {
        for (tag, c) in [("item5_level_c0", Some(0.0)), ("item5_level_cdef", None)] {
            let b = open_deck_bundle(tag, c, None);
            let r = roll(&b);
            println!("{tag}: drift {:e} m", r.drift_x);
            assert!(r.drift_x.abs() < 1e-4, "{tag}: drift {} m", r.drift_x);
        }
    });
}

// ---------------------------------------------------------------------------
// Item 6: determinism and play change.
// ---------------------------------------------------------------------------

/// REQ-CAB-008 with SPEC §2.4 determinism: two loads of the same bundle give
/// bit-identical gravity and equal `state_hash` after 2000 ticks with the
/// same spawn.
#[test]
fn req_cab_008_gravity_and_play_are_deterministic() {
    on_sim_thread(|| {
        let p = preset_data("classic_standard");
        for (tag, cab) in [
            ("item6_det_default", None),
            (
                "item6_det_unlevel",
                Some(cabinet_toml(
                    p.id,
                    Extensions {
                        fl: 1.5,
                        fr: 7.25,
                        rl: 12.0,
                        rr: 3.0,
                    },
                )),
            ),
        ] {
            let b = open_deck_bundle(tag, None, cab.as_deref());
            let mut a = load(&b).unwrap();
            let mut c = load(&b).unwrap();
            let ga = a.solver().config().gravity;
            let gc = c.solver().config().gravity;
            assert_eq!(
                [ga.x.to_bits(), ga.y.to_bits(), ga.z.to_bits()],
                [gc.x.to_bits(), gc.y.to_bits(), gc.z.to_bits()],
                "{tag}: gravity not bit-identical"
            );
            let ra = run_ball(&mut a, START, 2000);
            let rc = run_ball(&mut c, START, 2000);
            assert_eq!(ra.hash, rc.hash, "{tag}: state_hash differs");
        }
    });
}

/// REQ-CAB-008 "lengthening the rear levelers steepens the game": +10 mm
/// rear levelers change the `state_hash` after the same 2000-tick run, and
/// the down-table acceleration grows by sin θ₂ / sin θ₁ within 0.5 %
/// (c = 0). Swept over several rear extensions (R7).
#[test]
fn req_cab_008_rear_levelers_change_play_and_scale_acceleration() {
    on_sim_thread(|| {
        let p = preset_data("classic_standard");
        let base_b = open_deck_bundle("item6_base", Some(0.0), None);
        let (t1, _) = default_attitude();
        let base = roll(&base_b);
        let mut base_sim = load(&base_b).unwrap();
        let base_hash = run_ball(&mut base_sim, START, 2000).hash;
        for rear in [10.0, 4.0, 25.0] {
            let e = Extensions {
                fl: 0.0,
                fr: 0.0,
                rl: rear,
                rr: rear,
            };
            let text = cabinet_toml(p.id, e);
            let (t2, _) = oracle_attitude(&text);
            let b = open_deck_bundle(&format!("item6_rear{rear}"), Some(0.0), Some(&text));
            let mut sim = load(&b).unwrap();
            let h = run_ball(&mut sim, START, 2000).hash;
            assert_ne!(h, base_hash, "rear +{rear} mm: state_hash unchanged");
            let r = roll(&b);
            let ratio = r.a_down / base.a_down;
            let want = libm::sin(t2) / libm::sin(t1);
            let err = (ratio - want) / want;
            println!("rear +{rear} mm: ratio {ratio:.6} (expected {want:.6}), error {err:+e}");
            assert!(ratio > 1.0, "rear +{rear} mm must steepen the game");
            assert!(err.abs() <= 5e-3, "rear +{rear} mm: ratio error {err:e}");
        }
    });
}

// ---------------------------------------------------------------------------
// Item 7: hostile cabinet.toml.
// ---------------------------------------------------------------------------

/// Expects `Err(SimError::Build(msg))` naming `cabinet.toml`.
fn expect_cabinet_build_error(bytes: &[u8], tag: &str) -> Result<(), String> {
    let b = fixture_with_cabinet(tag, bytes);
    match load(&b) {
        Ok(_) => Err(format!("{tag}: hostile cabinet.toml loaded")),
        Err(SimError::Build(msg)) => {
            if msg.contains("cabinet.toml") {
                Ok(())
            } else {
                Err(format!("{tag}: error does not name cabinet.toml: {msg}"))
            }
        }
        Err(other) => Err(format!(
            "{tag}: API mismatch, expected SimError::Build, got {other:?}"
        )),
    }
}

/// REQ-CAB-008 with A.4 ("unknown keys, a schema_version other than 1 and
/// non-finite numbers are errors"; `extension_min ≤ extension ≤
/// extension_max`) and R11: each hostile `cabinet.toml` is a
/// `SimError::Build` naming `cabinet.toml`, no panic.
#[test]
fn req_cab_008_hostile_cabinet_toml_is_a_build_error() {
    on_sim_thread(|| {
        let cases: [(&str, &str); 8] = [
            (
                "unknown_key",
                "schema_version = 1\npreset = \"classic_standard\"\nwobble = 3\n",
            ),
            (
                "unknown_nested_key",
                "schema_version = 1\npreset = \"classic_standard\"\n\n[legs.rear_left.leveler]\nspin = 1.0\n",
            ),
            (
                "schema_version_2",
                "schema_version = 2\npreset = \"classic_standard\"\n",
            ),
            (
                "unknown_preset",
                "schema_version = 1\npreset = \"no_such_cabinet\"\n",
            ),
            (
                "nan",
                "schema_version = 1\npreset = \"classic_standard\"\n\n[legs.rear_left.leveler]\nextension = nan\n",
            ),
            (
                "inf",
                "schema_version = 1\npreset = \"classic_standard\"\n\n[legs.default]\nlength = inf\n",
            ),
            (
                "extension_above_max",
                "schema_version = 1\npreset = \"classic_standard\"\n\n[legs.rear_right.leveler]\nextension = 50.0\n",
            ),
            (
                "extension_below_min",
                "schema_version = 1\npreset = \"modern_standard\"\n\n[legs.front_left.leveler]\nextension = 10.0\n",
            ),
        ];
        let mut failures = Vec::new();
        for (tag, text) in cases {
            if let Err(e) = expect_cabinet_build_error(text.as_bytes(), &format!("item7_{tag}")) {
                failures.push(e);
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    });
}

/// splitmix64, the deterministic byte source of the random-bytes run.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// R11 with REQ-CAB-008: 240 bounded 4 KiB random `cabinet.toml` files
/// (deterministic seed; raw bytes, TOML-alphabet text, and TOML-alphabet
/// text after a valid header) never panic and each is a `SimError::Build`
/// naming `cabinet.toml`.
#[test]
fn req_cab_008_random_cabinet_bytes_are_build_errors() {
    on_sim_thread(|| {
        const ALPHA: &[u8] = b"abcdefghijklmnopqrstuvwxyz_.0123456789 =[]{}\"',\n-+#\t";
        let header = b"schema_version = 1\npreset = \"classic_standard\"\n";
        let mut state = 0x0113_C0FF_EE00_0032u64;
        let mut failures = Vec::new();
        for case in 0..240u32 {
            let mut bytes = Vec::with_capacity(4096);
            if case % 3 == 2 {
                bytes.extend_from_slice(header);
            }
            while bytes.len() < 4096 {
                let r = splitmix(&mut state);
                let b = if case % 3 == 0 {
                    (r & 0xff) as u8
                } else {
                    ALPHA[(r % ALPHA.len() as u64) as usize]
                };
                bytes.push(b);
            }
            if let Err(e) = expect_cabinet_build_error(&bytes, &format!("item7_rand{case}")) {
                failures.push(e);
            }
        }
        assert!(
            failures.is_empty(),
            "{} failures: {failures:#?}",
            failures.len()
        );
    });
}

// ---------------------------------------------------------------------------
// Item 8: property over leveler quadruples.
// ---------------------------------------------------------------------------

/// REQ-CAB-008 property (proptest, 64 cases): random leveler extensions in
/// 0..=44 mm (quarter-mm steps, so equal pairs occur exactly) on
/// classic_standard: |g| = 9.81 within 1e-6, g.z < 0, g equals the formula
/// for the oracle attitude within 1e-5; relative to the level cabinet, the
/// pitch gravity carries (atan2(−g.y, −g.z)) and the −y pull grow exactly
/// when the rear pair is longer than the front (and stay when equal), and g.x is negative / positive / zero as the
/// right pair is longer / shorter / equal to the left.
#[test]
fn req_cab_008_prop_leveler_quadruples() {
    on_sim_thread(|| {
        let level = DEFAULT_PITCH_DEG * DEG;
        let mut runner = TestRunner::new(PtConfig {
            cases: 64,
            failure_persistence: None,
            ..PtConfig::default()
        });
        let q = 0u32..=176;
        let result = runner.run(&(q.clone(), q.clone(), q.clone(), q), |(a, b, c, d)| {
            let e = Extensions {
                fl: f64::from(a) * 0.25,
                fr: f64::from(b) * 0.25,
                rl: f64::from(c) * 0.25,
                rr: f64::from(d) * 0.25,
            };
            let text = cabinet_toml("classic_standard", e);
            let (op, or) = oracle_attitude(&text);
            let bundle = fixture_with_cabinet("item8", text.as_bytes());
            let sim = load(&bundle).map_err(|e| TestCaseError::fail(format!("load: {e:?}")))?;
            let g = sim_gravity(&sim);
            let n = norm(g);
            prop_assert!(((n - G) / G).abs() <= 1e-6, "|g| = {}", n);
            prop_assert!(g[2] < 0.0, "g.z = {}", g[2]);
            let want = gravity_formula(op, or);
            let err = rel_err(g, want);
            prop_assert!(err <= 1e-5, "g {:?} vs {:?}: {}", g, want, err);
            let rear_minus_front = (e.rl + e.rr) - (e.fl + e.fr);
            let right_minus_left = (e.fr + e.rr) - (e.fl + e.rl);
            // The pitch the gravity carries, atan2(−g.y, −g.z) = θ for any φ.
            let pitch = libm::atan2(-g[1], -g[2]);
            let tol = 1e-5;
            if rear_minus_front > 0.0 {
                prop_assert!(
                    pitch > level + tol,
                    "rear longer: pitch {} vs {}",
                    pitch,
                    level
                );
                prop_assert!(g[1] < -G * libm::cos(or) * libm::sin(level));
            } else if rear_minus_front < 0.0 {
                prop_assert!(
                    pitch < level - tol,
                    "front longer: pitch {} vs {}",
                    pitch,
                    level
                );
                prop_assert!(g[1] > -G * libm::cos(or) * libm::sin(level));
            } else {
                prop_assert!((pitch - level).abs() <= tol, "equal: pitch {}", pitch);
            }
            if right_minus_left > 0.0 {
                prop_assert!(g[0] < 0.0, "right longer: g.x {}", g[0]);
            } else if right_minus_left < 0.0 {
                prop_assert!(g[0] > 0.0, "left longer: g.x {}", g[0]);
            } else {
                prop_assert!(g[0].abs() <= 1e-6 * G, "equal sides: g.x {}", g[0]);
            }
            Ok(())
        });
        if let Err(e) = result {
            panic!("{e}");
        }
    });
}
