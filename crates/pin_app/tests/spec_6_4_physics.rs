//! Subphase 6.4 spec tests (Independent Tester), brief S4: emergent physics
//! of the Feature Lab table, over parameter sweeps and without any named
//! special case.
//!
//! * (a) the Banana flipper sends a flipped ball up the table and curls it
//!   relative to the mirrored straight flipper;
//! * (b) the XL ball stays inside its restrict area until it drains;
//! * (c) the backbox: VUK feed to layer 4, the backbox flipper, the drain
//!   back to layer 0, and no momentum exchange between layers 0 and 4;
//! * (d) the Glo-Ball's Magnus force comes from `[physics]
//!   globall_magnus_coeff`, and only the Glo-Ball's;
//! * (e) a recorded game replays to the same `state_hash` 10 times.

#[path = "fixtures/spec_6_4/common.rs"]
mod common;

use common::*;
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::schema::table::{MechKind, SwitchKind};
use pin_core::sim::TICK_NS;
use pin_core::snapshot::BallState;
use pin_sim::replay::{Recorder, replay};
use pin_sim::{BallHandle, Simulation};

// ---------------------------------------------------------------------------
// (a) Banana flipper
// ---------------------------------------------------------------------------

/// Fractions of the bat length at which the ball is placed (9 offsets).
const BAT_OFFSETS: [f32; 9] = [0.30, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60, 0.65, 0.70];

/// Places a ball on the up-side face of `flipper` (at rest) at fraction `s`,
/// lets it touch down (10 ms), presses `button` and returns the ball's
/// velocity when it leaves the bat: the first tick, once it moves faster
/// than 1 m/s, from which its velocity changes by less than 3 cm/s per tick
/// for 3 ticks in a row (free flight, before it meets anything else).
fn flip_from(sim: &mut Simulation, button: &str, flipper: &str, s: f32) -> [f32; 3] {
    sim.release_switch(button).expect("button");
    sim.advance(400);
    let pos = on_bat(sim, flipper, s, 0.5, STEEL);
    let h = sim
        .spawn_ball(STEEL, pos, [0.0; 3])
        .expect("spawn on the bat");
    sim.advance(10);
    // Placed clear of the bat: it touched down without being pushed out of
    // an overlap (rolling under gravity for 10 ms stays far below this).
    let settled = len3(sim.ball(h).expect("ball on the bat").velocity_m_s);
    assert!(
        settled < 0.2,
        "{flipper} offset {s}: the ball placed on the bat moves at {settled:.3} m/s before the flip"
    );
    sim.set_switch(button, true).expect("press");
    let mut prev = sim.ball(h).expect("ball on the bat").velocity_m_s;
    let mut calm = 0;
    let mut out = None;
    for _ in 0..250 {
        sim.advance(1);
        let Some(b) = sim.ball(h) else { break };
        assert!(finite_ball(&b), "non-finite ball off {flipper}");
        let v = b.velocity_m_s;
        if len3(v) > 1.0 && len3(sub3(v, prev)) < 0.03 {
            calm += 1;
            if calm == 3 {
                out = Some(v);
                break;
            }
        } else {
            calm = 0;
        }
        prev = v;
    }
    sim.release_switch(button).expect("release");
    if sim.ball(h).is_some() {
        sim.remove_ball(h);
    }
    sim.advance(400);
    out.unwrap_or_else(|| panic!("{flipper} offset {s}: the flip sent the ball off the bat"))
}

fn heading_deg(v: [f32; 3]) -> f32 {
    v[1].atan2(v[0]).to_degrees()
}

fn wrap_deg(a: f32) -> f32 {
    let mut a = a % 360.0;
    if a > 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

/// The Banana placement follows the curved bat: the ball is `gap` clear of
/// the arc tube (checked inside `on_bat`), whereas the same offset along
/// the pivot's straight bat direction would overlap the curled bat.
#[test]
fn a_banana_placement_clears_the_curved_bat() {
    big_stack(|| {
        let sim = load();
        let f = banana_frame(&sim, "lower_right");
        let r = radius_m(STEEL);
        for s in BAT_OFFSETS {
            let p = on_bat(&sim, "lower_right", s, 0.5, STEEL);
            let clear = (f.side_clearance(p) - r) * 1e3;
            assert!(
                (clear - 0.5).abs() <= 0.05,
                "offset {s}: {clear:.3} mm clear (want 0.5 mm)"
            );
            assert!(
                (dot3(sub3(p, f.pivot), f.n) - r).abs() < 1e-6,
                "offset {s}: the ball touches the mounting plane"
            );
        }
        // The straight-centreline placement (the bat direction at the
        // pivot, `s` of the chord) is inside the curled bat at mid-length.
        let chord = 2.0 * f.arc_radius * (0.5 * f.phi).sin();
        let side = if dot3(f.to_centre, up_vector()) < 0.0 {
            scale3(f.to_centre, -1.0)
        } else {
            f.to_centre
        };
        let straight = add3(
            add3(
                add3(f.pivot, scale3(f.x, 0.5 * chord)),
                scale3(side, f.tube + r + 0.5e-3),
            ),
            scale3(f.n, r),
        );
        let clear = (f.side_clearance(straight) - r) * 1e3;
        assert!(
            clear < -1.0,
            "a straight placement would overlap the Banana (clearance {clear:.2} mm)"
        );
    });
}

#[test]
fn a_banana_flips_up_the_table_and_curls() {
    big_stack(|| {
        let mut sim = load();
        start_game(&mut sim);
        clear_table(&mut sim);
        let mut diffs = Vec::new();
        for s in BAT_OFFSETS {
            let vb = flip_from(&mut sim, "right_flipper", "lower_right", s);
            let vs = flip_from(&mut sim, "left_flipper", "lower_left", s);
            assert!(
                vb[1] > 0.0 && len3(vb) > 0.5,
                "offset {s}: the Banana sends the ball up the table (v = {vb:?})"
            );
            assert!(
                len3(vs) > 0.5,
                "offset {s}: the straight flipper struck the ball (v = {vs:?})"
            );
            // Mirror the straight flipper's heading about the table's
            // centre line (x -> -x) and compare.
            let hb = heading_deg(vb);
            let hs_mirror = 180.0 - heading_deg(vs);
            diffs.push(wrap_deg(hb - hs_mirror).abs());
        }
        let mean = diffs.iter().sum::<f32>() / diffs.len() as f32;
        assert!(
            mean >= 3.0,
            "the Banana's heading differs from the mirrored straight flipper's by {mean:.2}° on average (>= 3°): {diffs:?}"
        );
        assert_healthy(&sim, "banana");
    });
}

// ---------------------------------------------------------------------------
// (b) the XL ball
// ---------------------------------------------------------------------------

fn trough_hits(sim: &Simulation) -> u64 {
    table()
        .switches
        .iter()
        .filter(|s| s.kind == SwitchKind::Trough)
        .map(|s| u64::from(hits(sim, &s.name)))
        .sum()
}

/// Whether `p` is within a trough switch zone or a magnet's range.
fn exempt_from_stuck(p: [f32; 3], r: f32) -> bool {
    let t = table();
    t.switches.iter().any(|s| {
        s.kind == SwitchKind::Trough
            && s.position_m
                .is_some_and(|q| plan_dist(p, q) <= s.radius_m.unwrap_or(0.0) + r + 0.01)
    }) || t
        .magnets
        .iter()
        .any(|m| plan_dist(p, m.position_m) <= m.range_m + r)
}

#[test]
fn b_xl_ball_stays_in_its_area_until_it_drains() {
    big_stack(|| {
        let area = xl_area_mm();
        let feed = table()
            .dispensers
            .iter()
            .find(|d| d.name == "xl_feed")
            .expect("xl_feed");
        let r = radius_m(XL);
        let launches: [(f32, f32); 8] = [
            (90.0, 0.3),
            (60.0, 0.6),
            (120.0, 0.6),
            (90.0, 1.0),
            (45.0, 0.4),
            (135.0, 0.4),
            (75.0, 1.4),
            (105.0, 1.4),
        ];
        let mut sim = load();
        sim.advance(200);
        for (deg, speed) in launches {
            let what = format!("XL launch {deg}° {speed} m/s");
            let d = dir2(deg);
            let v = [d[0] * speed, d[1] * speed, 0.0];
            let escaped = sim.counters().balls_escaped;
            let troughs = trough_hits(&sim);
            let h = sim
                .spawn_ball(XL, feed.exit_position_m, v)
                .unwrap_or_else(|e| panic!("{what}: spawn: {e}"));
            let mut slow = 0u64;
            let mut drained = false;
            for t in 0..60_000u64 {
                sim.advance(1);
                let Some(b) = sim.ball(h) else {
                    drained = true;
                    break;
                };
                assert!(finite_ball(&b), "{what}: non-finite at {t} ms: {b:?}");
                assert_eq!(b.layer, 0, "{what}: on layer 0 at {t} ms");
                let p = b.position_m;
                assert!(
                    in_polygon(&area, [p[0] * 1e3, p[1] * 1e3]),
                    "{what}: centre ({:.1}, {:.1}) mm left the XL area at {t} ms",
                    p[0] * 1e3,
                    p[1] * 1e3
                );
                if len3(b.velocity_m_s) < 0.01 && !exempt_from_stuck(p, r) {
                    slow += 1;
                    assert!(
                        slow < 10_000,
                        "{what}: stuck (< 1 cm/s for 10 s) at ({:.1}, {:.1}) mm",
                        p[0] * 1e3,
                        p[1] * 1e3
                    );
                } else {
                    slow = 0;
                }
            }
            assert_eq!(
                sim.counters().balls_escaped,
                escaped,
                "{what}: the ball escaped"
            );
            assert!(
                drained,
                "{what}: the ball drained within 60 s (last seen {:?})",
                sim.ball(h)
            );
            assert!(
                trough_hits(&sim) > troughs,
                "{what}: the ball left through a trough switch"
            );
            sim.advance(200);
        }
    });
}

// ---------------------------------------------------------------------------
// (c) the backbox
// ---------------------------------------------------------------------------

fn vuk_spawn() -> (u8, [f32; 3]) {
    let t = table();
    let def = t
        .manifest
        .mechs
        .iter()
        .find(|m| m.id == BACKBOX_VUK)
        .expect("backbox VUK");
    assert_eq!(def.kind, MechKind::Vuk);
    let si = t.mechs.iter().find(|m| m.name == BACKBOX_VUK).expect("VUK");
    let p = si.position_m.expect("VUK position");
    (def.layer.unwrap_or(0), [p[0], p[1], p[2] + radius_m(STEEL)])
}

#[test]
fn c_vuk_feeds_the_backbox_and_the_drain_returns_the_ball() {
    big_stack(|| {
        let mut sim = load();
        start_game(&mut sim);
        clear_table(&mut sim);
        sim.advance(100);
        let (layer, p) = vuk_spawn();
        let t0 = sim.tick();
        let troughs = trough_hits(&sim);
        let h = sim
            .spawn_ball_on_layer(STEEL, layer, p, [0.0; 3])
            .expect("spawn into the VUK");
        let on4 = run_until(&mut sim, 10_000, |s| {
            s.ball(h).is_some_and(|b| b.layer == 4)
        });
        assert!(on4, "the VUK put the ball on collision layer 4");
        let left = 60_000 - (sim.tick() - t0);
        let back = run_until(&mut sim, left, |s| s.ball(h).is_none_or(|b| b.layer == 0));
        assert!(
            back && sim.ball(h).is_some(),
            "the ball came back to layer 0 through the backbox drain"
        );
        let left = 60_000 - (sim.tick() - t0);
        let gone = run_until(&mut sim, left, |s| s.ball(h).is_none());
        assert!(gone, "the ball reached a trough within 60 s");
        assert!(trough_hits(&sim) > troughs, "through a trough switch");
        assert_eq!(sim.counters().balls_escaped, 0);
        assert_healthy(&sim, "backbox round trip");
    });
}

#[test]
fn c_backbox_flipper_moves_and_strikes() {
    big_stack(|| {
        let mut sim = load();
        start_game(&mut sim);
        clear_table(&mut sim);
        sim.advance(400);
        let (a0, _) = sim.flipper(BACKBOX_FLIPPER).expect("backbox flipper");
        sim.set_switch("left_upper", true).unwrap();
        sim.advance(80);
        let (a1, _) = sim.flipper(BACKBOX_FLIPPER).unwrap();
        assert!(
            (a1 - a0).abs().to_degrees() > 10.0,
            "left_upper moves the backbox flipper ({:.1}° -> {:.1}°)",
            a0.to_degrees(),
            a1.to_degrees()
        );
        sim.release_switch("left_upper").unwrap();
        sim.advance(400);
        let (a2, _) = sim.flipper(BACKBOX_FLIPPER).unwrap();
        assert!((a2 - a0).abs().to_degrees() < 2.0, "it returns to rest");

        // A ball on the bat is struck.
        let mut struck = 0;
        for s in [0.4f32, 0.55, 0.7] {
            let pos = on_bat(&sim, BACKBOX_FLIPPER, s, 1.0, STEEL);
            let h = sim
                .spawn_ball_on_layer(STEEL, 4, pos, [0.0; 3])
                .expect("spawn at the backbox flipper");
            sim.advance(15);
            let before = sim.ball(h).map_or(0.0, |b| len3(b.velocity_m_s));
            sim.set_switch("left_upper", true).unwrap();
            let mut fastest = 0.0f32;
            for _ in 0..150 {
                sim.advance(1);
                if let Some(b) = sim.ball(h) {
                    assert!(finite_ball(&b));
                    fastest = fastest.max(len3(b.velocity_m_s));
                }
            }
            sim.release_switch("left_upper").unwrap();
            if fastest >= 1.0 && fastest > before + 0.5 {
                struck += 1;
            }
            if sim.ball(h).is_some() {
                sim.remove_ball(h);
            }
            sim.advance(400);
        }
        assert_eq!(
            struck, 3,
            "the backbox flipper struck the ball at all 3 offsets"
        );
        assert_healthy(&sim, "backbox flipper");
    });
}

/// Trajectory (positions per tick) of each spawned ball over `ticks`
/// ticks; a ball's trajectory ends when it leaves the table.
fn trajectories(spawns: &[(u8, [f32; 3])], ticks: u64) -> Vec<Vec<[f32; 3]>> {
    let mut sim = load();
    sim.advance(50);
    let hs: Vec<BallHandle> = spawns
        .iter()
        .map(|&(l, p)| {
            sim.spawn_ball_on_layer(STEEL, l, p, [0.0; 3])
                .expect("spawn")
        })
        .collect();
    let mut out = vec![Vec::new(); hs.len()];
    let mut alive = vec![true; hs.len()];
    for _ in 0..ticks {
        sim.advance(1);
        for (i, &h) in hs.iter().enumerate() {
            match sim.ball(h) {
                Some(b) if alive[i] => out[i].push(b.position_m),
                _ => alive[i] = false,
            }
        }
    }
    out
}

#[test]
fn c_layers_0_and_4_never_exchange_momentum() {
    big_stack(|| {
        let t = table();
        let sim = load();
        let def = t
            .manifest
            .flippers
            .iter()
            .find(|f| f.id == BACKBOX_FLIPPER)
            .unwrap();
        let id = sim.ids().flipper(BACKBOX_FLIPPER).unwrap();
        let pivot = sim.flipper_set().pivot_m(id).unwrap().to_array();
        let n = norm3(def.surface_normal);
        let up = up_vector();
        let in_zone = |p: [f32; 3]| {
            t.transitions.iter().any(|z| {
                (0..3).all(|k| p[k] >= z.zone_min_m[k] - 0.03 && p[k] <= z.zone_max_m[k] + 0.03)
            })
        };
        let r = radius_m(STEEL);
        // Two centres a small, nonzero distance apart in the backbox plane
        // (the solver skips a pair at zero separation, so coincident balls
        // would prove nothing): 8 mm across the box, well inside 2 r.
        let across = norm3(cross3(n, up));
        let offset = 0.008f32;
        let (p, q) = [0.08f32, 0.12, 0.05, 0.16]
            .iter()
            .map(|&h| {
                let p = add3(add3(pivot, scale3(n, r)), scale3(up, h));
                (p, add3(p, scale3(across, offset)))
            })
            .find(|&(p, q)| !in_zone(p) && !in_zone(q))
            .expect("two points in the backbox clear of every layer transition");
        drop(sim);
        let sep = len3(sub3(p, q));
        assert!(
            sep > 1e-3 && sep < 2.0 * r - 5e-3,
            "the pair overlaps by a clear margin at a nonzero separation ({:.2} mm, 2 r = {:.2} mm)",
            sep * 1e3,
            2.0 * r * 1e3
        );
        let ticks = 300;
        let deviation = |single: &[[f32; 3]], pair: &[[f32; 3]]| -> (usize, f32) {
            let n = single.len().min(pair.len());
            let worst = (0..n)
                .map(|i| len3(sub3(single[i], pair[i])))
                .fold(0.0f32, f32::max);
            (n, worst)
        };
        // Control: the same offset on one layer makes contact, so each
        // ball's path measurably differs from its single-ball run.
        let alone4p = trajectories(&[(4, p)], ticks);
        let alone4q = trajectories(&[(4, q)], ticks);
        let same = trajectories(&[(4, p), (4, q)], ticks);
        let (n0, d0) = deviation(&alone4p[0], &same[0]);
        let (n1, d1) = deviation(&alone4q[0], &same[1]);
        assert!(
            n0 >= 1 && n1 >= 1 && d0.max(d1) >= 1e-3,
            "control: two layer-4 balls {:.1} mm apart interact (deviations {:.3} / {:.3} mm over {n0} / {n1} ticks)",
            sep * 1e3,
            d0 * 1e3,
            d1 * 1e3
        );
        // Layers 4 and 0 at the same offset: no exchange at all.
        let alone0q = trajectories(&[(0, q)], ticks);
        let both = trajectories(&[(4, p), (0, q)], ticks);
        for (k, (single, pair)) in [(&alone4p[0], &both[0]), (&alone0q[0], &both[1])]
            .into_iter()
            .enumerate()
        {
            let n = single.len().min(pair.len());
            assert!(n >= 50, "ball {k}: compared {n} ticks");
            assert_eq!(single.len(), pair.len(), "ball {k}: same lifetime");
            for i in 0..n {
                let d = len3(sub3(single[i], pair[i]));
                assert!(
                    d <= 1e-6,
                    "ball {k} (layer {}) deviates {:.3} mm at tick {i} from its single-ball run",
                    if k == 0 { 4 } else { 0 },
                    d * 1e3
                );
            }
        }
    });
}

// ---------------------------------------------------------------------------
// (d) Magnus force of the Glo-Ball
// ---------------------------------------------------------------------------

/// The launch: from the table's centre line (midway between the two lower
/// flipper pivots), 150 mm above them, up the table at 0.8 m/s with
/// 200 rad/s of spin about the playfield normal (z): Magnus `S·ω×v` with
/// ω ∥ z and v ∥ y is along x, i.e. lateral.
fn launch(sim: &mut Simulation, profile: &str) -> BallHandle {
    let f = &table().manifest.flippers;
    let base = |id: &str| {
        f.iter()
            .find(|x| x.id == id)
            .expect("lower flipper")
            .base_position
    };
    let (l, r_) = (base("lower_left"), base("lower_right"));
    let x = 0.5 * (l[0] + r_[0]) * 1e-3;
    let y = 0.5 * (l[1] + r_[1]) * 1e-3 + 0.15;
    let r = radius_m(profile);
    let h = sim
        .spawn_ball(profile, [x, y, r], [0.0, 0.8, 0.0])
        .expect("spawn");
    let mut b: BallState = sim.ball(h).unwrap();
    b.angular_velocity_rad_s = [0.0, 0.0, 200.0];
    assert!(sim.set_ball_state(h, &b), "set the spin");
    h
}

/// Runs 1.5 s and returns the launched ball's state at that tick (it must
/// still be on the table).
fn magnus_run(bundle_path: &std::path::Path, profile: &str) -> (BallState, u64) {
    let mut sim = load_from(bundle_path);
    sim.advance(100);
    let h = launch(&mut sim, profile);
    sim.advance(1_500);
    let last = sim.ball(h).unwrap_or_else(|| {
        panic!(
            "{profile} ({}): the launched ball left the table before the 1.5 s sample",
            bundle_path.display()
        )
    });
    (last, sim.state_hash())
}

#[test]
fn d_magnus_comes_from_the_physics_override() {
    let coeff = table().physics.globall_magnus_coeff;
    assert!(
        coeff != 0.0,
        "FINDING: the table's globall_magnus_coeff is already 0 (no Magnus to test)"
    );
    assert!(
        (0.0..=5e-5).contains(&coeff),
        "globall_magnus_coeff {coeff} within the bounds 0..5e-5"
    );
    let zero = modified_bundle("magnus_zero", |t| {
        let phys = t
            .entry("physics")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .expect("[physics] is a table");
        phys.insert("globall_magnus_coeff".into(), toml::Value::Float(0.0));
    });
    big_stack(move || {
        let orig = bundle().to_path_buf();
        let (g1, _) = magnus_run(&orig, GLO);
        let (g0, _) = magnus_run(&zero, GLO);
        let dx = (g1.position_m[0] - g0.position_m[0]).abs();
        assert!(
            dx >= 0.002,
            "Glo-Ball: lateral difference {:.2} mm (>= 2 mm) between coeff {coeff} and 0 ({:?} vs {:?})",
            dx * 1e3,
            g1.position_m,
            g0.position_m
        );
        let (s1, h1) = magnus_run(&orig, STEEL);
        let (s0, h0) = magnus_run(&zero, STEEL);
        let bits = |b: &BallState| -> Vec<u32> {
            b.position_m
                .iter()
                .chain(&b.velocity_m_s)
                .chain(&b.angular_velocity_rad_s)
                .chain(&b.orientation)
                .map(|v| v.to_bits())
                .collect()
        };
        assert_eq!(
            bits(&s1),
            bits(&s0),
            "standard_steel: bit-identical final state"
        );
        assert_eq!(s1.layer, s0.layer);
        assert_eq!(s1.profile, s0.profile);
        assert_eq!(h1, h0, "standard_steel: identical state_hash");
    });
}

// ---------------------------------------------------------------------------
// (e) replay determinism
// ---------------------------------------------------------------------------

#[test]
fn e_recorded_game_replays_identically_10_times() {
    big_stack(|| {
        let sim = load();
        let mut rec = Recorder::new(sim);
        let ticks = 30_000u64;
        let mut schedule: Vec<(u64, CabinetInput, bool)> = vec![
            (100, CabinetInput::Start, true),
            (180, CabinetInput::Start, false),
            (1_500, CabinetInput::Launch, true),
            (1_600, CabinetInput::Launch, false),
        ];
        let mut k = 2_000;
        let mut i = 0u64;
        while k < ticks - 500 {
            let input = match i % 4 {
                0 => CabinetInput::LeftFlipper,
                1 => CabinetInput::RightFlipper,
                2 => CabinetInput::LeftUpper,
                _ => CabinetInput::RightFlipper,
            };
            schedule.push((k, input, true));
            schedule.push((k + 110, input, false));
            if i % 7 == 3 {
                schedule.push((k + 30, CabinetInput::Action, true));
                schedule.push((k + 80, CabinetInput::Action, false));
            }
            if i % 11 == 5 {
                schedule.push((k + 40, CabinetInput::Launch, true));
                schedule.push((k + 90, CabinetInput::Launch, false));
            }
            k += 331;
            i += 1;
        }
        schedule.sort_by_key(|e| e.0);
        let mut next = 0;
        let mut evs: Vec<InputEvent> = Vec::new();
        for tick in 1..=ticks {
            evs.clear();
            while next < schedule.len() && schedule[next].0 == tick {
                let (_, input, p) = schedule[next];
                evs.push(InputEvent::button(tick * TICK_NS - TICK_NS / 2, input, p));
                next += 1;
            }
            rec.step(&evs);
        }
        let (sim, recording) = rec.finish();
        assert!(hits(&sim, "start") >= 1);
        assert!(hits(&sim, "action") >= 1);
        assert!(hits(&sim, "left_flipper") >= 1);
        assert!(
            pv(&sim, "game_state") >= 1 || pv(&sim, "games_played") >= 1,
            "a game was started"
        );
        assert_healthy(&sim, "recorded game");
        let expected = sim.state_hash();
        for run in 0..10 {
            let got = replay(bundle(), &recording).expect("replays");
            assert_eq!(got, expected, "run {run}: replay diverged");
        }
    });
}
