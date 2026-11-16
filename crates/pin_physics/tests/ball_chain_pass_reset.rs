//! A free ball that passed a receding Newton-chain captive must retest it
//! once an impulse changes its velocity (fix a1fix-A2, docs/DECISIONS.md
//! "a1fix-A2-resolved"; CodeRabbit PR #28 thread r4580933195).
//!
//! The solver advances the chain to the end of the tick first. A ball whose
//! cast reaches a captive that recedes at least as fast as the ball follows
//! passes it: the casts skip that captive for the rest of the ball's
//! sub-step ("a1fix-A-resolved"). The record is only true for the ball's
//! velocity at the time of the pass. If another ball (or a wall) then sends
//! the ball toward the captive faster than the captive recedes, the casts
//! must find the captive again: the ball hits it (the captive is pushed
//! along its path) instead of tunnelling through it.
//!
//! Each case: a one-ball chain on a straight path along +x, pitched gravity,
//! the captive struck to path speed `u`. A twin solver without free balls
//! gives the captive's position and speed at the end of the tick. Ball A
//! starts exactly touching that position on its −x side (contact normal
//! `n`, from the captive toward A), closing in slower than the captive
//! recedes, so it passes the captive at the start of the tick. Then, in the
//! same sub-step of A:
//! - pair sweep: ball B, coming along `-m` from beyond A, hits A at about
//!   0.3 ms (a ball-ball impulse in the event loop);
//! - surface sweep: A slides into an inclined wall at about 0.3 ms (a
//!   static surface impulse), which turns it toward the captive.
//!
//! The controls run the same scenes without B or without the wall: A must
//! still pass the captive (no impact, keeps moving), as before the fix.

#[path = "ball_util.rs"]
mod util;

use glam::{Quat, Vec3};
use pin_physics::ball::{BallEvent, BallEvents, BallSolver, Geometry, ImpactTarget, NewtonChain};
use pin_physics::world::{CubicSpline, StaticShape, StaticWorld, StaticWorldBuilder, layers};
use util::*;

/// Closing speed below which touching balls rest (solver's `REST_SLOP_SPEED`).
const SLOP: f32 = 1.0e-3;
/// Time at which the second impulse reaches ball A.
const T_HIT: f32 = 0.3e-3;

fn straight_path() -> CubicSpline {
    let d = 2.0 * radius(STEEL);
    let pts: Vec<Vec3> = (0..12).map(|i| v(d * i as f32, 0.0, 0.0)).collect();
    CubicSpline::interpolate(&pts).unwrap()
}

/// A solver with one captive ball at path parameter 5, struck to `u`.
fn chain_solver(gravity: Vec3, u: f32) -> BallSolver {
    let mut cfg = config();
    cfg.gravity = gravity;
    let mut s = BallSolver::new(cfg, &profiles());
    let c = s.add_newton_chain(NewtonChain::new(straight_path(), STEEL, &[5.0]).unwrap());
    s.newton_chain_mut(c).strike(0, u);
    s
}

fn gravity_at(pitch: f32) -> Vec3 {
    9.81 * v(pitch.sin(), 0.0, -pitch.cos())
}

/// Captive position and path speed at the end of the tick, without free balls.
fn twin(gravity: Vec3, u: f32, geo: &Geometry<'_>) -> (Vec3, f32) {
    let mut t = chain_solver(gravity, u);
    t.step(geo, DT, &mut BallEvents::new());
    (
        t.newton_chain(0).ball_position(0),
        t.newton_chain(0).ball_speed(0),
    )
}

/// A's velocity at the end of the tick in the same scene without the
/// chain: the velocity the second impulse gives it (plus a tick of gravity).
fn free_velocity(
    gravity: Vec3,
    pa: Vec3,
    va: Vec3,
    b: Option<(Vec3, Vec3)>,
    world: &StaticWorld,
) -> Vec3 {
    let mut cfg = config();
    cfg.gravity = gravity;
    let mut s = BallSolver::new(cfg, &profiles());
    let ha = s.add_ball(ball_at(STEEL, pa, va)).unwrap();
    if let Some((pb_rel, vb)) = b {
        s.add_ball(ball_at(STEEL, pa + pb_rel, vb)).unwrap();
    }
    s.step(&Geometry::new(world), DT, &mut BallEvents::new());
    s.ball(ha).unwrap().velocity
}

/// Whether A, after the second impulse (velocity `va1`), closes in on the
/// captive clearly faster than it recedes.
fn closes_in(u_end: f32, n: Vec3, va1: Vec3) -> bool {
    let recede = -(Vec3::X * u_end).dot(n);
    -va1.dot(n) > 1.2 * recede + 0.05
}

/// What one step did.
struct Outcome {
    /// Captive path speed after the step, and as the twin predicts it.
    u1: f32,
    u_end: f32,
    /// Distance between the surfaces of A and the captive after the step
    /// (negative: overlap).
    gap: f32,
    /// A's velocity at the start and after the step, and how far it moved.
    va0: Vec3,
    va1: Vec3,
    travelled: f32,
    newton_impact: bool,
    ball_impact: bool,
    wall_impact: bool,
}

/// Runs one tick with ball A touching the captive (velocity `va`) and,
/// optionally, ball B (position, velocity) and a static world.
fn run(
    gravity: Vec3,
    u: f32,
    n: Vec3,
    va: Vec3,
    b: Option<(Vec3, Vec3)>,
    world: &StaticWorld,
) -> Outcome {
    let geo = Geometry::new(world);
    let (q_end, u_end) = twin(gravity, u, &geo);
    let rsum = 2.0 * radius(STEEL);
    let mut s = chain_solver(gravity, u);
    let pa = q_end + n * rsum;
    let ha = s.add_ball(ball_at(STEEL, pa, va)).unwrap();
    if let Some((pb_rel, vb)) = b {
        s.add_ball(ball_at(STEEL, pa + pb_rel, vb)).unwrap();
    }
    let mut ev = BallEvents::new();
    s.step(&geo, DT, &mut ev);
    assert_eq!(
        s.newton_chain(0).ball_position(0),
        q_end,
        "the twin predicts the captive"
    );
    let a = s.ball(ha).unwrap();
    let any = |f: &dyn Fn(&ImpactTarget) -> bool| {
        ev.as_slice()
            .iter()
            .any(|e| matches!(e, BallEvent::Impact { other, .. } if f(other)))
    };
    Outcome {
        u1: s.newton_chain(0).ball_speed(0),
        u_end,
        gap: a.position.distance(s.newton_chain(0).ball_position(0)) - rsum,
        va0: va,
        va1: a.velocity,
        travelled: a.position.distance(pa),
        newton_impact: any(&|t| matches!(t, ImpactTarget::Newton(0))),
        ball_impact: any(&|t| matches!(t, ImpactTarget::Ball(_))),
        wall_impact: any(&|t| matches!(t, ImpactTarget::Collider(_))),
    }
}

/// Contact normals on the −x side of the captive (it moves +x), at several
/// angles to the path and out of the plane.
fn normals() -> Vec<Vec3> {
    let mut out = Vec::new();
    for a in [0.0f32, 0.35, 0.7] {
        for b in [0.0f32, 0.25] {
            out.push(v(-a.cos() * b.cos(), a.sin() * b.cos(), b.sin()));
        }
    }
    out
}

/// A unit vector perpendicular to `n` (fixed choice).
fn perp(n: Vec3) -> Vec3 {
    n.cross(Vec3::Z).normalize_or_zero()
}

/// Whether A, closing at `closing` along `-n`, passes the captive: it closes
/// in faster than the resting slop but slower than the captive recedes, at
/// the start and with the boundary's gravity (as in ball_chain_receding).
fn passes(gravity: Vec3, u_end: f32, n: Vec3, va: Vec3) -> bool {
    let recede = -(Vec3::X * u_end).dot(n);
    let close0 = -va.dot(n);
    let close_mid = -(va + gravity * DT).dot(n);
    close0.min(close_mid) > 1.05 * SLOP && recede > 1.05 * close0.max(close_mid)
}

/// The deepest A can be inside the captive's end-of-tick position after
/// passing it for a whole tick (the captive is drawn where it is at the end
/// of the tick), plus the contact tolerance.
fn pass_depth(gravity: Vec3, n: Vec3, va: Vec3) -> f32 {
    let close = (-va.dot(n)).max(-(va + gravity * DT).dot(n));
    close * DT + 2.0e-6
}

/// The pair case: B approaches A along `-m` and reaches it at about
/// [`T_HIT`]. Returns B's position relative to A and its velocity.
fn ball_b(m: Vec3, vb: f32, va: Vec3) -> (Vec3, Vec3) {
    let rsum = 2.0 * radius(STEEL);
    let closing = vb + va.dot(m);
    (m * (rsum + closing * T_HIT), -m * vb)
}

#[test]
fn ball_knocked_toward_a_passed_captive_hits_it() {
    let world = StaticWorldBuilder::new().build();
    let mut cases = 0;
    let mut controls = 0;
    for pitch in [-0.12f32, 0.0, 0.12] {
        let gravity = gravity_at(pitch);
        for u in [0.3f32, 1.0] {
            let (q_end, u_end) = twin(gravity, u, &Geometry::new(&world));
            for n in normals() {
                for closing in [0.0025f32, 0.03] {
                    let va = -n * closing;
                    if !passes(gravity, u_end, n, va) {
                        continue;
                    }
                    let depth = pass_depth(gravity, n, va);
                    // Control: no B, A passes the captive and keeps moving.
                    let o = run(gravity, u, n, va, None, &world);
                    let label = format!("pitch {pitch} u {u} n {n:?} closing {closing}");
                    assert!(!o.newton_impact, "{label}: control: no impact");
                    assert_eq!(o.u1, o.u_end, "{label}: control: captive untouched");
                    let need = 0.99 * o.va1.length() * DT;
                    assert!(
                        o.travelled >= need,
                        "{label}: control: travelled {} m, expected >= {need} m (frozen?)",
                        o.travelled
                    );
                    controls += 1;
                    for beta in [0.0f32, 0.4, 0.8] {
                        // B comes from beyond A, at angle `beta` to `n`.
                        let m = n * beta.cos() + perp(n) * beta.sin();
                        for vb in [1.5f32, 3.0, 5.0] {
                            let b = Some(ball_b(m, vb, va));
                            let pa = q_end + n * 2.0 * radius(STEEL);
                            if !closes_in(u_end, n, free_velocity(gravity, pa, va, b, &world)) {
                                continue;
                            }
                            let o = run(gravity, u, n, va, b, &world);
                            let label = format!("{label} beta {beta} vb {vb}");
                            assert!(o.ball_impact, "{label}: B hits A in the tick");
                            cases += 1;
                            // A now closes in on the captive faster than it
                            // recedes: it must hit it, not pass through it.
                            assert!(
                                o.u1 > o.u_end + 1.0e-3,
                                "{label}: captive speed {} -> {}: A was not stopped by it",
                                o.u_end,
                                o.u1
                            );
                            assert!(
                                o.gap >= -depth,
                                "{label}: A ends {} m inside the captive (allowed {depth})",
                                -o.gap
                            );
                            assert!(o.va1 != o.va0, "{label}: A's velocity changed");
                        }
                    }
                }
            }
        }
    }
    assert!(controls >= 10, "only {controls} control cases were checked");
    assert!(cases >= 90, "only {cases} pair cases were checked");
}

/// A world with one wall whose face passes through `point` with outward
/// normal `k`.
fn wall_world(point: Vec3, k: Vec3) -> StaticWorld {
    let t = 0.05;
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: point - k * t,
            half_extents: v(t, 0.2, 0.2),
            rotation: Quat::from_rotation_arc(Vec3::X, k),
        },
        layers::ALL,
        true,
    );
    b.build()
}

#[test]
fn ball_bounced_toward_a_passed_captive_by_a_wall_hits_it() {
    let empty = StaticWorldBuilder::new().build();
    let r = radius(STEEL);
    let rsum = 2.0 * r;
    let mut cases = 0;
    for pitch in [-0.12f32, 0.0, 0.12] {
        let gravity = gravity_at(pitch);
        for u in [0.3f32, 1.0] {
            let (q_end, u_end) = twin(gravity, u, &Geometry::new(&empty));
            for n in normals() {
                let p = perp(n);
                for closing in [0.0025f32, 0.03] {
                    for w in [1.5f32, 3.0] {
                        for a in [0.5f32, 0.8] {
                            // A slides along `-p` into a wall whose normal
                            // `k` leans toward the captive (`k·n = -a`).
                            let k = (-n * a + p * (1.0 - a * a).sqrt()).normalize();
                            let va = -n * closing - p * w;
                            if !passes(gravity, u_end, n, va) {
                                continue;
                            }
                            let into = -va.dot(k);
                            let pa = q_end + n * rsum;
                            let face = pa - k * (r + into * T_HIT);
                            let world = wall_world(face, k);
                            if !closes_in(u_end, n, free_velocity(gravity, pa, va, None, &world)) {
                                continue;
                            }
                            let depth = pass_depth(gravity, n, va);
                            let label = format!(
                                "pitch {pitch} u {u} n {n:?} closing {closing} w {w} a {a}"
                            );
                            // Control: no wall, A passes the captive and
                            // keeps moving.
                            let c = run(gravity, u, n, va, None, &empty);
                            assert!(!c.newton_impact, "{label}: control: no impact");
                            assert_eq!(c.u1, c.u_end, "{label}: control: captive untouched");
                            let need = 0.99 * c.va1.length() * DT;
                            assert!(
                                c.travelled >= need,
                                "{label}: control: travelled {} m, expected >= {need} m",
                                c.travelled
                            );
                            let o = run(gravity, u, n, va, None, &world);
                            assert!(o.wall_impact, "{label}: A hits the wall in the tick");
                            cases += 1;
                            assert!(
                                o.u1 > o.u_end + 1.0e-3,
                                "{label}: captive speed {} -> {}: A was not stopped by it",
                                o.u_end,
                                o.u1
                            );
                            assert!(
                                o.gap >= -depth,
                                "{label}: A ends {} m inside the captive (allowed {depth})",
                                -o.gap
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(cases >= 60, "only {cases} wall cases were checked");
}

/// What one step of the far-side scene did.
struct FarOutcome {
    /// Captive path speed after the step.
    u1: f32,
    /// Distance between the surfaces of A and the captive after the step
    /// (negative: overlap); 0 without A.
    gap: f32,
    /// A's velocity after the step (zero without A).
    va1: Vec3,
    /// Impacts on the captive reported for A and for B.
    a_newton: bool,
    b_newton: bool,
}

/// Runs one tick of the far-side scene: ball A (optional) touching the
/// captive's end-of-tick position along `n` with velocity `va`, and ball B
/// (optional, absolute position and velocity).
fn run_far(
    gravity: Vec3,
    u: f32,
    n: Vec3,
    a: Option<Vec3>,
    b: Option<(Vec3, Vec3)>,
    world: &StaticWorld,
) -> FarOutcome {
    let geo = Geometry::new(world);
    let (q_end, _) = twin(gravity, u, &geo);
    let rsum = 2.0 * radius(STEEL);
    let mut s = chain_solver(gravity, u);
    let pa = q_end + n * rsum;
    let ha = a.map(|va| s.add_ball(ball_at(STEEL, pa, va)).unwrap());
    let hb = b.map(|(pb, vb)| s.add_ball(ball_at(STEEL, pb, vb)).unwrap());
    let mut ev = BallEvents::new();
    s.step(&geo, DT, &mut ev);
    assert_eq!(
        s.newton_chain(0).ball_position(0),
        q_end,
        "the twin predicts the captive"
    );
    let hit = |h: Option<pin_physics::ball::BallHandle>| {
        h.is_some_and(|h| {
            ev.as_slice().iter().any(|e| {
                matches!(e, BallEvent::Impact { ball, other: ImpactTarget::Newton(0), .. } if *ball == h)
            })
        })
    };
    let (gap, va1) = ha.map_or((0.0, Vec3::ZERO), |h| {
        let ball = s.ball(h).unwrap();
        (
            ball.position.distance(s.newton_chain(0).ball_position(0)) - rsum,
            ball.velocity,
        )
    });
    FarOutcome {
        u1: s.newton_chain(0).ball_speed(0),
        gap,
        va1,
        a_newton: hit(ha),
        b_newton: hit(hb),
    }
}

/// Fix a1fix-A3 (CodeRabbit PR #28 thread r4581052814, docs/DECISIONS.md
/// "a1fix-A3-resolved"): ball A passes a receding captive at the start of its
/// only sub-step, so its cast holds no event until the end of the tick. Ball
/// B then hits the captive from the far side at `t_b` and turns it toward A.
/// A's cast must be redone at that moment: A hits the captive inside the
/// event loop (its only boundary is at the start of the tick), instead of
/// sinking into it for the rest of the tick.
///
/// Swept: pitch, captive speed, A's contact normal and closing speed, B's
/// arrival time, approach direction and speed. Controls: without B (A passes
/// and sinks the whole tick: the stale depth) and without A (the captive
/// speed B alone leaves).
#[test]
fn captive_turned_toward_a_passing_ball_hits_it() {
    let world = StaticWorldBuilder::new().build();
    let rsum = 2.0 * radius(STEEL);
    let mut cases = 0;
    for pitch in [-0.12f32, 0.0, 0.12] {
        let gravity = gravity_at(pitch);
        for u in [0.2f32, 0.3, 0.5] {
            let (q_end, u_end) = twin(gravity, u, &Geometry::new(&world));
            for n in normals() {
                for closing in [0.01f32, 0.03, 0.06] {
                    let va = -n * closing;
                    if !passes(gravity, u_end, n, va) {
                        continue;
                    }
                    let label = format!("pitch {pitch} u {u} n {n:?} closing {closing}");
                    // No-B control: A passes the captive for the whole tick.
                    let c = run_far(gravity, u, n, Some(va), None, &world);
                    assert!(!c.a_newton, "{label}: control: no impact");
                    assert_eq!(c.u1, u_end, "{label}: control: captive untouched");
                    let close = (-va.dot(n)).max(-(va + gravity * DT).dot(n));
                    let close_min = (-va.dot(n)).min(-(va + gravity * DT).dot(n));
                    for t_b in [0.2e-3f32, 0.3e-3, 0.5e-3] {
                        // B comes from the far (+x) side of the captive.
                        for (g, h) in [(0.0f32, 0.0f32), (0.4, 0.0), (-0.5, 0.0), (0.2, 0.3)] {
                            let m = v(g.cos() * h.cos(), g.sin() * h.cos(), h.sin());
                            for vb in [1.0f32, 2.5] {
                                let b = Some((q_end + m * (rsum + vb * t_b), -m * vb));
                                let label = format!("{label} t_b {t_b} m {m:?} vb {vb}");
                                // No-A control: the captive speed B leaves.
                                let ob = run_far(gravity, u, n, None, b, &world);
                                assert!(ob.b_newton, "{label}: B hits the captive");
                                // A's approach on the captive once B turned it.
                                let approach = closing + ob.u1 * Vec3::X.dot(n);
                                if approach < 0.08 {
                                    continue;
                                }
                                cases += 1;
                                let o = run_far(gravity, u, n, Some(va), b, &world);
                                assert!(o.b_newton, "{label}: B hits the captive");
                                // A's only boundary is at the start of the
                                // tick, so any impulse it gets in the tick is
                                // applied in the event loop. (The pass left a
                                // resting touch, so the hit is not reported
                                // as an impact event.)
                                assert!(
                                    (o.va1 - c.va1).dot(n) > 0.25 * approach,
                                    "{label}: A must be pushed back by the captive turned \
                                     toward it (approach {approach} m/s) inside the tick: \
                                     {:?} vs control {:?}",
                                    o.va1,
                                    c.va1
                                );
                                assert!(
                                    o.u1 > ob.u1 + 1.0e-3,
                                    "{label}: captive speed {} (B alone {}): A's impulse missing",
                                    o.u1,
                                    ob.u1
                                );
                                // An immediate response stops A sinking at t_b.
                                let immediate = close * t_b + 2.0e-6;
                                assert!(
                                    -o.gap <= immediate,
                                    "{label}: A ends {} m inside the captive \
                                     (immediate response: {immediate} m)",
                                    -o.gap
                                );
                                assert!(
                                    -o.gap < -c.gap - 0.5 * close_min * (DT - t_b),
                                    "{label}: A ends {} m inside the captive, stale depth {} m",
                                    -o.gap,
                                    -c.gap
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(cases >= 1000, "only {cases} cases were checked");
}
