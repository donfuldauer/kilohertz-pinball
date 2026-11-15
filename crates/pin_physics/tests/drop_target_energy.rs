//! a1-glass, test (iii): guard on the drop-target reset contact.
//!
//! A reset drop target rises as a kinematic box at `kick_speed` (v_rise).
//! A steel ball that meets it (rolling onto the rising edge, or sitting on
//! it) can be thrown upward, but only as a rigid-body contact allows:
//!
//! * **Curb bound.** One contact with a body moving at `v_rise·ẑ`, normal
//!   `n` (from the box to the ball centre), restitution `e`, Coulomb `μ`:
//!   the normal impulse per kg is `J = (1 + e)·max(0, (v_box − v)·n)` and
//!   the friction impulse is at most `μ·J` along a tangent whose `z`
//!   component is at most `|n_h| = √(1 − n_z²)`. So
//!   `vz' ≤ vz + J·(n_z + μ·|n_h|)`; with the worst horizontal direction
//!   `(v_box − v)·n = (v_rise − vz)·n_z + |v_h|·|n_h|`. The bound is the
//!   maximum over `n_z ∈ [−1, 1]` (f64). Every tick in which the ball is in
//!   reach of the target is checked against the bound of the ball's state
//!   at the start of that tick, with 2 % on top.
//! * **No energy gain.** Per tick in reach, the ball's mechanical energy
//!   per kg (`½|v|² + ⅕r²|ω|² − g·x`) may grow by at most the work of the
//!   moving box, `v_box·Δp_z` per kg, where `Δp_z = Δvz − g_z·dt` is the
//!   whole non-gravity `z` impulse of the tick (static colliders do no work
//!   and only push up here, so counting their impulse too only loosens the
//!   check) and `v_box = v_rise` while the target rises, else 0.
//!
//! Only the public `pin_physics` API is used (MechSet, BallSolver).

use glam::{Quat, Vec3};
use pin_core::ids::{CoilId, ProfileId, SwitchId};
use pin_core::materials::Material;
use pin_core::profiles::builtin_profiles;
use pin_core::schema::table::MechKind;
use pin_core::{BallProfile, PhysicsDefaults};
use pin_physics::ball::{Ball, BallEvents, BallHandle, BallSolver, BallSolverConfig, Geometry};
use pin_physics::mech::{MechSet, MechSpec};
use pin_physics::world::{StaticShape, StaticWorld, StaticWorldBuilder, layers};

const DT: f32 = 1.0e-3;
const STEEL: ProfileId = ProfileId(0);
const RESET: CoilId = CoilId(3);
const TARGET_SW: SwitchId = SwitchId(28);
/// The grey-box table pitch, degrees.
const PITCH_DEG: f32 = 6.5;
/// Reset coil pulse of the grey-box rules, ms.
const PULSE_MS: u16 = 150;
/// A moving ball starts this far before the target centre, m (clear of the
/// 2.5 × 12.5 mm half footprint plus the ball radius).
const APPROACH_M: f32 = 0.04;

/// Energy tolerance per tick, J/kg: f32 rounding only. The energy terms
/// are at most ½·3.5² ≈ 6 J/kg (ulp ≈ 5e-7) and `g·x` ≈ 1 J/kg (ulp ≈
/// 1e-7), so one tick's difference of two rounded energies is off by a few
/// 1e-7 J/kg at most; 1e-5 J/kg leaves a 10× margin. Gravity integration
/// itself (semi-implicit Euler) loses `½(g·dt)² ≈ 4.8e-5 J/kg` per tick, so
/// it never needs an allowance. A launch above the glass needs 0.1–1 J/kg.
const ENERGY_TOL: f32 = 1.0e-5;

fn profiles() -> Vec<BallProfile> {
    builtin_profiles().collect()
}

fn radius() -> f32 {
    profiles()[STEEL.index()].radius_m()
}

fn gravity() -> Vec3 {
    let p = PITCH_DEG.to_radians();
    Vec3::new(0.0, -9.81 * libm::sinf(p), -9.81 * libm::cosf(p))
}

/// Wood playfield (top at z = 0) of ±0.5 m.
fn world() -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: Vec3::new(0.0, 0.0, -0.05),
            half_extents: Vec3::new(0.5, 0.5, 0.05),
            rotation: Quat::IDENTITY,
        },
        layers::MAIN_DECK,
        true,
    );
    b.build()
}

struct Table {
    world: StaticWorld,
    solver: BallSolver,
    mechs: MechSet,
    events: BallEvents,
}

impl Table {
    /// One drop target at the origin, face normal ±x (angle 0).
    fn new() -> Table {
        let d = PhysicsDefaults::builtin();
        let mut cfg = BallSolverConfig::from_defaults(&d);
        cfg.gravity = gravity();
        let mut spec = MechSpec::new(MechKind::DropTarget, Vec3::ZERO)
            .with_coil(RESET)
            .with_switch(TARGET_SW);
        spec.params.angle_deg = Some(0.0);
        Table {
            world: world(),
            solver: BallSolver::new(cfg, &profiles()),
            mechs: MechSet::new(&[spec], &d).expect("drop target"),
            events: BallEvents::new(),
        }
    }

    fn tick(&mut self) {
        self.events.clear();
        let geo = Geometry::new(&self.world);
        self.mechs
            .step(&mut self.solver, &geo, None, DT, &mut self.events);
    }

    fn rising(&self) -> bool {
        self.mechs.drop_target_down(0) == Some(true) && self.mechs.coil_on(RESET)
    }

    /// Knocks the target down with a face hit and clears the table.
    fn knock_down(&mut self) {
        let mut b = Ball::new(STEEL, Vec3::new(0.06, 0.0, radius()));
        b.velocity = Vec3::new(-1.2, 0.0, 0.0);
        let h = self.solver.add_ball(b).expect("slot");
        for _ in 0..400 {
            self.tick();
            if self.mechs.drop_target_down(0) == Some(true) {
                break;
            }
        }
        assert_eq!(self.mechs.drop_target_down(0), Some(true), "not dropped");
        self.solver.remove_ball(h);
        // Let the hole settle.
        for _ in 0..5 {
            self.tick();
        }
    }
}

/// Mechanical energy per kg.
fn energy(b: &Ball, r: f32) -> f32 {
    0.5 * b.velocity.length_squared() + 0.2 * r * r * b.angular_velocity.length_squared()
        - gravity().dot(b.position)
}

/// The curb bound on `vz` after one contact with a box rising at `v_rise`
/// for a ball arriving with velocity `v` (see the module docs).
fn curb_bound(v: Vec3, v_rise: f32, e: f32, mu: f32) -> f64 {
    let (vz, vh) = (f64::from(v.z), f64::from(v.x.hypot(v.y)));
    let (v_rise, e, mu) = (f64::from(v_rise), f64::from(e), f64::from(mu));
    let mut best = vz;
    let n = 4000;
    for i in 0..=n {
        let nz = -1.0 + 2.0 * f64::from(i) / f64::from(n);
        let nh = (1.0 - nz * nz).max(0.0).sqrt();
        let approach = (v_rise - vz) * nz + vh * nh;
        let j = (1.0 + e) * approach.max(0.0);
        best = best.max(vz + j * (nz + mu * nh));
    }
    best
}

#[derive(Debug, Clone, Copy)]
struct Worst {
    /// Largest `vz' / bound` over the contact ticks.
    ratio: f64,
    /// Largest energy gain above the box's work `v_box·Δp_z`, J/kg (before
    /// the tolerance; negative when every tick lost energy).
    excess: f32,
    peak_vz: f32,
    contact_ticks: u32,
}

impl Default for Worst {
    fn default() -> Self {
        Self {
            ratio: 0.0,
            excess: f32::NEG_INFINITY,
            peak_vz: 0.0,
            contact_ticks: 0,
        }
    }
}

/// Places a rolling ball so it arrives over the target centre (offset
/// `off` sideways) `phase` ticks after the rise starts; follows it.
fn trial(speed: f32, angle_deg: f32, phase: i32, off: f32, e: f32, mu: f32) -> Worst {
    let d = PhysicsDefaults::builtin();
    let v_rise = d.mechs.drop_target.kick_speed;
    let half_len = 0.5 * d.mechs.drop_target.length;
    let half_thk = 0.5 * d.mechs.drop_target.thickness;
    let r = radius();
    let mut t = Table::new();
    t.knock_down();
    let a = angle_deg.to_radians();
    let dir = Vec3::new(a.cos(), a.sin(), 0.0);
    let side = Vec3::new(-dir.y, dir.x, 0.0);
    // A ball at rest is placed at its offset before the rise (a ball
    // placed inside the rising box would be pushed out by depenetration,
    // not by a contact); a moving one starts APPROACH_M back along its
    // path, clear of the footprint, so it arrives `phase` ticks into the
    // rise.
    let (start, fire_at) = if speed == 0.0 {
        (side * off, 1)
    } else {
        let lead = (APPROACH_M / speed / DT).round() as i32;
        (side * off - dir * APPROACH_M, lead - phase)
    };
    // `fire_at` counts ticks after placing the ball (may be < 0).
    let mut b = Ball::new(STEEL, start + Vec3::Z * r);
    b.velocity = dir * speed;
    b.angular_velocity = Vec3::new(-b.velocity.y / r, b.velocity.x / r, 0.0);
    let mut k = 0i32;
    if fire_at < 0 {
        t.mechs.fire_coil(RESET, PULSE_MS);
        for _ in fire_at..0 {
            t.tick();
        }
    }
    let h: BallHandle = t.solver.add_ball(b).expect("slot");
    let mut w = Worst::default();
    let mut prev = *t.solver.ball(h).unwrap();
    for _ in 0..700 {
        if k == fire_at {
            t.mechs.fire_coil(RESET, PULSE_MS);
        }
        let rising_before = t.rising();
        t.tick();
        k += 1;
        let Some(&now) = t.solver.ball(h) else {
            break;
        };
        // In reach of the target's box (footprint + radius + 1 mm), widened
        // by the farthest the ball can travel in one tick (up to 3.5 mm at
        // 3.5 m/s). A ball that starts the tick outside a fixed margin can
        // still hit during it; skipping that tick would leave the next
        // baseline post-impact and hide an excessive impulse.
        let p = prev.position;
        let reach = r + 1.0e-3 + DT * prev.velocity.length().max(now.velocity.length());
        let near = p.x.abs() <= half_thk + reach && p.y.abs() <= half_len + reach;
        if near {
            w.contact_ticks += 1;
            let bound = curb_bound(prev.velocity, v_rise, e, mu);
            if bound > 0.0 {
                w.ratio = w.ratio.max(f64::from(now.velocity.z) / bound);
            }
            let v_box = if rising_before || t.rising() {
                v_rise
            } else {
                0.0
            };
            let dpz = now.velocity.z - prev.velocity.z - gravity().z * DT;
            let work = v_box * dpz.max(0.0);
            w.excess = w.excess.max(energy(&now, r) - energy(&prev, r) - work);
        }
        w.peak_vz = w.peak_vz.max(now.velocity.z);
        prev = now;
    }
    w
}

#[test]
fn reset_contact_obeys_the_curb_bound_and_gains_no_energy() {
    let d = PhysicsDefaults::builtin();
    let pair = d
        .material_pair(Material::Steel, Material::Plastic)
        .expect("steel–plastic pair");
    let (e, mu) = (pair.restitution, pair.friction);
    let mut worst = Worst::default();
    let mut worst_at = String::new();
    let mut worst_e_at = String::new();
    let mut n = 0;
    let mut contacts = 0u64;
    let mut peak = 0.0f32;
    for speed in [0.0f32, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5] {
        for angle in (0..360).step_by(45) {
            for phase in (0..=64).step_by(4) {
                for off in [-0.010f32, 0.0, 0.010] {
                    let w = trial(speed, angle as f32, phase, off, e, mu);
                    n += 1;
                    contacts += u64::from(w.contact_ticks);
                    peak = peak.max(w.peak_vz);
                    let at = format!("v={speed} angle={angle} phase={phase} off={off}");
                    if w.ratio > worst.ratio {
                        worst.ratio = w.ratio;
                        worst_at = at.clone();
                    }
                    if w.excess > worst.excess {
                        worst.excess = w.excess;
                        worst_e_at = at;
                    }
                }
            }
        }
    }
    println!(
        "{n} trials, {contacts} ticks in reach, peak vz {peak:.3} m/s; worst vz/bound {:.4} ({worst_at}); \
         worst ΔE − v_box·Δp_z {:.2e} J/kg ({worst_e_at})",
        worst.ratio, worst.excess
    );
    assert!(contacts > 0, "no ball ever reached the target");
    // The rising box must actually launch balls, or the guard tests nothing.
    assert!(peak > v_rise_floor(), "peak vz {peak}");
    assert!(
        worst.ratio <= 1.02,
        "vz exceeds the curb bound + 2 %: ratio {} at {worst_at}",
        worst.ratio
    );
    assert!(
        worst.excess <= ENERGY_TOL,
        "a contact gains energy: {} J/kg above v_box·Δp_z (tolerance {ENERGY_TOL}) at {worst_e_at}",
        worst.excess
    );
}

/// The rise speed: a ball resting on the target leaves at least this fast.
fn v_rise_floor() -> f32 {
    PhysicsDefaults::builtin().mechs.drop_target.kick_speed
}

#[test]
fn curb_bound_matches_the_closed_forms() {
    // Ball at rest: the top face (n_z ≈ 1) gives (1 + e)·v_rise, slightly
    // more with friction near vertical.
    let b = curb_bound(Vec3::ZERO, 0.5, 0.5, 0.2);
    assert!((0.75..0.76).contains(&b), "{b}");
    // Frictionless, static box, ball at speed s: max of (1+e)·s·n_h·n_z is
    // (1+e)·s/2 at 45°.
    let b = curb_bound(Vec3::new(2.0, 0.0, 0.0), 0.0, 0.5, 0.0);
    assert!((b - 1.5).abs() < 1.0e-3, "{b}");
    // Frictionless: a ball rising faster than the box is only reached from
    // above (n_z < 0), which cannot raise vz.
    let b = curb_bound(Vec3::new(0.0, 0.0, 3.0), 0.5, 0.5, 0.0);
    assert!((b - 3.0).abs() < 1.0e-9, "{b}");
    // With friction such a contact can add at most a little: the bound
    // stays within μ·(1 + e)·(vz − v_rise) of vz.
    let b = curb_bound(Vec3::new(0.0, 0.0, 3.0), 0.5, 0.5, 0.2);
    assert!((3.0..=3.0 + 0.2 * 1.5 * 2.5).contains(&b), "{b}");
}
