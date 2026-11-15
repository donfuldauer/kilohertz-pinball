//! Gate A1 fix (6.0 grey-box): no ball comes to rest in the inlanes or
//! outside the drain funnel.
//!
//! * A ball put at rest on either inlane slope (a sweep of start points
//!   80..200 mm up the slope, measured along the inlane wall from its lower
//!   end) rolls onto the flipper and past it within 4 simulated seconds: it
//!   drains into a trough switch or its centre passes below the flipper
//!   tips (flippers at rest).
//! * A ball at rest at (318, 14) mm, where the novice soak found a stuck
//!   ball beside `trough_2` on the unfixed table, reaches a trough switch
//!   within 4 s.
//! * A ball put at rest on the back of the (upright) drop target, where a
//!   novice soak found a stuck ball, rolls off within 4 s.
//!
//! Simulated time only (rule 5): fixed tick counts, state injection through
//! the public `Simulation` API.

mod support;

use pin_sim::{SimConfig, Simulation};
use support::{greybox_bundle, on_big_stack};

/// Ball radius of the standard steel ball, mm.
const BALL_R_MM: f32 = 13.5;

/// 4 simulated seconds.
const TICKS: u64 = 4000;

/// The inlane walls: (upper end, lower end), mm. The lower end is the one
/// next to the flipper pivot (tables/greybox/layout.toml).
const LEFT_INLANE: ([f32; 2], [f32; 2]) = ([0.0, 330.0], [152.0, 172.0]);
const RIGHT_INLANE: ([f32; 2], [f32; 2]) = ([470.0, 330.0], [362.0, 172.0]);

/// Flipper tips at rest (pivot, rest angle, length 76 mm; table.toml):
/// lower left (157, 150) at -30°, lower right (357, 150) at 210°.
fn tip_y_mm() -> f32 {
    150.0 - 76.0 * 30.0f32.to_radians().sin()
}

fn mm3(x: f32, y: f32) -> [f32; 3] {
    [x * 1.0e-3, y * 1.0e-3, BALL_R_MM * 1.0e-3]
}

/// The ball centre at rest on the playfield side of an inlane wall, `d` mm
/// up the wall from its lower end.
fn on_inlane(wall: ([f32; 2], [f32; 2]), d: f32) -> [f32; 3] {
    let (top, low) = wall;
    let (dx, dy) = (top[0] - low[0], top[1] - low[1]);
    let len = (dx * dx + dy * dy).sqrt();
    let (ux, uy) = (dx / len, dy / len);
    // Normal pointing up the table (towards the playfield side).
    let (mut nx, mut ny) = (-uy, ux);
    if ny < 0.0 {
        nx = -nx;
        ny = -ny;
    }
    let off = BALL_R_MM + 0.5;
    mm3(low[0] + ux * d + nx * off, low[1] + uy * d + ny * off)
}

fn load() -> Simulation {
    let mut s = Simulation::load(greybox_bundle(), SimConfig::default()).unwrap();
    s.advance(10);
    s
}

fn trough_hits(s: &Simulation) -> u32 {
    s.switch_hits("trough_1").unwrap() + s.switch_hits("trough_2").unwrap()
}

/// How a ball put at rest at `p` ended within [`TICKS`].
#[derive(Debug, Clone, Copy, PartialEq)]
enum Outcome {
    /// Closed a trough switch (drained).
    Drained,
    /// Its centre went below the flipper tips.
    PastFlippers,
    /// Still above the flipper tips after 4 s, at this position (mm).
    Held([f32; 2]),
}

fn run_from(s: &mut Simulation, p: [f32; 3]) -> Outcome {
    let t0 = trough_hits(s);
    let e0 = s.counters().balls_escaped;
    let h = s.spawn_ball("standard_steel", p, [0.0; 3]).unwrap();
    let tip = tip_y_mm() * 1.0e-3;
    let mut out = None;
    for _ in 0..TICKS {
        s.advance(1);
        if trough_hits(s) > t0 {
            out = Some(Outcome::Drained);
            break;
        }
        match s.ball(h) {
            Some(b) if b.position_m[1] < tip - BALL_R_MM * 1.0e-3 => {
                out = Some(Outcome::PastFlippers);
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    assert_eq!(s.counters().balls_escaped, e0, "ball from {p:?} escaped");
    let out = out.unwrap_or_else(|| {
        let b = s.ball(h).expect("ball neither drained nor on the table");
        Outcome::Held([b.position_m[0] * 1.0e3, b.position_m[1] * 1.0e3])
    });
    if s.ball(h).is_some() {
        s.remove_ball(h);
    }
    out
}

#[test]
fn ball_at_rest_120mm_up_either_inlane_leaves_past_the_flipper() {
    on_big_stack(|| {
        let mut s = load();
        for (side, wall) in [("left", LEFT_INLANE), ("right", RIGHT_INLANE)] {
            let p = on_inlane(wall, 120.0);
            let out = run_from(&mut s, p);
            assert_ne!(
                std::mem::discriminant(&out),
                std::mem::discriminant(&Outcome::Held([0.0; 2])),
                "{side} inlane: ball from {p:?} came to rest: {out:?}"
            );
        }
    });
}

#[test]
fn inlane_sweep_of_start_heights_never_holds_the_ball() {
    on_big_stack(|| {
        let mut s = load();
        let mut held = Vec::new();
        for (side, wall) in [("left", LEFT_INLANE), ("right", RIGHT_INLANE)] {
            for d in [80.0f32, 110.0, 140.0, 170.0, 200.0] {
                let p = on_inlane(wall, d);
                let out = run_from(&mut s, p);
                if let Outcome::Held(at) = out {
                    held.push((side, d, at));
                }
            }
        }
        assert!(
            held.is_empty(),
            "balls held (side, mm up, rest mm): {held:?}"
        );
    });
}

#[test]
fn ball_at_rest_beside_trough_2_drains() {
    on_big_stack(|| {
        let mut s = load();
        let t0 = trough_hits(&s);
        let e0 = s.counters().balls_escaped;
        let h = s
            .spawn_ball("standard_steel", mm3(318.0, 14.0), [0.0; 3])
            .unwrap();
        let mut drained = false;
        for _ in 0..TICKS {
            s.advance(1);
            if trough_hits(&s) > t0 {
                drained = true;
                break;
            }
        }
        let last = s.ball(h);
        assert_eq!(s.counters().balls_escaped, e0, "escaped");
        assert!(drained, "ball from (318, 14) did not drain; now {last:?}");
    });
}

#[test]
fn ball_at_rest_on_the_back_of_the_drop_target_rolls_off() {
    on_big_stack(|| {
        let mut s = load();
        // drop_1 (table.toml): centre (200, 470) mm, facing `angle_deg`; the
        // ball rests against the face pointing the other way (its back).
        let face = 290.0f32.to_radians();
        let back = (-face.cos(), -face.sin());
        let along = (-back.1, back.0);
        // Plate half thickness 2.5 mm (physics_defaults) + ball radius.
        let d = 2.5 + BALL_R_MM;
        let mut held = Vec::new();
        for t in [-8.0f32, -4.0, 0.0, 4.0, 8.0] {
            let x = 200.0 + back.0 * d + along.0 * t;
            let y = 470.0 + back.1 * d + along.1 * t;
            let e0 = s.counters().balls_escaped;
            let h = s.spawn_ball("standard_steel", mm3(x, y), [0.0; 3]).unwrap();
            s.advance(TICKS);
            assert_eq!(s.counters().balls_escaped, e0, "escaped from ({x}, {y})");
            if let Some(b) = s.ball(h) {
                let (bx, by) = (b.position_m[0] * 1.0e3, b.position_m[1] * 1.0e3);
                if (bx - 200.0).hypot(by - 470.0) < 40.0 {
                    held.push(((x, y), (bx, by)));
                }
                s.remove_ball(h);
            }
        }
        assert!(
            held.is_empty(),
            "balls resting on the drop target: {held:?}"
        );
    });
}
