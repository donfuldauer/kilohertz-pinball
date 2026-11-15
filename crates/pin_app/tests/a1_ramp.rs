//! Gate A1 fix (6.0 grey-box): the wireform ramp loses no ball and can be
//! made from the left flipper.
//!
//! * Escapes: balls injected on layer 0 on the approach to the ramp's entry
//!   lane (offsets 0..30 mm either side of the lane axis, headings 0..40
//!   degrees either side of straight up, 2, 3 and 4 m/s) never leave the
//!   table (`SimCounters::balls_escaped`, the counter behind the soak's
//!   EscapedBall detector) within 4 simulated seconds.
//! * Left flipper: a ball cradled on the lower left flipper and fired after a
//!   sweep of delays gives the flipper's real shot headings; at least two of
//!   those real shots make the ramp (`ramp_made` closes exactly once).
//!   Shots from the left flipper's line (from (220, 180) mm) at three speeds
//!   in 3..4 m/s and three headings inside that measured range all make the
//!   ramp exactly once.
//! * Entry width: balls rolling straight up the lane at offsets across it
//!   make the ramp over a band many millimetres wide, not only on one axis.
//!
//! Simulated time only (rule 5): fixed tick counts, state injection through
//! the public `Simulation` API.

mod support;

use pin_sim::{BallHandle, SimConfig, Simulation};
use support::{greybox_bundle, on_big_stack};

/// Ball radius of the standard steel ball, mm.
const BALL_R_MM: f32 = 13.5;

/// Axis of the ramp's entry lane and wire, mm (tables/greybox/layout.toml).
const LANE_X_MM: f32 = 425.0;

/// The left flipper's shot line starts here, mm (just off the bat).
const LINE_START_MM: [f32; 2] = [220.0, 180.0];

fn mm3(x: f32, y: f32) -> [f32; 3] {
    [x * 1.0e-3, y * 1.0e-3, BALL_R_MM * 1.0e-3]
}

fn load() -> Simulation {
    let mut s = Simulation::load(greybox_bundle(), SimConfig::default()).unwrap();
    s.advance(10);
    s
}

/// What one ball did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    /// `ramp_made` closing edges.
    made: u32,
    /// Balls the simulation removed as escaped.
    escaped: u32,
}

/// Follows ball `h` for `ticks` (or until it drains). Every ball runs in a
/// fresh simulation: a ball removed while a mech holds it (the VUK) would
/// leave the mech's hold on its slot.
fn follow(s: &mut Simulation, h: BallHandle, ticks: u64) -> Run {
    let m0 = s.switch_hits("ramp_made").unwrap();
    let e0 = s.counters().balls_escaped;
    for _ in 0..ticks {
        s.advance(1);
        match s.ball(h) {
            Some(b) => {
                let finite = b.position_m.iter().all(|v| v.is_finite())
                    && b.velocity_m_s.iter().all(|v| v.is_finite());
                assert!(finite, "non-finite ball {b:?}");
            }
            None => break,
        }
    }
    Run {
        made: s.switch_hits("ramp_made").unwrap() - m0,
        escaped: s.counters().balls_escaped - e0,
    }
}

/// Velocity of `speed` m/s at `heading_deg` right of straight up the table.
fn vel(speed: f32, heading_deg: f32) -> [f32; 3] {
    let a = heading_deg.to_radians();
    [speed * a.sin(), speed * a.cos(), 0.0]
}

#[test]
fn approach_sweep_at_the_ramp_entry_never_escapes() {
    on_big_stack(|| {
        let mut escaped = Vec::new();
        let mut made = 0;
        let mut runs = 0;
        for speed in [2.0f32, 3.0, 4.0] {
            for heading in [-40.0f32, -20.0, 0.0, 20.0, 40.0] {
                for offset in [-30.0f32, -15.0, 0.0, 15.0, 30.0] {
                    // 30 mm below the lane's right wall, on layer 0.
                    let p = mm3(LANE_X_MM + offset, 400.0);
                    let mut s = load();
                    let h = s
                        .spawn_ball("standard_steel", p, vel(speed, heading))
                        .unwrap();
                    let r = follow(&mut s, h, 4000);
                    runs += 1;
                    made += u32::from(r.made > 0);
                    if r.escaped > 0 {
                        escaped.push((speed, heading, offset));
                    }
                }
            }
        }
        println!("entry approach sweep: {runs} balls, {made} made the ramp, escaped {escaped:?}");
        assert!(escaped.is_empty(), "escaped (m/s, deg, mm): {escaped:?}");
        assert!(made > 0, "no ball of the sweep made the ramp");
    });
}

/// A real left-flipper shot: the cradled ball fired `delay` ms after the
/// bat drops. Returns the heading (degrees right of straight up) and speed
/// (m/s) as the ball crosses y = 250 mm, and what the ball did.
fn cradled_shot(delay: u64) -> (Option<(f32, f32)>, Run) {
    let mut s = load();
    s.set_switch("left_flipper", true).unwrap();
    s.advance(100);
    // Rolls down the left inlane into the cradle on the raised bat.
    let h = s
        .spawn_ball("standard_steel", mm3(100.0, 250.0), [0.0; 3])
        .unwrap();
    s.advance(2000);
    s.release_switch("left_flipper").unwrap();
    s.advance(delay);
    s.set_switch("left_flipper", true).unwrap();
    let mut shot = None;
    for _ in 0..300 {
        s.advance(1);
        let Some(b) = s.ball(h) else { break };
        if b.position_m[1] > 0.25 {
            let v = b.velocity_m_s;
            shot = Some((
                v[0].atan2(v[1]).to_degrees(),
                (v[0] * v[0] + v[1] * v[1]).sqrt(),
            ));
            break;
        }
    }
    s.release_switch("left_flipper").unwrap();
    let run = follow(&mut s, h, 3000);
    (shot, run)
}

#[test]
fn left_flipper_shots_make_the_ramp() {
    on_big_stack(|| {
        // The flipper's real headings: a cradled ball fired at a sweep of
        // timings (the ball rolls down the bat between release and flip).
        let mut range = (f32::MAX, f32::MIN);
        let mut made_headings = Vec::new();
        for delay in (500u64..=860).step_by(10) {
            let (shot, run) = cradled_shot(delay);
            assert_eq!(run.escaped, 0, "cradled shot after {delay} ms escaped");
            let Some((heading, speed)) = shot else {
                continue;
            };
            println!("cradled shot {delay} ms: {heading:.1} deg, {speed:.2} m/s, {run:?}");
            range = (range.0.min(heading), range.1.max(heading));
            if run.made > 0 {
                assert_eq!(run.made, 1, "ramp_made closes once per made shot");
                made_headings.push(heading);
            }
        }
        println!(
            "left flipper headings {range:?}; real shots that made the ramp: {made_headings:?}"
        );
        assert!(
            made_headings.len() >= 2,
            "real left-flipper shots that made the ramp: {made_headings:?}"
        );

        // Shots from the left flipper's line at three speeds and three
        // headings inside the measured range.
        let headings = [29.0f32, 31.0, 33.0];
        for hd in headings {
            assert!(
                (range.0..=range.1).contains(&hd),
                "heading {hd} outside the flipper's range {range:?}"
            );
        }
        for speed in [3.0f32, 3.5, 4.0] {
            for hd in headings {
                let mut s = load();
                let h = s
                    .spawn_ball(
                        "standard_steel",
                        mm3(LINE_START_MM[0], LINE_START_MM[1]),
                        vel(speed, hd),
                    )
                    .unwrap();
                let r = follow(&mut s, h, 3000);
                assert_eq!(
                    r,
                    Run {
                        made: 1,
                        escaped: 0
                    },
                    "shot {speed} m/s at {hd} deg from the left flipper's line"
                );
            }
        }

        // For the report: which headings from the line make it (1 degree
        // steps, 3.5 m/s).
        let mut band = Vec::new();
        for k in 0..=14u8 {
            let hd = 24.0 + f32::from(k);
            let mut s = load();
            let h = s
                .spawn_ball(
                    "standard_steel",
                    mm3(LINE_START_MM[0], LINE_START_MM[1]),
                    vel(3.5, hd),
                )
                .unwrap();
            let r = follow(&mut s, h, 3000);
            assert_eq!(r.escaped, 0, "heading {hd}");
            if r.made == 1 {
                band.push(hd);
            }
        }
        println!("headings from the line that make the ramp at 3.5 m/s: {band:?}");
    });
}

#[test]
fn ramp_is_made_across_a_band_of_the_entry_lane() {
    on_big_stack(|| {
        let mut made = Vec::new();
        for k in 0..=30u8 {
            let offset = -15.0 + f32::from(k);
            let mut s = load();
            let h = s
                .spawn_ball(
                    "standard_steel",
                    mm3(LANE_X_MM + offset, 400.0),
                    vel(2.5, 0.0),
                )
                .unwrap();
            let r = follow(&mut s, h, 3000);
            assert_eq!(r.escaped, 0, "offset {offset}");
            if r.made > 0 {
                assert_eq!(r.made, 1, "offset {offset}");
                made.push(offset);
            }
        }
        println!("straight-up 2.5 m/s balls that make the ramp, offsets (mm) from x 425: {made:?}");
        let width = match (made.first(), made.last()) {
            (Some(a), Some(b)) => b - a,
            _ => 0.0,
        };
        // Every offset between the extremes makes it, and the band is far
        // wider than the old +-2 mm.
        assert_eq!(made.len() as f32, width + 1.0, "made offsets {made:?}");
        assert!(width >= 10.0, "made band {made:?}");
    });
}
