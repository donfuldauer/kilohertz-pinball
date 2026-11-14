//! Subphase 2.5 spec tests (Independent Tester): the injection / query API
//! (deliverable 2), load errors, bundle forms, and that the loaded table
//! really simulates (floor, walls, gravity, sensors, bumper, flippers, trough
//! drain, out-of-bounds removal).

#[path = "fixtures/spec_2_5/support.rs"]
mod support;

use pin_core::input::CabinetInput;
use pin_core::sim::SimStep;
use pin_core::snapshot::{FrameStateSnapshot, MAX_BALLS};
use pin_sim::{BallHandle, SimConfig, SimError, Simulation};
use support::*;

fn speed(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[test]
fn pinned_types_have_the_pinned_traits() {
    fn copy_eq_debug<T: Copy + Eq + std::fmt::Debug>() {}
    fn error<T: std::fmt::Debug + std::fmt::Display + std::error::Error>() {}
    fn sim_step<T: SimStep + Send>() {}
    copy_eq_debug::<BallHandle>();
    error::<SimError>();
    sim_step::<Simulation>();
    assert_eq!(SimConfig::default().seed, 0x4B485A);
}

#[test]
fn load_starts_at_tick_zero_with_no_balls() {
    on_sim_thread(|| {
        let sim = load();
        assert_eq!(sim.tick(), 0);
        assert_eq!(sim.balls_in_play(), 0);
        assert_eq!(sim.snapshot().ball_count, 0);
        // Two flippers are declared.
        assert_eq!(sim.snapshot().flipper_count, 2);
        // Declared and cabinet switches are known, open, and never hit.
        for name in [
            "lane_opto",
            "side_opto",
            "trough_1",
            "bumper_1_ring",
            "left_flipper",
            "right_flipper",
            "left_upper",
            "right_upper",
            "action",
            "start",
            "launch",
            "tilt",
        ] {
            assert_eq!(sim.switch_closed(name), Some(false), "{name}");
            assert_eq!(sim.switch_hits(name), Some(0), "{name}");
        }
        assert!(sim.switch_closed("lower_left_eos").is_some());
        assert!(sim.switch_hits("lower_right_eos").is_some());
    });
}

#[test]
fn unknown_names_are_errors_or_none() {
    on_sim_thread(|| {
        let mut sim = load();
        assert!(sim.set_switch("no_such_switch", true).is_err());
        assert_eq!(sim.switch_closed("no_such_switch"), None);
        assert_eq!(sim.switch_hits("no_such_switch"), None);
        assert_eq!(sim.switch_closed(""), None);
        let e = sim
            .spawn_ball("no_such_profile", [0.2, 0.5, R], [0.0; 3])
            .unwrap_err();
        assert!(!e.to_string().is_empty());
        assert!(
            sim.spawn_ball_on_layer("no_such_profile", 0, [0.2, 0.5, R], [0.0; 3])
                .is_err()
        );
        assert_eq!(sim.balls_in_play(), 0, "a failed spawn must not add a ball");
        // Players are numbered 1..=4 as in the script API.
        assert_eq!(sim.score(0), None);
        assert_eq!(sim.score(5), None);
        assert_eq!(sim.score(255), None);
    });
}

#[test]
fn spawn_injects_the_exact_state() {
    on_sim_thread(|| {
        let mut sim = load();
        let p = [0.1234567, 0.7654321, 0.05];
        let v = [0.25, -1.5, 0.125];
        let h = sim.spawn_ball("standard_steel", p, v).unwrap();
        assert_eq!(sim.balls_in_play(), 1);
        let b = sim.ball(h).expect("spawned ball is queryable before any tick");
        assert_eq!(b.position_m.map(f32::to_bits), p.map(f32::to_bits));
        assert_eq!(b.velocity_m_s.map(f32::to_bits), v.map(f32::to_bits));
        assert_eq!(b.layer, 0, "spawn_ball spawns on layer 0");
        assert_eq!(b.angular_velocity_rad_s, [0.0; 3]);

        let h2 = sim
            .spawn_ball_on_layer("ceramic_powerball", 1, [0.3, 0.3, 0.2], [0.0; 3])
            .unwrap();
        assert_ne!(h, h2);
        assert_eq!(sim.balls_in_play(), 2);
        let b2 = sim.ball(h2).unwrap();
        assert_eq!(b2.layer, 1);
        assert_eq!(b2.position_m, [0.3, 0.3, 0.2]);
        assert_ne!(b2.profile, b.profile, "profiles are distinct ids");
        // The first ball is untouched.
        assert_eq!(sim.ball(h).unwrap().position_m, p);
        assert_eq!(sim.tick(), 0, "spawning does not advance time");
    });
}

#[test]
fn arena_holds_max_balls_then_refuses() {
    on_sim_thread(|| {
        let mut sim = load();
        let mut handles = Vec::new();
        for i in 0..MAX_BALLS {
            let x = 0.06 + 0.05 * (i % 8) as f32;
            let y = 0.75 + 0.06 * (i / 8) as f32;
            let h = sim
                .spawn_ball("standard_steel", [x, y, R], [0.0; 3])
                .unwrap_or_else(|e| panic!("ball {i} of {MAX_BALLS} refused: {e}"));
            assert!(!handles.contains(&h), "handles are unique");
            handles.push(h);
        }
        assert_eq!(sim.balls_in_play(), MAX_BALLS);
        let e = sim.spawn_ball("standard_steel", [0.45, 0.9, R], [0.0; 3]);
        assert!(e.is_err(), "spawning into a full arena must be an error");
        assert_eq!(sim.balls_in_play(), MAX_BALLS);
        sim.advance(1);
        assert_eq!(sim.snapshot().ball_count as usize, MAX_BALLS);
    });
}

#[test]
fn advance_counts_ticks_and_publishes_the_snapshot() {
    on_sim_thread(|| {
        let mut sim = load();
        sim.advance(0);
        assert_eq!(sim.tick(), 0);
        sim.advance(1);
        assert_eq!(sim.tick(), 1);
        assert_eq!(sim.snapshot().tick, 1);
        sim.advance(249);
        assert_eq!(sim.tick(), 250);
        assert_eq!(sim.snapshot().tick, 250);

        let h = sim
            .spawn_ball("standard_steel", [0.2, 0.6, R], [0.3, 0.1, 0.0])
            .unwrap();
        sim.advance(10);
        let snap = sim.snapshot();
        assert_eq!(snap.ball_count, 1);
        assert_eq!(sim.balls_in_play(), 1);
        let b = sim.ball(h).unwrap();
        assert_eq!(snap.balls()[0], b, "ball() and the snapshot agree");
        assert_ne!(b.position_m, [0.2, 0.6, R], "the ball moved");
    });
}

#[test]
fn step_1ms_fills_the_callers_snapshot() {
    on_sim_thread(|| {
        let mut sim = load();
        let h = sim
            .spawn_ball("standard_steel", [0.2, 0.6, R], [0.0, 0.5, 0.0])
            .unwrap();
        let mut out = Box::new(FrameStateSnapshot::default());
        for tick in 1..=20u64 {
            sim.step_1ms(tick, &[], &mut out);
            assert_eq!(out.tick, tick);
            assert_eq!(sim.tick(), tick);
            assert_eq!(*out, *sim.snapshot(), "tick {tick}");
        }
        assert_eq!(out.balls()[0], sim.ball(h).unwrap());
        // advance() and step_1ms() are the same clock.
        sim.advance(5);
        sim.step_1ms(26, &[], &mut out);
        assert_eq!((sim.tick(), out.tick), (26, 26));
    });
}

#[test]
fn advance_equals_stepping_without_inputs() {
    on_sim_thread(|| {
        let mut a = load();
        let mut b = load();
        for s in [&mut a, &mut b] {
            s.spawn_ball("standard_steel", [0.2, 0.9, R], [0.4, -0.3, 0.0])
                .unwrap();
        }
        a.advance(700);
        let mut out = Box::new(FrameStateSnapshot::default());
        for tick in 1..=700u64 {
            b.step_1ms(tick, &[], &mut out);
        }
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(*a.snapshot(), *b.snapshot());
        // ... and splitting advance() changes nothing.
        let mut c = load();
        c.spawn_ball("standard_steel", [0.2, 0.9, R], [0.4, -0.3, 0.0])
            .unwrap();
        c.advance(1);
        c.advance(299);
        c.advance(400);
        assert_eq!(a.state_hash(), c.state_hash());
    });
}

#[test]
fn free_fall_matches_gravity_at_one_millisecond_ticks() {
    on_sim_thread(|| {
        let mut sim = load();
        // 55 mm up: inside the playfield volume (well under the glass).
        let h = sim
            .spawn_ball("standard_steel", [0.25, 0.5, 0.055], [0.0; 3])
            .unwrap();
        sim.advance(60); // 0.06 s
        let b = sim.ball(h).expect("a ball 55 mm above the playfield is in bounds");
        let pitch = 6.5f32.to_radians();
        let dz = 0.055 - b.position_m[2];
        let dy = 0.5 - b.position_m[1];
        let want_z = 0.5 * 9.81 * pitch.cos() * 0.0036;
        let want_y = 0.5 * 9.81 * pitch.sin() * 0.0036;
        assert!(
            (dz - want_z).abs() < 0.05 * want_z,
            "fell {dz} m along -z in 60 ticks, expected about {want_z}"
        );
        assert!(
            (dy - want_y).abs() < 0.10 * want_y,
            "drifted {dy} m down the table in 60 ticks, expected about {want_y}"
        );
        assert!((b.position_m[0] - 0.25).abs() < 1e-4);
    });
}

#[test]
fn ball_rolls_down_the_pitched_floor_and_stays_inside_the_walls() {
    on_sim_thread(|| {
        let mut sim = load();
        // Away from every mech: left third of the table.
        let h = sim
            .spawn_ball("standard_steel", [0.06, 0.95, R], [0.0; 3])
            .unwrap();
        sim.advance(400);
        let b = sim.ball(h).unwrap();
        assert!(
            b.position_m[1] < 0.95 - 0.02,
            "the ball must roll down the 6.5 deg pitch: y = {}",
            b.position_m[1]
        );
        assert!(
            (b.position_m[2] - R).abs() < 0.002,
            "the ball must stay on the floor (playfield.glb collider): z = {}",
            b.position_m[2]
        );
        assert!(b.velocity_m_s[1] < -0.05);
        assert!(
            speed(b.angular_velocity_rad_s) > 1.0,
            "a rolling ball spins: {:?}",
            b.angular_velocity_rad_s
        );

        // A fast ball thrown at the walls stays on the table.
        let mut sim = load();
        let h = sim
            .spawn_ball("standard_steel", [0.25, 0.9, R], [-3.0, 2.0, 0.0])
            .unwrap();
        for tick in 0..3000 {
            sim.advance(1);
            let Some(b) = sim.ball(h) else {
                // Draining through the trough is the only legitimate exit.
                assert!(sim.switch_hits("trough_1").unwrap() >= 1, "tick {tick}");
                break;
            };
            let p = b.position_m;
            assert!(p.iter().all(|c| c.is_finite()));
            assert!(
                (0.0..=0.514).contains(&p[0]) && (0.0..=1.070).contains(&p[1]),
                "ball escaped the perimeter wall at tick {tick}: {p:?}"
            );
            assert!(p[2] > 0.0 && p[2] < 0.10, "tick {tick}: {p:?}");
        }
    });
}

#[test]
fn opto_closes_while_a_ball_is_inside_and_counts_one_hit_per_pass() {
    on_sim_thread(|| {
        let mut sim = load();
        // side_opto: centre (90, 300) mm, radius 20 mm. Roll through it.
        let h = sim
            .spawn_ball("standard_steel", [0.09, 0.36, R], [0.0, -0.5, 0.0])
            .unwrap();
        let mut closed_ticks = 0u32;
        let mut seen_inside = false;
        for _ in 0..600 {
            sim.advance(1);
            let closed = sim.switch_closed("side_opto").unwrap();
            if let Some(b) = sim.ball(h) {
                let d = ((b.position_m[0] - 0.09).powi(2) + (b.position_m[1] - 0.30).powi(2))
                    .sqrt();
                // Well inside / well outside the 20 mm zone (2 mm margin for
                // the one-tick sampling).
                if d < 0.018 {
                    assert!(closed, "ball {d} m from the opto centre but switch open");
                    seen_inside = true;
                }
                if d > 0.022 {
                    assert!(!closed, "ball {d} m from the opto centre but switch closed");
                }
            }
            closed_ticks += u32::from(closed);
        }
        assert!(seen_inside, "the ball never crossed the opto");
        assert!(closed_ticks >= 20, "closed for {closed_ticks} ticks");
        assert_eq!(sim.switch_hits("side_opto"), Some(1));
        assert_eq!(sim.switch_closed("side_opto"), Some(false));
        // The Lua script scored the pass (+5).
        assert_eq!(sim.score(P_SCORE), Some(5));
    });
}

#[test]
fn set_switch_forces_state_and_counts_closed_edges() {
    on_sim_thread(|| {
        let mut sim = load();
        sim.advance(10);
        sim.set_switch("lane_opto", true).unwrap();
        assert_eq!(sim.switch_closed("lane_opto"), Some(true));
        assert_eq!(sim.switch_hits("lane_opto"), Some(1));
        // Forcing the same state again is not a new edge.
        sim.set_switch("lane_opto", true).unwrap();
        assert_eq!(sim.switch_hits("lane_opto"), Some(1));
        sim.advance(3);
        assert_eq!(sim.score(P_SCORE), Some(10), "the script scored the lane");
        sim.set_switch("lane_opto", false).unwrap();
        assert_eq!(sim.switch_closed("lane_opto"), Some(false));
        assert_eq!(sim.switch_hits("lane_opto"), Some(1), "opening is not a hit");
        sim.advance(3);
        sim.set_switch("lane_opto", true).unwrap();
        sim.advance(3);
        assert_eq!(sim.switch_hits("lane_opto"), Some(2));
        assert_eq!(sim.score(P_SCORE), Some(20));
        // Other switches are unaffected.
        assert_eq!(sim.switch_hits("side_opto"), Some(0));
        assert_eq!(sim.switch_closed("start"), Some(false));
    });
}

#[test]
fn score_is_what_the_script_last_reported() {
    on_sim_thread(|| {
        let mut sim = load();
        sim.advance(2);
        // rules.lua reports all four players on its first on_tick.
        assert_eq!(sim.score(P_SCORE), Some(0));
        assert_eq!(sim.score(P_LANE_AT), Some(-1));
        assert_eq!(sim.score(P_ACTION_AT), Some(-1));
        assert_eq!(sim.score(P_RNG), Some(0));
        sim.set_switch("side_opto", true).unwrap();
        sim.advance(2);
        assert_eq!(sim.score(P_SCORE), Some(5));
        sim.advance(500);
        assert_eq!(sim.score(P_SCORE), Some(5), "the host keeps the last report");
    });
}

#[test]
fn bumper_ring_scores_and_the_scripted_coil_kicks_the_ball() {
    on_sim_thread(|| {
        let mut sim = load();
        // Bumper at (257, 640) mm; approach from above at 1 m/s, slightly off axis.
        let h = sim
            .spawn_ball("standard_steel", [0.259, 0.74, R], [0.0, -1.0, 0.0])
            .unwrap();
        let mut hit_tick = None;
        let mut max_speed_after = 0.0f32;
        for t in 1..=400u64 {
            sim.advance(1);
            if hit_tick.is_none() && sim.switch_hits("bumper_1_ring").unwrap() > 0 {
                hit_tick = Some(t);
            }
            if let (Some(ht), Some(b)) = (hit_tick, sim.ball(h)) {
                if t <= ht + 40 {
                    max_speed_after = max_speed_after.max(speed(b.velocity_m_s));
                }
            }
        }
        let ht = hit_tick.expect("the ball never closed the bumper ring switch");
        assert!(ht < 200, "ring closed at tick {ht}");
        let s = sim.score(P_SCORE).unwrap();
        assert!(s >= 100, "the script scores 100 per ring hit: {s}");
        // A passive rebound of a 1.0-1.2 m/s ball off steel (e = 0.6) leaves at
        // < 0.8 m/s; the coil the script pulses on the ring edge kicks it to
        // the bumper window (>= 1.5 m/s, Appendix C).
        assert!(
            max_speed_after >= 1.5,
            "bumper coil pulsed by the script did not kick the ball: max speed {max_speed_after} m/s"
        );
    });
}

#[test]
fn flipper_buttons_drive_the_flippers() {
    on_sim_thread(|| {
        let mut sim = load();
        let mut out = Box::new(FrameStateSnapshot::default());
        for _ in 0..50 {
            step(&mut sim, &[], &mut out);
        }
        let rest: Vec<f32> = out.flippers().iter().map(|f| f.angle_rad).collect();
        assert_eq!(rest.len(), 2);
        let eos_rest = sim.switch_closed("lower_left_eos").unwrap();

        // Press the left button and hold it for 100 ticks.
        let t = sim.tick() + 1;
        step(&mut sim, &[button(t, CabinetInput::LeftFlipper, true)], &mut out);
        assert_eq!(sim.switch_closed("left_flipper"), Some(true));
        assert_eq!(sim.switch_hits("left_flipper"), Some(1));
        let mut moved_by = 0;
        for i in 1..=100 {
            step(&mut sim, &[], &mut out);
            let moved = out
                .flippers()
                .iter()
                .zip(&rest)
                .filter(|(f, r)| (f.angle_rad - **r).abs() > 0.01)
                .count();
            if moved > 0 && moved_by == 0 {
                moved_by = i;
            }
        }
        assert!(
            (1..=10).contains(&moved_by),
            "a flipper must start moving within 10 ticks of the button (moved at +{moved_by})"
        );
        let held: Vec<f32> = out.flippers().iter().map(|f| f.angle_rad).collect();
        let deltas: Vec<f32> = held.iter().zip(&rest).map(|(h, r)| (h - r).abs()).collect();
        let sweep = 50f32.to_radians();
        let full = deltas.iter().filter(|d| (**d - sweep).abs() < 0.06).count();
        let still = deltas.iter().filter(|d| **d < 1e-3).count();
        assert_eq!(
            (full, still),
            (1, 1),
            "exactly the left flipper is at full stroke (50 deg) when held: deltas {deltas:?} rad"
        );
        assert_ne!(
            sim.switch_closed("lower_left_eos").unwrap(),
            eos_rest,
            "the EOS switch changes state at end of stroke"
        );
        assert_eq!(
            sim.switch_closed("lower_right_eos"),
            Some(eos_rest),
            "the right flipper did not move"
        );

        // Release: it returns to rest.
        let t = sim.tick() + 1;
        step(&mut sim, &[button(t, CabinetInput::LeftFlipper, false)], &mut out);
        assert_eq!(sim.switch_closed("left_flipper"), Some(false));
        for _ in 0..400 {
            step(&mut sim, &[], &mut out);
        }
        for (f, r) in out.flippers().iter().zip(&rest) {
            assert!((f.angle_rad - r).abs() < 0.02, "flipper back at rest");
        }
        assert_eq!(sim.switch_closed("lower_left_eos"), Some(eos_rest));
        assert_eq!(sim.switch_hits("left_flipper"), Some(1));
        // The script saw the press (+1).
        assert_eq!(sim.score(P_SCORE), Some(1));
    });
}

#[test]
fn flipper_stroke_launches_a_resting_ball() {
    on_sim_thread(|| {
        let mut sim = load();
        // Left flipper: pivot (157, 150) mm, rest -30 deg, 76 mm long. Drop a
        // ball onto the middle of the bat and let it settle against it.
        let h = sim
            .spawn_ball("standard_steel", [0.195, 0.160, R], [0.0; 3])
            .unwrap();
        let mut out = Box::new(FrameStateSnapshot::default());
        let mut pressed = false;
        let mut max_vy = 0.0f32;
        for i in 0..400u64 {
            let t = sim.tick() + 1;
            if i == 60 {
                step(&mut sim, &[button(t, CabinetInput::LeftFlipper, true)], &mut out);
                pressed = true;
            } else {
                step(&mut sim, &[], &mut out);
            }
            if pressed {
                if let Some(b) = sim.ball(h) {
                    max_vy = max_vy.max(b.velocity_m_s[1]);
                }
            }
        }
        assert!(
            max_vy > 1.0,
            "a flip must throw a ball lying on the bat up the table: max vy {max_vy} m/s"
        );
    });
}

#[test]
fn trough_switch_drains_the_ball() {
    on_sim_thread(|| {
        let mut sim = load();
        sim.advance(5);
        // Straight down the centre gap between the flippers into trough_1
        // (centre (257, 40) mm, radius 30 mm).
        let h = sim
            .spawn_ball("standard_steel", [0.257, 0.12, R], [0.0, -0.5, 0.0])
            .unwrap();
        assert_eq!(sim.balls_in_play(), 1);
        let mut drained_at = None;
        for t in 1..=1000u64 {
            sim.advance(1);
            if sim.ball(h).is_none() {
                drained_at = Some(t);
                break;
            }
        }
        let t = drained_at.expect("a ball in the trough must drain (ball() -> None)");
        assert!(t < 500, "drained after {t} ticks");
        assert_eq!(sim.balls_in_play(), 0);
        assert_eq!(sim.switch_hits("trough_1"), Some(1), "drain is a trough hit");
        sim.advance(3);
        assert_eq!(sim.snapshot().ball_count, 0);
        // The script saw the trough edge (+1000): drain is detected by the
        // trough switch, the way the script sees it.
        assert_eq!(sim.score(P_SCORE), Some(1000));
        assert!(sim.ball(h).is_none(), "a drained handle stays dead");
    });
}

#[test]
fn ball_outside_the_playfield_bounds_is_removed() {
    on_sim_thread(|| {
        let mut sim = load();
        let inside = sim
            .spawn_ball("standard_steel", [0.06, 0.95, R], [0.0; 3])
            .unwrap();
        // Far outside the 514 x 1070 mm playfield and below the floor: it can
        // only fall away.
        let outside = sim
            .spawn_ball("standard_steel", [3.0, 3.0, -1.0], [0.0, 0.0, -1.0])
            .unwrap();
        assert_eq!(sim.balls_in_play(), 2);
        sim.advance(3000);
        assert!(
            sim.ball(outside).is_none(),
            "a ball that left the playfield bounds must be removed"
        );
        assert!(sim.ball(inside).is_some());
        assert_eq!(sim.balls_in_play(), 1);
        assert_eq!(
            sim.switch_hits("trough_1"),
            Some(0),
            "leaving the bounds is not a trough drain"
        );
    });
}

#[test]
fn zip_bundle_loads_and_simulates_like_the_directory() {
    on_sim_thread(|| {
        let dir = temp_dir("zip");
        let zip = dir.join("spec_2_5.zip");
        zip_dir(&bundle(), &zip);
        let mut a = load_at(&zip, 7);
        let mut b = load_at(&bundle(), 7);
        for s in [&mut a, &mut b] {
            s.spawn_ball("standard_steel", [0.259, 0.9, R], [0.0, -0.5, 0.0])
                .unwrap();
            s.set_switch("start", true).unwrap();
            s.advance(1500);
        }
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(a.score(P_SCORE), b.score(P_SCORE));
        // table_hash is stable per bundle form.
        assert_eq!(a.table_hash(), load_at(&zip, 99).table_hash());
    });
}

#[test]
fn load_errors_carry_the_cause() {
    on_sim_thread(|| {
        // Missing bundle.
        let missing = temp_dir("missing").join("nope");
        let e = Simulation::load(&missing, SimConfig::default())
            .err()
            .expect("loading a missing bundle must fail");
        assert!(!e.to_string().is_empty());
        let _: &dyn std::error::Error = &e;

        // Manifest error: the AssetError text names the unknown key.
        let dir = bundle_copy("badtoml");
        let toml = std::fs::read_to_string(dir.join("table.toml")).unwrap();
        std::fs::write(
            dir.join("table.toml"),
            toml.replace("pitch_deg = 6.5", "pitch_deg = 6.5\nbogus_key_zzqq = 1"),
        )
        .unwrap();
        let e = Simulation::load(&dir, SimConfig::default())
            .err()
            .expect("an invalid table.toml must fail the load");
        assert!(
            e.to_string().contains("bogus_key_zzqq"),
            "SimError must carry the AssetError text: {e}"
        );

        // Script error: the ScriptError text carries Lua's message.
        let dir = bundle_with_script("badlua", "local x = = 1\n");
        let e = Simulation::load(&dir, SimConfig::default())
            .err()
            .expect("a script with a syntax error must fail the load");
        assert!(
            e.to_string().contains("unexpected symbol"),
            "SimError must carry the ScriptError text: {e}"
        );

        // Missing script file.
        let dir = bundle_copy("noscript");
        std::fs::remove_file(dir.join("rules.lua")).unwrap();
        assert!(Simulation::load(&dir, SimConfig::default()).is_err());

        // Missing playfield mesh.
        let dir = bundle_copy("noglb");
        std::fs::remove_file(dir.join("playfield.glb")).unwrap();
        assert!(Simulation::load(&dir, SimConfig::default()).is_err());
    });
}

#[test]
fn state_hash_covers_the_pinned_state() {
    on_sim_thread(|| {
        let base = load();
        let h0 = base.state_hash();
        assert_eq!(h0, load().state_hash(), "same load, same hash");
        assert_eq!(base.state_hash(), h0, "state_hash() is a pure query");

        // RNG state: another seed, another hash.
        assert_ne!(load_at(&bundle(), 1).state_hash(), h0);

        // Tick.
        let mut s = load();
        s.advance(1);
        let h1 = s.state_hash();
        assert_ne!(h1, h0);
        s.advance(1);
        assert_ne!(s.state_hash(), h1);

        // Balls: presence, and each exact f32 bit of position and velocity.
        let hash_with = |p: [f32; 3], v: [f32; 3], profile: &str, layer: u8| {
            let mut s = load();
            s.spawn_ball_on_layer(profile, layer, p, v).unwrap();
            s.state_hash()
        };
        let p = [0.2f32, 0.5, 0.05];
        let v = [0.1f32, 0.2, 0.3];
        let hb = hash_with(p, v, "standard_steel", 0);
        assert_ne!(hb, h0);
        assert_eq!(hb, hash_with(p, v, "standard_steel", 0));
        for axis in 0..3 {
            let mut p2 = p;
            p2[axis] = f32::from_bits(p2[axis].to_bits() + 1);
            assert_ne!(hash_with(p2, v, "standard_steel", 0), hb, "position[{axis}] ulp");
            let mut v2 = v;
            v2[axis] = f32::from_bits(v2[axis].to_bits() + 1);
            assert_ne!(hash_with(p, v2, "standard_steel", 0), hb, "velocity[{axis}] ulp");
        }
        assert_ne!(hash_with(p, v, "ceramic_powerball", 0), hb, "profile");
        assert_ne!(hash_with(p, v, "standard_steel", 1), hb, "layer");

        // Switch bitset and hit counts.
        let mut s = load();
        s.set_switch("lane_opto", true).unwrap();
        let closed = s.state_hash();
        assert_ne!(closed, h0, "switch bitset / hit count");
        s.set_switch("lane_opto", false).unwrap();
        assert_ne!(s.state_hash(), h0, "the hit count stays in the hash");
        assert_ne!(s.state_hash(), closed);

        // Scores: two runs that differ only in what the script reported.
        let quiet = "function on_tick() end\n";
        let loud = "local done = false\nfunction on_tick()\n  if not done then pinball.set_score(1, 42) done = true end\nend\n";
        let mut a = load_at(&bundle_with_script("hash_quiet", quiet), DEFAULT_SEED);
        let mut b = load_at(&bundle_with_script("hash_loud", loud), DEFAULT_SEED);
        a.advance(3);
        b.advance(3);
        assert_eq!(b.score(1), Some(42));
        assert_ne!(a.state_hash(), b.state_hash(), "reported scores are hashed");
    });
}

#[test]
fn table_hash_identifies_the_bundle_bytes() {
    on_sim_thread(|| {
        let a = load();
        assert_eq!(a.table_hash(), load_at(&bundle(), 5).table_hash());
        let copy = bundle_copy("same");
        assert_eq!(
            a.table_hash(),
            load_at(&copy, DEFAULT_SEED).table_hash(),
            "the hash depends on the bundle's bytes, not on its path"
        );
        // One more byte in the script: another table.
        let mut lua = std::fs::read_to_string(copy.join("rules.lua")).unwrap();
        lua.push_str("\n-- edited\n");
        std::fs::write(copy.join("rules.lua"), lua).unwrap();
        assert_ne!(a.table_hash(), load_at(&copy, DEFAULT_SEED).table_hash());
        // A changed manifest value: another table.
        let copy = bundle_copy("pitch");
        let toml = std::fs::read_to_string(copy.join("table.toml")).unwrap();
        std::fs::write(
            copy.join("table.toml"),
            toml.replace("pitch_deg = 6.5", "pitch_deg = 6.0"),
        )
        .unwrap();
        assert_ne!(a.table_hash(), load_at(&copy, DEFAULT_SEED).table_hash());
        // table_hash does not change while simulating.
        let mut s = load();
        s.advance(100);
        assert_eq!(s.table_hash(), a.table_hash());
    });
}
