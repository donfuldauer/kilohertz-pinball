//! 6.2 T2: the Sky Deck's two levels in the headless simulation.
//!
//! * (a) A ball in the centre VUK (layer 0) is lifted onto the upper deck
//!   (layer 1) and rolls on the deck at deck height; `vuk_sw` and
//!   `deck_opto` count it.
//! * (b) A ball on the upper deck shot by the `upper_right` flipper (button
//!   `right_upper`) leaves up the deck towards the drop-target bank, over a
//!   sweep of start positions along the bat.
//! * (c) Balls on different layers never collide: a layer-0 steel ball and
//!   a layer-1 Glo-Ball driven through the same point (offsets, speeds and
//!   approach angles, head-on included) pass through each other with no
//!   velocity change beyond gravity's, while two layer-0 balls on the same
//!   paths collide (control).

mod sky_deck_support;

use pin_sim::Simulation;
use sky_deck_support::{
    DECK_Z_MM, VUK_HOLD_TICKS, at, bundle_manifest, healthy, key, load, on_big_stack, only_ball,
    put_in_vuk, start_game,
};

#[test]
fn vuk_lifts_the_ball_onto_the_upper_deck() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        start_game(&mut s);
        let h = only_ball(&s);
        let (vuk0, deck0) = (
            s.switch_hits("vuk_sw").unwrap(),
            s.switch_hits("deck_opto").unwrap(),
        );
        put_in_vuk(&mut s, h);
        s.advance(5);
        assert_eq!(s.switch_closed("vuk_sw"), Some(true), "captured");
        assert_eq!(s.switch_hits("vuk_sw").unwrap(), vuk0 + 1);
        // Held for the rules' hold time, then ejected onto the deck.
        let mut lifted_at = None;
        for t in 0..(VUK_HOLD_TICKS as u32 + 3000) {
            s.advance(1);
            let b = s.ball(h).expect("ball on the table");
            if b.layer == 1 {
                lifted_at = Some(t);
                break;
            }
        }
        assert!(
            lifted_at.is_some_and(|t| u64::from(t) + 100 > VUK_HOLD_TICKS),
            "lifted after the hold: {lifted_at:?}"
        );
        // On the deck: at deck height, on layer 1, rolling down the deck.
        s.advance(300);
        let b = s.ball(h).unwrap();
        assert_eq!(b.layer, 1);
        let rest_z = (DECK_Z_MM + 13.5) * 1.0e-3;
        assert!(
            (b.position_m[2] - rest_z).abs() < 1.0e-3,
            "z {}",
            b.position_m[2]
        );
        assert!(
            b.position_m[0] > 0.25 && b.position_m[0] < 0.465,
            "{:?}",
            b.position_m
        );
        assert!(
            b.position_m[1] > 0.77 && b.position_m[1] < 1.04,
            "{:?}",
            b.position_m
        );
        assert_eq!(s.switch_hits("deck_opto").unwrap(), deck0 + 1, "deck_opto");
        // It comes back to the main deck through the acrylic return funnel:
        // the layer 1 -> 0 change happens inside the `deck_return`
        // transition zone (not a safety exit such as `deck_exit_left`).
        let ret = bundle_manifest()
            .layer_transitions
            .iter()
            .find(|z| z.id == "deck_return")
            .cloned()
            .expect("deck_return transition");
        assert_eq!((ret.from_layer, ret.to_layer), (1, 0));
        // The engine tests the zone at a CCD sub-step end point; the ball
        // moves on for the rest of the tick after the switch, so the plan
        // position read at the end of that tick may lie just outside the
        // zone. Tolerance: one ball radius (13.5 mm), far less than the
        // distance to any other 1 -> 0 zone's interior.
        let tol_mm = 13.5f32;
        let mut back = None;
        for _ in 0..8000 {
            s.advance(1);
            match s.ball(h) {
                Some(b) if b.layer == 0 => {
                    back = Some(b.position_m);
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        let p = back.expect("returned to the main deck");
        let (x, y) = (p[0] * 1.0e3, p[1] * 1.0e3);
        let (lo, hi) = (ret.zone.min, ret.zone.max);
        assert!(
            x >= lo[0] - tol_mm
                && x <= hi[0] + tol_mm
                && y >= lo[1] - tol_mm
                && y <= hi[1] + tol_mm,
            "layer change at ({x}, {y}) mm, outside deck_return {lo:?}..{hi:?}"
        );
        healthy(&s);
    });
}

/// Rest position of a ball on the upper_right flipper's bat, `d` mm from
/// the pivot (pivot (440, 805), rest 202 degrees, radius 9 -> 5.5 mm).
fn on_bat(d: f32) -> (f32, f32) {
    let a = 202f32.to_radians();
    let (ux, uy) = (a.cos(), a.sin());
    // Upper face normal of the bat (pointing up and left).
    let (nx, ny) = (-uy, ux);
    let (nx, ny) = if ny < 0.0 { (-nx, -ny) } else { (nx, ny) };
    let r = 9.0 - 3.5 * d / 52.0 + 13.5 + 0.5;
    (440.0 + ux * d + nx * r, 805.0 + uy * d + ny * r)
}

/// The x (m) at which a ball at `pos` moving with `vel` reaches the line
/// y = `line_y` (m), projected from that one state. A ball already at or
/// past the line (t <= 0) has crossed it: its sampled x is returned.
fn x_at_line(pos: [f32; 3], vel: [f32; 3], line_y: f32) -> f32 {
    let t = (line_y - pos[1]) / vel[1];
    if t <= 0.0 {
        pos[0]
    } else {
        pos[0] + vel[0] * t
    }
}

#[test]
fn bank_line_projection_uses_one_state() {
    // The review's example: pre-stroke y 0.800 m, sample (0.420, 0.880) m,
    // v (0.5, 2.0) m/s: from the sample the ball reaches y 0.995 at
    // x 0.420 + 0.5 * 0.0575 = 0.44875 m (the mixed projection from the
    // pre-stroke y gave 0.4688).
    let x = x_at_line([0.420, 0.880, 0.0], [0.5, 2.0, 0.0], 0.995);
    assert!((x - 0.449).abs() < 1.0e-3, "x {x}");
    assert!((0.360..0.465).contains(&x));
    // Past the line: the sampled x, not a back-extrapolation.
    let x = x_at_line([0.430, 1.000, 0.0], [0.5, 2.0, 0.0], 0.995);
    assert_eq!(x, 0.430);
}

#[test]
fn upper_right_flipper_shoots_up_the_deck_towards_the_drop_bank() {
    on_big_stack(|| {
        for d in [15.0f32, 22.0, 29.0, 36.0, 43.0] {
            let mut s = load();
            s.advance(10);
            let (x, y) = on_bat(d);
            let h = s
                .spawn_ball_on_layer("standard_steel", 1, at(x, y, DECK_Z_MM), [0.0; 3])
                .unwrap();
            // Let it settle against the bat for a moment (it rolls slowly
            // towards the tip).
            s.advance(10);
            s.set_switch("right_upper", true).unwrap();
            s.advance(40);
            let b = s.ball(h).unwrap();
            let v = b.velocity_m_s;
            let speed = v[0].hypot(v[1]);
            let ang = v[1].atan2(v[0]).to_degrees();
            assert_eq!(b.layer, 1, "d {d}: still on the deck");
            assert!((1.0..8.0).contains(&speed), "d {d}: speed {speed} m/s");
            assert!((60.0..130.0).contains(&ang), "d {d}: direction {ang} deg");
            // Its path crosses the bank's line (y 980..1010) between the
            // bank's ends (x 372..452). Projected from the sampled state
            // alone; a sample already at or past the line counts as crossed
            // at its own x (no back-extrapolation).
            let x_at_bank = x_at_line(b.position_m, v, 0.995);
            assert!(
                (0.360..0.465).contains(&x_at_bank),
                "d {d}: reaches the bank line at x {x_at_bank}"
            );
            s.release_switch("right_upper").unwrap();
            healthy(&s);
        }
    });
}

/// One pass of two balls through the point (220, 380) mm, 95 mm up (in the
/// air above the main deck, under the glass: no floor or glass contact
/// during the pass; both fall alike, so they meet in 3-D too). Ball A moves along `angle_a` degrees at `speed` m/s, ball B
/// along `angle_a + approach` at the same speed, B offset sideways by
/// `offset` mm. Returns the largest velocity change of either ball beyond
/// free fall (m/s) and the closest plan-view centre distance (mm).
fn pass(layer_b: u8, profile_b: &str, speed: f32, approach: f32, offset: f32) -> (f32, f32) {
    let mut s = load();
    s.advance(5);
    let (cx, cy, z) = (220.0f32, 380.0f32, 95.0f32);
    // Each ball starts 40 mm before the meeting point.
    let lead_mm = 40.0f32;
    let dir = |deg: f32| {
        let a = deg.to_radians();
        (a.cos(), a.sin())
    };
    let (ax, ay) = dir(0.0);
    let (bx, by) = dir(approach);
    let start = |ux: f32, uy: f32, off: f32| {
        // `off` mm to the left of the path.
        let d = lead_mm;
        [
            (cx - ux * d - uy * off) * 1.0e-3,
            (cy - uy * d + ux * off) * 1.0e-3,
            z * 1.0e-3,
        ]
    };
    let a = s
        .spawn_ball_on_layer(
            "standard_steel",
            0,
            start(ax, ay, 0.0),
            [ax * speed, ay * speed, 0.0],
        )
        .unwrap();
    let b = s
        .spawn_ball_on_layer(
            profile_b,
            layer_b,
            start(bx, by, offset),
            [bx * speed, by * speed, 0.0],
        )
        .unwrap();
    let va0 = s.ball(a).unwrap().velocity_m_s;
    let vb0 = s.ball(b).unwrap().velocity_m_s;
    let g = s.solver().config().gravity;
    let mut closest = f32::INFINITY;
    let mut worst = 0.0f32;
    let ticks = (lead_mm / speed) as u32 + 25;
    for t in 1..=ticks {
        s.advance(1);
        let (pa, pb) = (s.ball(a).unwrap(), s.ball(b).unwrap());
        let d = (pa.position_m[0] - pb.position_m[0]).hypot(pa.position_m[1] - pb.position_m[1]);
        closest = closest.min(d * 1000.0);
        let secs = t as f32 * 1.0e-3;
        for (v0, now) in [(va0, pa.velocity_m_s), (vb0, pb.velocity_m_s)] {
            let dv = (0..3)
                .map(|i| now[i] - (v0[i] + g[i] * secs))
                .map(|x| x * x)
                .sum::<f32>()
                .sqrt();
            worst = worst.max(dv);
        }
    }
    healthy(&s);
    (worst, closest)
}

#[test]
fn balls_on_different_layers_never_collide() {
    on_big_stack(|| {
        for speed in [0.5f32, 1.5, 3.0] {
            for approach in [180.0f32, 135.0, 90.0] {
                for offset in [0.0f32, 8.0, 18.0] {
                    let (dv, closest) = pass(1, "uv_globall", speed, approach, offset);
                    assert!(
                        closest < 27.0,
                        "{speed} m/s, {approach} deg, {offset} mm: closest {closest} mm"
                    );
                    assert!(
                        dv < 1.0e-3,
                        "different layers interacted: {speed} m/s, {approach} deg, \
                         {offset} mm: dv {dv} m/s"
                    );
                    // Control: the same paths on one layer collide.
                    let (dv, _) = pass(0, "uv_globall", speed, approach, offset);
                    assert!(
                        dv > 0.1 * speed,
                        "same layer did not collide: {speed} m/s, {approach} deg, \
                         {offset} mm: dv {dv} m/s"
                    );
                }
            }
        }
    });
}

#[test]
fn a_ball_on_the_deck_passes_over_a_ball_below() {
    // The real table: a layer-1 Glo-Ball rolling down the deck over a
    // layer-0 steel ball rolling up the main deck under it.
    on_big_stack(|| {
        let mut s: Simulation = load();
        s.advance(5);
        let up = s
            .spawn_ball_on_layer(
                "uv_globall",
                1,
                at(300.0, 900.0, DECK_Z_MM),
                [0.0, -0.3, 0.0],
            )
            .unwrap();
        let down = s
            .spawn_ball_on_layer("standard_steel", 0, at(300.0, 820.0, 0.0), [0.0, 0.6, 0.0])
            .unwrap();
        let mut closest = f32::INFINITY;
        for _ in 0..120 {
            s.advance(1);
            let (a, b) = (s.ball(up).unwrap(), s.ball(down).unwrap());
            assert_eq!((a.layer, b.layer), (1, 0));
            closest = closest
                .min((a.position_m[0] - b.position_m[0]).hypot(a.position_m[1] - b.position_m[1]));
        }
        assert!(closest < 0.027, "plan-view closest {closest} m");
        assert!(
            s.ball(up).unwrap().velocity_m_s[1] < 0.0,
            "Glo-Ball still rolling down"
        );
        assert!(
            s.ball(down).unwrap().velocity_m_s[1] > 0.0,
            "steel ball still rolling up"
        );
        let _ = key::BALL;
        healthy(&s);
    });
}
