//! Subphase 6.2 spec tests (Independent Tester): multi-layer physics of the
//! "Sky Deck" table (brief C3).
//!
//! * (a) the VUK (layer 0) lifts a ball onto the upper deck (layer 1), where
//!   it rolls at deck height; `vuk_sw` and `deck_opto` count it;
//! * (b) the 52 mm upper-right flipper on layer 1 shoots a resting deck ball
//!   out in a plausible speed band, along its sweep;
//! * (c) balls on different layers never collide: a layer-0 steel ball and a
//!   layer-1 Glo-Ball sent through the same plan-view point follow exactly
//!   the trajectories they follow alone, while two balls on one layer on the
//!   same paths exchange momentum;
//! * (d) every spawned ball stays finite and inside the table box.
//!
//! Black-box: geometry is read from the bundle (`load_table`, its
//! `layout.toml`), behaviour from the public `pin_sim::Simulation` API on
//! simulated time.

#[path = "fixtures/spec_6_2/common.rs"]
mod common;

use common::{
    DECK_Z_M, GLO, GLO_ID, STEEL, add, assert_healthy, big_stack, cabinet, clear_table, dir, dot,
    edge_distance, finite, game_seeded, glo_radius_m, hits, inside, layout, len, load, plan_dist,
    scale, steel_radius_m, table,
};
use pin_core::PhysicsDefaults;
use pin_core::input::CabinetInput;
use pin_core::schema::table::FlipperShapeDef;
use pin_core::snapshot::BallState;
use pin_sim::{BallHandle, Simulation};

// ---------------------------------------------------------------------------
// (a) VUK to the upper deck
// ---------------------------------------------------------------------------

/// A ball rolled into the VUK on layer 0 is captured (`vuk_sw`), ejected
/// onto layer 1 and then rolls on the acrylic deck: its centre is at deck
/// height plus its radius (65 + 13.5 mm) within 3 mm for 50 consecutive
/// ticks ("within a few mm": 3 mm is about a tenth of a ball diameter and
/// well under the 13.5 mm a ball would sit lower if it were on another
/// surface); the snapshot shows it on layer 1, and the deck opto counts it
/// within 5 s of the capture (the deck shot `vuk_sw` -> `deck_opto`).
#[test]
fn c3a_vuk_lifts_a_ball_onto_the_deck() {
    big_stack(|| {
        let t = table();
        let vuk = t
            .manifest
            .mechs
            .iter()
            .find(|m| m.id == "vuk")
            .expect("vuk");
        let p = vuk.position.expect("vuk position");
        let layer = vuk.layer.unwrap_or(0);
        assert_eq!(layer, 0);
        let r = steel_radius_m();
        // Approaches: dropped in at rest, and rolled in from 40 mm below
        // (the pocket's open side, down the table) straight and at +-20 deg.
        let approaches: [(f32, f32); 5] = [
            (0.0, 0.0),
            (-90.0, 0.3),
            (-90.0, 0.6),
            (-70.0, 0.4),
            (-110.0, 0.4),
        ];
        for (k, &(from_deg, speed)) in approaches.iter().enumerate() {
            let mut sim = game_seeded(1 + k as u64);
            clear_table(&mut sim);
            sim.advance(20);
            let c = [p[0] * 1e-3, p[1] * 1e-3, p[2] * 1e-3 + r];
            let u = dir(from_deg);
            let start = if speed > 0.0 {
                add(c, scale(u, 0.04))
            } else {
                c
            };
            let h = sim
                .spawn_ball_on_layer(STEEL, layer, start, scale(u, -speed))
                .expect("spawn towards the VUK");
            let vuk0 = hits(&sim, "vuk_sw");
            let deck0 = hits(&sim, "deck_opto");
            let what = format!("approach {k} ({from_deg} deg, {speed} m/s)");
            let captured = run_watch(&mut sim, 3_000, |s| hits(s, "vuk_sw") > vuk0);
            assert!(captured, "{what}: vuk_sw counted the capture");
            let cap_tick = sim.tick();
            let target = DECK_Z_M + r;
            let mut lifted = false;
            let mut snap_ok = false;
            let mut run = 0u32;
            let mut settled = false;
            let mut deck_tick = None;
            let mut zs = Vec::new();
            for _ in 0..8_000 {
                sim.advance(1);
                for o in sim.ball_handles() {
                    let b = sim.ball(o).expect("live");
                    assert!(finite(&b), "{what}: non-finite ball {b:?}");
                    assert_in_box(&sim, &b, &what);
                }
                if deck_tick.is_none() && hits(&sim, "deck_opto") > deck0 {
                    deck_tick = Some(sim.tick());
                }
                let Some(b) = sim.ball(h) else { break };
                if !lifted && b.layer == 1 {
                    lifted = true;
                    let snap = sim.snapshot();
                    snap_ok = snap.balls[..usize::from(snap.ball_count)]
                        .iter()
                        .any(|s| s.layer == 1 && s.position_m == b.position_m);
                }
                if lifted && !settled {
                    zs.push(b.position_m[2]);
                    if b.layer == 1 && (b.position_m[2] - target).abs() <= 0.003 {
                        run += 1;
                        settled = run >= 50;
                    } else {
                        run = 0;
                    }
                }
                if settled && deck_tick.is_some() {
                    break;
                }
            }
            assert!(lifted, "{what}: the VUK put the ball on layer 1");
            assert!(snap_ok, "{what}: the snapshot shows the ball on layer 1");
            assert!(
                settled,
                "{what}: the ball rolled at deck height {target} m for 50 ticks (z after the \
                 lift: {:?}...)",
                &zs[..zs.len().min(40)]
            );
            let deck_tick = deck_tick.unwrap_or_else(|| panic!("{what}: deck_opto never counted"));
            assert!(
                deck_tick - cap_tick <= 5_000,
                "{what}: deck_opto counted {} ticks after the capture (> 5000)",
                deck_tick - cap_tick
            );
            assert_healthy(&sim, &what);
        }
    });
}

/// C3d: ball `b` is inside the simulation's escape box (the same per-tick
/// check `run` makes).
fn assert_in_box(sim: &Simulation, b: &BallState, what: &str) {
    let (lo, hi) = sim.escape_bounds();
    for k in 0..3 {
        assert!(
            (lo[k]..=hi[k]).contains(&b.position_m[k]),
            "C3d: {what}: ball left the table box: {b:?}"
        );
    }
}

/// Steps until `cond` (checked after each tick), asserting every ball stays
/// finite and inside the table box (C3d); `false` after `max` ticks.
fn run_watch(sim: &mut Simulation, max: u64, mut cond: impl FnMut(&Simulation) -> bool) -> bool {
    for _ in 0..max {
        sim.advance(1);
        for h in sim.ball_handles() {
            let b = sim.ball(h).expect("live");
            assert!(finite(&b), "non-finite ball {b:?}");
            assert_in_box(sim, &b, "run_watch");
        }
        if cond(sim) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// (b) the upper-right flipper
// ---------------------------------------------------------------------------

/// The layer-1 upper-right flipper (52 mm) strikes a deck ball resting
/// against its face, for 6 positions along the bat (0.3 to 0.8 of its
/// length; brief: >= 5).
///
/// Band (m/s): above 1 (a resting ball struck by a powered bat must leave
/// clearly faster than a roll-off, which on a 6.5 deg deck reaches about
/// 0.3 m/s over the bat length), and at most the Appendix C full-shot
/// window maximum of the 76 mm lower flippers (`flipper_exit_speed_m_s.max`
/// = 7 m/s) scaled by the length ratio 52/76: the ball leaves at most at the
/// bat's surface speed `omega * d`; for the same stroke the contact point of
/// the shorter bat is at most 52/76 as far from the pivot, so 7 * 52 / 76 =
/// 4.79 m/s.
///
/// Direction: the ball's velocity at its fastest has a positive component
/// along the sweep normal of the bat at rest (the in-plane normal pointing
/// the way the face moves).
#[test]
fn c3b_upper_right_flipper_shoots_deck_balls() {
    big_stack(|| {
        let t = table();
        let f = t
            .manifest
            .flippers
            .iter()
            .find(|f| f.id == "upper_right")
            .expect("upper_right")
            .clone();
        let (length, base_r, tip_r) = match f.shape {
            FlipperShapeDef::StraightCapsule {
                length,
                base_radius,
                tip_radius,
            } => (length * 1e-3, base_radius * 1e-3, tip_radius * 1e-3),
            s => panic!("StraightCapsule expected: {s:?}"),
        };
        let window = PhysicsDefaults::builtin()
            .feel_windows
            .flipper_exit_speed_m_s;
        let max_speed = window.max * 52.0 / 76.0;
        let pivot = [
            f.base_position[0] * 1e-3,
            f.base_position[1] * 1e-3,
            f.base_position[2] * 1e-3,
        ];
        let u = dir(f.rest_angle_deg);
        let s = f.sweep_angle_deg.signum();
        // Sweep normal: u rotated by +-90 deg in the plane.
        let n = [-s * u[1], s * u[0], 0.0];
        let r = steel_radius_m();
        let mut sim = game_seeded(7);
        let mut results = Vec::new();
        for frac in [0.3f32, 0.4, 0.5, 0.6, 0.7, 0.8] {
            // Flipper at rest.
            let rest = common::run_until(&mut sim, 1_000, |s| {
                s.flipper("upper_right")
                    .is_some_and(|(a, w)| a.abs() < 1e-3 && w.abs() < 1e-2)
            });
            assert!(rest, "upper_right returns to rest");
            let bat_r = base_r + (tip_r - base_r) * frac;
            let c = add(
                add(pivot, scale(u, frac * length)),
                scale(n, bat_r + r + 0.0005),
            );
            let c = [c[0], c[1], pivot[2] + r];
            let h = sim
                .spawn_ball_on_layer(STEEL, 1, c, [0.0; 3])
                .expect("spawn on the deck");
            sim.advance(1);
            cabinet(&mut sim, CabinetInput::RightUpper, true);
            let mut best = (0.0f32, [0.0f32; 3]);
            for _ in 0..80 {
                sim.advance(1);
                let Some(b) = sim.ball(h) else { break };
                assert!(finite(&b), "finite");
                assert_in_box(&sim, &b, &format!("strike at {frac}"));
                let v = b.velocity_m_s;
                let sp = v[0].hypot(v[1]);
                if sp > best.0 {
                    best = (sp, v);
                }
            }
            cabinet(&mut sim, CabinetInput::RightUpper, false);
            let along = dot(best.1, n) / best.0.max(1e-6);
            println!(
                "upper_right strike at {frac:.1} of the bat: {:.3} m/s, direction cos to the \
                 sweep normal {along:.3}",
                best.0
            );
            results.push((frac, best.0, along));
            if sim.ball(h).is_some() {
                sim.remove_ball(h);
            }
            sim.advance(300);
            assert_healthy(&sim, &format!("strike at {frac}"));
        }
        for (frac, sp, along) in results {
            assert!(
                sp > 1.0 && sp <= max_speed,
                "strike at {frac} of the bat: {sp} m/s outside (1, {max_speed}]"
            );
            assert!(
                along > 0.0,
                "strike at {frac}: direction along the sweep ({along})"
            );
        }
    });
}

// ---------------------------------------------------------------------------
// (c) balls on different layers never collide
// ---------------------------------------------------------------------------

/// One ball of a run.
#[derive(Clone, Copy, Debug)]
struct Spawn {
    profile: &'static str,
    layer: u8,
    pos: [f32; 3],
    vel: [f32; 3],
}

/// Trajectory of each spawned ball: its state after every tick (`None`
/// once gone).
type Traj = Vec<Vec<Option<BallState>>>;

/// Loads a fresh simulation (attract mode, default seed), spawns `balls`
/// and records `ticks` ticks. Asserts C3d on the way (finite, inside the
/// table box, nothing escaped).
fn run(balls: &[Spawn], ticks: u64) -> Traj {
    let mut sim = load();
    sim.advance(10);
    let hs: Vec<BallHandle> = balls
        .iter()
        .map(|b| {
            sim.spawn_ball_on_layer(b.profile, b.layer, b.pos, b.vel)
                .expect("spawn")
        })
        .collect();
    let (lo, hi) = sim.escape_bounds();
    let mut out: Traj = vec![Vec::new(); balls.len()];
    for _ in 0..ticks {
        sim.advance(1);
        for (i, &h) in hs.iter().enumerate() {
            let b = sim.ball(h);
            if let Some(b) = &b {
                assert!(finite(b), "C3d: non-finite ball {b:?}");
                for k in 0..3 {
                    assert!(
                        (lo[k]..=hi[k]).contains(&b.position_m[k]),
                        "C3d: ball left the table box: {b:?}"
                    );
                }
            }
            out[i].push(b);
        }
    }
    assert_eq!(sim.counters().balls_escaped, 0, "C3d: no ball escaped");
    for (i, t) in out.iter().enumerate() {
        assert!(t.iter().all(Option::is_some), "C3d: ball {i} was lost");
    }
    out
}

/// Largest position (m) and velocity (m/s) difference of two trajectories.
fn max_diff(a: &[Option<BallState>], b: &[Option<BallState>]) -> (f32, f32) {
    let mut dp = 0.0f32;
    let mut dv = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x.expect("present"), y.expect("present"));
        for k in 0..3 {
            dp = dp.max((x.position_m[k] - y.position_m[k]).abs());
            dv = dv.max((x.velocity_m_s[k] - y.velocity_m_s[k]).abs());
        }
    }
    (dp, dv)
}

/// Smallest plan-view centre distance of two trajectories, m.
fn min_plan(a: &[Option<BallState>], b: &[Option<BallState>]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            plan_dist(
                x.expect("present").position_m,
                y.expect("present").position_m,
            )
        })
        .fold(f32::INFINITY, f32::min)
}

/// One crossing: ball A (steel) and ball B (Glo-Ball) with velocities
/// `va`, `vb` reach plan point `p` at the same time `t_meet`; `offset` is
/// added to B's path (m, perpendicular to A's direction).
#[derive(Clone, Copy, Debug)]
struct Crossing {
    name: &'static str,
    va: [f32; 3],
    vb: [f32; 3],
    offset: f32,
}

/// The sweep (brief: >= 6 offsets/speeds including head-on).
const CROSSINGS: [Crossing; 7] = [
    Crossing {
        name: "head-on 1.0 m/s",
        va: [1.0, 0.0, 0.0],
        vb: [-1.0, 0.0, 0.0],
        offset: 0.0,
    },
    Crossing {
        name: "head-on 2.0 m/s",
        va: [2.0, 0.0, 0.0],
        vb: [-2.0, 0.0, 0.0],
        offset: 0.0,
    },
    Crossing {
        name: "head-on 3.0 m/s, 4 mm offset",
        va: [3.0, 0.0, 0.0],
        vb: [-3.0, 0.0, 0.0],
        offset: 0.004,
    },
    Crossing {
        name: "head-on 1.5 m/s, 8 mm offset",
        va: [1.5, 0.0, 0.0],
        vb: [-1.5, 0.0, 0.0],
        offset: 0.008,
    },
    Crossing {
        name: "head-on 1.5 m/s, 16 mm offset",
        va: [1.5, 0.0, 0.0],
        vb: [-1.5, 0.0, 0.0],
        offset: 0.016,
    },
    Crossing {
        name: "90 deg crossing 1.5 m/s",
        va: [1.5, 0.0, 0.0],
        vb: [0.0, 1.5, 0.0],
        offset: 0.0,
    },
    Crossing {
        name: "oblique 2.0 / 1.0 m/s",
        va: [2.0, 0.0, 0.0],
        vb: [
            -std::f32::consts::FRAC_1_SQRT_2,
            -std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        ],
        offset: 0.0,
    },
];

/// Time to the meeting point, s (ball starts at least 25 mm from it; the
/// pair starts at least 1.5 diameters apart).
const T_MEET: f32 = 0.03;
/// Ticks recorded per run: the meeting plus 30 ms of aftermath.
const RUN_TICKS: u64 = 60;
/// Float tolerance of "identical" trajectories: position (m) and velocity
/// (m/s). A ball-ball contact at these speeds changes a velocity by
/// 0.3-3 m/s, four orders of magnitude above the velocity tolerance.
const TOL_P: f32 = 1.0e-5;
const TOL_V: f32 = 1.0e-4;

fn start(p: [f32; 3], v: [f32; 3]) -> [f32; 3] {
    add(p, scale(v, -T_MEET))
}

/// Smallest 3-D centre distance of two trajectories, m.
fn min_3d(a: &[Option<BallState>], b: &[Option<BallState>]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let (x, y) = (
                x.expect("present").position_m,
                y.expect("present").position_m,
            );
            (0..3).map(|k| (x[k] - y[k]).powi(2)).sum::<f32>().sqrt()
        })
        .fold(f32::INFINITY, f32::min)
}

/// Runs crossing `c` at plan point `p` with A on `layer_a` at height `za`
/// and B on `layer_b` at `zb` (both on different layers), plus the
/// same-layer control (`control_layer`, both at `zc`), and asserts the
/// never-collide property and the control's momentum exchange. With
/// `bodies_overlap`, it first asserts that the two balls' solo paths bring
/// the bodies into 3-D overlap (centre distance below the sum of the radii),
/// so the pair run really tests layer separation and not a near miss.
#[allow(clippy::too_many_arguments)]
fn crossing(
    c: &Crossing,
    p: [f32; 2],
    layer_a: u8,
    za: f32,
    layer_b: u8,
    zb: f32,
    control_layer: u8,
    zc: f32,
    bodies_overlap: bool,
) {
    assert_ne!(layer_a, layer_b);
    let n = [-c.va[1], c.va[0], 0.0];
    let nl = len(n).max(1e-6);
    let off = scale(n, c.offset / nl);
    let pa = [p[0], p[1], za];
    let pb = add([p[0], p[1], zb], off);
    let a = Spawn {
        profile: STEEL,
        layer: layer_a,
        pos: start(pa, c.va),
        vel: c.va,
    };
    let b = Spawn {
        profile: GLO,
        layer: layer_b,
        pos: start(pb, c.vb),
        vel: c.vb,
    };
    let pair = run(&[a, b], RUN_TICKS);
    let solo_a = run(&[a], RUN_TICKS);
    let solo_b = run(&[b], RUN_TICKS);
    let closest = min_plan(&pair[0], &pair[1]);
    let dia = steel_radius_m() + glo_radius_m();
    if bodies_overlap {
        let d3 = min_3d(&solo_a[0], &solo_b[0]);
        assert!(
            d3 < dia,
            "{}: the solo paths overlap in 3-D ({:.2} mm >= {:.2} mm)",
            c.name,
            d3 * 1e3,
            dia * 1e3
        );
    }
    let (dpa, dva) = max_diff(&pair[0], &solo_a[0]);
    let (dpb, dvb) = max_diff(&pair[1], &solo_b[0]);
    // Same-layer control on the same plan paths.
    let ca = Spawn {
        layer: control_layer,
        pos: [a.pos[0], a.pos[1], zc],
        ..a
    };
    let cb = Spawn {
        layer: control_layer,
        pos: [b.pos[0], b.pos[1], zc],
        ..b
    };
    let control = run(&[ca, cb], RUN_TICKS);
    let control_a = run(&[ca], RUN_TICKS);
    let (_, dvc) = max_diff(&control[0], &control_a[0]);
    println!(
        "{}: layers {layer_a}/{layer_b} closest plan distance {:.2} mm; trajectory deltas \
         A {dpa:e} m {dva:e} m/s, B {dpb:e} m {dvb:e} m/s; same-layer control dv {dvc:.3} m/s",
        c.name,
        closest * 1e3
    );
    assert!(
        closest < dia,
        "{}: the plan-view paths cross ({:.2} mm >= {:.2} mm)",
        c.name,
        closest * 1e3,
        dia * 1e3
    );
    assert!(
        dpa <= TOL_P && dva <= TOL_V && dpb <= TOL_P && dvb <= TOL_V,
        "{}: a ball on layer {layer_a} and one on layer {layer_b} interacted (A {dpa} m \
         {dva} m/s, B {dpb} m {dvb} m/s)",
        c.name
    );
    assert!(
        dvc > 0.2,
        "{}: two balls on layer {control_layer} on the same paths exchanged momentum \
         (dv {dvc} m/s)",
        c.name
    );
}

/// The table's plan extents (m), from the escape box minus its margin.
fn plan_extents() -> ([f32; 2], [f32; 2]) {
    let sim = load();
    let (lo, hi) = sim.escape_bounds();
    let m = pin_sim::load::BOUNDS_MARGIN_M;
    ([lo[0] + m, lo[1] + m], [hi[0] - m, hi[1] - m])
}

/// How far (m/s) a lone ball's horizontal velocity strays from its start
/// over a run: 0 for a free path.
fn stray(t: &[Option<BallState>], v0: [f32; 3]) -> f32 {
    t.iter()
        .map(|b| {
            let v = b.expect("present").velocity_m_s;
            (v[0] - v0[0]).hypot(v[1] - v0[1])
        })
        .fold(0.0, f32::max)
}

/// Picks, from `candidates`, the plan point where a lone steel ball on
/// `layer_a` at `za` and a lone Glo-Ball on `layer_b` at `zb`, on the
/// head-on 1.5 m/s paths, stray least (a free crossing point).
fn pick_point(candidates: &[[f32; 2]], layer_a: u8, za: f32, layer_b: u8, zb: f32) -> [f32; 2] {
    let mut best = (f32::INFINITY, candidates[0]);
    for &p in candidates {
        let va = [1.5, 0.0, 0.0];
        let vb = [-1.5, 0.0, 0.0];
        let a = run(
            &[Spawn {
                profile: STEEL,
                layer: layer_a,
                pos: start([p[0], p[1], za], va),
                vel: va,
            }],
            RUN_TICKS,
        );
        let b = run(
            &[Spawn {
                profile: GLO,
                layer: layer_b,
                pos: start([p[0], p[1], zb], vb),
                vel: vb,
            }],
            RUN_TICKS,
        );
        let s = stray(&a[0], va).max(stray(&b[0], vb));
        if s < best.0 {
            best = (s, p);
        }
        if s < 0.1 {
            break;
        }
    }
    println!(
        "crossing point {:?} (lone-ball stray {:.3} m/s)",
        best.1, best.0
    );
    best.1
}

/// C3c on the real multi-layer geometry: a layer-1 Glo-Ball rolling on the
/// acrylic deck and a layer-0 steel ball rolling on the playfield beneath
/// it cross the same plan-view point (7 crossings). Control: both on layer
/// 1 on the deck.
#[test]
fn c3c_deck_and_floor_balls_never_collide() {
    big_stack(|| {
        let deck = layout()
            .decks
            .iter()
            .find(|d| d.layer == 1)
            .expect("layer-1 deck")
            .clone();
        let (min, max) = deck.outline.iter().fold(
            ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
            |(lo, hi), p| {
                (
                    [lo[0].min(p[0]), lo[1].min(p[1])],
                    [hi[0].max(p[0]), hi[1].max(p[1])],
                )
            },
        );
        // Grid points inside the deck, at least 60 mm from its edge (mm),
        // in fixed order, nearest the deck's centre first.
        let centre = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
        let mut cands: Vec<[f32; 2]> = Vec::new();
        let steps = 12;
        for i in 0..=steps {
            for j in 0..=steps {
                let q = [
                    min[0] + (max[0] - min[0]) * i as f32 / steps as f32,
                    min[1] + (max[1] - min[1]) * j as f32 / steps as f32,
                ];
                if inside(&deck.outline, q) && edge_distance(&deck.outline, q) >= 60.0 {
                    cands.push(q);
                }
            }
        }
        assert!(!cands.is_empty(), "the deck has room for a crossing");
        cands.sort_by(|a, b| {
            let da = (a[0] - centre[0]).hypot(a[1] - centre[1]);
            let db = (b[0] - centre[0]).hypot(b[1] - centre[1]);
            da.total_cmp(&db)
        });
        cands.truncate(8);
        let cands: Vec<[f32; 2]> = cands.iter().map(|q| [q[0] * 1e-3, q[1] * 1e-3]).collect();
        let z_floor = steel_radius_m();
        let z_deck = deck.z * 1e-3 + glo_radius_m();
        let p = pick_point(&cands, 0, z_floor, 1, z_deck);
        for c in &CROSSINGS {
            crossing(c, p, 0, z_floor, 1, z_deck, 1, z_deck, false);
        }
    });
}

/// C3c with the two balls at the same height (the bodies overlap in 3D,
/// not only in plan view): a layer-0 steel ball skimming the playfield and a
/// layer-1 Glo-Ball at the same height (no layer-1 surface below it, so it
/// is in free fall, about 4 mm in the 30 ms to the meeting) cross the same
/// point (7 crossings). Control: both on layer 0.
#[test]
fn c3c_overlapping_bodies_on_different_layers_never_collide() {
    big_stack(|| {
        let (lo, hi) = plan_extents();
        let deck = layout()
            .decks
            .iter()
            .find(|d| d.layer == 1)
            .expect("layer-1 deck")
            .clone();
        // Points on the open lower playfield, outside the deck (no layer-1
        // floor under the Glo-Ball), fixed order.
        let mut cands = Vec::new();
        for fy in [0.35f32, 0.45, 0.3, 0.5, 0.55, 0.4] {
            for fx in [0.5f32, 0.4, 0.6, 0.3, 0.7] {
                let q = [lo[0] + (hi[0] - lo[0]) * fx, lo[1] + (hi[1] - lo[1]) * fy];
                let qmm = [q[0] * 1e3, q[1] * 1e3];
                if !inside(&deck.outline, qmm) && edge_distance(&deck.outline, qmm) > 60.0 {
                    cands.push(q);
                }
            }
        }
        assert!(!cands.is_empty(), "open playfield points");
        let z = steel_radius_m() + 0.001;
        let p = pick_point(&cands, 0, z, 1, z);
        for c in &CROSSINGS {
            crossing(c, p, 0, z, 1, z, 0, z, true);
        }
    });
}

/// The profile ids the tests match balls by are those of the built-in
/// profile list (`ProfileId` = index in `BUILTIN_PROFILE_NAMES`).
#[test]
fn c3c_profile_ids() {
    assert_eq!(
        pin_core::profiles::BUILTIN_PROFILE_NAMES[usize::from(GLO_ID)],
        GLO
    );
    assert_eq!(
        pin_core::profiles::BUILTIN_PROFILE_NAMES[usize::from(common::STEEL_ID)],
        STEEL
    );
}
