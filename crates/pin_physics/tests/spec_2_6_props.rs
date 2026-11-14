//! Subphase 2.6 spec tests (Independent Tester): property tests of the
//! banana bat through `FlipperSet` (public API only):
//!
//! * `closest_point` against brute-force sampling of the analytic tube,
//!   with outward unit normals on the concave face, the convex face, the
//!   tip cap and the base cap (and inside the rubber);
//! * no tunnelling through the arc at the maximum angular velocity, over
//!   contact positions along the arc, both faces, the tip cap, ball speeds
//!   and flip timings;
//! * the 2.3 energy bound (Orchestrator ruling "2.3 energy bound for a
//!   per-tick drive impulse", and `drive_work_j`'s documented bound).

#[path = "spec_2_6_common.rs"]
mod common;

use common::*;
use glam::{Quat, Vec3};
use pin_core::ids::FlipperId;
use pin_core::{FlipperConfig, FlipperShape};
use pin_physics::ball::MovingBodies;
use proptest::prelude::*;

const ID: FlipperId = FlipperId(0);

fn unit(theta: f32, phi: f32) -> Vec3 {
    v(
        libm::sinf(theta) * libm::cosf(phi),
        libm::sinf(theta) * libm::sinf(phi),
        libm::cosf(theta),
    )
}

// --------------------------------------------------------- closest point

#[derive(Copy, Clone, Debug, PartialEq)]
enum Region {
    Concave,
    Convex,
    TipCap,
    BaseCap,
}

/// Builds a banana flipper on an arbitrary mount and checks
/// `closest_point` at one query point given in bat-frame terms.
#[allow(clippy::too_many_arguments)]
fn closest_case(
    b: Banana,
    neg_sweep: bool,
    neg_bend: bool,
    n: Vec3,
    rest: f32,
    region: Region,
    frac: f32,
    beta_deg: f32,
    offset: f32,
    z: f32,
) -> Result<Region, String> {
    let sweep = if neg_sweep { -50.0 } else { 50.0 };
    let mut c = banana_config(0, [12.0, -40.0, 7.0], n, rest, sweep, b);
    if neg_bend {
        c.shape = FlipperShape::CurvedBanana {
            length_mm: b.length_mm,
            arc_radius_mm: b.arc_radius_mm,
            arc_sweep_deg: -b.arc_sweep_deg(),
            tube_radius_mm: b.tube_radius_mm,
        };
    }
    let s_side = sweep.signum() * if neg_bend { -1.0 } else { 1.0 };
    let model = arc_model_side(b, s_side);
    let mut sc = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    sc.advance();
    let nn = n.normalize();
    let lift = measure_lift(&sc.set, 0, ID, nn, &model);
    if (lift - HALF_HEIGHT).abs() > 1.0e-4 {
        return Err(format!("mid-plane lift {lift} m, contract says 15 mm"));
    }
    let pose = Pose::of(&sc.set, ID, nn, lift);
    let tube = model.tube();
    let cs = model.concave_side();
    // Query point in the bat plane (2D), and the expected normal direction.
    let (q, expect_hint): ([f32; 2], [f32; 2]) = match region {
        Region::Concave | Region::Convex => {
            let side = if region == Region::Concave { cs } else { -cs };
            let a = model.point(frac);
            let m = model.side_normal(frac, side);
            let d = tube + offset;
            ([a[0] + m[0] * d, a[1] + m[1] * d], m)
        }
        Region::TipCap | Region::BaseCap => {
            let (f, t) = if region == Region::TipCap {
                (1.0, model.tangent(1.0))
            } else {
                let t = model.tangent(0.0);
                (0.0, [-t[0], -t[1]])
            };
            let (sb, cb) = (
                libm::sinf(beta_deg.to_radians()),
                libm::cosf(beta_deg.to_radians()),
            );
            let dir = [t[0] * cb - t[1] * sb, t[0] * sb + t[1] * cb];
            let a = model.point(f);
            let d = tube + offset;
            ([a[0] + dir[0] * d, a[1] + dir[1] * d], dir)
        }
    };
    let w = pose.to_world(q, z);
    let bp = sc
        .set
        .closest_point(0, 0.0, w)
        .ok_or("closest_point returned None")?;
    // Brute force: the extruded tube, query inside the height range. A
    // query inside the rubber that is nearer a flat (top/bottom) face than
    // the side has that face as its closest point (normal ±n).
    let (d2, fb) = model.distance_2d(q);
    let side_brute = d2 - tube;
    let face_depth = HALF_HEIGHT - z.abs();
    let on_flat_face = side_brute < 0.0 && -side_brute > face_depth;
    let brute = if on_flat_face {
        -face_depth
    } else {
        side_brute
    };
    // The expected outward normal: from the nearest centreline point
    // (brute force) to the query. Which region the query really landed in
    // (a cap query turned toward the concave side can land on the face) is
    // returned for the coverage count.
    let at_end = !(1.0e-3..=1.0 - 1.0e-3).contains(&fb);
    let landed = if at_end {
        if fb < 0.5 {
            Region::BaseCap
        } else {
            Region::TipCap
        }
    } else {
        let _ = expect_hint;
        let side = model.arc_side(q, 0.0);
        if side == Some(true) {
            Region::Concave
        } else {
            Region::Convex
        }
    };
    let foot = model.point(fb);
    let dq = [q[0] - foot[0], q[1] - foot[1]];
    let lq = libm::hypotf(dq[0], dq[1]);
    let expect = [dq[0] / lq, dq[1] / lq];
    let tag = format!("{region:?} frac {frac:.3} β {beta_deg:.1} offset {offset:.5} z {z:.4}");
    if !(bp.distance.is_finite() && bp.point.is_finite() && bp.normal.is_finite()) {
        return Err(format!("{tag}: non-finite closest point: {bp:?}"));
    }
    if (bp.distance - brute).abs() > 3.0e-5 {
        return Err(format!(
            "{tag}: distance {} vs brute force {brute}",
            bp.distance
        ));
    }
    if (bp.normal.length() - 1.0).abs() > 1.0e-4 {
        return Err(format!("{tag}: normal not unit: {:?}", bp.normal));
    }
    if on_flat_face {
        let want = nn * z.signum();
        if bp.normal.dot(want) < 0.999 {
            return Err(format!(
                "{tag}: inside, nearest a flat face: normal {:?}, expected {want:?}",
                bp.normal
            ));
        }
        return Ok(landed);
    }
    if bp.normal.dot(nn).abs() > 1.0e-3 {
        return Err(format!(
            "{tag}: normal leaves the bat plane between the flat faces: {:?}",
            bp.normal
        ));
    }
    let back = bp.point + bp.normal * bp.distance;
    if (back - w).length() > 5.0e-5 {
        return Err(format!(
            "{tag}: point + normal·distance = {back:?}, query {w:?}"
        ));
    }
    // Outward: along the expected direction (concave → toward the centre
    // of curvature, convex → away from it, caps → away from the cap centre).
    let ew = pose.vec_to_world(expect);
    if bp.normal.dot(ew) < 0.98 {
        return Err(format!(
            "{tag}: normal {:?} not outward (expected ≈ {ew:?})",
            bp.normal
        ));
    }
    // The reported point is on the surface.
    let (pq, _) = pose.local(bp.point);
    let (pd, _) = model.distance_2d(pq);
    if (pd - tube).abs() > 3.0e-5 {
        return Err(format!(
            "{tag}: reported point {} m from the centreline, tube {tube}",
            pd
        ));
    }
    Ok(landed)
}

fn banana_strategy() -> impl Strategy<Value = Banana> {
    (50.0f32..100.0, 0.0f32..1.0, 4.0f32..12.0).prop_map(|(l, r01, t)| {
        // Arc radius from just above L/2 (near half circle) to 4 m.
        let rmin = 0.5 * l * 1.02;
        let rad = rmin * libm::powf(4000.0 / rmin, r01);
        Banana {
            length_mm: l,
            arc_radius_mm: rad.max(t * 1.5),
            tube_radius_mm: t,
        }
    })
}

#[test]
fn closest_point_near_minus_z_mounts_fixed_cases() {
    // Reviewer N6: mounting normals within ~0.06° of −Z (where the plane
    // axes switch construction). Every region, both sweeps and bends:
    // point + normal·distance reconstructs the query and the normal is
    // outward (both checked inside `closest_case`).
    let mut n = 0;
    for (theta, phi) in [(3.140_496f32, -2.691_378f32), (3.1405, 0.0)] {
        let normal = unit(theta, phi);
        assert!(normal.z < -0.9999, "not near −Z: {normal:?}");
        for neg_sweep in [false, true] {
            for neg_bend in [false, true] {
                for region in [
                    Region::Concave,
                    Region::Convex,
                    Region::TipCap,
                    Region::BaseCap,
                ] {
                    for frac in [0.1f32, 0.5, 0.9] {
                        for (beta, offset) in [(0.0f32, -0.004f32), (-60.0, 0.0005), (60.0, 0.02)] {
                            for rest in [-150.0f32, 20.0, 110.0] {
                                let r = closest_case(
                                    BANANA, neg_sweep, neg_bend, normal, rest, region, frac, beta,
                                    offset, -0.006,
                                );
                                assert!(
                                    r.is_ok(),
                                    "θ {theta} φ {phi} rest {rest}: {}",
                                    r.unwrap_err()
                                );
                                n += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    println!("{n} near −Z closest-point cases");
}

#[test]
fn closest_point_regions_fixed_cases() {
    // Every region on the reference banana, both sweeps and bends, flat.
    let mut n = 0;
    let mut seen = [0u32; 4];
    for neg_sweep in [false, true] {
        for neg_bend in [false, true] {
            for region in [
                Region::Concave,
                Region::Convex,
                Region::TipCap,
                Region::BaseCap,
            ] {
                for frac in [0.1f32, 0.25, 0.5, 0.85, 0.95] {
                    for beta in [-75.0f32, 0.0, 75.0] {
                        for offset in [-0.004f32, 0.0005, 0.01, 0.03] {
                            let r = closest_case(
                                BANANA,
                                neg_sweep,
                                neg_bend,
                                Vec3::Z,
                                20.0,
                                region,
                                frac,
                                beta,
                                offset,
                                0.003,
                            );
                            match r {
                                Ok(l) => seen[l as usize] += 1,
                                Err(e) => panic!("{e}"),
                            }
                            n += 1;
                        }
                    }
                }
            }
        }
    }
    println!("{n} closest-point cases; landed concave/convex/tip/base: {seen:?}");
    assert!(
        seen.iter().all(|&k| k >= 20),
        "region not covered: {seen:?}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024, .. ProptestConfig::default() })]

    #[test]
    fn closest_point_matches_brute_force_with_outward_normals(
        b in banana_strategy(),
        neg_sweep in any::<bool>(),
        neg_bend in any::<bool>(),
        theta in 0.0f32..std::f32::consts::PI, phi in -std::f32::consts::PI..std::f32::consts::PI,
        rest in -180.0f32..180.0,
        region in prop_oneof![Just(Region::Concave), Just(Region::Convex), Just(Region::TipCap), Just(Region::BaseCap)],
        frac in 0.02f32..0.98,
        beta in -85.0f32..85.0,
        off01 in 0.0f32..1.0,
        z in -0.0145f32..0.0145,
    ) {
        let tube = b.tube_radius_mm * 1.0e-3;
        // Outside up to 40 mm, inside the rubber down to 0.9 tube; on the
        // concave face never beyond the centre of curvature.
        let mut offset = -0.9 * tube + off01 * (0.04 + 0.9 * tube);
        if region == Region::Concave {
            offset = offset.min(0.8 * (b.arc_radius_mm * 1.0e-3 - tube));
        }
        let r = closest_case(b, neg_sweep, neg_bend, unit(theta, phi), rest, region, frac, beta, offset, z);
        prop_assert!(r.is_ok(), "{:?}: {}", b, r.unwrap_err());
    }
}

// ------------------------------------------------------------ tunnelling

/// Free-space shot at a banana bat (flat mount at the origin, rest
/// direction +x… `rest`, sweep `sweep`): ball in the bat mid-plane toward
/// the face point at `frac` of `face` (or the tip cap when `frac > 1`,
/// at angle `beta`), flip at the arrival tick + `flip_off`. Err on a tunnel
/// or a deep penetration. Returns (max tip speed, hit).
#[allow(clippy::too_many_arguments)]
fn tunnel_case(
    b: Banana,
    n: Vec3,
    sweep: f32,
    power: f32,
    speed: f32,
    frac: f32,
    concave_face: bool,
    beta_deg: f32,
    flip_off: i64,
) -> Result<(f32, bool), String> {
    let mut c = banana_config(0, [0.0, 0.0, 0.0], n, 10.0, sweep, b);
    c.power_torque_nm = power;
    let model = arc_model(b, sweep);
    let nn = n.normalize();
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    let pose0 = Pose::of(&s.set, ID, nn, HALF_HEIGHT);
    let cs = model.concave_side();
    let tube = model.tube();
    // Aim point and outward direction (bat frame).
    let (a, out) = if frac <= 1.0 {
        let side = if concave_face { cs } else { -cs };
        (model.point(frac), model.side_normal(frac, side))
    } else {
        let t = model.tangent(1.0);
        let (sb, cb) = (
            libm::sinf(beta_deg.to_radians()),
            libm::cosf(beta_deg.to_radians()),
        );
        (
            model.point(1.0),
            [t[0] * cb - t[1] * sb, t[0] * sb + t[1] * cb],
        )
    };
    let dist = (speed * 0.04).max(0.12);
    let arrive = (dist / speed / DT) as i64;
    let flip = (arrive + flip_off).max(0) as u64;
    let start = pose0.to_world([a[0] + out[0] * dist, a[1] + out[1] * dist], 0.0);
    let h = s
        .solver
        .add_ball(ball(start, -pose0.vec_to_world(out) * speed))
        .unwrap();
    let mut prev: Option<bool> = None;
    let mut max_tip = 0.0f32;
    let mut hit = false;
    let ticks = arrive as u64 + 80;
    let margin = 0.2
        * match model {
            ArcModel::Arc { dphi, .. } => dphi.abs(),
            ArcModel::Segment { .. } => 0.2,
        };
    while s.tick < ticks {
        if s.tick == flip {
            s.set.set_energised(ID, true);
        }
        s.advance();
        hit |= s.hit(h, 0);
        max_tip = max_tip.max(s.set.angular_velocity(ID).abs() * (b.length_mm * 1.0e-3));
        let p = s.ball(h).position;
        if !p.is_finite() {
            return Err(format!("non-finite ball at tick {}", s.tick));
        }
        let pose = Pose::of(&s.set, ID, nn, HALF_HEIGHT);
        let (q, z) = pose.local(p);
        if z.abs() < HALF_HEIGHT {
            let d = model.distance_2d_fast(q);
            if d < R + tube - 1.0e-3 {
                return Err(format!(
                    "tick {}: ball centre {d} m from the centreline (≥ {} expected), q {q:?}",
                    s.tick,
                    R + tube
                ));
            }
        }
        let now = if z.abs() < 0.01 && model.distance_2d_fast(q) < 0.06 {
            model.arc_side(q, margin)
        } else {
            None
        };
        if let (Some(a0), Some(a1)) = (prev, now)
            && a0 != a1
        {
            return Err(format!("tick {}: ball crossed the arc (q {q:?})", s.tick));
        }
        prev = now;
    }
    Ok((max_tip, hit))
}

#[test]
fn no_tunnelling_at_max_angular_velocity_fixed_sweep() {
    // Appendix C maximum coil (3 N·m) and a far stronger one (8 N·m) so the
    // bat runs at its highest angular velocity when the ball arrives; both
    // faces, tip cap, three arcs.
    let mut fastest = 0.0f32;
    let (mut cases, mut hits) = (0u32, 0u32);
    for rad in [45.0f32, 120.0, 400.0] {
        let b = Banana {
            arc_radius_mm: rad,
            ..BANANA
        };
        for power in [3.0f32, 8.0] {
            for speed in [5.0f32, 20.0, 50.0] {
                for off in [-10i64, -6, -3, -1, 0, 2] {
                    for (frac, concave) in [
                        (0.25f32, true),
                        (0.5, true),
                        (0.85, true),
                        (0.5, false),
                        (0.85, false),
                        (1.5, true),
                    ] {
                        let r = tunnel_case(
                            b,
                            Vec3::Z,
                            50.0,
                            power,
                            speed,
                            frac,
                            concave,
                            if concave { 20.0 } else { -20.0 },
                            off,
                        );
                        match r {
                            Ok((tip, hit)) => {
                                fastest = fastest.max(tip);
                                cases += 1;
                                hits += u32::from(hit);
                            }
                            Err(e) => panic!(
                                "radius {rad} power {power} speed {speed} offset {off} frac {frac} concave {concave}: {e}"
                            ),
                        }
                    }
                }
            }
        }
    }
    println!("max bat chord-end speed {fastest:.2} m/s; {hits}/{cases} cases hit the bat");
    assert!(
        hits * 10 >= cases * 8,
        "scenario mostly misses: {hits}/{cases}"
    );
    assert!(fastest >= 15.0, "never reached a 15 m/s tip: {fastest}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, .. ProptestConfig::default() })]

    #[test]
    fn no_tunnelling_any_mount_position_speed_and_timing(
        b in banana_strategy(),
        theta in 0.0f32..std::f32::consts::PI, phi in -std::f32::consts::PI..std::f32::consts::PI,
        pos_sweep in any::<bool>(),
        power in 1.2f32..8.0,
        speed in 0.5f32..50.0,
        frac in 0.05f32..1.6,
        concave in any::<bool>(),
        beta in -70.0f32..70.0,
        off in -30i64..30,
    ) {
        let sweep = if pos_sweep { 50.0 } else { -50.0 };
        let r = tunnel_case(b, unit(theta, phi), sweep, power, speed, frac, concave, beta, off);
        prop_assert!(r.is_ok(), "{:?}: {}", b, r.unwrap_err());
    }
}

// ---------------------------------------------------------------- energy

/// Energy audit on the pitched table (the 2.3 Tester's audit on a banana):
/// ball + bat energy (kinetic + gravitational) never exceeds its start
/// plus `(τ_power + τ_spring)·Σ|Δθ|`, nor start plus `drive_work_j`; with
/// the coil off (after its torque decayed) no tick gains more than the
/// spring's work.
#[allow(clippy::too_many_arguments)]
fn energy_case(
    b: Banana,
    speed: f32,
    frac: f32,
    beta_deg: f32,
    flip_off: i64,
    release_after: Option<u64>,
    damping: f32,
    restitution: f32,
) -> Result<(), String> {
    let model = arc_model(b, 50.0);
    let (mut c, lift) = aligned(
        |base, rest| banana_config(0, base, Vec3::Z, rest, 50.0, b),
        &model,
        [0.0, 0.0],
        -30.0,
    );
    c.rubber_damping = damping;
    c.rubber_restitution = restitution;
    let g = table_gravity(Quat::IDENTITY);
    let mut s = Scene::new(table_world(Quat::IDENTITY), g, std::slice::from_ref(&c));
    let pose = Pose::of(&s.set, ID, Vec3::Z, lift);
    let a = pose.to_world(model.point(frac), 0.0);
    let dir = Quat::from_rotation_z(beta_deg.to_radians()) * v(0.0, -1.0, 0.0);
    let start = v(a.x, a.y, R) - dir * 0.15;
    let h = s.solver.add_ball(ball(start, dir * speed)).unwrap();
    let arrive = (0.15 / speed / DT) as i64;
    let flip = (arrive + flip_off).max(0) as u64;
    let rel = release_after.map(|r| flip + r);
    let inertia = [c.inertia_kg_m2];
    let tau = c.power_torque_nm.max(c.hold_torque_nm) + c.return_spring_torque_nm;
    let mut e_prev = energy_of(&s, g, &inertia);
    let e0 = e_prev;
    let mut a_prev = s.set.angle_deg(ID);
    let mut work = 0.0f64;
    let mut energised = false;
    let mut off_ticks = u64::MAX / 2;
    while s.tick < arrive as u64 + 250 {
        if s.tick == flip {
            s.set.set_energised(ID, true);
            energised = true;
        }
        if Some(s.tick) == rel {
            s.set.set_energised(ID, false);
            energised = false;
            off_ticks = 0;
        }
        s.advance();
        off_ticks = off_ticks.saturating_add(1);
        let e = energy_of(&s, g, &inertia);
        let a1 = s.set.angle_deg(ID);
        let dth = f64::from((a1 - a_prev).abs().to_radians());
        work += f64::from(tau) * dth;
        let bl = s.ball(h);
        let ke = 0.5 * 0.08 * f64::from(bl.velocity.length_squared());
        let tol = 2.0e-4 + 3.0e-3 * ke;
        if !energised && off_ticks > 10 {
            let w = f64::from(c.return_spring_torque_nm) * dth;
            if e - e_prev > w + tol {
                return Err(format!(
                    "tick {}: coil off, energy rose by {:.6} J, spring work bound {:.6} J",
                    s.tick,
                    e - e_prev,
                    w
                ));
            }
        }
        if e - e0 > work + 1.0e-3 + 3.0e-3 * ke {
            return Err(format!(
                "tick {}: energy {:.6} J above start, coil+spring path work {:.6} J",
                s.tick,
                e - e0,
                work
            ));
        }
        let dw = s.set.drive_work_j(ID);
        if e - e0 > dw + 1.0e-3 + 3.0e-3 * ke {
            return Err(format!(
                "tick {}: energy {:.6} J above start, drive_work_j {:.6} J",
                s.tick,
                e - e0,
                dw
            ));
        }
        e_prev = e;
        a_prev = a1;
    }
    Ok(())
}

#[test]
fn energy_unpowered_banana_never_gains() {
    for speed in [1.0f32, 3.5, 8.0] {
        for frac in [0.15f32, 0.5, 0.9] {
            for beta in [-30.0f32, 0.0, 30.0] {
                let r = energy_case(BANANA, speed, frac, beta, 100_000, None, 0.35, 0.65);
                assert!(
                    r.is_ok(),
                    "speed {speed} frac {frac} beta {beta}: {}",
                    r.unwrap_err()
                );
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 192, .. ProptestConfig::default() })]

    #[test]
    fn energy_never_exceeds_drive_bound(
        rad in 45.0f32..1000.0,
        speed in 0.5f32..8.0,
        frac in 0.1f32..0.95,
        beta in -40.0f32..40.0,
        flip_off in -30i64..30,
        release in proptest::option::of(0u64..60),
        damping in 0.1f32..0.7,
        restitution in 0.3f32..0.9,
    ) {
        let b = Banana { arc_radius_mm: rad, ..BANANA };
        let r = energy_case(b, speed, frac, beta, flip_off, release, damping, restitution);
        prop_assert!(r.is_ok(), "radius {}: {}", rad, r.unwrap_err());
    }
}

// ---------------------------------------------------------- validation

#[test]
fn invalid_banana_is_rejected_and_valid_ones_build() {
    let mk = |l: f32, r: f32, sw: f32, t: f32| -> FlipperConfig {
        let mut c = config(0, [0.0, 0.0, 0.0], Vec3::Z, 0.0, 50.0);
        c.shape = FlipperShape::CurvedBanana {
            length_mm: l,
            arc_radius_mm: r,
            arc_sweep_deg: sw,
            tube_radius_mm: t,
        };
        c
    };
    for (l, r, sw, t) in [
        (76.0f32, 120.0f32, 37.0f32, 8.0f32),
        (76.0, 38.0, 180.0, 8.0), // half circle
        (76.0, 1.0e5, 0.04, 8.0),
        (76.0, 120.0, -37.0, 8.0),
    ] {
        assert!(
            pin_physics::flipper::FlipperSet::new(&[mk(l, r, sw, t)]).is_ok(),
            "valid banana rejected: {l} {r} {sw} {t}"
        );
    }
    for (l, r, sw, t) in [
        (76.0f32, 30.0f32, 37.0f32, 8.0f32), // R < L/2
        (0.0, 120.0, 37.0, 8.0),
        (76.0, 120.0, 37.0, 0.0),
        (76.0, 6.0, 37.0, 8.0),
        (f32::NAN, 120.0, 37.0, 8.0),
        (76.0, f32::INFINITY, 37.0, 8.0),
        (76.0, 120.0, f32::NAN, 8.0),
        (76.0, 120.0, 37.0, -1.0),
    ] {
        assert!(
            pin_physics::flipper::FlipperSet::new(&[mk(l, r, sw, t)]).is_err(),
            "invalid banana accepted: {l} {r} {sw} {t}"
        );
    }
}

// ------------------------------------------------------- surface velocity

/// Mid-stroke (coil on since tick 0, `k` ticks run), inside a tick opened
/// with `begin_tick`: the velocity field the bat reports at its surface
/// (concave and convex faces, tip and base caps) is one rigid rotation
/// about the pivot, `ω n × (p − pivot)`, with ω the bat's angular velocity
/// (between its values at the start and end of the tick), and its speed is
/// bounded by `max_surface_speed`.
fn surface_velocity_case(b: Banana, sweep: f32, k: u64, t_frac: f32) -> Result<f32, String> {
    let c = banana_config(0, [5.0, -7.0, 0.0], Vec3::Z, 15.0, sweep, b);
    let model = arc_model(b, sweep);
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    s.set.set_energised(ID, true);
    for _ in 0..k {
        s.advance();
    }
    let w0 = s.set.angular_velocity(ID);
    let a0 = s.set.angle_deg(ID);
    let pose0 = Pose::of(&s.set, ID, Vec3::Z, HALF_HEIGHT);
    MovingBodies::begin_tick(&mut s.set, DT);
    let t = t_frac * DT;
    let tube = model.tube();
    // Surface points of the pose at the tick start. The field of a rigid
    // rotation is `ω n × (p − pivot)` at every point, so querying these
    // points at time t (the bat has turned by < 0.11 rad since) is valid.
    let mut pts = Vec::new();
    for f in [0.2f32, 0.5, 0.8] {
        for side in [-1.0f32, 1.0] {
            let a = model.point(f);
            let m = model.side_normal(f, side);
            pts.push([a[0] + m[0] * tube, a[1] + m[1] * tube]);
        }
    }
    let tip = model.point(1.0);
    let tt = model.tangent(1.0);
    pts.push([tip[0] + tt[0] * tube, tip[1] + tt[1] * tube]);
    let t0 = model.tangent(0.0);
    pts.push([-t0[0] * tube, -t0[1] * tube]);
    let o = pose0.o;
    let mut ws = Vec::new();
    let vmax = MovingBodies::max_surface_speed(&s.set, 0);
    for p in &pts {
        let w = pose0.to_world(*p, 0.0);
        let v = MovingBodies::velocity_at(&s.set, 0, t, w);
        let r = w - o;
        let r = r - Vec3::Z * r.dot(Vec3::Z);
        let tang = Vec3::Z.cross(r);
        if v.dot(Vec3::Z).abs() > 1.0e-4 + 1.0e-3 * v.length() {
            return Err(format!("velocity leaves the plane at {p:?}: {v:?}"));
        }
        let radial = v.dot(r.normalize());
        if radial.abs() > 1.0e-3 + 1.0e-2 * v.length() {
            return Err(format!(
                "velocity at {p:?} has a radial part {radial} (v {v:?}): not a rotation about the pivot"
            ));
        }
        ws.push(v.dot(tang) / tang.length_squared());
        if v.length() > vmax * 1.0001 + 1.0e-6 {
            return Err(format!(
                "surface speed {} above max_surface_speed {vmax}",
                v.length()
            ));
        }
    }
    let (lo, hi) = ws.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |m, &w| {
        (m.0.min(w), m.1.max(w))
    });
    if hi - lo > 1.0e-3 * hi.abs().max(1.0) {
        return Err(format!("ω differs between surface points: {ws:?}"));
    }
    MovingBodies::end_tick(&mut s.set, DT);
    let w1 = s.set.angular_velocity(ID);
    let a1 = s.set.angle_deg(ID);
    let w = ws[0];
    let (wl, wh) = (w0.min(w1), w0.max(w1));
    let slack = 1.0e-3 * wh.abs().max(1.0);
    if w < wl - slack || w > wh + slack {
        // The drive acts as one impulse at the tick start, so ω inside the
        // tick is ω after that impulse; it must still match the angle
        // change of the tick unless a stop was reached.
        let mean = (a1 - a0).to_radians() / DT;
        if (w - mean).abs() > 0.05 * mean.abs().max(1.0) && (a1 - c.end_angle_deg()).abs() > 1e-3 {
            return Err(format!(
                "field ω {w} outside [{w0}, {w1}] and far from the tick's mean {mean}"
            ));
        }
    }
    Ok(w)
}

#[test]
fn surface_velocity_is_rigid_rotation_mid_stroke() {
    let mut fastest = 0.0f32;
    for sweep in [50.0f32, -50.0] {
        for k in [3u64, 8, 12, 16, 19] {
            for tf in [0.0f32, 0.5, 0.95] {
                match surface_velocity_case(BANANA, sweep, k, tf) {
                    Ok(w) => fastest = fastest.max(w.abs()),
                    Err(e) => panic!("sweep {sweep} tick {k} t {tf}: {e}"),
                }
            }
        }
    }
    println!("largest ω seen {fastest:.1} rad/s");
    assert!(fastest > 20.0, "bat never moving fast: {fastest}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    #[test]
    fn surface_velocity_is_rigid_rotation_any_arc(
        b in banana_strategy(),
        pos in any::<bool>(),
        k in 1u64..22,
        tf in 0.0f32..1.0,
    ) {
        let r = surface_velocity_case(b, if pos { 50.0 } else { -50.0 }, k, tf);
        prop_assert!(r.is_ok(), "{:?}: {}", b, r.unwrap_err());
    }
}
