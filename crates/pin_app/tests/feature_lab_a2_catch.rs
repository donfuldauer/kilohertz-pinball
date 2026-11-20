//! 6.4-a2fix Fix 1 (Gate A2 run 1, class A EscapedBall): a layer-4 ball
//! thrown left in the backbox's entry channel (off the top of backbox
//! walls[1]) falls through the entry hole (x 67..137) and crosses z 25 mm
//! left of `bb_catch` (x min 52). Before `bb_catch_left` it stayed on
//! layer 4 and left the table bounds.
//!
//! The sweep drops standard balls on layer 4 from the backbox shelf height
//! across the hole, x 70..137, with vx -1.5..+0.3 m/s and vy -0.6..+0.6 m/s
//! (a grid), plus the reported state just before the bounce, position
//! (128, 927, 165) mm, velocity (-0.324, -0.217, -1.90) m/s. Every ball
//! must come back to layer 0 and either drain through a trough or stay on
//! the playfield, never leave the bounds, and never be in the closed
//! pocket between the cabinet wall and the orbit's outer rail.
//!
//! PR #142 (Gate A2, SPEC §4 Phase 6: no stuck and no escaped balls): no
//! 4 -> 0 zone's footprint contains a point of that pocket (`bb_catch`
//! used to, x 52..64.75 at y 955.75..990), and the two verified
//! reproductions come back to the playfield outside the pocket without
//! leaving the table: a layer-4 ball nudged towards the back with a
//! gamepad's right stick, and a falling one after a keyboard forward nudge.
//! The pocket predicate (`in_pocket`) is in `feature_lab_support`.

mod feature_lab_support;

use feature_lab_support::*;
use pin_app::platform::InputRouter;
use pin_core::input::{CabinetAxis, InputEvent, InputEventKind};
use pin_core::schema::table::TableManifest;
use pin_core::sim::{SimStep, TICK_NS};
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::{SimConfig, Simulation};
use std::path::Path;

/// What happened to one dropped ball.
#[derive(Debug, Default)]
struct Drop {
    on_0: bool,
    drained: bool,
    alive_on_0: bool,
    left_bounds: bool,
    in_pocket: bool,
    escaped: u32,
}

impl Drop {
    fn ok(&self) -> bool {
        self.on_0
            && (self.drained || self.alive_on_0)
            && !self.left_bounds
            && !self.in_pocket
            && self.escaped == 0
    }
}

/// A standard ball on layer 4 at `p` (mm) with velocity `v` (m/s), run for
/// up to 20 s of simulated time.
fn drop_layer_4(dir: &Path, p: [f32; 3], v: [f32; 3]) -> Drop {
    let mut s = Simulation::load(dir, SimConfig::default()).expect("load");
    let h = s
        .spawn_ball_on_layer(
            "standard_steel",
            4,
            [p[0] * 1.0e-3, p[1] * 1.0e-3, p[2] * 1.0e-3],
            v,
        )
        .expect("spawn on layer 4");
    let mut d = Drop::default();
    for _ in 0..20_000 {
        s.advance(1);
        let Some(b) = s.ball(h) else { break };
        let q = b.position_m;
        d.left_bounds |= outside(s.escape_bounds(), q);
        if b.layer == 0 {
            d.on_0 = true;
            d.in_pocket |= in_pocket([q[0] * 1.0e3, q[1] * 1.0e3]);
        }
    }
    d.alive_on_0 = s.ball(h).is_some_and(|b| b.layer == 0);
    let troughs = s.switch_hits("trough_1").unwrap() + s.switch_hits("trough_2").unwrap();
    d.drained = troughs >= 1 && s.ball(h).is_none();
    d.escaped = s.counters().balls_escaped;
    d
}

/// The reported Gate A2 state (standard 424) just before the bounce off
/// backbox walls[1], then the grid over the hole.
#[test]
fn layer_4_drops_through_the_entry_hole_all_return_to_the_playfield() {
    on_big_stack(|| {
        let mut cases = vec![([128.0, 927.0, 165.0], [-0.324, -0.217, -1.90])];
        for x in [70.0f32, 85.0, 100.0, 115.0, 128.0, 137.0] {
            for vx in [-1.5f32, -1.04, -0.6, -0.3, 0.0, 0.3] {
                for vy in [-0.6f32, -0.2, 0.2, 0.6] {
                    cases.push(([x, 917.5, 150.0], [vx, vy, -1.9]));
                }
            }
        }
        let mut bad = Vec::new();
        for (p, v) in &cases {
            let d = drop_layer_4(bundle(), *p, *v);
            if !d.ok() {
                bad.push(format!("p {p:?} v {v:?}: {d:?}"));
            }
        }
        assert!(
            bad.is_empty(),
            "{} of {} drops failed:\n{}",
            bad.len(),
            cases.len(),
            bad.join("\n")
        );
    });
}

/// The zone's data: a 4 -> 0 transition left of the entry hole that keeps
/// out of the generated entry zone (x >= 67 at z 46..90) and the drain
/// zone, and returns a ball only in the orbit lane's clear band
/// (y 895..935), never into the closed pocket.
#[test]
fn bb_catch_left_is_left_of_the_hole_in_the_orbit_lane() {
    let text = std::fs::read_to_string(bundle().join("table.toml")).unwrap();
    let t = TableManifest::from_toml_str(&text).unwrap();
    let z = t
        .layer_transitions
        .iter()
        .find(|z| z.id == "bb_catch_left")
        .expect("bb_catch_left");
    assert_eq!((z.from_layer, z.to_layer), (4, 0));
    let (min, max) = (z.zone.min, z.zone.max);
    assert!(min[1] >= 895.0 && max[1] <= 935.0, "{min:?} {max:?}");
    assert!(max[0] < 67.0, "left of the entry hole: {max:?}");
    assert!(min[0] >= 0.0, "inside the cabinet: {min:?}");
    for id in ["tablegen:backbox.entry", "tablegen:backbox.drain"] {
        let o = t
            .layer_transitions
            .iter()
            .find(|o| o.id == id)
            .unwrap_or_else(|| panic!("{id}"));
        let overlap = (0..3).all(|k| min[k] < o.zone.max[k] && o.zone.min[k] < max[k]);
        assert!(!overlap, "{id} overlaps bb_catch_left");
    }
    // The zone hands a ball back over the orbit lane: the middle of its
    // footprint is inside the lane, and every corner and a few points along
    // each edge are within the lane's half width of its centre line (no ball
    // radius added) and outside the closed pocket.
    let mid = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
    let (d, _) = orbit_offset(mid);
    assert!(
        d < ORBIT_HALF_WIDTH,
        "middle {mid:?} is {d} mm off the lane"
    );
    let mut pts = Vec::new();
    for i in 0..=4 {
        let f = i as f32 / 4.0;
        let x = min[0] + (max[0] - min[0]) * f;
        let y = min[1] + (max[1] - min[1]) * f;
        pts.extend([[x, min[1]], [x, max[1]], [min[0], y], [max[0], y]]);
    }
    for p in pts {
        let (d, _) = orbit_offset(p);
        assert!(!in_pocket(p), "{p:?} is in the closed pocket ({d} mm)");
        assert!(
            d <= ORBIT_HALF_WIDTH,
            "{p:?} is {d} mm off the lane centre line"
        );
    }
}

// --- PR #142: no 4 -> 0 zone returns a ball into the closed pocket -------------

/// The points `min, min + 0.5, ..., max` (mm; `max` included).
fn half_mm_grid(min: f32, max: f32) -> impl Iterator<Item = f32> {
    let steps = ((max - min) / 0.5).ceil() as u32;
    (0..=steps).map(move |i| (min + 0.5 * i as f32).min(max))
}

/// Gate A2 (SPEC §4 Phase 6, zero stuck balls): a 4 -> 0 transition puts
/// the ball on layer 0 where its centre entered the zone, so no point of a
/// 4 -> 0 zone's x/y footprint may be in the closed pocket between the
/// cabinet wall and the orbit's outer rail (a ball returned there stays
/// stuck). Walks every footprint on a 0.5 mm grid, edges included.
#[test]
fn no_layer_4_to_0_zone_footprint_contains_a_pocket_point() {
    let text = std::fs::read_to_string(bundle().join("table.toml")).unwrap();
    let t = TableManifest::from_toml_str(&text).unwrap();
    let mut ids = Vec::new();
    let mut hits = Vec::new();
    for z in t
        .layer_transitions
        .iter()
        .filter(|z| z.from_layer == 4 && z.to_layer == 0)
    {
        ids.push(z.id.as_str());
        let (min, max) = (z.zone.min, z.zone.max);
        for x in half_mm_grid(min[0], max[0]) {
            for y in half_mm_grid(min[1], max[1]) {
                if in_pocket([x, y]) {
                    hits.push(format!("{} ({x}, {y})", z.id));
                }
            }
        }
    }
    assert!(
        ids.contains(&"bb_catch") && ids.contains(&"bb_catch_left"),
        "4 -> 0 zones: {ids:?}"
    );
    assert!(
        hits.is_empty(),
        "{} footprint points in the closed pocket, first {:?}",
        hits.len(),
        &hits[..hits.len().min(5)]
    );
}

/// Where one layer-4 ball came back to layer 0 and how it ended.
#[derive(Debug, Default)]
struct Run {
    /// Centre (mm) on the first tick the ball was on layer 0.
    returned_at: Option<[f32; 3]>,
    /// The ball centre was outside the escape bounds on some tick.
    left_bounds: bool,
    escaped: u32,
    /// A main trough switch closed and the ball left the sim.
    drained: bool,
    /// Still in play on layer 0 at the end.
    alive_on_0: bool,
    /// Last centre (mm).
    final_at: [f32; 3],
}

impl Run {
    /// Back on layer 0 outside the closed pocket, never out of the table,
    /// and either drained or alive on the playfield at a rest point
    /// outside the pocket (the return and final points only: the pocket
    /// predicate is not valid on every tick of a ball that runs the orbit).
    fn ok(&self) -> bool {
        self.returned_at.is_some_and(|p| !in_pocket([p[0], p[1]]))
            && !self.left_bounds
            && self.escaped == 0
            && (self.drained
                || (self.alive_on_0 && !in_pocket([self.final_at[0], self.final_at[1]])))
    }
}

/// A standard ball on layer 4 at `p` (mm) with velocity `v` (m/s), run for
/// 20 s of simulated time through `step_1ms`. Before tick `k` (1-based from
/// the spawn) `inputs(k, stamp_ns, out)` may push input events stamped
/// within that tick (`stamp_ns` is 1 ns before the tick's end).
fn run_layer_4(
    dir: &Path,
    p: [f32; 3],
    v: [f32; 3],
    mut inputs: impl FnMut(u64, u64, &mut Vec<InputEvent>),
) -> Run {
    let mut s = Simulation::load(dir, SimConfig::default()).expect("load");
    let h = s
        .spawn_ball_on_layer(
            "standard_steel",
            4,
            [p[0] * 1.0e-3, p[1] * 1.0e-3, p[2] * 1.0e-3],
            v,
        )
        .expect("spawn on layer 4");
    let mut snap = Box::new(FrameStateSnapshot::default());
    let mut ev = Vec::new();
    let start = s.tick();
    let mut r = Run::default();
    for k in 1..=20_000u64 {
        let tick = start + k;
        ev.clear();
        inputs(k, tick * TICK_NS - 1, &mut ev);
        s.step_1ms(tick, &ev, &mut snap);
        let Some(b) = s.ball(h) else { break };
        let q = b.position_m;
        r.final_at = [q[0] * 1.0e3, q[1] * 1.0e3, q[2] * 1.0e3];
        r.left_bounds |= outside(s.escape_bounds(), q);
        if b.layer == 0 && r.returned_at.is_none() {
            r.returned_at = Some(r.final_at);
        }
    }
    r.alive_on_0 = s.ball(h).is_some_and(|b| b.layer == 0);
    let troughs = s.switch_hits("trough_1").unwrap() + s.switch_hits("trough_2").unwrap();
    r.drained = troughs >= 1 && s.ball(h).is_none();
    r.escaped = s.counters().balls_escaped;
    r
}

/// The verified stuck ball (PR #142): a layer-4 ball at rest in the entry
/// channel, 30 mm above the shelf, while a gamepad's right stick nudges
/// (NudgeX +0.15, NudgeY -0.75 from tick 41 through tick 140: the ball
/// gains up to 2.25 m/s towards the back). `bb_catch` used to return it at
/// (55.1, 983.7, 25.0) mm, in the pocket, where it stayed for good.
#[test]
fn layer_4_ball_nudged_toward_the_back_is_not_returned_into_the_pocket() {
    on_big_stack(|| {
        let r = run_layer_4(bundle(), [87.0, 920.8, 137.8], [0.0; 3], |k, ns, out| {
            let (x, y) = match k {
                41 => (0.15, -0.75),
                141 => (0.0, 0.0),
                _ => return,
            };
            out.push(InputEvent::axis(ns, CabinetAxis::NudgeX, x));
            out.push(InputEvent::axis(ns, CabinetAxis::NudgeY, y));
        });
        assert!(r.ok(), "{r:?}");
    });
}

/// The verified escape (PR #142): the Gate A2 run-1 state just before the
/// bounce off backbox walls[1], position (128, 927, 165) mm, velocity
/// (-0.324, -0.217, -1.90) m/s, and one keyboard forward nudge 20 ms
/// later, through the app's input router (the X-Arcade keyboard profile's
/// Up arrow, scancode 82: NudgeY 0.6 for 60 ms). No zone used to fire and
/// the ball left the table at (16.2, 832.9, -49.3) mm.
#[test]
fn layer_4_ball_after_a_keyboard_forward_nudge_does_not_escape() {
    on_big_stack(|| {
        let profile = pin_input::profiles::builtin_profile("xarcade_keyboard")
            .expect("xarcade_keyboard profile");
        let mut router = InputRouter::with_keyboard_profile(profile);
        let mut nudged = false;
        let r = run_layer_4(
            bundle(),
            [128.0, 927.0, 165.0],
            [-0.324, -0.217, -1.90],
            |k, ns, out| {
                let mut push = |e: InputEvent| {
                    nudged |= matches!(
                        e.kind,
                        InputEventKind::Axis {
                            axis: CabinetAxis::NudgeY,
                            value
                        } if value > 0.0
                    );
                    out.push(e);
                };
                match k {
                    21 => router.key(ns, 82, true, &mut push),
                    121 => router.key(ns, 82, false, &mut push),
                    _ => {}
                }
                router.poll(ns, &mut push);
            },
        );
        assert!(nudged, "the router sent the forward nudge");
        assert!(r.ok(), "{r:?}");
    });
}
