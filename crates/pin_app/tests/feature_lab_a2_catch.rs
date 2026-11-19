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

mod feature_lab_support;

use feature_lab_support::*;
use pin_core::schema::table::TableManifest;
use pin_sim::{SimConfig, Simulation};
use std::path::Path;

/// The orbit lane's centre line and width (layout.toml `[[lanes]]`), mm.
const ORBIT: [[f32; 2]; 8] = [
    [85.0, 615.0],
    [55.0, 650.0],
    [38.0, 700.0],
    [36.0, 780.0],
    [45.0, 880.0],
    [90.0, 1000.0],
    [170.0, 1040.0],
    [260.0, 1046.0],
];
const ORBIT_HALF_WIDTH: f32 = 20.0;

/// From this height up the orbit's outer rail is within 15.7 mm of the
/// cabinet wall: the strip between them is a closed pocket, mm.
const POCKET_Y_MIN: f32 = 744.0;

/// Whether a layer-0 ball centre `p` (mm) is in the closed pocket: up the
/// table from `POCKET_Y_MIN`, outside the orbit lane on its cabinet side
/// (left of the centre line, beyond the lane's half width).
fn in_pocket(p: [f32; 2]) -> bool {
    let (d, side) = orbit_offset(p);
    p[1] >= POCKET_Y_MIN && d > ORBIT_HALF_WIDTH && side > 0.0
}

/// Distance (mm) of `p` from the orbit's centre line, and its side
/// (positive on the left of the lane's travel direction: the cabinet side
/// of its up-going left leg).
fn orbit_offset(p: [f32; 2]) -> (f32, f32) {
    let (mut best_d, mut best_side) = (f32::INFINITY, 0.0f32);
    for w in ORBIT.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = [b[0] - a[0], b[1] - a[1]];
        let ap = [p[0] - a[0], p[1] - a[1]];
        let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1])).clamp(0.0, 1.0);
        let d = (ap[0] - ab[0] * t).hypot(ap[1] - ab[1] * t);
        if d < best_d {
            best_d = d;
            best_side = ab[0] * ap[1] - ab[1] * ap[0];
        }
    }
    (best_d, best_side)
}

/// Whether `p` (m) is outside the simulation's escape bounds (the box the
/// EscapedBall check uses).
fn outside(bounds: ([f32; 3], [f32; 3]), p: [f32; 3]) -> bool {
    (0..3).any(|k| !(p[k] >= bounds.0[k] && p[k] <= bounds.1[k]))
}

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
    // footprint is inside the lane, and no corner is beyond the outer rail
    // (a ball centre there would at most touch the rail from the lane
    // side). The sweep above checks no returned ball ends in the pocket.
    let mid = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
    let (d, _) = orbit_offset(mid);
    assert!(
        d < ORBIT_HALF_WIDTH,
        "middle {mid:?} is {d} mm off the lane"
    );
    for x in [min[0], max[0]] {
        for y in [min[1], max[1]] {
            let (d, _) = orbit_offset([x, y]);
            assert!(
                d < ORBIT_HALF_WIDTH + R * 1.0e3,
                "corner ({x}, {y}): {d} mm"
            );
        }
    }
}
