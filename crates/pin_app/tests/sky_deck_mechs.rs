//! 6.2 T3: every Sky Deck mechanism closes its switch by physics (balls
//! placed with `spawn_ball_on_layer` / `set_ball_state` and run with
//! `advance`; switches are never forced here): the Newton ball, each drop
//! target and the bank reset, both slingshots, the loop and ramp shot
//! sequences, the flipper EOS switches, the shooter lane, the plunger tip
//! and the trough.

mod sky_deck_support;

use pin_sim::Simulation;
use sky_deck_support::{
    DECK_Z_MM, at, healthy, load, mm, on_big_stack, only_ball, press, start_game,
};

/// Spawns a ball on `layer`, runs up to `ticks` ticks (stops when it
/// drains), removes it if still there; returns the hits of `names` gained.
fn trial(
    s: &mut Simulation,
    layer: u8,
    p: [f32; 3],
    v: [f32; 3],
    ticks: u64,
    names: &[&str],
) -> Vec<u32> {
    let before: Vec<u32> = names.iter().map(|n| s.switch_hits(n).unwrap()).collect();
    let h = s
        .spawn_ball_on_layer("standard_steel", layer, p, v)
        .unwrap();
    for _ in 0..ticks {
        s.advance(1);
        if s.ball(h).is_none() {
            break;
        }
    }
    if s.ball(h).is_some() {
        s.remove_ball(h);
    }
    names
        .iter()
        .zip(before)
        .map(|(n, b)| s.switch_hits(n).unwrap() - b)
        .collect()
}

#[test]
fn newton_ball_slings_and_trough_close_by_physics() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        // The Newton ball ("Reactor Core" guard) at (330, 415): a ball
        // rolling up into it from below.
        for (x, speed) in [(330.0f32, 1.2f32), (325.0, 2.0), (335.0, 3.0)] {
            let got = trial(
                &mut s,
                0,
                mm(x, 345.0),
                [0.0, speed, 0.0],
                300,
                &["newton_sw"],
            );
            assert!(got[0] >= 1, "Newton ball from x {x} at {speed} m/s");
            s.advance(500);
        }
        // Slingshots (the rules fire the coil only in a game; the leaf
        // switch closes on contact either way).
        let got = trial(
            &mut s,
            0,
            mm(149.0, 324.0),
            [-0.8, -0.6, 0.0],
            300,
            &["sling_left_sw"],
        );
        assert!(got[0] >= 1, "left sling");
        let got = trial(
            &mut s,
            0,
            mm(326.0, 324.0),
            [0.8, -0.6, 0.0],
            300,
            &["sling_right_sw"],
        );
        assert!(got[0] >= 1, "right sling");
        // Trough: a ball dropped below the flippers (under the centre post)
        // drains.
        let got = trial(
            &mut s,
            0,
            mm(257.0, 80.0),
            [0.0; 3],
            2000,
            &["trough_1", "trough_2"],
        );
        assert!(got[0] + got[1] >= 1, "trough {got:?}");
        assert_eq!(s.balls_in_play(), 0);
        // Outlanes drain too: a ball dropped into each outlane.
        for x in [15.0f32, 455.0] {
            let before = s.counters().balls_drained;
            let h = s
                .spawn_ball("standard_steel", mm(x, 380.0), [0.0; 3])
                .unwrap();
            for _ in 0..4000 {
                s.advance(1);
                if s.ball(h).is_none() {
                    break;
                }
            }
            assert!(s.ball(h).is_none(), "outlane at x {x} drains");
            assert_eq!(s.counters().balls_drained, before + 1);
        }
        healthy(&s);
    });
}

#[test]
fn each_drop_target_drops_and_the_bank_resets() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        start_game(&mut s);
        // Face normal 290 degrees: approach from in front of each target.
        let n = 290f32.to_radians();
        let (nx, ny) = (n.cos(), n.sin());
        for (i, (x, y)) in [(392.6f32, 987.8f32), (418.0, 997.0), (443.4, 1006.2)]
            .into_iter()
            .enumerate()
        {
            let name = format!("drop_{}_sw", i + 1);
            let p = at(x + nx * 40.0, y + ny * 40.0, DECK_Z_MM);
            let got = trial(
                &mut s,
                1,
                p,
                [-nx * 1.2, -ny * 1.2, 0.0],
                200,
                &[name.as_str()],
            );
            assert_eq!(got[0], 1, "{name} dropped");
            assert_eq!(s.switch_closed(&name), Some(true), "{name} stays down");
        }
        // The bank is complete: the rules reset it (1.5 s), the switches open.
        s.advance(2500);
        for i in 1..=3 {
            let name = format!("drop_{i}_sw");
            assert_eq!(s.switch_closed(&name), Some(false), "{name} reset");
        }
        // A slow touch does not drop a target (threshold 0.5 m/s).
        let p = at(418.0 + nx * 20.0, 997.0 + ny * 20.0, DECK_Z_MM);
        let got = trial(
            &mut s,
            1,
            p,
            [-nx * 0.2, -ny * 0.2, 0.0],
            100,
            &["drop_2_sw"],
        );
        assert_eq!(got[0], 0, "slow touch");
        healthy(&s);
    });
}

#[test]
fn loop_and_ramp_shots_complete_their_sequences() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        // Loop: up the left leg from its lower end, round the top.
        for speed in [2.5f32, 3.0, 3.5] {
            let (dx, dy) = (-30.0f32, 35.0f32);
            let l = dx.hypot(dy);
            let before_top = s.switch_hits("loop_top").unwrap();
            let before_entry = s.switch_hits("loop_entry").unwrap();
            let h = s
                .spawn_ball(
                    "standard_steel",
                    mm(80.0, 621.0),
                    [dx / l * speed, dy / l * speed, 0.0],
                )
                .unwrap();
            let mut entry_at = None;
            let mut top_at = None;
            for t in 0..3000u32 {
                s.advance(1);
                if entry_at.is_none() && s.switch_hits("loop_entry").unwrap() > before_entry {
                    entry_at = Some(t);
                }
                if top_at.is_none() && s.switch_hits("loop_top").unwrap() > before_top {
                    top_at = Some(t);
                }
            }
            let (e, t) = (entry_at.expect("loop_entry"), top_at.expect("loop_top"));
            assert!(e < t && t - e < 3000, "{speed} m/s: entry {e}, top {t}");
            s.remove_ball(h);
        }
        // Ramp: up the entry lane onto the wire.
        for speed in [2.2f32, 2.8, 3.4] {
            let got = trial(
                &mut s,
                0,
                mm(175.0, 470.0),
                [0.0, speed, 0.0],
                2500,
                &["ramp_entry", "ramp_made"],
            );
            assert!(got[0] >= 1 && got[1] >= 1, "ramp at {speed} m/s: {got:?}");
        }
        healthy(&s);
    });
}

#[test]
fn flippers_shooter_lane_and_plunger_close_their_switches() {
    on_big_stack(|| {
        let mut s = load();
        s.advance(10);
        start_game(&mut s);
        assert_eq!(
            s.switch_closed("plunger_tip"),
            Some(true),
            "ball on the plunger"
        );
        for (button, flipper, eos) in [
            ("left_flipper", "lower_left", "lower_left_eos"),
            ("right_flipper", "lower_right", "lower_right_eos"),
            ("right_upper", "upper_right", "upper_right_eos"),
        ] {
            let before = s.switch_hits(eos).unwrap();
            s.set_switch(button, true).unwrap();
            s.advance(80);
            let (angle, _) = s.flipper(flipper).unwrap();
            assert!(angle.abs() > 0.7, "{flipper} raised: {angle}");
            s.release_switch(button).unwrap();
            s.advance(300);
            assert_eq!(s.switch_hits(eos).unwrap() - before, 1, "{eos}");
        }
        let before = s.switch_hits("shooter_lane").unwrap();
        press(&mut s, "launch", 400);
        s.advance(600);
        assert!(
            s.switch_hits("shooter_lane").unwrap() > before,
            "shooter lane"
        );
        let h = only_ball(&s);
        assert!(
            s.ball(h).unwrap().position_m[1] > 0.5,
            "launched up the table"
        );
        healthy(&s);
    });
}

/// Regression (Independent Tester's pre-restart report): every plunge rode
/// the loop, hit the left sling and was kicked across into the right
/// outlane (x about 456 mm), draining about 3.6 s after the launch with no
/// flip. With the Newton ball across that line, a plunged ball that is
/// never flipped rides the loop and does not enter either outlane channel
/// (x < 30 or x > 440 mm below the separators' tops) within 8 s of the
/// launch, over a seed sweep and a launch-hold sweep. (Unflipped, it may
/// still roll down between the flippers: that is play, not the defect.)
#[test]
fn an_unflipped_plunge_does_not_go_down_an_outlane() {
    on_big_stack(|| {
        for seed in [1u64, 2, 3, 4, 5] {
            for hold in [60u64, 300] {
                let mut s = sky_deck_support::load_seeded(seed);
                s.advance(10);
                start_game(&mut s);
                let loop_top = s.switch_hits("loop_top").unwrap();
                let h = only_ball(&s);
                s.set_switch("launch", true).unwrap();
                for t in 0..8000u64 {
                    if t == hold {
                        s.release_switch("launch").unwrap();
                    }
                    s.advance(1);
                    let Some(b) = s.ball(h) else { break };
                    let (x, y) = (b.position_m[0] * 1e3, b.position_m[1] * 1e3);
                    assert!(
                        !(b.layer == 0 && y < 380.0 && (x < 30.0 || (440.0..470.0).contains(&x))),
                        "seed {seed} hold {hold}: in an outlane at ({x:.0}, {y:.0}) mm, tick {t}"
                    );
                }
                assert!(
                    s.switch_hits("loop_top").unwrap() > loop_top,
                    "seed {seed} hold {hold}: the plunge rides the loop"
                );
                healthy(&s);
            }
        }
    });
}
