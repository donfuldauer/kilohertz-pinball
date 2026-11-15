//! Gate A1 on the grey-box bundle: the provisional feel windows of PLAN
//! Appendix C, measured on the bundle's own flippers, bumpers and slings in
//! the headless [`Simulation`] (the windows are read from the engine's
//! `physics_defaults.toml`, `PhysicsDefaults::builtin().feel_windows`):
//!
//! * (a) unloaded flipper stroke time, rest to EOS, 20–35 ms, for
//!   `lower_left`, `lower_right` and `upper_left`;
//! * (b) exit speed of a full shot from a cradled standard ball, 3–7 m/s, for
//!   both lower flippers;
//! * (c) bumper kick exit speed, 1.5–4 m/s, for the three bumpers, each
//!   approached at 1, 2 and 3 m/s from three directions;
//! * (d) slingshot kick exit speed, 1.5–4 m/s, for both slings, likewise.
//!
//! The bumper and sling coils are fired by the bundle's own `rules.lua` (a
//! game is started so that its switch handlers run). The window is
//! guaranteed for a coil fired on the tick after the switch edge (DECISIONS
//! 2.4: edges collected in tick N reach the script in tick N+1, whose coil
//! commands apply in that tick); every trial checks that the coil came on
//! in exactly that tick.
//!
//! Definitions (as in the 2.4 rig tests, crates/pin_physics/tests):
//!
//! * Stroke time: the button closes before tick 1; tick 1 is the first
//!   tick of motion; the stroke time is the index of the first tick at
//!   whose end the flipper's EOS switch has changed state (the grey-box EOS
//!   switches are closed from rest up to `eos_angle_fraction` of the sweep
//!   and open beyond, like `FlipperSet::eos_closed`), in 1 ms ticks.
//! * Full shot: the flipper is held up, a ball is put just above the
//!   raised bat near the pivot and settles in the cradle (the low end of the
//!   held bat, against the pivot cap and the inlane); the button is released,
//!   the ball rolls down the lowered bat, and the flipper is fired again
//!   after a delay swept from 300 to 900 ms (the 2.4 rig procedure,
//!   `full_shot_from_a_cradle_exits_in_the_feel_window`). Exit speed is the
//!   ball speed at the first tick its centre is 100 mm from the raised
//!   tip. Shots that hit the ball between 0.4 and 0.8 of the bat length (the
//!   rig's "full shot" band) must be in the window, and so must the best.
//! * Bumper: horizontal ball speed 7 ticks after the ring-switch edge
//!   (6 after the coil fired; the 2.4 bumper rig).
//! * Sling: over the 40 ticks after the switch edge, the horizontal velocity
//!   with the largest component along the kick direction; its magnitude is
//!   the exit speed (the 2.4 sling rig, which uses 80 ticks; 40 keeps the
//!   ball clear of the rest of the table, and the kick ends within 25 ms).
//!
//! Every measured number is printed (`--nocapture`). Simulated time only
//! (rule 5), deterministic (D9).

mod support;

use pin_core::schema::table::{FlipperDef, FlipperShapeDef, MechDef, MechKind};
use pin_core::{CoilId, PhysicsDefaults, TableManifest};
use pin_sim::{SimConfig, Simulation};
use support::{greybox_bundle, on_big_stack};

/// Ball radius of the standard steel ball, mm.
const BALL_R_MM: f32 = 13.5;

fn mm3(x: f32, y: f32) -> [f32; 3] {
    [x * 1.0e-3, y * 1.0e-3, BALL_R_MM * 1.0e-3]
}

fn load() -> Simulation {
    let mut s = Simulation::load(greybox_bundle(), SimConfig::default()).unwrap();
    s.advance(10);
    s
}

/// The bundle's merged `table.toml` (positions and angles of the flippers
/// and mechs as built).
fn manifest() -> TableManifest {
    let text = std::fs::read_to_string(greybox_bundle().join("table.toml")).unwrap();
    TableManifest::from_toml_str(&text).unwrap()
}

fn flipper_def<'a>(t: &'a TableManifest, id: &str) -> &'a FlipperDef {
    t.flippers.iter().find(|f| f.id == id).expect(id)
}

fn mech_def<'a>(t: &'a TableManifest, id: &str) -> &'a MechDef {
    t.mechs.iter().find(|m| m.id == id).expect(id)
}

fn bat_length_mm(f: &FlipperDef) -> f32 {
    match f.shape {
        FlipperShapeDef::StraightCapsule { length, .. }
        | FlipperShapeDef::CurvedBanana { length, .. } => length,
    }
}

fn dir_deg(a: f32) -> (f32, f32) {
    let r = a.to_radians();
    (r.cos(), r.sin())
}

fn horiz_speed(v: [f32; 3]) -> f32 {
    v[0].hypot(v[1])
}

/// Starts a game (the rules fire bumper and sling coils only in a game):
/// START, then the first ball is served onto the plunger, where it stays.
fn start_game(s: &mut Simulation) {
    s.set_switch("start", true).unwrap();
    s.advance(5);
    s.release_switch("start").unwrap();
    s.advance(1500);
    assert_eq!(s.persist(3), 1, "game state (persist key game_state)");
    assert_eq!(s.balls_in_play(), 1, "the served ball");
}

fn healthy(s: &Simulation) {
    assert!(!s.script_faulted(), "script fault: {:?}", s.script_error());
    let c = s.counters();
    assert_eq!(c.balls_escaped, 0, "{c:?}");
    assert_eq!(c.edges_dropped, 0, "{c:?}");
}

fn min_max(v: &[f32]) -> (f32, f32) {
    v.iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &x| {
            (a.min(x), b.max(x))
        })
}

// --- (a) unloaded stroke time ----------------------------------------------

/// (flipper id, cabinet button, EOS switch).
const FLIPPERS: [(&str, &str, &str); 3] = [
    ("lower_left", "left_flipper", "lower_left_eos"),
    ("lower_right", "right_flipper", "lower_right_eos"),
    ("upper_left", "left_upper", "upper_left_eos"),
];

/// Stroke time of `flipper` with no ball near it: (ticks from the first
/// tick of motion to the EOS change, ticks from the button edge to it).
fn stroke_time(s: &mut Simulation, flipper: &str, button: &str, eos: &str) -> (u64, u64) {
    let (a0, w0) = s.flipper(flipper).unwrap();
    assert!(
        a0.abs() < 1.0e-4 && w0.abs() < 1.0e-3,
        "{flipper} at rest: {a0} rad, {w0} rad/s"
    );
    let eos0 = s.switch_closed(eos).unwrap();
    s.set_switch(button, true).unwrap();
    let mut first_motion = None;
    let mut eos_tick = None;
    for t in 1..=200u64 {
        s.advance(1);
        let (a, _) = s.flipper(flipper).unwrap();
        if first_motion.is_none() && a.abs() > 0.0 {
            first_motion = Some(t);
        }
        if s.switch_closed(eos).unwrap() != eos0 {
            eos_tick = Some(t);
            break;
        }
    }
    s.release_switch(button).unwrap();
    s.advance(400);
    let m = first_motion.unwrap_or_else(|| panic!("{flipper} never moved"));
    let e = eos_tick.unwrap_or_else(|| panic!("{flipper}: {eos} never changed"));
    (e - m + 1, e)
}

#[test]
fn unloaded_stroke_time_of_every_flipper_is_in_the_window() {
    on_big_stack(|| {
        let w = PhysicsDefaults::builtin()
            .feel_windows
            .flipper_stroke_time_ms;
        let mut s = load();
        assert_eq!(s.balls_in_play(), 0, "no ball on the table");
        let mut out = Vec::new();
        for (flipper, button, eos) in FLIPPERS {
            let (ms, from_button) = stroke_time(&mut s, flipper, button, eos);
            println!(
                "stroke {flipper}: {ms} ms from the first tick of motion to {eos} \
                 ({from_button} ticks after the button edge); window {}-{} ms",
                w.min, w.max
            );
            out.push((flipper, ms));
        }
        let ms: Vec<f32> = out.iter().map(|(_, m)| *m as f32).collect();
        let (lo, hi) = min_max(&ms);
        println!("stroke time min/max: {lo} / {hi} ms");
        for (flipper, ms) in out {
            assert!(
                (w.min..=w.max).contains(&(ms as f32)),
                "{flipper}: unloaded stroke {ms} ms outside {}-{} ms",
                w.min,
                w.max
            );
        }
        healthy(&s);
    });
}

// --- (b) full shot from a cradle ---------------------------------------------

/// One full shot of `flipper` from its cradle, the re-flip `wait` ticks after
/// the release: (position of the ball along the bat at the flip, 0 = pivot,
/// 1 = tip length; exit speed at 100 mm from the tip, if it got there).
fn cradle_shot(s: &mut Simulation, f: &FlipperDef, button: &str, wait: u64) -> (f32, Option<f32>) {
    let len = bat_length_mm(f);
    let (px, py) = (f.base_position[0], f.base_position[1]);
    let up = f.rest_angle_deg + f.sweep_angle_deg;
    // Hold the bat up, then put the ball 20 mm along the table's x from the
    // pivot towards the tip side and 35 mm above it: it drops onto the
    // raised bat and rolls down into the cradle at the pivot.
    let side = if dir_deg(up).0 >= 0.0 { 1.0 } else { -1.0 };
    s.set_switch(button, true).unwrap();
    s.advance(100);
    let h = s
        .spawn_ball("standard_steel", mm3(px + side * 20.0, py + 35.0), [0.0; 3])
        .unwrap();
    s.advance(1500);
    let b = s.ball(h).expect("cradled ball");
    let rest_speed = horiz_speed(b.velocity_m_s);
    assert!(
        rest_speed < 0.01,
        "{}: the cradle holds the ball still ({rest_speed} m/s at {:?})",
        f.id,
        b.position_m
    );
    // Release; the ball rolls down the lowered bat; flip again.
    s.release_switch(button).unwrap();
    s.advance(wait);
    let b = s.ball(h).expect("ball on the bat");
    let a = f.rest_angle_deg + s.flipper(&f.id).unwrap().0.to_degrees();
    let (dx, dy) = dir_deg(a);
    let along = ((b.position_m[0] * 1.0e3 - px) * dx + (b.position_m[1] * 1.0e3 - py) * dy) / len;
    s.set_switch(button, true).unwrap();
    let (tx, ty) = dir_deg(up);
    let tip = (px + tx * len, py + ty * len);
    let mut exit = None;
    for _ in 0..300 {
        s.advance(1);
        let Some(b) = s.ball(h) else { break };
        let d = (b.position_m[0] * 1.0e3 - tip.0).hypot(b.position_m[1] * 1.0e3 - tip.1);
        if d >= 100.0 {
            let v = b.velocity_m_s;
            exit = Some((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt());
            break;
        }
    }
    s.release_switch(button).unwrap();
    if s.ball(h).is_some() {
        s.remove_ball(h);
    }
    s.advance(400);
    (along, exit)
}

#[test]
fn full_shot_from_a_cradle_exits_in_the_window_on_both_lower_flippers() {
    on_big_stack(|| {
        let w = PhysicsDefaults::builtin()
            .feel_windows
            .flipper_exit_speed_m_s;
        let t = manifest();
        let mut s = load();
        let mut failures = Vec::new();
        for (id, button) in [
            ("lower_left", "left_flipper"),
            ("lower_right", "right_flipper"),
        ] {
            let f = flipper_def(&t, id).clone();
            let mut best: Option<(f32, f32, u64)> = None;
            let mut band = Vec::new();
            for wait in (300..=900u64).step_by(25) {
                let (at, exit) = cradle_shot(&mut s, &f, button, wait);
                match exit {
                    Some(v) => {
                        println!(
                            "cradle shot {id}: flip {wait} ms after release, ball at {at:.2} \
                             of the bat: exit {v:.3} m/s"
                        );
                        if best.is_none_or(|b| v > b.0) {
                            best = Some((v, at, wait));
                        }
                        if (0.4..=0.8).contains(&at) {
                            band.push(v);
                            if !(w.min..=w.max).contains(&v) {
                                failures.push(format!("{id}: shot from {at:.2}: {v:.3} m/s"));
                            }
                        }
                    }
                    None => println!(
                        "cradle shot {id}: flip {wait} ms after release, ball at {at:.2} \
                         of the bat: never 100 mm from the tip"
                    ),
                }
            }
            let (v, at, wait) = best.unwrap_or_else(|| panic!("{id}: no shot left the bat"));
            let (lo, hi) = min_max(&band);
            println!(
                "full shot {id}: best {v:.3} m/s (ball at {at:.2} of the bat, flip {wait} ms \
                 after release); band 0.4-0.8: {} shots, min/max {lo:.3} / {hi:.3} m/s; \
                 window {}-{} m/s",
                band.len(),
                w.min,
                w.max
            );
            assert!(!band.is_empty(), "{id}: no shot from 0.4-0.8 of the bat");
            if !(w.min..=w.max).contains(&v) {
                failures.push(format!("{id}: best full shot {v:.3} m/s"));
            }
        }
        healthy(&s);
        assert!(
            failures.is_empty(),
            "full shots outside {}-{} m/s: {failures:?}",
            w.min,
            w.max
        );
    });
}

// --- (c), (d) bumper and sling kicks -----------------------------------------

/// Result of one kick trial.
#[derive(Debug, Clone, Copy)]
struct Kick {
    /// Exit speed, m/s.
    exit: f32,
    /// Ticks from the switch edge to the first tick with the coil on.
    fire_delay: u64,
}

/// Rolls a ball from `start` at `v` and waits for the closing edge of
/// `switch`; then measures per `measure` (called every tick after the
/// edge with the ticks since the edge and the ball velocity; returns the
/// exit speed when done).
fn kick_trial(
    s: &mut Simulation,
    start: [f32; 3],
    v: [f32; 3],
    switch: &str,
    coil: CoilId,
    mut measure: impl FnMut(u64, [f32; 3]) -> Option<f32>,
) -> Kick {
    let hits0 = s.switch_hits(switch).unwrap();
    assert!(!s.mech_set().coil_on(coil), "{switch}: coil idle before");
    let h = s.spawn_ball("standard_steel", start, v).unwrap();
    let mut edge = None;
    let mut fired = None;
    let mut exit = None;
    for t in 1..=600u64 {
        s.advance(1);
        let Some(b) = s.ball(h) else {
            panic!("{switch}: ball from {start:?} left the table")
        };
        if edge.is_none() && s.switch_hits(switch).unwrap() > hits0 {
            edge = Some(t);
        }
        let Some(e) = edge else { continue };
        if fired.is_none() && s.mech_set().coil_on(coil) {
            fired = Some(t - e);
        }
        if t > e
            && let Some(x) = measure(t - e, b.velocity_m_s)
        {
            exit = Some(x);
            break;
        }
    }
    s.remove_ball(h);
    // Let the coil drop out and the switch open before the next trial.
    s.advance(200);
    let e = edge.unwrap_or_else(|| panic!("{switch}: ball from {start:?} at {v:?} never hit"));
    Kick {
        exit: exit.unwrap_or_else(|| panic!("{switch}: no exit measured (edge at tick {e})")),
        fire_delay: fired.unwrap_or_else(|| panic!("{switch}: coil never fired")),
    }
}

/// Checks every kick: fired on the tick after the edge, exit in `w`.
fn check_kicks(what: &str, kicks: &[(String, Kick)], w: pin_core::schema::physics::FeelWindow) {
    let exits: Vec<f32> = kicks.iter().map(|(_, k)| k.exit).collect();
    let (lo, hi) = min_max(&exits);
    println!(
        "{what}: {} kicks, exit min/max {lo:.3} / {hi:.3} m/s; window {}-{} m/s",
        kicks.len(),
        w.min,
        w.max
    );
    let late: Vec<_> = kicks.iter().filter(|(_, k)| k.fire_delay != 1).collect();
    assert!(
        late.is_empty(),
        "{what}: coil not fired on the tick after the switch edge: {late:?}"
    );
    let out: Vec<_> = kicks
        .iter()
        .filter(|(_, k)| !(w.min..=w.max).contains(&k.exit))
        .collect();
    assert!(
        out.is_empty(),
        "{what}: exit speed outside {}-{} m/s: {out:?}",
        w.min,
        w.max
    );
}

/// Directions (degrees, from the bumper centre to the start) from which
/// each bumper is approached: open sides only.
fn bumper_directions(id: &str) -> [f32; 3] {
    match id {
        // Left bumper: from the left, lower left, below.
        "bumper_1" => [180.0, 225.0, 270.0],
        // Right bumper: mirrored.
        "bumper_2" => [0.0, 315.0, 270.0],
        // Top bumper (between and above the others): from above, right, left.
        _ => [90.0, 0.0, 180.0],
    }
}

#[test]
fn bumper_kicks_are_in_the_window() {
    on_big_stack(|| {
        let w = PhysicsDefaults::builtin()
            .feel_windows
            .bumper_exit_speed_m_s;
        let d = PhysicsDefaults::builtin().mechs;
        let t = manifest();
        let mut s = load();
        start_game(&mut s);
        let mut kicks = Vec::new();
        for id in ["bumper_1", "bumper_2", "bumper_3"] {
            let m = mech_def(&t, id);
            assert_eq!(m.kind, MechKind::Bumper);
            let p = m.position.unwrap();
            let ring = m.radius.unwrap_or(d.bumper.radius * 1.0e3);
            let switch = m.switch.clone().unwrap();
            let coil = s.ids().coil(m.coil.as_deref().unwrap()).unwrap();
            let mut per = Vec::new();
            for dir in bumper_directions(id) {
                let (ux, uy) = dir_deg(dir);
                let reach = ring + BALL_R_MM + 20.0;
                let start = mm3(p[0] + ux * reach, p[1] + uy * reach);
                for speed in [1.0f32, 2.0, 3.0] {
                    let k = kick_trial(
                        &mut s,
                        start,
                        [-ux * speed, -uy * speed, 0.0],
                        &switch,
                        coil,
                        |since, v| (since == 7).then(|| horiz_speed(v)),
                    );
                    println!(
                        "bumper {id}: from {dir}° at {speed} m/s -> exit {:.3} m/s \
                         (coil on {} tick after the edge)",
                        k.exit, k.fire_delay
                    );
                    per.push(k.exit);
                    kicks.push((format!("{id} from {dir}° at {speed} m/s"), k));
                }
            }
            let (lo, hi) = min_max(&per);
            println!("bumper {id}: exit min/max {lo:.3} / {hi:.3} m/s");
        }
        healthy(&s);
        check_kicks("bumpers", &kicks, w);
    });
}

#[test]
fn sling_kicks_are_in_the_window() {
    on_big_stack(|| {
        let w = PhysicsDefaults::builtin().feel_windows.sling_exit_speed_m_s;
        let d = PhysicsDefaults::builtin().mechs;
        let t = manifest();
        let mut s = load();
        start_game(&mut s);
        let mut kicks = Vec::new();
        for id in ["sling_left", "sling_right"] {
            let m = mech_def(&t, id);
            assert_eq!(m.kind, MechKind::Sling);
            let p = m.position.unwrap();
            let kick = m.angle_deg.unwrap_or(90.0);
            let thick = m.thickness.unwrap_or(d.sling.thickness * 1.0e3);
            let switch = m.switch.clone().unwrap();
            let coil = s.ids().coil(m.coil.as_deref().unwrap()).unwrap();
            let (kx, ky) = dir_deg(kick);
            // Centre of the face.
            let face = (p[0] + kx * 0.5 * thick, p[1] + ky * 0.5 * thick);
            let mut per = Vec::new();
            // Head on and 30° either side of the face normal.
            for off in [0.0f32, 30.0, -30.0] {
                let (ux, uy) = dir_deg(kick + off);
                // Start 20 mm clear of the face (along the face normal).
                let reach = (BALL_R_MM + 20.0) / off.to_radians().cos();
                let start = mm3(face.0 + ux * reach, face.1 + uy * reach);
                for speed in [1.0f32, 2.0, 3.0] {
                    let mut best = (f32::NEG_INFINITY, 0.0f32);
                    let k = kick_trial(
                        &mut s,
                        start,
                        [-ux * speed, -uy * speed, 0.0],
                        &switch,
                        coil,
                        |since, v| {
                            let along = v[0] * kx + v[1] * ky;
                            if along > best.0 {
                                best = (along, horiz_speed(v));
                            }
                            (since == 40).then_some(best.1)
                        },
                    );
                    println!(
                        "sling {id}: {off:+}° off the normal at {speed} m/s -> exit {:.3} m/s \
                         (coil on {} tick after the edge)",
                        k.exit, k.fire_delay
                    );
                    per.push(k.exit);
                    kicks.push((format!("{id} {off:+}° at {speed} m/s"), k));
                }
            }
            let (lo, hi) = min_max(&per);
            println!("sling {id}: exit min/max {lo:.3} / {hi:.3} m/s");
        }
        healthy(&s);
        check_kicks("slings", &kicks, w);
    });
}
