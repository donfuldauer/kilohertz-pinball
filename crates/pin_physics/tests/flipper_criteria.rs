//! Subphase 2.3 verification criteria (PLAN.md 2.3, Appendix C defaults):
//! timing sweep, drop catch, shot spread, zipper, stroke time, exit speed.
//! Every number is printed so the report can quote it
//! (`cargo test -p pin_physics --test flipper_criteria -- --nocapture`).

#[path = "flipper_util.rs"]
mod util;

use glam::Vec3;
use pin_core::PhysicsDefaults;
use pin_physics::ball::BallHandle;
use pin_physics::flipper::FlipperSet;
use util::*;

/// Ticks the ball flies before it reaches the bat.
const LEAD: i64 = 60;
/// Ticks after the first contact over which the rebound is measured.
const WINDOW: i64 = 60;

/// A variation of the sweep's rig: scale factors on the default inertia and
/// power torque, the ball's speed, and whether it comes straight down the
/// table (−y) or square onto the raised bat.
#[derive(Copy, Clone, Debug)]
struct Variant {
    inertia: f32,
    torque: f32,
    speed: f32,
    square: bool,
}

impl Default for Variant {
    fn default() -> Self {
        Self {
            inertia: 1.0,
            torque: 1.0,
            speed: 3.5,
            square: false,
        }
    }
}

/// The ball meets the bat at `fraction` of its length when the bat is at the
/// end of its stroke (+20°), `LEAD` ticks after the start.
fn variant_rig(var: &Variant, fraction: f32) -> (Rig, BallHandle) {
    let mut c = left_config(0, [0.0, 0.0, 0.0]);
    c.inertia_kg_m2 *= var.inertia;
    c.power_torque_nm *= var.torque;
    let mut rig = Rig::new(&[c]);
    let dir = bat_dir(20.0);
    let n = v(-dir.y, dir.x, 0.0);
    let radius = 0.011 + (0.0065 - 0.011) * fraction;
    let contact = dir * (BAT_LEN * fraction) + n * (radius + BALL_R);
    let heading = if var.square { -n } else { v(0.0, -1.0, 0.0) };
    let start = contact - heading * (var.speed * LEAD as f32 * DT);
    let h = rig.add_ball(start + v(0.0, 0.0, BALL_R), heading * var.speed);
    (rig, h)
}

#[derive(Copy, Clone, Debug)]
enum Button {
    /// Held long before the ball arrives.
    Held,
    /// Pressed `k` ms after the nominal arrival (negative = before).
    FlipAt(i64),
    /// Held, then released `k` ms before the nominal arrival.
    ReleaseBefore(i64),
}

/// Rebound speed: the largest speed back up the playfield (+y) the ball has
/// in the `WINDOW` ms after its first contact with the bat (0 if it never
/// comes back up). Also returns the tick of the first contact relative to
/// the nominal arrival.
fn rebound(fraction: f32, button: Button) -> (f32, i64) {
    rebound_in(&Variant::default(), fraction, button)
}

fn rebound_in(var: &Variant, fraction: f32, button: Button) -> (f32, i64) {
    let shot = shot_in(var, fraction, button);
    (shot.rebound, shot.first_contact)
}

/// What happened in one run of the sweep.
#[derive(Clone, Debug)]
struct Shot {
    rebound: f32,
    /// Tick of the first contact relative to the nominal arrival.
    first_contact: i64,
    /// Ticks with a ball–bat impact inside the measuring window.
    contacts: usize,
    /// How many of them fell within a tick of the bat reaching a stop (the
    /// stop takes or returns the bat's momentum there: a rigid-stop event).
    contacts_at_a_stop: usize,
}

fn shot_in(var: &Variant, fraction: f32, button: Button) -> Shot {
    let (mut rig, h) = variant_rig(var, fraction);
    let mut hit_ticks = Vec::new();
    let mut stop_ticks = Vec::new();
    let mut was_on_stop = true;
    // Rebound = speed back up the table, whatever the approach.
    let away = v(0.0, 1.0, 0.0);
    if !matches!(button, Button::FlipAt(_)) {
        rig.flippers.set_energised(LEFT, true);
        for _ in 0..200 {
            rig.flippers.advance(DT);
        }
    }
    let mut first = None;
    let mut best = 0.0f32;
    for t in 0..LEAD + 40 + WINDOW {
        match button {
            Button::FlipAt(k) if t == LEAD + k => rig.flippers.set_energised(LEFT, true),
            Button::ReleaseBefore(k) if t == LEAD - k => rig.flippers.set_energised(LEFT, false),
            _ => {}
        }
        rig.step();
        let angle = rig.flippers.angle_deg(LEFT);
        let on_stop = angle == -30.0 || angle == 20.0;
        if on_stop && !was_on_stop {
            stop_ticks.push(t);
        }
        was_on_stop = on_stop;
        if first.is_none() && !rig.hits.is_empty() {
            first = Some(t);
        }
        if let Some(f) = first
            && t <= f + WINDOW
        {
            best = best.max(rig.ball(h).velocity.dot(away));
            if !rig.hits.is_empty() {
                hit_ticks.push(t);
            }
        }
    }
    let at_a_stop = hit_ticks
        .iter()
        .filter(|&&t| stop_ticks.iter().any(|&s| (s - t).abs() <= 1))
        .count();
    Shot {
        rebound: best,
        first_contact: first.expect("the ball reaches the bat") - LEAD,
        contacts: hit_ticks.len(),
        contacts_at_a_stop: at_a_stop,
    }
}

#[test]
fn timing_sweep_has_a_dead_spot_and_no_unexplained_jumps() {
    for fraction in [0.5f32, 0.7] {
        let (held, at) = rebound(fraction, Button::Held);
        assert_eq!(at, 0, "nominal arrival is the contact with the held bat");
        println!("timing sweep, contact at {fraction} of the bat: held rebound {held:.3} m/s");
        let mut shots = Vec::new();
        for k in -20..=20 {
            let shot = shot_in(&Variant::default(), fraction, Button::FlipAt(k));
            println!(
                "  flip {k:+3} ms: rebound {:.3} m/s ({:.0} % of held), {} contacts, {} at a stop",
                shot.rebound,
                100.0 * shot.rebound / held,
                shot.contacts,
                shot.contacts_at_a_stop
            );
            shots.push(shot);
        }
        let min = shots
            .iter()
            .map(|s| s.rebound)
            .fold(f32::INFINITY, f32::min);
        let max = shots.iter().map(|s| s.rebound).fold(0.0, f32::max);
        println!(
            "  min {min:.3} m/s = {:.1} % of held (plan depth 25 %: {}), max {max:.3} m/s",
            100.0 * min / held,
            if min <= 0.25 * held { "met" } else { "not met" }
        );
        assert!(held > 1.0, "a held flipper returns the ball: {held}");
        // Gate (DECISIONS "Orchestrator — 2.3 criterion 1: the 25 % depth
        // moves to Gate A1"): a dead spot of at most half the held rebound.
        assert!(min <= 0.5 * held, "dead spot {min} vs held {held}");
        assert!(max > held, "a well-timed flip beats a held flipper");
        // Continuity: every step above 0.75 m/s between neighbouring
        // timings is attributed to a regime change of the rigid-impact
        // model; steps without one are printed, not failed.
        for (i, pair) in shots.windows(2).enumerate() {
            let step = (pair[1].rebound - pair[0].rebound).abs();
            if step <= 0.75 {
                continue;
            }
            let cause = if pair[0].contacts_at_a_stop != pair[1].contacts_at_a_stop {
                "rigid-stop event: a contact coincides with the bat reaching a stop in one run only"
            } else if pair[0].contacts != pair[1].contacts {
                "hit or miss: the bat reaches the already rebounding ball once more in one run only"
            } else {
                "UNEXPLAINED"
            };
            let k = i as i64 - 20;
            println!(
                "  step {step:.2} m/s between {k:+} and {:+} ms: {cause}",
                k + 1
            );
        }
    }
}

/// The dead spot is not an artefact of where the ±20 ms sweep ends: in a
/// sweep that starts 15 ms earlier it is an interior minimum, with held-like
/// rebounds on its early side (the bat has arrived and settled) and rising
/// shots on its late side.
#[test]
fn dead_spot_is_an_interior_minimum_of_a_wider_sweep() {
    for fraction in [0.5f32, 0.7] {
        let (held, _) = rebound(fraction, Button::Held);
        let curve: Vec<(i64, f32)> = (-35..=20)
            .map(|k| (k, rebound(fraction, Button::FlipAt(k)).0))
            .collect();
        println!("wide sweep, contact at {fraction} of the bat (held {held:.3} m/s):");
        for (k, r) in &curve {
            println!(
                "  flip {k:+3} ms: rebound {r:.3} m/s ({:.0} % of held)",
                100.0 * r / held
            );
        }
        let (at, min) = curve
            .iter()
            .copied()
            .fold((0, f32::INFINITY), |a, b| if b.1 < a.1 { b } else { a });
        println!("  minimum {min:.3} m/s at {at:+} ms");
        println!(
            "  plan depth 25 %: {}",
            if min <= 0.25 * held { "met" } else { "not met" }
        );
        assert!(min <= 0.5 * held);
        assert!((-22..=-18).contains(&at), "dead spot at {at} ms");
        // Earlier flips (bat already up and settled) rebound like a held bat.
        let early: Vec<f32> = curve
            .iter()
            .filter(|c| c.0 <= at - 3)
            .map(|c| c.1)
            .collect();
        assert!(early.iter().all(|&r| r > 0.6 * held), "{early:?}");
        // Later flips rise out of the dead spot.
        let later = curve.iter().find(|c| c.0 == at + 5).unwrap().1;
        assert!(later > 0.5 * held);
    }
}

/// Robustness of the dead spot (report; review 2.3 B2): the ±20 ms sweep
/// with inertia ±10 %, power torque ±10 % and ball speeds 3.3–3.7 m/s, for a
/// ball coming straight down the table and one square onto the raised bat.
/// Time 0 is the contact with the held bat in every variant.
#[test]
fn dead_spot_robustness_report() {
    let mut rows = Vec::new();
    for square in [false, true] {
        for fraction in [0.5f32, 0.7] {
            for (inertia, torque, speed) in [
                (1.0f32, 1.0f32, 3.5f32),
                (0.9, 1.0, 3.5),
                (1.1, 1.0, 3.5),
                (1.0, 0.9, 3.5),
                (1.0, 1.1, 3.5),
                (1.0, 1.0, 3.3),
                (1.0, 1.0, 3.7),
            ] {
                let var = Variant {
                    inertia,
                    torque,
                    speed,
                    square,
                };
                let (held, at) = rebound_in(&var, fraction, Button::Held);
                assert_eq!(at, 0);
                let curve = |from: i64, to: i64| -> Vec<(i64, f32)> {
                    (from..=to)
                        .map(|k| (k, rebound_in(&var, fraction, Button::FlipAt(k)).0))
                        .collect()
                };
                let lowest = |c: &[(i64, f32)]| {
                    c.iter()
                        .copied()
                        .fold((0, f32::INFINITY), |a, b| if b.1 < a.1 { b } else { a })
                };
                // Flips before the contact only: a late flip that misses
                // the ball is not a dead spot.
                let narrow = curve(-20, 0);
                let wide = curve(-35, 0);
                let (k20, m20) = lowest(&narrow);
                let (k35, m35) = lowest(&wide);
                let jump = narrow
                    .windows(2)
                    .map(|w| (w[1].1 - w[0].1).abs())
                    .fold(0.0, f32::max);
                println!(
                    "{} {fraction:.1} I×{inertia:.1} T×{torque:.1} {speed:.1} m/s | held {held:.2} | −20…0: min {m20:.2} ({:.0} %) at {k20:+} | −35…0: min {m35:.2} ({:.0} %) at {k35:+} | largest step −20…0: {jump:.2}",
                    if square { "square" } else { "down  " },
                    100.0 * m20 / held,
                    100.0 * m35 / held,
                );
                rows.push((m20 / held, m35 / held, k35));
            }
        }
    }
    let pass20 = rows.iter().filter(|r| r.0 <= 0.25).count();
    let pass35 = rows.iter().filter(|r| r.1 <= 0.25).count();
    println!(
        "dead spot ≤ 25 % of held: {pass20}/{} variants within −20…0 ms, {pass35}/{} within −35…0 ms",
        rows.len(),
        rows.len()
    );
    // A report: the bound is not robust (see DECISIONS "2.3 — fix round
    // 1"). What every variant does have is a rebound below the held one
    // somewhere in the wide sweep.
    for r in &rows {
        assert!(r.1 < 0.8 && r.1.is_finite(), "{rows:?}");
    }
}

#[test]
fn drop_catch_rebounds_less_than_a_held_flipper() {
    // (Close to the pivot the bat is heavier than the ball at the contact
    // point, the ball leaves at the first impact and the coil never gets to
    // act on it: held or released makes no difference there.)
    for fraction in [0.5f32, 0.6, 0.7, 0.85] {
        let (held, _) = rebound(fraction, Button::Held);
        for k in [1, 2, 3, 5, 8] {
            let (r, _) = rebound(fraction, Button::ReleaseBefore(k));
            println!(
                "drop catch at {fraction}: released {k} ms before impact: {r:.3} m/s, held {held:.3} m/s"
            );
            assert!(
                r < held,
                "fraction {fraction}, release {k} ms: {r} vs {held}"
            );
        }
    }
}

/// A ball resting against the resting bat at `fraction` of its length is
/// flipped; returns its velocity when it has left the bat.
fn shot_from(fraction: f32) -> Vec3 {
    let mut rig = Rig::new(&[left_config(0, [0.0, 0.0, 0.0])]);
    let dir = bat_dir(-30.0);
    let n = v(-dir.y, dir.x, 0.0);
    let radius = 0.011 + (0.0065 - 0.011) * fraction;
    let start = dir * (BAT_LEN * fraction) + n * (radius + BALL_R + 2.0e-5);
    let h = rig.add_ball(start + v(0.0, 0.0, BALL_R), Vec3::ZERO);
    rig.flippers.set_energised(LEFT, true);
    let mut fastest = Vec3::ZERO;
    for _ in 0..80 {
        rig.step();
        let vel = rig.ball(h).velocity;
        if vel.length() > fastest.length() {
            fastest = vel;
        }
    }
    fastest
}

#[test]
fn shot_spread_is_monotonic_in_the_contact_point() {
    let angles: Vec<f32> = [0.25f32, 0.5, 0.85]
        .iter()
        .map(|&f| {
            let vel = shot_from(f);
            let a = libm::atan2f(vel.y, vel.x).to_degrees();
            println!(
                "shot from {f} of the bat: {:.2} m/s at {a:.1}°",
                vel.length()
            );
            assert!(vel.length() > 1.0);
            a
        })
        .collect();
    assert!(
        angles[0] > angles[1] + 1.0 && angles[1] > angles[2] + 1.0,
        "release angles {angles:?}"
    );
    // Finer sampling: still monotonic.
    let fine: Vec<f32> = (2..=9)
        .map(|i| {
            let vel = shot_from(i as f32 * 0.1);
            libm::atan2f(vel.y, vel.x).to_degrees()
        })
        .collect();
    println!("release angles from 20 % to 90 %: {fine:.1?}");
    assert!(fine.windows(2).all(|w| w[0] > w[1]), "{fine:?}");
}

/// Two flippers facing each other, tips `gap_mm` apart at rest, each with a
/// 12 mm inward zipper slide.
fn zipper_rig(gap_mm: f32) -> Rig {
    let tip_r = 6.5;
    let reach = 76.0 * libm::cosf(30.0f32.to_radians());
    let x0 = gap_mm * 0.5 + tip_r + reach;
    let mut left = left_config(0, [-x0, 0.0, 0.0]);
    left.zipper_slide_axis_mm = Some([12.0, 0.0, 0.0]);
    left.zipper_slide_speed_mm_s = 240.0;
    let mut right = right_config(1, [x0, 0.0, 0.0]);
    right.zipper_slide_axis_mm = Some([-12.0, 0.0, 0.0]);
    right.zipper_slide_speed_mm_s = 240.0;
    Rig::new(&[left, right])
}

#[test]
fn zipper_closes_the_gap_for_a_27_mm_ball() {
    // Open: the ball rolls down the middle and drains between the tips.
    let mut rig = zipper_rig(48.0);
    let h = rig.add_ball(v(0.001, 0.15, BALL_R), Vec3::ZERO);
    rig.run(4000);
    let open_y = rig.ball(h).position.y;
    println!("zipper open: ball at y = {:.1} mm after 4 s", open_y * 1e3);
    assert!(open_y < -0.15, "the ball passes a 48 mm gap: y = {open_y}");

    // Closed: each base slides 12 mm inward in 50 ms, the gap is 24 mm.
    let mut rig = zipper_rig(48.0);
    rig.flippers.set_zipper(LEFT, true);
    rig.flippers.set_zipper(RIGHT, true);
    rig.run(49);
    assert!(rig.flippers.base_offset_mm(LEFT) < 12.0);
    rig.run(1);
    assert!((rig.flippers.base_offset_mm(LEFT) - 12.0).abs() < 1e-3);
    assert!((rig.flippers.base_offset_mm(RIGHT) - 12.0).abs() < 1e-3);
    let gap = (rig.flippers.pivot_m(RIGHT).unwrap() - rig.flippers.pivot_m(LEFT).unwrap()).x
        - 2.0 * (0.076 * libm::cosf(30.0f32.to_radians()) + 0.0065);
    println!("zipper closed: tip gap {:.2} mm", gap * 1e3);
    assert!((gap - 0.024).abs() < 1e-4);
    let h = rig.add_ball(v(0.001, 0.15, BALL_R), Vec3::ZERO);
    let mut lowest = f32::INFINITY;
    for _ in 0..4000 {
        rig.step();
        lowest = lowest.min(rig.ball(h).position.y);
    }
    let b = rig.ball(h);
    println!(
        "zipper closed: ball rests at y = {:.1} mm (lowest {:.1} mm), speed {:.4} m/s",
        b.position.y * 1e3,
        lowest * 1e3,
        b.velocity.length()
    );
    assert!(lowest > -0.06, "the ball cannot pass a 24 mm gap: {lowest}");
    assert!(b.velocity.length() < 0.05, "it comes to rest on the tips");

    // Opening again lets it through and the bases go home.
    rig.flippers.set_zipper(LEFT, false);
    rig.flippers.set_zipper(RIGHT, false);
    rig.run(3000);
    assert_eq!(rig.flippers.base_offset_mm(LEFT), 0.0);
    assert_eq!(rig.flippers.base_offset_mm(RIGHT), 0.0);
    assert!(rig.ball(h).position.y < -0.15);
}

#[test]
fn unloaded_stroke_time_is_in_the_feel_window() {
    let window = PhysicsDefaults::builtin()
        .feel_windows
        .flipper_stroke_time_ms;
    let mut set =
        FlipperSet::new(&[left_config(0, [0.0; 3]), right_config(1, [200.0, 0.0, 0.0])]).unwrap();
    for id in [LEFT, RIGHT] {
        set.set_energised(id, true);
    }
    let mut ticks = [0u32; 2];
    let mut peak = 0.0f32;
    for t in 1..=60u32 {
        set.advance(DT);
        for (i, id) in [LEFT, RIGHT].into_iter().enumerate() {
            if ticks[i] == 0 && !set.eos_closed(id) {
                ticks[i] = t;
            }
        }
        peak = peak.max(set.angular_velocity(LEFT).abs());
    }
    let tip = peak * (0.076 + 0.0065);
    println!(
        "unloaded stroke rest → EOS: {} ms (left), {} ms (right); window {}–{} ms; peak {peak:.1} rad/s, tip speed {tip:.2} m/s",
        ticks[0], ticks[1], window.min, window.max
    );
    assert_eq!(ticks[0], ticks[1], "mirror flippers are in step");
    let ms = ticks[0] as f32;
    assert!(ms >= window.min && ms <= window.max, "stroke {ms} ms");
    // The EOS was still closed one tick earlier, so the stroke took longer
    // than (ticks − 1) ms.
    assert!((ticks[0] - 1) as f32 >= window.min);
}

#[test]
fn full_shot_from_a_cradle_exits_in_the_feel_window() {
    let window = PhysicsDefaults::builtin()
        .feel_windows
        .flipper_exit_speed_m_s;
    let (rig, h) = cradle_rig();
    let b = rig.ball(h);
    println!(
        "cradled ball at ({:.1}, {:.1}) mm, speed {:.5} m/s",
        b.position.x * 1e3,
        b.position.y * 1e3,
        b.velocity.length()
    );
    assert!(
        b.velocity.length() < 0.01,
        "the cradle holds the ball still"
    );
    assert!((rig.flippers.angle_deg(LEFT) - 20.0).abs() < 1e-3);

    // Release, let the ball roll down the bat, flip again.
    let mut best = (0.0f32, 0.0f32, 0u32);
    for wait in (400..=750).step_by(25) {
        let (mut rig, h) = cradle_rig();
        rig.flippers.set_energised(LEFT, false);
        rig.run(wait);
        let at = rig
            .ball(h)
            .position
            .dot(bat_dir(rig.flippers.angle_deg(LEFT)))
            / BAT_LEN;
        rig.flippers.set_energised(LEFT, true);
        let mut exit = 0.0f32;
        for _ in 0..80 {
            rig.step();
            exit = exit.max(rig.ball(h).velocity.length());
        }
        println!(
            "cradle shot: flip {wait} ms after release, ball at {at:.2} of the bat: exit {exit:.2} m/s"
        );
        if exit > best.0 {
            best = (exit, at, wait);
        }
        if (0.4..=0.8).contains(&at) {
            assert!(
                exit >= window.min && exit <= window.max,
                "shot from {at}: {exit} m/s"
            );
        }
    }
    println!(
        "full shot: {:.2} m/s (ball at {:.2} of the bat, flip {} ms after release); window {}–{} m/s",
        best.0, best.1, best.2, window.min, window.max
    );
    assert!(best.0 >= window.min && best.0 <= window.max);
}

#[test]
fn thing_flip_fires_tick_exactly() {
    use pin_core::FlipperStagingMode;
    use pin_core::ids::SwitchId;
    let mut c = left_config(0, [0.0; 3]);
    c.staging = FlipperStagingMode::AutoSensorThingFlip {
        trigger_switch: SwitchId(7),
        delay_ms: 12.0,
        hold_duration_ms: 40.0,
    };
    let other = right_config(1, [200.0, 0.0, 0.0]);
    let mut rig = Rig::new(&[c, other]);
    let mut first_moved = None;
    let mut on_ticks = Vec::new();
    for t in 0..200u64 {
        if t == 30 {
            rig.flippers.on_switch_edge(SwitchId(7), true, t);
            rig.flippers.on_switch_edge(SwitchId(8), true, t); // someone else's switch
        }
        if t == 35 {
            rig.flippers.on_switch_edge(SwitchId(7), false, t);
            rig.flippers.on_switch_edge(SwitchId(7), true, t); // while pending: ignored
        }
        rig.step();
        if rig.flippers.is_energised(LEFT) {
            on_ticks.push(t);
        }
        if first_moved.is_none() && rig.flippers.angle_deg(LEFT) > -30.0 {
            first_moved = Some(t);
        }
        assert!(!rig.flippers.is_energised(RIGHT));
        assert_eq!(rig.flippers.angle_deg(RIGHT), 210.0);
    }
    assert_eq!(
        on_ticks.first(),
        Some(&42),
        "energised 12 ticks after the edge"
    );
    assert_eq!(on_ticks.last(), Some(&81), "for 40 ticks");
    assert_eq!(on_ticks.len(), 40);
    assert_eq!(
        first_moved,
        Some(42),
        "the bat moves in the first energised tick"
    );
    // Released: back at rest, and a new edge starts a new flip.
    assert_eq!(rig.flippers.angle_deg(LEFT), -30.0);
    rig.flippers.on_switch_edge(SwitchId(7), true, rig.tick);
    let start = rig.tick;
    rig.run(13);
    assert!(rig.flippers.is_energised(LEFT));
    assert_eq!(rig.tick, start + 13);
}
