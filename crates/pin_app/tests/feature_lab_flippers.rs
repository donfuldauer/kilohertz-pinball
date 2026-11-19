//! 6.4 R3: the Banana flipper on the Feature Lab.
//!
//! * A ball at rest on the Banana and flipped leaves up the table.
//! * Mirrored runs on the straight lower-left flipper and the Banana
//!   (the same fraction of the bat, mirrored about the table's centre line)
//!   leave on different headings over 11 start offsets: the curl effect.
//! * The `banana_shots` shot (`vuk_lane`, `bb_vuk_sw`) is made by a Banana
//!   flip: the shot ball goes up the VUK lane into the scoop.

mod feature_lab_support;

use feature_lab_support::*;
use pin_sim::Simulation;

/// Rest angle of each lower flipper, degrees; pivots (157, 150) and (357,
/// 150), mirror images about x = 257.
const LEFT: (f32, f32, &str) = (157.0, -30.0, "left_flipper");
const RIGHT: (f32, f32, &str) = (357.0, 210.0, "right_flipper");

/// Places a ball at rest 45 mm above the chord of a flipper at fraction
/// `f` of its 76 mm length, lets it touch down, waits `delay` ms and flips
/// (150 ms). Returns the sim and the ball.
fn flip(side: (f32, f32, &str), f: f32, delay: u64) -> (Simulation, pin_sim::BallHandle) {
    let mut s = load();
    let (px, a, button) = side;
    let a = a.to_radians();
    let p = [px + f * 76.0 * a.cos(), 150.0 + f * 76.0 * a.sin() + 45.0];
    let h = s
        .spawn_ball("standard_steel", mm(p[0], p[1]), [0.0; 3])
        .unwrap();
    for _ in 0..600 {
        s.advance(1);
        if s.ball(h).unwrap().velocity_m_s[1] > -0.01 {
            break;
        }
    }
    s.advance(10 + delay);
    s.set_switch(button, true).unwrap();
    (s, h)
}

/// The exit heading (degrees, 0 = +x, 90 = up the table) and speed of the
/// flipped ball: the first velocity faster than 1 m/s that changes by less
/// than 3 cm/s per tick for 3 ticks in a row (free flight).
fn exit_heading(side: (f32, f32, &str), f: f32) -> Option<(f32, f32)> {
    let (mut s, h) = flip(side, f, 0);
    let mut prev = [0.0f32; 3];
    let mut stable = 0;
    for t in 0..400 {
        s.advance(1);
        if t == 150 {
            s.release_switch(side.2).unwrap();
        }
        let v = s.ball(h)?.velocity_m_s;
        let speed = v[0].hypot(v[1]);
        let dv = (v[0] - prev[0]).hypot(v[1] - prev[1]);
        prev = v;
        if speed > 1.0 && dv < 0.03 {
            stable += 1;
            if stable >= 3 {
                return Some((v[1].atan2(v[0]).to_degrees(), speed));
            }
        } else {
            stable = 0;
        }
    }
    None
}

#[test]
fn a_ball_flipped_by_the_banana_leaves_up_the_table() {
    on_big_stack(|| {
        for k in 0..11 {
            let f = 0.35 + 0.05 * k as f32;
            let (heading, speed) =
                exit_heading(RIGHT, f).unwrap_or_else(|| panic!("f {f}: no free flight"));
            assert!(
                (20.0..160.0).contains(&heading),
                "f {f}: heading {heading} is up the table"
            );
            assert!(speed > 1.0, "f {f}: {speed} m/s");
        }
    });
}

#[test]
fn mirrored_flips_leave_the_banana_and_the_straight_bat_on_different_headings() {
    on_big_stack(|| {
        let mut diffs = Vec::new();
        for k in 0..11 {
            let f = 0.35 + 0.05 * k as f32;
            let (left, _) = exit_heading(LEFT, f).unwrap_or_else(|| panic!("left f {f}"));
            let (right, _) = exit_heading(RIGHT, f).unwrap_or_else(|| panic!("right f {f}"));
            // Mirror the Banana's heading about the centre line.
            let mirrored = 180.0 - right;
            let d = mirrored - left;
            println!("f {f:.2}: straight {left:.1} deg, Banana mirrored {mirrored:.1} deg");
            assert!(d.abs() >= 2.0, "f {f}: headings {left} vs {mirrored}");
            diffs.push(d);
        }
        // The curl turns every Banana shot the same way (towards the
        // flipper's own side of the table).
        assert!(
            diffs.iter().all(|d| *d > 0.0) || diffs.iter().all(|d| *d < 0.0),
            "{diffs:?}"
        );
    });
}

/// The declared window, in ticks (1 ms each), of the `banana_vuk`
/// `[[shots]]` entry (`vuk_lane`, `bb_vuk_sw`) in the bundle's table.toml.
fn banana_window_ticks() -> u64 {
    let text = std::fs::read_to_string(bundle().join("table.toml")).expect("table.toml");
    let t: toml::Table = text.parse().expect("table.toml is TOML");
    let shot = t["shots"]
        .as_array()
        .expect("[[shots]]")
        .iter()
        .find(|s| s["id"].as_str() == Some("banana_vuk"))
        .expect("banana_vuk shot");
    assert_eq!(
        shot["sequence"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>()),
        Some(vec!["vuk_lane", "bb_vuk_sw"])
    );
    u64::try_from(shot["window_ms"].as_integer().expect("window_ms")).expect("window_ms >= 0")
}

#[test]
fn a_banana_flip_makes_the_banana_shot() {
    on_big_stack(|| {
        let window = banana_window_ticks();
        let mut made = Vec::new();
        for k in 0..12 {
            let f = 0.3 + 0.05 * k as f32;
            let (mut s, _) = flip(RIGHT, f, 0);
            // New hits only, sampled from the first flip tick.
            let lane0 = s.switch_hits("vuk_lane").unwrap();
            let scoop0 = s.switch_hits("bb_vuk_sw").unwrap();
            let mut lane_seen = lane0;
            let mut lane_at: Option<u64> = None;
            let mut scoop_at: Option<u64> = None;
            for t in 0..3150u64 {
                if t == 150 {
                    s.release_switch("right_flipper").unwrap();
                }
                s.advance(1);
                // The rules restart the shot window on every new lane hit.
                let lane_now = s.switch_hits("vuk_lane").unwrap();
                if lane_now > lane_seen {
                    lane_seen = lane_now;
                    lane_at = Some(t);
                }
                if scoop_at.is_none() && s.switch_hits("bb_vuk_sw").unwrap() > scoop0 {
                    scoop_at = Some(t);
                }
                if scoop_at.is_some() {
                    break;
                }
            }
            // Made only when the scoop follows the latest lane hit within
            // the declared window; an expired candidate is skipped.
            if let (Some(l), Some(c)) = (lane_at, scoop_at)
                && l < c
                && c - l <= window
            {
                made.push(f);
            }
        }
        println!("Banana shot made from offsets {made:?}");
        assert!(!made.is_empty(), "some Banana flip makes the shot");
    });
}
