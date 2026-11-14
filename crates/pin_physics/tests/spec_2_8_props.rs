//! Subphase 2.8 spec tests (Independent Tester, black box): property sweeps
//! over ball diameters 15.88–79.4 mm (deliverables 1 and 3).
//!
//! * no tunnelling through thin walls, thin posts and guide walls at up to
//!   30 m/s (the 2.2 spec's top speed), in free flight and rolling on a floor;
//! * resting contact is stable for every diameter;
//! * ball–ball collisions between any two diameters keep the 2.2 momentum
//!   tolerances;
//! * a 27 mm ball behaves exactly as before where a black-box test can
//!   tell: a profile with standard_steel's numbers under another name and
//!   profile index reproduces the built-in ball bit for bit, and the
//!   presence of larger profiles in the solver's table does not change it.
//!
//! Only merged public API is used.

#[path = "spec_ball_common.rs"]
mod bc;

use bc::DT;
use glam::{DVec3, Quat, Vec3};
use pin_core::{BallProfile, ProfileId};
use pin_physics::ball::{Ball, BallEvent, BallSolver, BallSolverConfig, ImpactTarget};
use pin_physics::world::{
    CubicSpline, GuideCurve, StaticShape, StaticWorld, StaticWorldBuilder, layers,
};
use proptest::prelude::*;

const D_MIN: f32 = 0.01588;
const D_MAX: f32 = 0.0794;
/// Top ball speed of the sweeps (as in the 2.2 tunnelling spec), m/s.
const V_MAX: f32 = 30.0;

/// A ball profile of diameter `d` (m) made from the built-in `base` (its
/// material, restitution and couplings) with the mass of a solid ball of
/// `density` kg/m³.
fn custom(base: &str, d: f32, density: f32) -> BallProfile {
    let mut p = bc::profile(base);
    p.name = format!("spec_{base}_{:.0}um", d * 1e6);
    p.diameter_m = d;
    p.mass_kg = density * std::f32::consts::PI / 6.0 * d * d * d;
    p
}

const BASES: [&str; 3] = ["standard_steel", "ceramic_powerball", "jumbo_xl_resin"];

/// The built-in profiles plus `extra`; the first extra profile is
/// `ProfileId(7)`.
fn table_with(extra: &[BallProfile]) -> Vec<BallProfile> {
    let mut v = bc::profiles();
    v.extend(extra.iter().cloned());
    v
}

fn first_extra() -> ProfileId {
    ProfileId(bc::profiles().len() as u16)
}

fn solver_for(profs: &[BallProfile], gravity: Vec3) -> BallSolver {
    let mut cfg = BallSolverConfig::from_defaults(&bc::defaults());
    cfg.gravity = gravity;
    BallSolver::new(cfg, profs)
}

fn diameter() -> impl Strategy<Value = f32> {
    prop_oneof![
        6 => D_MIN..=D_MAX,
        1 => Just(D_MIN),
        1 => Just(D_MAX),
        1 => Just(0.027f32),
        1 => Just(0.03175f32),
    ]
}

fn density() -> impl Strategy<Value = f32> {
    // Resin (1.7 g/cm³) to steel (7.85 g/cm³).
    1700.0f32..7850.0
}

// ------------------------------------------------------------ thin walls

/// A wall of half-thickness `half` (x ∈ [−half, +half]) spanning
/// y ∈ [−2, 2] m, z ∈ [−2, 2] m, and optionally a floor with its top at
/// z = `floor_top` on the −x side.
fn wall_world(half: f32, floor: bool) -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        bc::cuboid(Vec3::ZERO, Vec3::new(half, 2.0, 2.0)),
        layers::ALL,
        true,
    );
    if floor {
        b.add_collider(
            bc::cuboid(Vec3::new(-1.0, 0.0, -0.05), Vec3::new(1.0 - half, 2.0, 0.05)),
            layers::ALL,
            true,
        );
    }
    b.build()
}

#[allow(clippy::too_many_arguments)]
fn thin_wall_case(
    base: &str,
    d: f32,
    rho: f32,
    half: f32,
    speed: f32,
    dir: Vec3,
    w: Vec3,
    rolling_on_floor: bool,
) -> Result<f32, String> {
    let prof = custom(base, d, rho);
    let profs = table_with(std::slice::from_ref(&prof));
    let r = 0.5 * d;
    let world = wall_world(half, rolling_on_floor);
    let g = bc::geo(&world);
    let (mut s, dir) = if rolling_on_floor {
        (
            solver_for(&profs, Vec3::new(0.0, 0.0, -9.81)),
            Vec3::new(dir.x, dir.y, 0.0).normalize(),
        )
    } else {
        (solver_for(&profs, Vec3::ZERO), dir.normalize())
    };
    let run_up = 0.05f32;
    let z0 = if rolling_on_floor { r } else { 0.0 };
    let mut b = Ball::new(first_extra(), Vec3::new(-half - run_up - r, 0.0, z0));
    b.velocity = dir * speed;
    b.angular_velocity = if rolling_on_floor {
        // Rolling without slipping: ω = n × v / r.
        Vec3::Z.cross(b.velocity) / r
    } else {
        w
    };
    let h = s.add_ball(b).ok_or_else(|| "add_ball refused the ball".to_owned())?;
    let mut ev = bc::events();
    let vx = (dir * speed).x;
    let ticks = (run_up / vx / DT) as usize + 40;
    let tag = format!(
        "{base} d={:.2} mm rho={rho:.0} wall={:.2} mm v={speed} dir={dir:?} floor={rolling_on_floor}",
        d * 1e3,
        2.0 * half * 1e3
    );
    let mut max_x = f32::NEG_INFINITY;
    let mut hit = false;
    let mut rebounded = false;
    for tick in 0..ticks {
        ev.clear();
        s.step(&g, DT, &mut ev);
        hit |= ev.as_slice().iter().any(|e| {
            matches!(
                e,
                BallEvent::Impact {
                    other: ImpactTarget::Collider(_),
                    ..
                }
            )
        });
        let ball = s.ball(h).unwrap();
        let p = ball.position;
        rebounded |= ball.velocity.x < 0.0;
        if !(p.is_finite() && ball.velocity.is_finite() && ball.angular_velocity.is_finite()) {
            return Err(format!("tick {tick}: non-finite state ({tag})"));
        }
        max_x = max_x.max(p.x);
        if p.x > -half - r + 1e-4 {
            return Err(format!(
                "tick {tick}: tunnelled/penetrated, centre x = {} m, limit {} ({tag})",
                p.x,
                -half - r
            ));
        }
        if rolling_on_floor && p.z < r - 1e-4 {
            return Err(format!(
                "tick {tick}: sank into the floor, z = {} ({tag})",
                p.z
            ));
        }
    }
    // (On the floor, topspin may turn a slow ball round again afterwards.)
    let v = s.ball(h).unwrap().velocity;
    if !rebounded {
        return Err(format!("no rebound off the wall, v = {v:?} ({tag})"));
    }
    if !hit && speed > 0.2 {
        return Err(format!("no static Impact event ({tag})"));
    }
    // Gap left at the closest approach.
    Ok(-half - r - max_x)
}

#[test]
fn no_tunnelling_thin_wall_fixed_grid_of_diameters() {
    // Deterministic grid: both ends, the built-in sizes and values between.
    let ds = [
        D_MIN, 0.020, 0.027, 0.03175, 0.040, 0.050, 0.060, 0.070, 0.0794,
    ];
    let mut n = 0;
    for &d in &ds {
        for &v in &[0.5f32, 3.0, 10.0, 20.0, V_MAX] {
            for &half in &[0.00025f32, 0.001] {
                for floor in [false, true] {
                    thin_wall_case("jumbo_xl_resin", d, 1700.0, half, v, Vec3::X, Vec3::ZERO, floor)
                        .unwrap_or_else(|e| panic!("{e}"));
                    thin_wall_case("standard_steel", d, 7850.0, half, v, Vec3::X, Vec3::ZERO, floor)
                        .unwrap_or_else(|e| panic!("{e}"));
                    n += 2;
                }
            }
        }
    }
    println!("thin wall grid: {n} cases, 9 diameters 15.88–79.4 mm, up to {V_MAX} m/s");
}

#[test]
fn no_tunnelling_builtin_xl_ball_at_max_speed() {
    // The built-in profile itself (not a clone), head-on and oblique.
    let world = wall_world(0.0005, false);
    let g = bc::geo(&world);
    let r = bc::profile("jumbo_xl_resin").radius_m();
    for &v in &[5.0f32, 15.0, V_MAX, 50.0] {
        for &(dy, dz) in &[(0.0f32, 0.0f32), (0.8, 0.0), (0.0, -0.9), (0.6, 0.6)] {
            let mut s = bc::solver_with_gravity(Vec3::ZERO);
            let dir = Vec3::new(1.0, dy, dz).normalize();
            let mut b = bc::ball("jumbo_xl_resin", Vec3::new(-0.0005 - 0.05 - r, 0.0, 0.0));
            b.velocity = dir * v;
            let h = s.add_ball(b).unwrap();
            let mut ev = bc::events();
            for tick in 0..((0.05 / (dir.x * v) / DT) as usize + 30) {
                ev.clear();
                s.step(&g, DT, &mut ev);
                let p = s.ball(h).unwrap().position;
                assert!(
                    p.x <= -0.0005 - r + 1e-4,
                    "XL at {v} m/s dir {dir:?}: x = {} at tick {tick}",
                    p.x
                );
            }
            assert!(s.ball(h).unwrap().velocity.x < 0.0, "XL at {v}: no rebound");
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 160, .. ProptestConfig::default() })]

    #[test]
    fn no_tunnelling_thin_wall_any_diameter(
        base in 0usize..3,
        d in diameter(),
        rho in density(),
        half in 0.0002f32..0.0015,
        v in 0.3f32..V_MAX,
        max_speed in any::<bool>(),
        dy in -1.0f32..1.0, dz in -1.0f32..1.0,
        w in prop::array::uniform3(-300.0f32..300.0),
        floor in any::<bool>(),
    ) {
        let v = if max_speed { V_MAX } else { v };
        // Spin limited to a 15 m/s surface speed for the larger balls.
        let w = Vec3::from(w) * (0.0135 / (0.5 * d)).min(1.0);
        let r = thin_wall_case(BASES[base], d, rho, half, v, Vec3::new(1.0, dy, dz), w, floor);
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

// ------------------------------------------------------------ thin posts

/// A ball shot at a thin vertical post (radius `post_r`) with a lateral
/// offset: the centre never comes closer to the post axis than
/// `r + post_r` (less 0.1 mm).
fn post_case(base: &str, d: f32, rho: f32, post_r: f32, speed: f32, offset: f32) -> Result<(), String> {
    let prof = custom(base, d, rho);
    let profs = table_with(std::slice::from_ref(&prof));
    let r = 0.5 * d;
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cylinder {
            base: Vec3::new(0.0, 0.0, -1.0),
            axis: Vec3::new(0.0, 0.0, 2.0),
            radius: post_r,
        },
        layers::ALL,
        true,
    );
    let world = b.build();
    let g = bc::geo(&world);
    let mut s = solver_for(&profs, Vec3::ZERO);
    let reach = r + post_r;
    let mut ball = Ball::new(first_extra(), Vec3::new(-reach - 0.04, offset * reach, 0.0));
    ball.velocity = Vec3::new(speed, 0.0, 0.0);
    let h = s.add_ball(ball).ok_or_else(|| "add_ball refused the ball".to_owned())?;
    let mut ev = bc::events();
    let ticks = ((2.0 * reach + 0.08) / speed / DT) as usize + 20;
    for tick in 0..ticks {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let p = s.ball(h).unwrap().position;
        if !p.is_finite() {
            return Err(format!("tick {tick}: non-finite position"));
        }
        let axis_d = Vec3::new(p.x, p.y, 0.0).length();
        if axis_d < reach - 1e-4 {
            return Err(format!(
                "tick {tick}: {base} d={:.2} mm at {speed} m/s, offset {offset}: centre {:.3} mm from the axis of a {:.2} mm post (contact at {:.3} mm)",
                d * 1e3,
                axis_d * 1e3,
                post_r * 1e3,
                reach * 1e3
            ));
        }
    }
    // A hit within the post's silhouette must have deflected the ball.
    if offset.abs() < 0.9 {
        let v = s.ball(h).unwrap().velocity;
        if (v - Vec3::new(speed, 0.0, 0.0)).length() < 1e-3 * speed {
            return Err(format!(
                "{base} d={:.2} mm at {speed} m/s, offset {offset}: passed a {:.2} mm post undeflected",
                d * 1e3,
                post_r * 1e3
            ));
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    #[test]
    fn no_tunnelling_thin_post_any_diameter(
        base in 0usize..3,
        d in diameter(),
        rho in density(),
        post_r in 0.001f32..0.006,
        v in 0.5f32..V_MAX,
        max_speed in any::<bool>(),
        offset in -1.2f32..1.2,
    ) {
        let v = if max_speed { V_MAX } else { v };
        let r = post_case(BASES[base], d, rho, post_r, v, offset);
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

// ------------------------------------------------------------ guide walls

/// Closed circular guide wall of radius `rad` and height `height`.
fn orbit(rad: f32, height: f32) -> GuideCurve {
    let pts: Vec<Vec3> = (0..24)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 24.0;
            Vec3::new(rad * libm::cosf(a), rad * libm::sinf(a), 0.0)
        })
        .chain(std::iter::once(Vec3::new(rad, 0.0, 0.0)))
        .collect();
    GuideCurve::new(CubicSpline::interpolate_closed(&pts).unwrap(), height).unwrap()
}

/// A ball on a floor inside a circular guide wall (a zero-thickness ribbon
/// of height `HEIGHT` over the circle): it never overlaps the ribbon and
/// never passes through it. With `rolling` the ball starts rolling without
/// slipping; wall friction on that spin can legitimately throw a fast ball
/// *over* the wall, so "escaped" is judged against the ribbon itself, not
/// against the circle.
fn guide_case(
    base: &str,
    d: f32,
    rho: f32,
    speed: f32,
    angle: f32,
    start: [f32; 2],
    rolling: bool,
) -> Result<(), String> {
    const HEIGHT: f32 = 0.12;
    let prof = custom(base, d, rho);
    let profs = table_with(std::slice::from_ref(&prof));
    let r = 0.5 * d;
    let rad = 0.3f32;
    let world = bc::floor_world();
    let guides = [orbit(rad, HEIGHT)];
    let g = bc::geo_full(&world, &guides, &[]);
    let mut s = solver_for(&profs, Vec3::new(0.0, 0.0, -9.81));
    let dir = Vec3::new(libm::cosf(angle), libm::sinf(angle), 0.0);
    let mut b = Ball::new(first_extra(), Vec3::new(start[0], start[1], r));
    b.velocity = dir * speed;
    if rolling {
        b.angular_velocity = Vec3::Z.cross(b.velocity) / r;
    }
    let h = s
        .add_ball(b)
        .ok_or_else(|| "add_ball refused the ball".to_owned())?;
    let mut ev = bc::events();
    let mut guide_hit = false;
    let ticks = (0.7 / speed / DT) as usize + 60;
    let tag = format!(
        "{base} d={:.2} mm at {speed} m/s, angle {angle}, rolling {rolling}",
        d * 1e3
    );
    let mut prev = (Vec3::new(start[0], start[1], 0.0).length(), r);
    for tick in 0..ticks {
        ev.clear();
        s.step(&g, DT, &mut ev);
        guide_hit |= ev.as_slice().iter().any(|e| {
            matches!(
                e,
                BallEvent::Impact {
                    other: ImpactTarget::Guide(0),
                    ..
                }
            )
        });
        let p = s.ball(h).unwrap().position;
        if !p.is_finite() {
            return Err(format!("tick {tick}: non-finite position ({tag})"));
        }
        let dist = Vec3::new(p.x, p.y, 0.0).length();
        // Distance from the centre to the ribbon {ρ = rad, 0 ≤ z ≤ HEIGHT}
        // (the spline circle is within 20 µm of the true one).
        let dz = p.z - p.z.clamp(0.0, HEIGHT);
        let to_wall = ((dist - rad) * (dist - rad) + dz * dz).sqrt();
        if to_wall < r - 2e-4 {
            return Err(format!(
                "tick {tick}: ball overlaps the guide wall by {:.3} mm (centre {:.3} mm from the middle at z = {:.3} mm, ring {:.0} mm, wall top {:.0} mm) ({tag})",
                (r - to_wall) * 1e3,
                dist * 1e3,
                p.z * 1e3,
                rad * 1e3,
                HEIGHT * 1e3
            ));
        }
        // Through the wall: inside → outside between two tick ends with the
        // centre below the wall top at both.
        if prev.0 < rad && dist >= rad && prev.1 < HEIGHT && p.z < HEIGHT {
            return Err(format!(
                "tick {tick}: ball went through the guide wall ({:.3} → {:.3} mm from the middle, z {:.3} → {:.3} mm) ({tag})",
                prev.0 * 1e3,
                dist * 1e3,
                prev.1 * 1e3,
                p.z * 1e3
            ));
        }
        prev = (dist, p.z);
        if p.z < r - 1e-4 && dist < rad {
            return Err(format!(
                "tick {tick}: sank into the floor, z = {} ({tag})",
                p.z
            ));
        }
    }
    if !guide_hit {
        return Err(format!("no Guide impact event ({tag})"));
    }
    Ok(())
}

#[test]
fn no_tunnelling_guide_wall_fixed_grid_of_diameters() {
    for &d in &[D_MIN, 0.027, 0.040, 0.055, 0.070, D_MAX] {
        for &v in &[1.0f32, 8.0, V_MAX] {
            for k in 0..5 {
                for rolling in [false, true] {
                    guide_case("jumbo_xl_resin", d, 1700.0, v, k as f32 * 1.3, [0.0, 0.0], rolling)
                        .unwrap_or_else(|e| panic!("{e}"));
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, .. ProptestConfig::default() })]

    #[test]
    fn no_tunnelling_guide_wall_any_diameter(
        base in 0usize..3,
        d in diameter(),
        rho in density(),
        v in 1.0f32..V_MAX,
        max_speed in any::<bool>(),
        angle in 0.0f32..6.28,
        sx in -0.1f32..0.1, sy in -0.1f32..0.1,
        rolling in any::<bool>(),
    ) {
        let v = if max_speed { V_MAX } else { v };
        let r = guide_case(BASES[base], d, rho, v, angle, [sx, sy], rolling);
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

// ------------------------------------------------------------ resting contact

/// Largest displacement of a ball set down on a level floor over `ticks`.
fn rest_drift(base: &str, d: f32, rho: f32, xy: [f32; 2], floor_top: f32, ticks: usize) -> Result<f32, String> {
    let prof = custom(base, d, rho);
    let profs = table_with(std::slice::from_ref(&prof));
    let r = prof.radius_m();
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        bc::cuboid(Vec3::new(0.0, 0.0, floor_top - 0.05), Vec3::new(2.0, 2.0, 0.05)),
        layers::ALL,
        true,
    );
    let world = b.build();
    let g = bc::geo(&world);
    let mut s = solver_for(&profs, Vec3::new(0.0, 0.0, -9.81));
    let p0 = Vec3::new(xy[0], xy[1], floor_top + r);
    let h = s
        .add_ball(Ball::new(first_extra(), p0))
        .ok_or_else(|| "add_ball refused the ball".to_owned())?;
    let mut ev = bc::events();
    let mut max = 0.0f32;
    for tick in 0..ticks {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let p = s.ball(h).unwrap().position;
        if !p.is_finite() {
            return Err(format!("tick {tick}: non-finite position"));
        }
        max = max.max((p - p0).length());
        if let Some(e) = ev
            .as_slice()
            .iter()
            .find(|e| matches!(e, BallEvent::Impact { .. }))
        {
            return Err(format!(
                "tick {tick}: resting {base} ball d={:.2} mm reported {e:?}",
                d * 1e3
            ));
        }
    }
    Ok(max)
}

#[test]
fn resting_contact_stable_fixed_grid_10_s() {
    let mut worst = 0.0f32;
    for &d in &[D_MIN, 0.027, 0.03175, 0.045, 0.060, D_MAX] {
        for (base, rho) in [("jumbo_xl_resin", 1700.0f32), ("standard_steel", 7850.0)] {
            let drift = rest_drift(base, d, rho, [0.1, -0.2], 0.0, 10_000)
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(
                drift < 1e-6,
                "{base} d={:.2} mm: drift {drift:e} m over 10 s",
                d * 1e3
            );
            worst = worst.max(drift);
        }
    }
    println!("resting grid: worst drift {worst:e} m over 10 s (limit 1e-6)");
}

/// A ball lying in the corner between the playfield and a bottom wall on a
/// 6.5° table (two simultaneous resting contacts): after `settle` ticks it
/// must be at rest and stay put. Returns (drift m, max speed m/s) after
/// settling.
fn corner_rest(base: &str, d: f32, rho: f32, x: f32) -> Result<(f32, f32), String> {
    let prof = custom(base, d, rho);
    let profs = table_with(std::slice::from_ref(&prof));
    let r = prof.radius_m();
    let pitch = 6.5f32.to_radians();
    let gravity = Vec3::new(0.0, -9.81 * libm::sinf(pitch), -9.81 * libm::cosf(pitch));
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        bc::cuboid(Vec3::new(0.0, 0.0, -0.05), Vec3::new(1.0, 1.0, 0.05)),
        layers::ALL,
        true,
    );
    // Bottom wall: its up-table face is the plane y = 0, 150 mm tall.
    b.add_collider(
        bc::cuboid(Vec3::new(0.0, -0.01, 0.075), Vec3::new(1.0, 0.01, 0.075)),
        layers::ALL,
        true,
    );
    let world = b.build();
    let g = bc::geo(&world);
    let mut s = solver_for(&profs, gravity);
    let h = s
        .add_ball(Ball::new(first_extra(), Vec3::new(x, r, r)))
        .ok_or_else(|| "add_ball refused the ball".to_owned())?;
    let mut ev = bc::events();
    let settle = 1500;
    let mut p_ref = Vec3::ZERO;
    let (mut drift, mut vmax) = (0.0f32, 0.0f32);
    for tick in 0..4500 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let ball = s.ball(h).unwrap();
        let p = ball.position;
        if !(p.is_finite() && ball.velocity.is_finite()) {
            return Err(format!("tick {tick}: non-finite state"));
        }
        if p.y < r - 1e-4 || p.z < r - 1e-4 {
            return Err(format!(
                "tick {tick}: {base} d={:.2} mm pushed into the corner: y = {}, z = {}, r = {r}",
                d * 1e3,
                p.y,
                p.z
            ));
        }
        if tick == settle {
            p_ref = p;
        }
        if tick >= settle {
            drift = drift.max((p - p_ref).length());
            vmax = vmax.max(ball.velocity.length());
        }
    }
    Ok((drift, vmax))
}

#[test]
fn resting_in_a_corner_is_as_stable_as_the_standard_ball() {
    // Reference: what the 27 mm, 80 g steel ball does in the same corner.
    let (d0, v0) = corner_rest("standard_steel", 0.027, 80.0e-3 / (std::f32::consts::PI / 6.0 * 0.027f32.powi(3)), 0.1)
        .unwrap_or_else(|e| panic!("{e}"));
    println!("corner rest, standard ball: drift {d0:e} m, max speed {v0:e} m/s");
    let (lim_d, lim_v) = (d0.max(1e-5), v0.max(1e-3));
    for &d in &[D_MIN, 0.020, 0.03175, 0.045, 0.060, D_MAX] {
        for (base, rho) in [("jumbo_xl_resin", 1700.0f32), ("standard_steel", 7850.0)] {
            let (drift, v) = corner_rest(base, d, rho, 0.1).unwrap_or_else(|e| panic!("{e}"));
            println!(
                "corner rest, {base} d={:.2} mm: drift {drift:e} m, max speed {v:e} m/s",
                d * 1e3
            );
            assert!(
                drift <= 10.0 * lim_d && v <= 10.0 * lim_v,
                "{base} d={:.2} mm: corner drift {drift:e} m, speed {v:e} m/s (standard ball: {d0:e}, {v0:e})",
                d * 1e3
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    #[test]
    fn resting_contact_stable_any_diameter(
        base in 0usize..3,
        d in diameter(),
        rho in density(),
        x in -1.0f32..1.0, y in -1.0f32..1.0,
        top in -0.2f32..0.2,
    ) {
        let r = rest_drift(BASES[base], d, rho, [x, y], top, 3000);
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
        let drift = r.unwrap();
        prop_assert!(drift < 1e-6, "{} d={} mm: drift {:e} m over 3 s", BASES[base], d * 1e3, drift);
    }
}

// ------------------------------------------------------------ ball–ball, any two diameters

fn pair_case(
    d1: f32, rho1: f32, d2: f32, rho2: f32,
    b: f32, vrel: f32, rot: Quat, w1: Vec3, w2: Vec3, v2: Vec3,
) -> Result<(), String> {
    let p1 = custom("standard_steel", d1, rho1);
    let mut p2 = custom("jumbo_xl_resin", d2, rho2);
    p2.name.push_str("_b");
    let profs = table_with(&[p1.clone(), p2.clone()]);
    let (i1, i2) = (first_extra(), ProfileId(first_extra().0 + 1));
    let reach = p1.radius_m() + p2.radius_m();
    let mut s = solver_for(&profs, Vec3::ZERO);
    let mut a = Ball::new(i1, rot * Vec3::new(-(reach + 0.02), b * reach, 0.0));
    a.velocity = rot * Vec3::new(vrel, 0.0, 0.0) + v2;
    a.angular_velocity = w1;
    let mut c = Ball::new(i2, Vec3::ZERO);
    c.velocity = v2;
    c.angular_velocity = w2;
    s.add_ball(a).ok_or_else(|| "add_ball refused the ball".to_owned())?;
    s.add_ball(c).ok_or_else(|| "add_ball refused the ball".to_owned())?;
    let world = bc::empty_world();
    let g = bc::geo(&world);
    let mut ev = bc::events();
    let totals = |s: &BallSolver| {
        let mut m_tot = 0.0f64;
        let (mut com, mut p) = (DVec3::ZERO, DVec3::ZERO);
        for (_, b) in s.balls() {
            let m = f64::from(profs[b.profile.index()].mass_kg);
            m_tot += m;
            com += b.position.as_dvec3() * m;
            p += b.velocity.as_dvec3() * m;
        }
        com /= m_tot;
        let (mut l, mut ke) = (DVec3::ZERO, 0.0f64);
        for (_, b) in s.balls() {
            let pr = &profs[b.profile.index()];
            let m = f64::from(pr.mass_kg);
            l += (b.position.as_dvec3() - com).cross(b.velocity.as_dvec3() * m)
                + b.angular_velocity.as_dvec3() * f64::from(pr.moment_of_inertia());
            ke += bc::kinetic(b, pr);
        }
        (p, l, ke)
    };
    let (p0, l0, ke0) = totals(&s);
    let mu = f64::from(p1.mass_kg * p2.mass_kg / (p1.mass_kg + p2.mass_kg));
    let p_scale = p0.length().max(mu * f64::from(vrel));
    let l_scale = l0.length().max(mu * f64::from(vrel) * f64::from(reach));
    let mut prev = ke0;
    let mut collided = false;
    let tag = format!(
        "d1={:.2} mm ({:.1} g) d2={:.2} mm ({:.1} g) b={b} v={vrel}",
        d1 * 1e3, p1.mass_kg * 1e3, d2 * 1e3, p2.mass_kg * 1e3
    );
    for tick in 0..((reach + 0.02) / vrel / DT) as usize + 100 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        collided |= ev.as_slice().iter().any(|e| matches!(e, BallEvent::Impact { .. }));
        let (p, l, ke) = totals(&s);
        if !(p.is_finite() && l.is_finite() && ke.is_finite()) {
            return Err(format!("tick {tick}: non-finite state ({tag})"));
        }
        let dp = (p - p0).length() / p_scale;
        let dl = (l - l0).length() / l_scale;
        if dp > 1e-4 {
            return Err(format!("tick {tick}: linear momentum drift {dp:e} ({tag})"));
        }
        if dl > 1e-3 {
            return Err(format!("tick {tick}: angular momentum drift {dl:e} ({tag})"));
        }
        if ke > prev * (1.0 + 1e-6) + 1e-12 {
            return Err(format!("tick {tick}: KE rose {prev:e} -> {ke:e} ({tag})"));
        }
        prev = ke;
        let ps: Vec<Vec3> = s.balls().map(|(_, x)| x.position).collect();
        let dist = (ps[0] - ps[1]).length();
        if dist < reach - 1e-4 {
            return Err(format!("tick {tick}: overlap, centres {dist} m apart, contact at {reach} ({tag})"));
        }
    }
    if b < 0.93 && !collided {
        return Err(format!("no impact reported ({tag})"));
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, .. ProptestConfig::default() })]

    #[test]
    fn ball_ball_momentum_any_two_diameters(
        d1 in diameter(), rho1 in density(),
        d2 in diameter(), rho2 in density(),
        b in 0.0f32..0.9,
        v in 0.3f32..20.0,
        axis in prop::array::uniform3(-1.0f32..1.0),
        angle in 0.0f32..6.28,
        w1 in prop::array::uniform3(-100.0f32..100.0),
        w2 in prop::array::uniform3(-100.0f32..100.0),
        v2 in prop::array::uniform3(-3.0f32..3.0),
    ) {
        let axis = Vec3::from(axis);
        let rot = if axis.length() > 0.05 { Quat::from_axis_angle(axis.normalize(), angle) } else { Quat::IDENTITY };
        let r = pair_case(d1, rho1, d2, rho2, b, v, rot, Vec3::from(w1), Vec3::from(w2), Vec3::from(v2));
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

// ------------------------------------------------------------ 27 mm unchanged

/// A busy single-ball run (floor, walls, posts, a guide arc): the state bits
/// after every 50th tick.
fn standard_trace(profs: &[BallProfile], id: ProfileId, v: Vec3, w: Vec3) -> Vec<[u32; 13]> {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(bc::floor_shape(), layers::ALL, true);
    for x in [-0.25f32, 0.25] {
        b.add_collider(bc::cuboid(Vec3::new(x, 0.0, 0.03), Vec3::new(0.001, 0.5, 0.04)), layers::ALL, true);
    }
    for y in [-0.5f32, 0.5] {
        b.add_collider(bc::cuboid(Vec3::new(0.0, y, 0.03), Vec3::new(0.25, 0.001, 0.04)), layers::ALL, true);
    }
    for (x, y) in [(-0.1f32, 0.2f32), (0.12, -0.1), (0.0, 0.33), (-0.15, -0.3)] {
        b.add_collider(
            StaticShape::Cylinder { base: Vec3::new(x, y, -0.005), axis: Vec3::Z * 0.05, radius: 0.004 },
            layers::ALL,
            true,
        );
    }
    let world = b.build();
    let pts: Vec<Vec3> = (0..7)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 6.0;
            Vec3::new(0.15 * libm::cosf(a), 0.1 + 0.1 * libm::sinf(a), 0.0)
        })
        .collect();
    let guides = [GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.05).unwrap()];
    let g = bc::geo_full(&world, &guides, &[]);
    let mut s = solver_for(profs, Vec3::new(0.0, -1.11, -9.75));
    let mut ball = Ball::new(id, Vec3::new(0.05, -0.2, 0.0135));
    ball.velocity = v;
    ball.angular_velocity = w;
    let h = s.add_ball(ball).unwrap();
    let mut ev = bc::events();
    let mut out = Vec::new();
    for tick in 0..3000 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        if tick % 50 == 49 {
            out.push(bc::state_bits(s.ball(h).unwrap()));
        }
    }
    out
}

#[test]
fn standard_27mm_ball_is_bit_identical_under_another_name_and_profile_table() {
    let builtin = bc::profiles();
    let std_id = bc::pid("standard_steel");
    // The same numbers under another name, after profiles of every size.
    let mut twin = bc::profile("standard_steel");
    twin.name = "spec_twin".to_owned();
    let big = table_with(&[
        custom("jumbo_xl_resin", 0.0794, 7850.0),
        custom("standard_steel", 0.05, 4000.0),
        twin,
    ]);
    let twin_id = ProfileId(builtin.len() as u16 + 2);
    // A table with only the standard profile.
    let only = vec![bc::profile("standard_steel")];
    for (v, w) in [
        (Vec3::new(1.3, 2.9, 0.0), Vec3::new(20.0, -10.0, 5.0)),
        (Vec3::new(-6.0, 7.5, 0.0), Vec3::ZERO),
        (Vec3::new(0.4, -18.0, 0.0), Vec3::new(0.0, 0.0, 150.0)),
    ] {
        let reference = standard_trace(&builtin, std_id, v, w);
        assert!(reference.len() == 60);
        assert!(
            reference == standard_trace(&big, std_id, v, w),
            "v={v:?}: standard_steel changed when larger profiles were added to the table"
        );
        assert!(
            reference == standard_trace(&big, twin_id, v, w),
            "v={v:?}: a profile with standard_steel's numbers under another name/index differs"
        );
        assert!(
            reference == standard_trace(&only, ProfileId(0), v, w),
            "v={v:?}: standard_steel differs in a one-profile table"
        );
    }
}

/// 2.2 behaviour that must hold unchanged for the 27 mm ball and that the
/// spec can state exactly: the steel–wood rebound off a wall is the pair
/// restitution, at every speed (a CCD margin that grew for the 27 mm ball
/// would show up as an early or soft bounce).
#[test]
fn standard_27mm_wall_rebound_still_the_pair_restitution() {
    let e = bc::defaults()
        .material_pair(pin_core::Material::Steel, pin_core::Material::Wood)
        .unwrap()
        .restitution;
    let world = wall_world(0.001, false);
    let g = bc::geo(&world);
    let r = 0.0135f32;
    for &v in &[0.5f32, 2.0, 8.0, 20.0, V_MAX] {
        let mut s = bc::solver_with_gravity(Vec3::ZERO);
        let mut b = bc::ball("standard_steel", Vec3::new(-0.001 - 0.05 - r, 0.0, 0.0));
        b.velocity = Vec3::new(v, 0.0, 0.0);
        let h = s.add_ball(b).unwrap();
        let mut ev = bc::events();
        let mut closest = f32::NEG_INFINITY;
        for _ in 0..((0.05 / v / DT) as usize + 20) {
            ev.clear();
            s.step(&g, DT, &mut ev);
            closest = closest.max(s.ball(h).unwrap().position.x);
        }
        let vx = s.ball(h).unwrap().velocity.x;
        assert!(
            (vx + e * v).abs() <= 0.02 * v,
            "27 mm ball at {v} m/s: rebound {vx}, expected {}",
            -e * v
        );
        // It really reached the wall (closest approach within a ball's
        // travel in one tick of touching).
        let gap = -0.001 - r - closest;
        assert!(
            (-1e-4..=v * DT + 1e-4).contains(&gap),
            "27 mm ball at {v} m/s: closest approach leaves {gap} m"
        );
    }
}
