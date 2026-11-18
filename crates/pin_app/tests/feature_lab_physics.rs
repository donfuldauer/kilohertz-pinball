//! 6.4 R4..R7: the Feature Lab's restored features in the headless sim.
//!
//! * R4 backbox: a ball fed through the main-playfield VUK reaches the
//!   backbox layer, can be flipped by the vertical flipper, leaves through
//!   the backbox drain to layer 0 and on to a trough; layer-4 and layer-0
//!   balls at the same centre never touch.
//! * R5 XL ball: over 8 launch velocities from `xl_feed`'s exit the XL
//!   ball's centre stays inside its restriction area on layer 0 until
//!   `xl_trough` drains it.
//! * R6 magnet comparison: over 3 launch variants the rules measure a
//!   `standard_steel` release at least 10 ms longer than the `low_rem_steel`
//!   one, and both balls are held at the pole while the coil is on.
//! * R7 Glo-Ball: the same spinning launch curves measurably differently
//!   with the table's Magnus coefficient than with 0, while a
//!   `standard_steel` ball is bit-identical under both loads.

mod feature_lab_support;

use feature_lab_support::*;
use pin_core::ids::MagnetId;
use pin_core::schema::table::TableManifest;
use pin_sim::{BallHandle, SimConfig, Simulation};
use pin_tablegen::backbox::{BackboxFrame, parse_backbox_layout};
use std::path::{Path, PathBuf};

// --- helpers -------------------------------------------------------------

fn frame() -> BackboxFrame {
    let t = std::fs::read_to_string(feature_lab_src().join("backbox_layout.toml")).unwrap();
    let l = parse_backbox_layout(&t).unwrap();
    BackboxFrame::new(l.backbox.origin, 6.5)
}

/// Table point (m) to backbox plane coordinates `[u, depth, v]`, mm.
fn local(f: &BackboxFrame, p: [f32; 3]) -> [f32; 3] {
    let (o, n, up) = (f.origin_m, f.normal(), f.up());
    let d = [p[0] - o.x, p[1] - o.y, p[2] - o.z];
    let y = -(d[0] * n.x + d[1] * n.y + d[2] * n.z);
    let v = d[0] * up.x + d[1] * up.y + d[2] * up.z;
    [d[0] * 1.0e3, y * 1.0e3, v * 1.0e3]
}

/// Backbox plane coordinates (mm) to a table point (m).
fn to_table(f: &BackboxFrame, q: [f32; 3]) -> [f32; 3] {
    let (o, n, up) = (f.origin_m, f.normal(), f.up());
    let (u, y, v) = (q[0] * 1.0e-3, q[1] * 1.0e-3, q[2] * 1.0e-3);
    [
        o.x + u - y * n.x + v * up.x,
        o.y - y * n.y + v * up.y,
        o.z - y * n.z + v * up.z,
    ]
}

/// A copy of the bundle whose manifest `edit` changed.
fn edited(tag: &str, edit: impl FnOnce(&mut TableManifest)) -> PathBuf {
    let dir = bundle_copy(tag);
    let text = std::fs::read_to_string(dir.join("table.toml")).unwrap();
    let mut t = TableManifest::from_toml_str(&text).unwrap();
    edit(&mut t);
    std::fs::write(dir.join("table.toml"), t.to_toml_string().unwrap()).unwrap();
    dir
}

fn load_dir(dir: &Path) -> Simulation {
    Simulation::load(dir, SimConfig::default()).expect("load")
}

// --- R4: backbox -----------------------------------------------------------

/// Feeds a ball into the VUK at the top of the VUK lane (rolled up into the
/// scoop); returns the sim, the ball and the tick it reached layer 4.
fn feed_backbox(seed: u64) -> (Simulation, BallHandle, u32) {
    let mut s = load_seeded(bundle(), seed);
    let h = s
        .spawn_ball("standard_steel", mm(425.0, 580.0), [0.0, 0.6, 0.0])
        .unwrap();
    for t in 0..2000u32 {
        s.advance(1);
        let b = s.ball(h).expect("ball stays in play");
        if b.layer == 4 {
            assert!(s.switch_hits("bb_vuk_sw").unwrap() >= 1, "captured first");
            return (s, h, t);
        }
    }
    panic!("seed {seed}: the VUK never lifted the ball onto layer 4");
}

#[test]
fn vuk_feeds_the_backbox_and_the_ball_drains_back_to_a_trough() {
    on_big_stack(|| {
        let f = frame();
        for seed in 0..4u64 {
            let (mut s, h, _) = feed_backbox(seed);
            // Inside the box: between the glass and the back panel.
            let mut drained_bb = false;
            let mut back_on_0 = false;
            for t in 0..30_000u32 {
                s.advance(1);
                let Some(b) = s.ball(h) else {
                    break;
                };
                if b.layer == 4 {
                    let q = local(&f, b.position_m);
                    assert!(
                        q[1] > 12.0 && q[1] < 23.0 && q[0] > -200.0 && q[0] < 200.0,
                        "seed {seed} tick {t}: inside the box: {q:?}"
                    );
                } else if !back_on_0 {
                    back_on_0 = true;
                    drained_bb = s.switch_hits("bb_drain").unwrap() >= 1;
                }
            }
            healthy(&s);
            assert!(back_on_0, "seed {seed}: the ball came back to layer 0");
            assert!(drained_bb, "seed {seed}: through the backbox drain");
            assert!(s.ball(h).is_none(), "seed {seed}: and drained");
            let troughs = s.switch_hits("trough_1").unwrap() + s.switch_hits("trough_2").unwrap();
            assert!(troughs >= 1, "seed {seed}: through a main trough switch");
        }
    });
}

#[test]
fn the_backbox_flipper_flips_a_ball_on_layer_4() {
    on_big_stack(|| {
        let f = frame();
        let (mut s, h, _) = feed_backbox(0);
        // Wait until the ball rests on or rolls along the bat: near the
        // pivot (-60, 120) and above the bat's line.
        let pivot = [-60.0f32, 120.0f32];
        let mut on_bat = false;
        for _ in 0..2000 {
            s.advance(1);
            let b = s.ball(h).unwrap();
            let q = local(&f, b.position_m);
            let du = q[0] - pivot[0];
            let line = pivot[1] - 0.58 * du;
            if b.layer == 4 && (0.0..60.0).contains(&du) && q[2] > line - 5.0 && q[2] < line + 30.0
            {
                on_bat = true;
                break;
            }
        }
        assert!(on_bat, "the ball reaches the backbox bat");
        let eos = s.switch_hits("bb_flipper_eos").unwrap();
        s.set_switch("left_upper", true).unwrap();
        let mut best_up = 0.0f32;
        for _ in 0..80 {
            s.advance(1);
            let b = s.ball(h).unwrap();
            let up = f.up();
            let v = b.velocity_m_s;
            best_up = best_up.max(v[0] * up.x + v[1] * up.y + v[2] * up.z);
        }
        s.release_switch("left_upper").unwrap();
        s.advance(300);
        assert!(
            best_up > 0.8,
            "the flip throws the ball up the box: {best_up} m/s"
        );
        let (angle, _) = s.flipper("tablegen:backbox.flippers[0]").unwrap();
        assert!(angle.abs() < 0.05, "back at rest: {angle}");
        assert_eq!(
            s.switch_hits("bb_flipper_eos").unwrap(),
            eos + 1,
            "EOS cycled"
        );
        healthy(&s);
    });
}

/// Positions (mm) of a ball spawned at `p` on `layer`, per tick.
fn path(dir: &Path, balls: &[(u8, [f32; 3])], ticks: u32) -> Vec<Vec<[f32; 3]>> {
    let mut s = load_dir(dir);
    let hs: Vec<BallHandle> = balls
        .iter()
        .map(|&(l, p)| {
            s.spawn_ball_on_layer("standard_steel", l, p, [0.0; 3])
                .unwrap()
        })
        .collect();
    let mut out = vec![Vec::new(); hs.len()];
    for _ in 0..ticks {
        s.advance(1);
        for (o, h) in out.iter_mut().zip(&hs) {
            if let Some(b) = s.ball(*h) {
                o.push(b.position_m);
            }
        }
    }
    out
}

#[test]
fn layer_4_and_layer_0_balls_never_collide() {
    on_big_stack(|| {
        let f = frame();
        // Above the backbox flipper, mid-depth: clear of every transition.
        for q in [
            [-40.0f32, 17.5, 200.0],
            [60.0, 17.5, 150.0],
            [-150.0, 17.5, 250.0],
        ] {
            let p = to_table(&f, q);
            let alone4 = path(bundle(), &[(4, p)], 300).remove(0);
            let alone0 = path(bundle(), &[(0, p)], 300).remove(0);
            let both = path(bundle(), &[(4, p), (0, p)], 300);
            assert_eq!(both[0], alone4, "{q:?}: the layer-4 ball is undisturbed");
            assert_eq!(both[1], alone0, "{q:?}: the layer-0 ball is undisturbed");
            assert_eq!(both[0][0], alone4[0]);
        }
    });
}

// --- R5: XL ball -------------------------------------------------------------

#[test]
fn xl_ball_stays_in_its_area_and_drains_through_xl_trough() {
    on_big_stack(|| {
        let area = xl_area(
            &TableManifest::from_toml_str(
                &std::fs::read_to_string(bundle().join("table.toml")).unwrap(),
            )
            .unwrap(),
        );
        let exit = [0.055f32, 0.210, 0.0397];
        let velocities: [[f32; 2]; 9] = [
            [0.1, -0.1],
            [0.0, 0.0],
            [0.6, 0.0],
            [-0.6, 0.3],
            [0.0, 0.8],
            [1.2, 0.5],
            [-1.0, -1.0],
            [0.8, -1.4],
            [1.5, 1.5],
        ];
        for v in velocities {
            let mut s = load();
            let before = s.switch_hits("xl_trough").unwrap();
            let h = s
                .spawn_ball("jumbo_xl_resin", exit, [v[0], v[1], 0.0])
                .unwrap();
            let mut drained_at = None;
            for t in 0..20_000u32 {
                s.advance(1);
                match s.ball(h) {
                    Some(b) => {
                        assert_eq!(b.layer, 0, "{v:?} tick {t}: layer 0");
                        let p = [b.position_m[0] * 1.0e3, b.position_m[1] * 1.0e3];
                        assert!(
                            inside_even_odd(&area, p),
                            "{v:?} tick {t}: centre {p:?} outside the XL area"
                        );
                    }
                    None => {
                        drained_at = Some(t);
                        break;
                    }
                }
            }
            let t = drained_at.unwrap_or_else(|| panic!("{v:?}: never drained (stuck)"));
            assert_eq!(s.switch_hits("xl_trough").unwrap(), before + 1, "{v:?}");
            assert_eq!(s.counters().balls_escaped, 0, "{v:?}");
            assert_eq!(s.counters().balls_drained, 1, "{v:?}");
            println!("XL launch {v:?}: drained after {t} ms");
        }
    });
}

// --- R6: magnet comparison ---------------------------------------------------

/// Starts a game and the magnet_compare mode (shot magnet_lane: the served
/// ball rolled over the magnet into the VUK lane).
fn start_magnet_compare(seed: u64) -> Simulation {
    let mut s = load_seeded(bundle(), seed);
    s.advance(10);
    start_game(&mut s);
    select_mode(&mut s, 3);
    let h = balls(&s)[0];
    let mut b = s.ball(h).unwrap();
    b.position_m = mm(359.0, 392.0);
    b.velocity_m_s = [0.73, 1.86, 0.0];
    s.set_ball_state(h, &b);
    for _ in 0..1500 {
        s.advance(1);
        if persist(&s, "mode:magnet_compare") == 1 {
            break;
        }
    }
    assert_eq!(
        persist(&s, "mode:magnet_compare"),
        1,
        "mode started by the shot"
    );
    // Park the player's ball on the plunger, out of the way.
    let mut b = s.ball(h).unwrap();
    b.position_m = mm(492.0, 70.0);
    b.velocity_m_s = [0.0; 3];
    b.angular_velocity_rad_s = [0.0; 3];
    s.set_ball_state(h, &b);
    s
}

#[test]
fn standard_steel_releases_later_than_low_rem_steel() {
    on_big_stack(|| {
        let pole = [0.370f32, 0.420];
        // Variants: initial kick of the served ball (m/s) and seed.
        for (seed, kick) in [(1u64, [0.0f32, 0.0]), (2, [0.03, 0.0]), (3, [-0.02, 0.02])] {
            let mut s = start_magnet_compare(seed);
            let main = balls(&s)[0];
            let mut seen = Vec::new();
            let mut held_ticks = [0u32; 2];
            for _ in 0..10_000 {
                s.advance(1);
                for h in balls(&s) {
                    if h != main && !seen.contains(&h) {
                        seen.push(h);
                        let mut b = s.ball(h).unwrap();
                        b.velocity_m_s = [kick[0], kick[1], 0.0];
                        s.set_ball_state(h, &b);
                    }
                }
                let on = s.solver().magnet_power(MagnetId(0)) > 0.0;
                if on && let Some(&h) = seen.last() {
                    let b = s.ball(h).unwrap();
                    let d = ((b.position_m[0] - pole[0]).powi(2)
                        + (b.position_m[1] - pole[1]).powi(2))
                    .sqrt();
                    assert!(
                        d < 0.005,
                        "seed {seed}: comparison ball held at the pole ({d} m)"
                    );
                    held_ticks[seen.len() - 1] += 1;
                }
                if persist(&s, "cmp_release_ms_low_rem") > 0 {
                    break;
                }
            }
            healthy(&s);
            assert_eq!(seen.len(), 2, "seed {seed}: two comparison balls served");
            assert!(
                held_ticks[0] >= 1000 && held_ticks[1] >= 1000,
                "{held_ticks:?}"
            );
            let std = persist(&s, "cmp_release_ms_standard");
            let low = persist(&s, "cmp_release_ms_low_rem");
            println!("variant seed {seed} kick {kick:?}: standard {std} ms, low_rem {low} ms");
            assert!(low > 0, "seed {seed}: low_rem measured");
            assert!(
                std >= low + 10,
                "seed {seed}: standard {std} ms vs low_rem {low} ms"
            );
        }
    });
}

// --- R7: Glo-Ball Magnus -------------------------------------------------------

/// Spins `profile` up the middle (0.8 m/s, 200 rad/s about the normal) and
/// returns its positions over 1.5 s.
fn spin_run(dir: &Path, profile: &str) -> Vec<[f32; 3]> {
    let mut s = load_dir(dir);
    let h = s
        .spawn_ball(profile, mm(257.0, 300.0), [0.0, 0.8, 0.0])
        .unwrap();
    let mut b = s.ball(h).unwrap();
    b.angular_velocity_rad_s = [0.0, 0.0, 200.0];
    s.set_ball_state(h, &b);
    (0..1500)
        .map(|_| {
            s.advance(1);
            s.ball(h).map_or([f32::NAN; 3], |b| b.position_m)
        })
        .collect()
}

#[test]
fn glo_ball_curves_with_magnus_and_steel_does_not() {
    on_big_stack(|| {
        let table = TableManifest::from_toml_str(
            &std::fs::read_to_string(bundle().join("table.toml")).unwrap(),
        )
        .unwrap();
        let coeff = table
            .physics
            .as_ref()
            .and_then(|p| p.globall_magnus_coeff)
            .expect("the table sets globall_magnus_coeff");
        assert!(coeff > 0.0 && coeff <= 5.0e-5, "{coeff}");
        let zero = edited("magnus_zero", |t| {
            t.physics.as_mut().unwrap().globall_magnus_coeff = Some(0.0);
        });
        let with = spin_run(bundle(), "uv_globall");
        let without = spin_run(&zero, "uv_globall");
        let dx = (with[1499][0] - without[1499][0]).abs() * 1.0e3;
        println!("Glo-Ball lateral difference after 1.5 s: {dx:.2} mm");
        assert!(dx >= 2.0, "Magnus moves the Glo-Ball sideways: {dx} mm");
        assert_eq!(
            spin_run(bundle(), "standard_steel"),
            spin_run(&zero, "standard_steel"),
            "a steel ball does not feel the Glo-Ball coefficient"
        );
    });
}
