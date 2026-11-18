//! 6.1 T3: every switch-bearing mechanism of the Apex Circuit closes its
//! switch from a ball's physical contact (balls spawned or moved onto it,
//! no forced switch), the lock scoop ejects its ball back into play, the
//! drop bank drops in order and resets, the magnet pair tells steel from the
//! Powerball, and the zipper closes the flipper gap by the declared amount.

mod apex_support;

use apex_support::{apex_bundle, mm, on_big_stack};
use pin_core::ids::FlipperId;
use pin_sim::{SimConfig, Simulation};
use std::collections::BTreeMap;

/// Every non-cabinet switch of the table.
const SWITCHES: [&str; 30] = [
    "trough_1",
    "trough_2",
    "left_flipper_eos",
    "right_flipper_eos",
    "left_upper_eos",
    "sling_left_sw",
    "sling_right_sw",
    "bumper_1_ring",
    "bumper_2_ring",
    "bumper_3_ring",
    "left_spinner",
    "orbit_top",
    "ramp_entry",
    "ramp_made",
    "lock_scoop_sw",
    "magnet_eddy",
    "magnet_opto",
    "lock_lane",
    "drop_1_sw",
    "drop_2_sw",
    "drop_3_sw",
    "shooter_lane",
    "plunger_tip",
    "right_orbit_entry",
    "lane_1",
    "lane_2",
    "lane_3",
    "left_inlane",
    "right_inlane",
    "newton_sw",
];

type Hits = BTreeMap<&'static str, u32>;

fn hits(s: &Simulation) -> Hits {
    SWITCHES
        .iter()
        .map(|n| (*n, s.switch_hits(n).expect(n)))
        .collect()
}

fn delta(a: &Hits, b: &Hits, n: &str) -> u32 {
    b[n] - a[n]
}

fn press(s: &mut Simulation, name: &str, ticks: u64) {
    s.set_switch(name, true).unwrap();
    s.advance(ticks);
    s.release_switch(name).unwrap();
}

/// Spawns a ball at `p` (mm) with velocity `v` (m/s), runs `ticks` (or
/// until it leaves), removes it if still there; returns the hit counts
/// before and after.
fn trial(
    s: &mut Simulation,
    profile: &str,
    p: (f32, f32),
    v: [f32; 3],
    ticks: u64,
) -> (Hits, Hits) {
    let before = hits(s);
    let h = s.spawn_ball(profile, mm(p.0, p.1), v).unwrap();
    for _ in 0..ticks {
        s.advance(1);
        if s.ball(h).is_none() {
            break;
        }
    }
    if s.ball(h).is_some() {
        s.remove_ball(h);
    }
    (before, hits(s))
}

fn steel(s: &mut Simulation, p: (f32, f32), v: [f32; 3], ticks: u64) -> (Hits, Hits) {
    trial(s, "standard_steel", p, v, ticks)
}

fn load_in_game() -> Simulation {
    let mut s = Simulation::load(apex_bundle(), SimConfig::default()).unwrap();
    s.advance(10);
    press(&mut s, "start", 5);
    s.advance(1500);
    assert_eq!(s.balls_in_play(), 1, "ball 1 served");
    s
}

fn healthy(s: &Simulation) {
    assert!(!s.script_faulted(), "{:?}", s.script_error());
    assert_eq!(s.script_queue_overflows(), 0);
    let c = s.counters();
    assert_eq!(c.edges_dropped, 0, "{c:?}");
    assert_eq!(c.balls_escaped, 0, "{c:?}");
}

#[test]
fn every_switch_closes_from_ball_contact() {
    on_big_stack(|| {
        let mut s = load_in_game();
        let served = s.ball_handles()[0];
        assert_eq!(s.switch_closed("plunger_tip"), Some(true), "on the plunger");

        // Flippers: the button raises the bat; the EOS switch changes.
        for (button, flipper, eos) in [
            ("left_flipper", "left_flipper", "left_flipper_eos"),
            ("right_flipper", "right_flipper", "right_flipper_eos"),
            ("left_upper", "left_upper", "left_upper_eos"),
        ] {
            let before = hits(&s);
            s.set_switch(button, true).unwrap();
            s.advance(80);
            let (angle, _) = s.flipper(flipper).unwrap();
            assert!(angle.abs() > 0.7, "{flipper} raised: {angle}");
            s.release_switch(button).unwrap();
            s.advance(300);
            assert_eq!(delta(&before, &hits(&s), eos), 1, "{eos}");
        }

        // Slingshots.
        let (a, b) = steel(&mut s, (149.0, 324.0), [-0.8, -0.6, 0.0], 300);
        assert!(delta(&a, &b, "sling_left_sw") >= 1, "left sling");
        let (a, b) = steel(&mut s, (326.0, 324.0), [0.8, -0.6, 0.0], 300);
        assert!(delta(&a, &b, "sling_right_sw") >= 1, "right sling");

        // Pop bumpers.
        for (name, x, y, v) in [
            ("bumper_1_ring", 190.0, 640.0, 1.0),
            ("bumper_2_ring", 324.0, 660.0, 1.0),
            ("bumper_3_ring", 257.0, 700.0, 1.5),
        ] {
            let (a, b) = steel(&mut s, (x, y), [0.0, v, 0.0], 250);
            assert!(delta(&a, &b, name) >= 1, "{name}");
        }

        // Left orbit: up the lane through the spinner and round the top.
        let (a, b) = steel(&mut s, (80.0, 621.0), [-1.628, 1.898, 0.0], 1500);
        assert!(delta(&a, &b, "left_spinner") >= 1, "spinner");
        assert_eq!(delta(&a, &b, "orbit_top"), 1, "orbit top");

        // Right orbit: up the corridor into the shooter-lane top, through
        // the magnet pair.
        let (a, b) = steel(&mut s, (395.0, 680.0), [0.6, 1.6, 0.0], 1500);
        assert!(delta(&a, &b, "right_orbit_entry") >= 1, "right orbit entry");
        assert!(
            delta(&a, &b, "magnet_opto") >= 1,
            "right orbit to the magnet"
        );

        // Ramp: entry and made (layers 0 -> 2 -> 1).
        let (a, b) = steel(&mut s, (420.0, 330.0), [0.0, 2.3, 0.0], 1500);
        assert!(delta(&a, &b, "ramp_entry") >= 1);
        assert_eq!(delta(&a, &b, "ramp_made"), 1);

        // Top lanes: a ball rolling down each lane.
        for (name, x) in [("lane_1", 300.0), ("lane_2", 340.0), ("lane_3", 380.0)] {
            let (a, b) = steel(&mut s, (x, 952.0), [0.0, -0.3, 0.0], 200);
            assert_eq!(delta(&a, &b, name), 1, "{name}");
        }

        // Inlanes: a ball rolling down each inlane.
        let (a, b) = steel(&mut s, (60.0, 300.0), [0.5, -0.5, 0.0], 300);
        assert!(delta(&a, &b, "left_inlane") >= 1, "left inlane");
        let (a, b) = steel(&mut s, (430.0, 300.0), [-0.4, -0.6, 0.0], 300);
        assert!(delta(&a, &b, "right_inlane") >= 1, "right inlane");

        // Newton ball.
        let (a, b) = steel(&mut s, (330.0, 320.0), [0.0, 1.5, 0.0], 300);
        assert!(delta(&a, &b, "newton_sw") >= 1, "newton");

        // Plunger: the served ball is launched up the shooter lane.
        let before = hits(&s);
        press(&mut s, "launch", 20);
        s.advance(400);
        assert_eq!(delta(&before, &hits(&s), "shooter_lane"), 1);
        assert_eq!(s.switch_closed("plunger_tip"), Some(false));
        s.remove_ball(served);

        // Trough: a ball between the flippers drains.
        let drained = s.counters().balls_drained;
        let (a, b) = steel(&mut s, (257.0, 200.0), [0.0, -0.5, 0.0], 400);
        assert!(delta(&a, &b, "trough_1") + delta(&a, &b, "trough_2") >= 1);
        assert_eq!(s.counters().balls_drained, drained + 1);

        // Tilt switch (cabinet).
        let tilts = s.switch_hits("tilt").unwrap();
        press(&mut s, "tilt", 5);
        assert_eq!(s.switch_hits("tilt"), Some(tilts + 1));
        healthy(&s);
    });
}

#[test]
fn drop_bank_drops_in_order_and_resets() {
    on_big_stack(|| {
        let mut s = load_in_game();
        // The in-line bank faces 290 degrees: shots along its face normal
        // drop drop_1, then (with drop_1 down) drop_2, then drop_3.
        let n = (290.0f32.to_radians().cos(), 290.0f32.to_radians().sin());
        let start = (262.0 + 70.0 * n.0, 425.0 + 70.0 * n.1);
        let v = [-1.5 * n.0, -1.5 * n.1, 0.0];
        for (i, name) in ["drop_1_sw", "drop_2_sw", "drop_3_sw"].iter().enumerate() {
            let (a, b) = steel(&mut s, start, v, 120);
            assert_eq!(delta(&a, &b, name), 1, "{name} dropped by shot {i}");
            assert_eq!(s.switch_closed(name), Some(true), "{name} down");
        }
        // The bank is complete: the rules reset it a second later.
        s.advance(1300);
        for name in ["drop_1_sw", "drop_2_sw", "drop_3_sw"] {
            assert_eq!(s.switch_closed(name), Some(false), "{name} reset");
        }
        healthy(&s);
    });
}

#[test]
fn lock_scoop_captures_and_ejects_back_into_play() {
    on_big_stack(|| {
        let mut s = load_in_game();
        let before = hits(&s);
        let h = s
            .spawn_ball("standard_steel", mm(257.0, 560.0), [0.0, 0.6, 0.0])
            .unwrap();
        s.advance(300);
        let a = hits(&s);
        assert_eq!(delta(&before, &a, "lock_lane"), 1, "approach opto");
        assert_eq!(delta(&before, &a, "lock_scoop_sw"), 1, "captured");
        assert_eq!(s.switch_closed("lock_scoop_sw"), Some(true), "held");
        let held = s.ball(h).unwrap().position_m;
        // The rules eject it after 1 s.
        s.advance(1000);
        assert_eq!(s.switch_closed("lock_scoop_sw"), Some(false), "ejected");
        // Back in play on the playfield layer: it gets well clear of the
        // scoop (up the table, into the bumpers).
        let mut furthest = 0.0f32;
        for _ in 0..300 {
            s.advance(1);
            let b = s.ball(h).expect("ball still in play");
            assert_eq!(b.layer, 0);
            let d =
                ((b.position_m[0] - held[0]).powi(2) + (b.position_m[1] - held[1]).powi(2)).sqrt();
            furthest = furthest.max(d);
        }
        assert!(furthest > 0.06, "left the scoop: {furthest} m");
        healthy(&s);
    });
}

#[test]
fn magnet_pair_tells_steel_from_the_powerball() {
    on_big_stack(|| {
        let mut s = load_in_game();
        let (a, b) = trial(
            &mut s,
            "standard_steel",
            (492.0, 860.0),
            [0.0, 1.2, 0.0],
            150,
        );
        assert_eq!(
            (delta(&a, &b, "magnet_eddy"), delta(&a, &b, "magnet_opto")),
            (1, 1)
        );
        let (a, b) = trial(
            &mut s,
            "ceramic_powerball",
            (492.0, 860.0),
            [0.0, 1.2, 0.0],
            150,
        );
        assert_eq!(
            (delta(&a, &b, "magnet_eddy"), delta(&a, &b, "magnet_opto")),
            (0, 1)
        );
        healthy(&s);
    });
}

fn zipper_mm(s: &Simulation, name: &str) -> f32 {
    let id: FlipperId = s.ids().flipper(name).unwrap();
    s.flipper_set().base_offset_mm(id)
}

#[test]
fn zipper_closes_the_gap_to_below_a_ball() {
    on_big_stack(|| {
        let mut s = Simulation::load(apex_bundle(), SimConfig::default()).unwrap();
        s.advance(10);
        let tip_gap = |s: &Simulation| {
            let fs = s.flipper_set();
            let tip = |n: &str| {
                let id: FlipperId = s.ids().flipper(n).unwrap();
                let p = fs.pivot_m(id).unwrap();
                let d = fs.bat_direction(id).unwrap();
                p + d * 0.076
            };
            // Tip centres minus the two 6.5 mm tip radii, mm.
            tip("left_flipper").distance(tip("right_flipper")) * 1.0e3 - 13.0
        };
        let open = tip_gap(&s);
        assert!((open - 48.0).abs() < 0.1, "open tip gap {open} mm");
        // Firewall Save drives `set_zipper` from the rules; the physics path
        // is the same as the flipper set's command, exercised here directly
        // through a scripted Overdrive in apex_game.rs. Here: a ball between
        // the tips passes open and is held closed.
        let (a, b) = trial(
            &mut s,
            "standard_steel",
            (257.0, 180.0),
            [0.0, -0.4, 0.0],
            600,
        );
        assert!(
            delta(&a, &b, "trough_1") + delta(&a, &b, "trough_2") >= 1,
            "passes the open gap"
        );
        // Close via the rules (Overdrive by combo steps, inlane, action).
        press(&mut s, "start", 5);
        s.advance(1500);
        press(&mut s, "left_spinner", 1);
        for _ in 0..5 {
            press(&mut s, "right_orbit_entry", 1);
            s.advance(300);
        }
        press(&mut s, "left_inlane", 1);
        press(&mut s, "action", 1);
        s.advance(200);
        for f in ["left_flipper", "right_flipper"] {
            assert!(
                (zipper_mm(&s, f) - 12.0).abs() < 1e-3,
                "{f}: {}",
                zipper_mm(&s, f)
            );
        }
        let closed = tip_gap(&s);
        assert!((closed - 24.0).abs() < 0.1, "closed tip gap {closed} mm");
        assert!(closed < 27.0, "a 27 mm ball can no longer pass");
        let (a, b) = trial(
            &mut s,
            "standard_steel",
            (257.0, 180.0),
            [0.0, -0.4, 0.0],
            600,
        );
        assert_eq!(
            delta(&a, &b, "trough_1") + delta(&a, &b, "trough_2"),
            0,
            "held by the closed zipper"
        );
        healthy(&s);
    });
}
