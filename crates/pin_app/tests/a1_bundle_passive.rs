//! Gate A1 on the grey-box bundle: "Energy never increases in any passive
//! scene" (PLAN Gate A1), measured on the real grey-box bundle loaded into
//! the headless [`Simulation`], not on a physics rig.
//!
//! Scenes (cabinet inputs idle, no coil fired by the test; the rules script
//! stays in attract mode, where it fires no coil):
//!
//! * (a) a standard steel ball released at rest at five points of the open
//!   playfield;
//! * (b) a ball fired at 1, 2 and 3 m/s at the body of `bumper_1`, at the
//!   face of `sling_left` at rest (its coil is not fired) and at the
//!   shooter-lane wall;
//! * (c) two balls colliding (one moving into one at rest, and head on);
//! * a deterministic `proptest` sweep (fixed ChaCha seed, D9) of rest and
//!   moving starts over the open playfield.
//!
//! Each scene runs 5 simulated seconds (or until a ball leaves the table,
//! e.g. drains). At every tick the total mechanical energy of the balls
//!
//! `E = Σ m (−g·p) + ½ m |v|² + ½ I |ω|²` with `I = 2/5 m r²`
//!
//! (`g` the solver's gravity vector: the pitch is in it, so `−m g·p` is
//! `m g h` along the pitch plus the normal component; position, velocity
//! and spin from the public [`Simulation::ball`] state; the spin *is*
//! exposed, so no rolling-ball 7/5 bound is needed) must not exceed the
//! lowest value recorded at any earlier tick (the start included) by more than `1e-6 J + N(t)`, where `N(t)` is the f32
//! rounding bound derived at [`rounding_bound`]. Collisions with static
//! geometry, flippers at rest, mechs at rest and other balls are counted:
//! they may only dissipate (energy handed to a movable body at rest, such as
//! a flipper pushed off its stop, is not in `E`, so it only lowers `E`; it
//! can at most give it back).
//!
//! Coils: every tick the test checks every coil of the table
//! (`MechSet::coil_on`) and the haptic events (coil fires and flipper
//! energise edges). If anything fires, the energy reference restarts after
//! the activity ends, so the assertion covers only coil-free windows; the
//! count of such ticks is printed (it is 0 in attract mode).
//!
//! Newton chains: the energy of a captive ball is outside `E`, and the
//! captive model has no single conserved energy. Impulses use the captive
//! mass `m`, so `1/2 m u^2` is conserved in the exchange, but gravity along
//! the captive's path is scaled by 5/7, so `7/10 m u^2` is conserved in
//! rolling. A free ball that hits a captive one can therefore get energy
//! back from the captive (and its end stop) without any violation. Ticks
//! with any captive ball faster than `CAPTIVE_ACTIVE_M_S`, or a free ball
//! within `2r + 0.1 mm` of a captive ball, are excluded exactly like coil
//! windows (the count is printed). A second reason is a deferred pin_physics
//! defect (a frozen ball at a receding captive still gets full gravity),
//! see DECISIONS "a1-bundle-passive — Newton captive windows excluded;
//! receding-captive gravity defect deferred"; this test does not cover it.
//!
//! Simulated time only (rule 5); deterministic (D9).

mod support;

use pin_assets::IdKind;
use pin_core::ids::CoilId;
use pin_core::snapshot::BallState;
use pin_sim::{BallHandle, SimConfig, Simulation};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use support::{greybox_bundle, on_big_stack};

/// Ball radius of the standard steel ball, mm.
const BALL_R_MM: f32 = 13.5;

/// 5 simulated seconds.
const TICKS: u64 = 5000;

/// Captive speed above which a Newton chain counts as active, m/s (the end
/// stop hold leaves about 2.9e-4 m/s).
const CAPTIVE_ACTIVE_M_S: f64 = 1.0e-3;

/// Fixed part of the tolerance, J (the brief).
const TOL_J: f64 = 1.0e-6;

/// f32 unit roundoff (half an ulp relative to the value), `2^-24`.
const U32: f64 = 5.960_464_477_539_063e-8;

/// Upper bound on the solver's sub-steps per tick that the noise bound
/// assumes (the ball solver's `max_substeps`, pin_physics `BallSolverConfig`
/// default 64).
const MAX_SUBSTEPS: f64 = 64.0;

fn mm3(x: f32, y: f32) -> [f32; 3] {
    [x * 1.0e-3, y * 1.0e-3, BALL_R_MM * 1.0e-3]
}

fn load() -> Simulation {
    let mut s = Simulation::load(greybox_bundle(), SimConfig::default()).unwrap();
    s.advance(10);
    s
}

/// Mass, moment of inertia and radius of a standard steel ball.
fn ball_constants() -> (f64, f64) {
    let p = pin_core::builtin_profile("standard_steel").unwrap();
    (f64::from(p.mass_kg), f64::from(p.moment_of_inertia()))
}

/// The solver's gravity vector (table frame, pitch included), m/s².
fn gravity(s: &Simulation) -> [f64; 3] {
    let g = s.solver().config().gravity;
    [f64::from(g.x), f64::from(g.y), f64::from(g.z)]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn f64v(a: [f32; 3]) -> [f64; 3] {
    [f64::from(a[0]), f64::from(a[1]), f64::from(a[2])]
}

/// Total mechanical energy of one ball, J (potential relative to the
/// table origin).
fn energy(b: &BallState, g: [f64; 3], m: f64, i: f64) -> f64 {
    let p = f64v(b.position_m);
    let v = f64v(b.velocity_m_s);
    let w = f64v(b.angular_velocity_rad_s);
    -m * dot(g, p) + 0.5 * m * dot(v, v) + 0.5 * i * dot(w, w)
}

/// Per-tick f32 rounding bound of the energy of one ball, J.
///
/// Derivation: the solver keeps position `p`, velocity `v` and spin `ω` in
/// f32. Every store of a component `x` is exact to `u·|x|` (`u = 2^-24`, half
/// an ulp). A rounding `δp` of the position changes the potential by
/// `m |g·δp| ≤ m |g| u ‖p‖₁`; a rounding `δv` changes `½ m v²` by
/// `m |v·δv| + O(δv²) ≤ m u Σ v_k² = 2 u KE_t`, likewise `2 u KE_r` for the
/// spin. Each sub-step stores each quantity at most a few times (integrate,
/// contact solve, position correction): 4 stores per sub-step are assumed,
/// and at most [`MAX_SUBSTEPS`] sub-steps per tick. So
///
/// `η = 4 · S · u · (m |g| ‖p‖₁ + 2 KE_t + 2 KE_r)` per tick, with `S`
/// the sub-step count bound, `S = min(64, 1 + |v| · 1 ms / (0.25 r))` (the
/// solver's travel-per-sub-step rule: a quarter radius).
///
/// Worst case the per-tick errors add up linearly, so `N(t) = Σ η` over the
/// ticks so far. Over 5 s with a 3 m/s ball this is of the order of 1e-4 J,
/// well under the energy any coil or a restitution above 1 would add (a
/// 1.5 m/s kick of a steel ball is 0.09 J).
fn rounding_bound(b: &BallState, g: [f64; 3], m: f64, i: f64, r: f64) -> f64 {
    let p = f64v(b.position_m);
    let v = f64v(b.velocity_m_s);
    let w = f64v(b.angular_velocity_rad_s);
    let gl = dot(g, g).sqrt();
    let p1 = p[0].abs() + p[1].abs() + p[2].abs();
    let speed = dot(v, v).sqrt();
    let subs = (1.0 + speed * 1.0e-3 / (0.25 * r)).ceil().min(MAX_SUBSTEPS);
    4.0 * subs * U32 * (m * gl * p1 + m * dot(v, v) + i * dot(w, w))
}

/// Every coil id of the table.
fn coils(s: &Simulation) -> Vec<CoilId> {
    s.ids()
        .entries(IdKind::Coil)
        .iter()
        .map(|(_, id)| CoilId(*id))
        .collect()
}

/// What a scene measured.
#[derive(Debug, Clone, Copy)]
struct SceneResult {
    /// Start energy, J.
    e0: f64,
    /// Largest `E(t) − E_ref` seen, J.
    max_rise: f64,
    /// Tick (1-based) at which `max_rise` was seen.
    tick_at_max: u64,
    /// The tolerance at the tick of `max_rise` (`1e-6 + N(t)`), J.
    tol_at_max: f64,
    /// Largest `E(t) − E_ref − tol(t)` (must be ≤ 0).
    worst_margin: f64,
    /// Lowest energy seen relative to the start, J.
    min_rel: f64,
    /// Final noise bound N, J.
    final_bound: f64,
    /// Largest single-tick energy increase seen in a coil-free window, J
    /// (the observed numerical noise).
    max_step_rise: f64,
    /// Ticks run.
    ticks: u64,
    /// Ticks with a coil on or a haptic event (excluded windows).
    coil_ticks: u64,
    /// Ticks with a Newton captive moving or a free ball touching one
    /// (excluded windows; may overlap `coil_ticks`).
    chain_ticks: u64,
}

/// Spawns `balls` (position, velocity) on layer 0, runs up to [`TICKS`]
/// and checks the energy at every tick. Removes the balls afterwards.
fn run_scene(s: &mut Simulation, balls: &[([f32; 3], [f32; 3])]) -> SceneResult {
    let (m, i) = ball_constants();
    let r = f64::from(BALL_R_MM) * 1.0e-3;
    let g = gravity(s);
    let coil_ids = coils(s);
    let escaped0 = s.counters().balls_escaped;
    let hs: Vec<BallHandle> = balls
        .iter()
        .map(|(p, v)| s.spawn_ball("standard_steel", *p, *v).unwrap())
        .collect();
    let total = |s: &Simulation| -> Option<(f64, f64)> {
        let mut e = 0.0;
        let mut n = 0.0;
        for h in &hs {
            let b = s.ball(*h)?;
            e += energy(&b, g, m, i);
            n += rounding_bound(&b, g, m, i, r);
        }
        Some((e, n))
    };
    let (e0, _) = total(s).unwrap();
    // Reference energy: the LOWEST E recorded at an earlier tick of the
    // current coil-free window (the start tick included). A later rise above
    // that minimum is a violation even if E stays under the start value.
    //
    // Noise choice: `noise` is reset to 0 whenever the minimum updates. The
    // rise `E(t) - E_min` only spans the ticks after the minimum was taken,
    // so the rounding errors that can account for it are the per-tick bounds
    // summed over those ticks alone; the older ones belong to the reference.
    let mut e_ref = e0;
    let mut noise = 0.0f64;
    let mut res = SceneResult {
        e0,
        max_rise: f64::NEG_INFINITY,
        tick_at_max: 0,
        tol_at_max: 0.0,
        worst_margin: f64::NEG_INFINITY,
        min_rel: 0.0,
        final_bound: 0.0,
        max_step_rise: f64::NEG_INFINITY,
        ticks: 0,
        coil_ticks: 0,
        chain_ticks: 0,
    };
    let mut in_coil_window = false;
    let mut e_prev = e0;
    for _ in 0..TICKS {
        s.advance(1);
        res.ticks += 1;
        let Some((e, n)) = total(s) else { break };
        assert!(
            e.is_finite() && n.is_finite(),
            "non-finite energy sample in scene {balls:?} at tick {}: E {e}, noise bound {n}",
            res.ticks
        );
        let step = e - e_prev;
        e_prev = e;
        let coil_active =
            !s.haptic_events().is_empty() || coil_ids.iter().any(|c| s.mech_set().coil_on(*c));
        let mut chain_active = false;
        for k in 0..s.solver().newton_chain_count() {
            let chain = s.solver().newton_chain(k);
            for j in 0..chain.len() {
                if f64::from(chain.ball_speed(j)).abs() > CAPTIVE_ACTIVE_M_S {
                    chain_active = true;
                }
                let c = chain.ball_position(j);
                for h in &hs {
                    let Some(b) = s.ball(*h) else { continue };
                    let dx = f64::from(b.position_m[0] - c.x);
                    let dy = f64::from(b.position_m[1] - c.y);
                    let dz = f64::from(b.position_m[2] - c.z);
                    if (dx * dx + dy * dy + dz * dz).sqrt() < 2.0 * r + 1.0e-4 {
                        chain_active = true;
                    }
                }
            }
        }
        res.coil_ticks += u64::from(coil_active);
        res.chain_ticks += u64::from(chain_active);
        if coil_active || chain_active {
            in_coil_window = true;
            continue;
        }
        if in_coil_window {
            // A new coil-free window starts here.
            in_coil_window = false;
            e_ref = e;
            noise = 0.0;
            continue;
        }
        res.max_step_rise = res.max_step_rise.max(step);
        noise += n;
        let tol = TOL_J + noise;
        let rise = e - e_ref;
        if rise > res.max_rise {
            res.max_rise = rise;
            res.tol_at_max = tol;
            res.tick_at_max = res.ticks;
        }
        res.worst_margin = res.worst_margin.max(rise - tol);
        res.min_rel = res.min_rel.min(e - e0);
        if e < e_ref {
            e_ref = e;
            noise = 0.0;
        }
    }
    res.final_bound = noise;
    assert_eq!(
        s.counters().balls_escaped,
        escaped0,
        "a ball escaped the table in scene {balls:?}"
    );
    for h in hs {
        s.remove_ball(h);
    }
    res
}

fn check(label: &str, r: &SceneResult) {
    println!(
        "{label}: E0 {:.6} J, max rise {:+.3e} J (tol there {:.3e} J), lowest {:+.4e} J, \
         final N {:.3e} J, largest one-tick rise {:+.3e} J, {} ticks, {} coil ticks, {} chain ticks",
        r.e0,
        r.max_rise,
        r.tol_at_max,
        r.min_rel,
        r.final_bound,
        r.max_step_rise,
        r.ticks,
        r.coil_ticks,
        r.chain_ticks
    );
    assert!(
        r.worst_margin <= 0.0,
        "{label}: energy rose above the start by more than 1e-6 J + N(t): {r:?}"
    );
}

/// (a) A ball released at rest at five points of the open playfield.
#[test]
fn ball_released_at_rest_never_gains_energy() {
    on_big_stack(|| {
        let mut s = load();
        for (x, y) in [
            (257.0, 400.0),
            (150.0, 420.0),
            (360.0, 400.0),
            (257.0, 640.0),
            (120.0, 650.0),
        ] {
            let r = run_scene(&mut s, &[(mm3(x, y), [0.0; 3])]);
            check(&format!("rest at ({x}, {y}) mm"), &r);
            assert_eq!(r.coil_ticks, 0, "attract mode fires no coil");
        }
    });
}

/// (b) A ball fired at 1, 2 and 3 m/s at a bumper body, at a sling face at
/// rest (sling coil not fired) and at a wall.
#[test]
fn ball_fired_at_bumper_sling_and_wall_never_gains_energy() {
    on_big_stack(|| {
        let mut s = load();
        let d = pin_core::PhysicsDefaults::builtin().mechs;
        // bumper_1 (table.toml): ring axis (190, 720) mm; start 40 mm below
        // the ring, moving straight at it.
        let ring = d.bumper.radius * 1.0e3;
        let bump = (190.0f32, 720.0f32);
        // sling_left: footprint centre (100, 290) mm, kick direction 35°;
        // start 40 mm off the face on its normal, moving at it.
        let a = 35.0f32.to_radians();
        let u = (a.cos(), a.sin());
        let face = 0.5 * d.sling.thickness * 1.0e3;
        let sling = (100.0f32, 290.0f32);
        for speed in [1.0f32, 2.0, 3.0] {
            let gap = 40.0;
            let p = mm3(bump.0, bump.1 - ring - BALL_R_MM - gap);
            let r = run_scene(&mut s, &[(p, [0.0, speed, 0.0])]);
            check(&format!("bumper_1 body at {speed} m/s"), &r);
            assert_eq!(r.coil_ticks, 0);

            let off = face + BALL_R_MM + gap;
            let p = mm3(sling.0 + u.0 * off, sling.1 + u.1 * off);
            let r = run_scene(&mut s, &[(p, [-u.0 * speed, -u.1 * speed, 0.0])]);
            check(&format!("sling_left face (at rest) at {speed} m/s"), &r);
            assert_eq!(r.coil_ticks, 0);

            // The shooter-lane wall (x = 470 mm, layout.toml).
            let r = run_scene(&mut s, &[(mm3(400.0, 380.0), [speed, 0.0, 0.0])]);
            check(&format!("shooter-lane wall at {speed} m/s"), &r);
            assert_eq!(r.coil_ticks, 0);
        }
    });
}

/// (c) Two balls colliding.
#[test]
fn two_colliding_balls_never_gain_energy() {
    on_big_stack(|| {
        let mut s = load();
        let r = run_scene(
            &mut s,
            &[
                (mm3(150.0, 400.0), [2.0, 0.0, 0.0]),
                (mm3(260.0, 400.0), [0.0; 3]),
            ],
        );
        check("ball at 2 m/s into a ball at rest", &r);
        assert_eq!(r.coil_ticks, 0);
        let r = run_scene(
            &mut s,
            &[
                (mm3(150.0, 420.0), [1.5, 0.0, 0.0]),
                (mm3(360.0, 420.0), [-1.5, 0.0, 0.0]),
            ],
        );
        check("two balls head on at 1.5 m/s each", &r);
        assert_eq!(r.coil_ticks, 0);
    });
}

/// Deterministic property sweep (fixed ChaCha seed, D9): a ball starting
/// anywhere in the open middle of the playfield (x 120..360, y 360..660 mm,
/// clear of the drop target and the Newton ball)
/// with a velocity of up to 3 m/s in any direction never gains energy.
#[test]
fn sweep_of_starts_never_gains_energy() {
    on_big_stack(|| {
        let s = std::cell::RefCell::new(load());
        let cfg = Config {
            cases: 6,
            failure_persistence: None,
            ..Config::default()
        };
        let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &[0x4b; 32]);
        let mut runner = TestRunner::new_with_rng(cfg, rng);
        let strat = (
            120.0f32..360.0,
            360.0f32..660.0,
            0.0f32..3.0,
            0.0f32..std::f32::consts::TAU,
        );
        runner
            .run(&strat, |(x, y, speed, dir)| {
                // Not overlapping the drop target (200, 470) or the Newton
                // ball (330, 450..500) at the start.
                prop_assume!((x - 200.0).hypot(y - 470.0) > 45.0);
                prop_assume!((x - 330.0).hypot(y - 475.0) > 60.0);
                let v = [speed * dir.cos(), speed * dir.sin(), 0.0];
                let r = run_scene(&mut s.borrow_mut(), &[(mm3(x, y), v)]);
                check(
                    &format!("sweep ({x:.1}, {y:.1}) mm, {speed:.2} m/s at {dir:.2} rad"),
                    &r,
                );
                prop_assert!(r.worst_margin <= 0.0);
                Ok(())
            })
            .unwrap();
    });
}
