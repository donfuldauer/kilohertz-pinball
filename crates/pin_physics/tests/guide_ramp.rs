//! Subphase 2.1 verification (PLAN.md §4): a ball at 8.0 m/s through a 180°
//! guide of radius 150 mm, then up a ramp from Z = 0 to 65 mm.
//!
//! * kinetic + potential energy never increases;
//! * the normal force changes by less than 5 % between consecutive ticks,
//!   away from entry and exit;
//! * no tunnelling;
//! * the layer switches exactly once per transition.
//!
//! The ball solver arrives in Subphase 2.2. This file carries a minimal
//! frictionless point-mass integrator: 1 kHz ticks, CCD sub-steps, sphere
//! casts for approaching contacts and closest-point projection for sliding
//! contacts. All collision geometry comes from `pin_physics::world`.

use arrayvec::ArrayVec;
use glam::{Quat, Vec3};
use pin_physics::world::{
    CubicSpline, GuideCurve, LayerTransition, SphereContact, StaticShape, StaticWorld,
    StaticWorldBuilder, TransitionSet, Wireform, layers,
};

const BALL_R: f32 = 0.0135;
const BALL_M: f32 = 0.0806;
const G: f32 = 9.81;
const DT: f32 = 1.0e-3;
const SUBSTEPS: usize = 8;

const GUIDE_R: f32 = 0.150;
const GAUGE: f32 = 0.020;
const LAYER_MAIN: u8 = 0;
const LAYER_UPPER: u8 = 1;
const LAYER_WIRE: u8 = 2;

/// Ramp profile: height gain as a function of distance along the ramp.
/// The slope eases in (cosine) over `EASE` metres, then stays constant.
const SLOPE: f32 = 0.1;
const EASE: f32 = 1.0;
const RAMP_START_Y: f32 = -0.40;
const RAMP_TOP_D: f32 = 1.15; // where the rise reaches 65 mm

fn ramp_rise(d: f32) -> f32 {
    if d <= 0.0 {
        0.0
    } else if d <= EASE {
        let k = std::f32::consts::PI / EASE;
        0.5 * SLOPE * (d - libm::sinf(k * d) / k)
    } else {
        0.5 * SLOPE * EASE + SLOPE * (d - EASE)
    }
}

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

struct Table {
    world: StaticWorld,
    guide: GuideCurve,
    ramp: Wireform,
    transitions: TransitionSet,
    /// Height of the ball centre above the rail centerline when it sits on both rails.
    seat: f32,
}

fn build_table() -> Table {
    assert!((ramp_rise(RAMP_TOP_D) - 0.065).abs() < 1e-6);

    // Floor: top face at z = 0, main deck only.
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: v(0.0, -0.5, -0.02),
            half_extents: v(0.5, 1.5, 0.02),
            rotation: Quat::IDENTITY,
        },
        layers::MAIN_DECK,
        true,
    );
    let world = b.build();

    // Guide wall: straight lead-in up the right side, a 180° arc of radius
    // 150 mm (5° spacing), straight lead-out down the left side.
    // Through-points are evenly spaced (the arc's 5° chord, about 13 mm):
    // the spline parameter is uniform per point, so uneven spacing would
    // make the curve wiggle where the spacing changes.
    let step = 2.0 * GUIDE_R * libm::sinf(2.5f32.to_radians());
    let lead = 24;
    let mut pts = Vec::new();
    for k in (1..=lead).rev() {
        pts.push(v(GUIDE_R, -step * k as f32, 0.0));
    }
    for k in 0..=36 {
        let a = (5.0 * k as f32).to_radians();
        pts.push(v(GUIDE_R * libm::cosf(a), GUIDE_R * libm::sinf(a), 0.0));
    }
    for k in 1..=lead {
        pts.push(v(-GUIDE_R, -step * k as f32, 0.0));
    }
    let guide = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap();

    // Ramp: a wireform straight down the left side (heading -Y), flat for the
    // first 100 mm, then rising along `ramp_rise`.
    let seat = (BALL_R * BALL_R - 0.25 * GAUGE * GAUGE).sqrt();
    let x = -(GUIDE_R - BALL_R);
    let mut pts = Vec::new();
    let mut d = -0.10f32;
    while d <= 1.3001 {
        pts.push(v(x, RAMP_START_Y - d, BALL_R - seat + ramp_rise(d)));
        d += 0.05;
    }
    let rolls = vec![0.0; pts.len()];
    let ramp = Wireform::new(CubicSpline::interpolate(&pts).unwrap(), &rolls, GAUGE, Vec3::Z).unwrap();

    // Ramp entrance (on the flat lead-in) and ramp top.
    let transitions = TransitionSet::new(vec![
        LayerTransition {
            from_layer: LAYER_MAIN,
            to_layer: LAYER_WIRE,
            min: v(x - 0.03, RAMP_START_Y + 0.02, 0.0),
            max: v(x + 0.03, RAMP_START_Y + 0.08, 0.05),
        },
        LayerTransition {
            from_layer: LAYER_WIRE,
            to_layer: LAYER_UPPER,
            min: v(x - 0.03, RAMP_START_Y - 1.22, 0.0),
            max: v(x + 0.03, RAMP_START_Y - 1.17, 0.2),
        },
    ]);
    Table {
        world,
        guide,
        ramp,
        transitions,
        seat,
    }
}

#[derive(Default, Clone, Copy)]
struct TickLog {
    pos: Vec3,
    vel: Vec3,
    layer: u8,
    /// Normal impulse this tick / DT, per surface (newtons).
    guide_force: f32,
    rail_force: f32,
    floor_force: f32,
    /// Smallest gaps seen this tick (negative = penetration), metres.
    min_guide_gap: f32,
    min_rail_gap: f32,
    min_floor_gap: f32,
    /// Largest distance from the arc centre while beside the arc.
    max_arc_radius: f32,
    /// Smallest height of the ball centre above the rail centerline.
    min_rail_height: f32,
    layer_switches: u32,
}

struct Ball {
    pos: Vec3,
    vel: Vec3,
    layer: u8,
    guide_seed: Option<f32>,
    rail_seed: Option<f32>,
}

/// Removes the approaching normal velocity (perfectly inelastic, no
/// friction) and returns the impulse magnitude.
fn stop_normal(vel: &mut Vec3, normal: Vec3) -> f32 {
    let vn = vel.dot(normal);
    if vn < 0.0 {
        *vel -= normal * vn;
        -vn * BALL_M
    } else {
        0.0
    }
}

fn tick(table: &Table, ball: &mut Ball) -> TickLog {
    let mut log = TickLog {
        min_guide_gap: f32::INFINITY,
        min_rail_gap: f32::INFINITY,
        min_floor_gap: f32::INFINITY,
        min_rail_height: f32::INFINITY,
        ..TickLog::default()
    };
    let h = DT / SUBSTEPS as f32;
    let mut contacts: ArrayVec<SphereContact, 16> = ArrayVec::new();
    let (mut guide_impulse, mut rail_impulse, mut floor_impulse) = (0.0f32, 0.0f32, 0.0f32);

    for _ in 0..SUBSTEPS {
        let mask = layers::layer_bit(ball.layer);
        let on_main = ball.layer == LAYER_MAIN;
        let on_wire = ball.layer == LAYER_WIRE;
        let prev_pos = ball.pos;
        ball.vel += v(0.0, 0.0, -G) * h;

        // CCD: advance to the earliest impact, stop the normal velocity, repeat.
        let mut remaining = h;
        for _ in 0..4 {
            let world_hit = table.world.cast_sphere(ball.pos, BALL_R, ball.vel, remaining, mask);
            let guide_hit = if on_main {
                table.guide.cast_sphere(ball.pos, BALL_R, ball.vel, remaining, ball.guide_seed)
            } else {
                None
            };
            let world_time = world_hit.map_or(f32::INFINITY, |hit| hit.time);
            let guide_time = guide_hit.map_or(f32::INFINITY, |hit| hit.time);
            if world_time.is_infinite() && guide_time.is_infinite() {
                ball.pos += ball.vel * remaining;
                remaining = 0.0;
                break;
            }
            if world_time <= guide_time {
                let hit = world_hit.unwrap();
                ball.pos += ball.vel * hit.time;
                remaining -= hit.time;
                floor_impulse += stop_normal(&mut ball.vel, hit.normal);
            } else {
                let hit = guide_hit.unwrap();
                ball.pos += ball.vel * hit.time;
                remaining -= hit.time;
                guide_impulse += stop_normal(&mut ball.vel, hit.normal);
            }
        }
        // Whatever time is left after four impacts is spent without moving.
        let _ = remaining;

        // Sliding contacts: project out of penetration, stop normal velocity.
        table.world.sphere_contacts(ball.pos, BALL_R, mask, &mut contacts);
        for c in &contacts {
            log.min_floor_gap = log.min_floor_gap.min(-c.depth);
            floor_impulse += stop_normal(&mut ball.vel, c.normal);
            ball.pos += c.normal * c.depth;
        }
        if on_main {
            let cp = table.guide.closest_point(ball.pos, ball.guide_seed);
            ball.guide_seed = Some(cp.t);
            let gap = cp.distance - BALL_R;
            log.min_guide_gap = log.min_guide_gap.min(gap);
            if gap < 0.0 {
                let normal = (ball.pos - cp.point) / cp.distance;
                guide_impulse += stop_normal(&mut ball.vel, normal);
                ball.pos -= normal * gap;
            }
            if ball.pos.y > 0.0 {
                log.max_arc_radius = log.max_arc_radius.max(v(ball.pos.x, ball.pos.y, 0.0).length());
            }
        }
        if on_wire {
            for _pass in 0..2 {
                let rails = table.ramp.closest_on_rails(ball.pos, ball.rail_seed);
                ball.rail_seed = Some(0.5 * (rails[0].t + rails[1].t));
                for cp in rails {
                    let gap = cp.distance - BALL_R;
                    log.min_rail_gap = log.min_rail_gap.min(gap);
                    if gap < 0.0 {
                        let normal = (ball.pos - cp.point) / cp.distance;
                        rail_impulse += stop_normal(&mut ball.vel, normal);
                        ball.pos -= normal * gap;
                    }
                }
            }
            let t = ball.rail_seed.unwrap();
            let frame = table.ramp.frame_at(t);
            let height = (ball.pos - table.ramp.centerline().point(t)).dot(frame.normal);
            log.min_rail_height = log.min_rail_height.min(height);
        }

        let layer = table.transitions.update(ball.layer, prev_pos, ball.pos);
        if layer != ball.layer {
            log.layer_switches += 1;
            ball.layer = layer;
        }
    }

    log.pos = ball.pos;
    log.vel = ball.vel;
    log.layer = ball.layer;
    log.guide_force = guide_impulse / DT;
    log.rail_force = rail_impulse / DT;
    log.floor_force = floor_impulse / DT;
    log
}

fn energy(pos: Vec3, vel: Vec3) -> f64 {
    let v = vel.as_dvec3();
    0.5 * f64::from(BALL_M) * v.length_squared()
        + f64::from(BALL_M) * f64::from(G) * f64::from(pos.z)
}

fn run() -> (Table, Vec<TickLog>) {
    let table = build_table();
    let mut ball = Ball {
        pos: v(GUIDE_R - BALL_R, -0.25, BALL_R),
        vel: v(0.0, 8.0, 0.0),
        layer: LAYER_MAIN,
        guide_seed: None,
        rail_seed: None,
    };
    let mut logs = Vec::new();
    // Until the ball passes the ramp top (distance 1.24 m along the ramp).
    for _ in 0..2000 {
        let log = tick(&table, &mut ball);
        logs.push(log);
        if log.pos.y < RAMP_START_Y - 1.24 {
            break;
        }
    }
    (table, logs)
}

#[test]
fn ball_completes_the_course() {
    let (_, logs) = run();
    let last = logs.last().unwrap();
    assert!(last.pos.y < RAMP_START_Y - 1.24, "ball stopped at {}", last.pos);
    // 0.25 + π·0.1365 + 0.4 + 1.24 m at just under 8 m/s.
    assert!(logs.len() > 280 && logs.len() < 320, "{} ticks", logs.len());
    // It went around the arc (passed x < 0 with y > 0) and up the ramp.
    assert!(logs.iter().any(|l| l.pos.y > 0.12 && l.pos.x.abs() < 0.01));
    assert!((last.pos.z - (BALL_R + ramp_rise(1.24))).abs() < 1e-3, "final height {}", last.pos.z);
    // Frictionless: only numerical dissipation and the climb slow the ball.
    let speed = last.vel.length();
    let ideal = (64.0 - 2.0 * G * ramp_rise(1.24)).sqrt();
    assert!(speed <= ideal + 1e-4 && speed > 0.97 * ideal, "speed {speed} vs ideal {ideal}");
}

#[test]
fn energy_never_increases() {
    let (_, logs) = run();
    let e0 = energy(v(GUIDE_R - BALL_R, -0.25, BALL_R), v(0.0, 8.0, 0.0));
    let mut prev = e0;
    for (i, log) in logs.iter().enumerate() {
        let e = energy(log.pos, log.vel);
        // f32 state: allow rounding noise of 1e-6 relative per tick.
        assert!(e <= prev + 1e-6 * e0, "tick {i}: energy rose from {prev} to {e}");
        prev = e;
    }
    assert!(prev < e0);
    assert!(prev > 0.94 * e0, "excessive numerical dissipation: {prev} of {e0}");
}

#[test]
fn normal_force_is_smooth_away_from_entry_and_exit() {
    let (_, logs) = run();

    // Guide: between 15° and 165° of the arc.
    let mut guide_samples = Vec::new();
    for w in logs.windows(2) {
        let on_arc = |l: &TickLog| {
            let a = libm::atan2f(l.pos.y, l.pos.x).to_degrees();
            l.layer == LAYER_MAIN && l.pos.y > 0.0 && (15.0..=165.0).contains(&a)
        };
        if on_arc(&w[0]) && on_arc(&w[1]) {
            let change = (w[1].guide_force - w[0].guide_force).abs() / w[0].guide_force;
            assert!(change < 0.05, "guide force {} -> {}", w[0].guide_force, w[1].guide_force);
            guide_samples.push((w[1].guide_force, w[1].vel.length()));
        }
    }
    assert!(guide_samples.len() > 30, "{} guide samples", guide_samples.len());
    for (force, speed) in &guide_samples {
        // Centripetal force m v² / r of the ball centre path.
        let expected = BALL_M * speed * speed / (GUIDE_R - BALL_R);
        assert!((force / expected - 1.0).abs() < 0.02, "guide force {force} vs {expected}");
    }

    // Ramp: from 50 mm after the rails take over to the top of the ramp.
    let mut rail_samples = 0;
    for w in logs.windows(2) {
        let on_ramp = |l: &TickLog| {
            let d = RAMP_START_Y - l.pos.y;
            l.layer == LAYER_WIRE && (-0.03..=RAMP_TOP_D).contains(&d)
        };
        if on_ramp(&w[0]) && on_ramp(&w[1]) {
            assert!(w[0].rail_force > 0.5, "ball left the rails: {}", w[0].rail_force);
            let change = (w[1].rail_force - w[0].rail_force).abs() / w[0].rail_force;
            assert!(change < 0.05, "rail force {} -> {} at y = {}", w[0].rail_force, w[1].rail_force, w[1].pos.y);
            rail_samples += 1;
        }
    }
    assert!(rail_samples > 100, "{rail_samples} rail samples");

    // Floor: the ball's weight while it rolls on the main deck.
    for log in logs.iter().filter(|l| l.layer == LAYER_MAIN).skip(2) {
        assert!((log.floor_force / (BALL_M * G) - 1.0).abs() < 0.01, "floor force {}", log.floor_force);
    }
}

#[test]
fn no_tunnelling() {
    let (table, logs) = run();
    let tol = 1.0e-4;
    for (i, log) in logs.iter().enumerate() {
        match log.layer {
            LAYER_MAIN => {
                // Never through the floor, never through the guide wall.
                assert!(log.pos.z > BALL_R - tol, "tick {i}: z = {}", log.pos.z);
                assert!(log.min_floor_gap > -tol || log.min_floor_gap.is_infinite());
                assert!(log.min_guide_gap > -tol, "tick {i}: guide gap {}", log.min_guide_gap);
                assert!(log.max_arc_radius < GUIDE_R - BALL_R + tol, "tick {i}: radius {}", log.max_arc_radius);
                assert!(log.pos.x.abs() < GUIDE_R - BALL_R + tol);
            }
            LAYER_WIRE => {
                assert!(log.min_rail_gap > -tol, "tick {i}: rail gap {}", log.min_rail_gap);
                // Still sitting on top of both rails.
                assert!(
                    log.min_rail_height > table.seat - tol,
                    "tick {i}: height above rails {}",
                    log.min_rail_height
                );
                assert!((log.pos.x + (GUIDE_R - BALL_R)).abs() < tol);
            }
            _ => {}
        }
        assert!(log.pos.is_finite() && log.vel.is_finite());
    }
}

#[test]
fn layer_switches_exactly_once_per_transition() {
    let (_, logs) = run();
    let total: u32 = logs.iter().map(|l| l.layer_switches).sum();
    assert_eq!(total, 2);
    let mut sequence = vec![LAYER_MAIN];
    for log in &logs {
        if *sequence.last().unwrap() != log.layer {
            sequence.push(log.layer);
        }
    }
    assert_eq!(sequence, vec![LAYER_MAIN, LAYER_WIRE, LAYER_UPPER]);
    // The floor (main deck only) stops colliding once the ball is on the wireform.
    let first_wire = logs.iter().position(|l| l.layer == LAYER_WIRE).unwrap();
    assert!(logs[first_wire + 1..].iter().all(|l| l.floor_force == 0.0));
    assert!(logs[..first_wire].iter().all(|l| l.rail_force == 0.0));
}

#[test]
fn run_is_deterministic() {
    let (_, a) = run();
    let (_, b) = run();
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.pos.to_array().map(f32::to_bits), y.pos.to_array().map(f32::to_bits));
        assert_eq!(x.vel.to_array().map(f32::to_bits), y.vel.to_array().map(f32::to_bits));
    }
}

#[test]
#[ignore = "debugging aid: prints the per-tick log"]
fn dump() {
    let (_, logs) = run();
    let mut prev = 0.0;
    for (i, l) in logs.iter().enumerate() {
        let e = energy(l.pos, l.vel);
        println!(
            "{i:3} L{} pos {:8.5} {:8.5} {:8.5} |v| {:7.5} E {:9.6} dE {:+.2e} Ng {:8.4} Nr {:8.4} Nf {:7.4} gg {:+.2e} rg {:+.2e} fg {:+.2e}",
            l.layer, l.pos.x, l.pos.y, l.pos.z, l.vel.length(), e, e - prev, l.guide_force, l.rail_force,
            l.floor_force, l.min_guide_gap, l.min_rail_gap, l.min_floor_gap
        );
        prev = e;
    }
}
