//! Reviewer probes for 2.2 (temporary; deleted after review).
#![allow(dead_code, clippy::all)]

use glam::{Quat, Vec3};
use pin_core::profiles::builtin_profiles;
use pin_core::{BallProfile, PhysicsDefaults, ProfileId};
use pin_physics::ball::{
    Ball, BallEvent, BallEvents, BallSolver, BallSolverConfig, Geometry, ImpactTarget, Magnet,
    NewtonChain,
};
use pin_physics::world::{
    CubicSpline, GuideCurve, StaticShape, StaticWorld, StaticWorldBuilder, Wireform, layers,
};

const DT: f32 = 1.0e-3;
const STEEL: ProfileId = ProfileId(0);
const POWER: ProfileId = ProfileId(2);
const GLO: ProfileId = ProfileId(3);
const MINI: ProfileId = ProfileId(4);
const JUMBO: ProfileId = ProfileId(5);

fn profiles() -> Vec<BallProfile> {
    builtin_profiles().collect()
}
fn rad(p: ProfileId) -> f32 {
    profiles()[p.index()].radius_m()
}
fn mass(p: ProfileId) -> f32 {
    profiles()[p.index()].mass_kg
}
fn inertia(p: ProfileId) -> f32 {
    profiles()[p.index()].moment_of_inertia()
}
fn solver(g: Vec3) -> BallSolver {
    let mut c = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    c.gravity = g;
    BallSolver::new(c, &profiles())
}
fn cub(center: Vec3, he: Vec3, rot: Quat) -> StaticShape {
    StaticShape::Cuboid {
        center,
        half_extents: he,
        rotation: rot,
    }
}
fn g0() -> Vec3 {
    Vec3::new(0.0, 0.0, -9.81)
}
fn ke(s: &BallSolver) -> f64 {
    s.balls()
        .map(|(_, b)| {
            0.5 * mass(b.profile) as f64 * b.velocity.as_dvec3().length_squared()
                + 0.5 * inertia(b.profile) as f64 * b.angular_velocity.as_dvec3().length_squared()
        })
        .sum()
}
fn mech(s: &BallSolver, g: Vec3) -> f64 {
    ke(s)
        + s.balls()
            .map(|(_, b)| -(mass(b.profile) as f64) * g.as_dvec3().dot(b.position.as_dvec3()))
            .sum::<f64>()
}
fn impacts(ev: &BallEvents) -> usize {
    ev.as_slice()
        .iter()
        .filter(|e| matches!(e, BallEvent::Impact { .. }))
        .count()
}

/// Tilted playfield (pitch `deg`, downhill toward -y) through the origin,
/// as a cuboid or a two-triangle mesh, plus a post (cylinder along the
/// normal) at the origin. Returns (world, normal, downhill dir).
fn slope_world(deg: f32, mesh: bool, post_r: f32) -> (StaticWorld, Vec3, Vec3) {
    let th = deg.to_radians();
    let rot = Quat::from_rotation_x(th);
    let n = rot * Vec3::Z;
    let mut b = StaticWorldBuilder::new();
    if mesh {
        let u = rot * Vec3::X;
        let w = rot * Vec3::Y;
        let s = 2.0;
        let vs = vec![
            -u * s - w * s,
            u * s - w * s,
            u * s + w * s,
            -u * s + w * s,
        ];
        b.add_collider(
            StaticShape::TriMesh {
                vertices: vs,
                indices: vec![[0, 1, 2], [0, 2, 3]],
            },
            layers::ALL,
            true,
        );
    } else {
        b.add_collider(cub(-n * 0.05, Vec3::new(2.0, 2.0, 0.05), rot), layers::ALL, true);
    }
    if post_r > 0.0 {
        b.add_collider(
            StaticShape::Cylinder {
                base: -n * 0.005,
                axis: n * 0.04,
                radius: post_r,
            },
            layers::ALL,
            true,
        );
    }
    let g = g0();
    let gt = g - n * g.dot(n);
    (b.build(), n, gt.normalize())
}

#[test]
fn p_post_on_slope_drift() {
    for &mesh in &[false, true] {
        for &p in &[STEEL, POWER, MINI, JUMBO] {
            for &deg in &[6.5f32, 3.0, 10.0] {
                let (w, n, d) = slope_world(deg, mesh, 0.005);
                let geo = Geometry::new(&w);
                let r = rad(p);
                let mut s = solver(g0());
                let pos = n * r - d * (0.005 + r) + n * 1e-6 - d * 1e-6;
                let h = s.add_ball(Ball::new(p, pos)).unwrap();
                let mut ev = BallEvents::new();
                for _ in 0..1000 {
                    s.step(&geo, DT, &mut ev);
                }
                let settle_imp = impacts(&ev);
                let p0 = s.ball(h).unwrap().position;
                let mut max_d = 0.0f32;
                let mut nimp = 0;
                let mut maxv = 0.0f32;
                for _ in 0..10000 {
                    ev.clear();
                    s.step(&geo, DT, &mut ev);
                    nimp += impacts(&ev);
                    let b = s.ball(h).unwrap();
                    max_d = max_d.max(b.position.distance(p0));
                    maxv = maxv.max(b.velocity.length());
                }
                println!(
                    "post mesh={mesh} p={:?} deg={deg}: settle_impacts={settle_imp} drift={:.3e} m impacts_10s={nimp} max|v|={:.2e} final={:?}",
                    p,
                    max_d,
                    maxv,
                    s.ball(h).unwrap().position
                );
            }
        }
    }
}

#[test]
fn p_flat_rest_and_vgroove() {
    // Flat floor rest, all profiles.
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cub(Vec3::new(0.0, 0.0, -0.05), Vec3::new(2.0, 2.0, 0.05), Quat::IDENTITY),
        layers::ALL,
        true,
    );
    let w = b.build();
    let geo = Geometry::new(&w);
    for p in [STEEL, POWER, MINI, JUMBO, ProfileId(6)] {
        let mut s = solver(g0());
        let h = s.add_ball(Ball::new(p, Vec3::new(0.0, 0.0, rad(p)))).unwrap();
        let mut ev = BallEvents::new();
        let p0 = s.ball(h).unwrap().position;
        let mut md = 0.0f32;
        for _ in 0..10000 {
            s.step(&geo, DT, &mut ev);
            md = md.max(s.ball(h).unwrap().position.distance(p0));
        }
        println!("flat p={p:?} drift={md:.3e} impacts={}", impacts(&ev));
    }
    // V-groove on a 6.5 deg pitch (gravity tilted), with an end wall.
    for half_angle in [20.0f32, 45.0, 70.0] {
        let a = half_angle.to_radians();
        let mut b = StaticWorldBuilder::new();
        // planes through the x axis with normals (±sin a', 0, cos a') where a' = 90-a
        let tilt = std::f32::consts::FRAC_PI_2 - a;
        for sgn in [-1.0f32, 1.0] {
            let rot = Quat::from_rotation_y(sgn * tilt);
            let n = rot * Vec3::Z;
            b.add_collider(cub(-n * 0.05, Vec3::new(1.0, 1.0, 0.05), rot), layers::ALL, true);
        }
        // end wall at y = -0.02
        b.add_collider(
            cub(Vec3::new(0.0, -0.07, 0.0), Vec3::new(1.0, 0.05, 1.0), Quat::IDENTITY),
            layers::ALL,
            true,
        );
        let w = b.build();
        let geo = Geometry::new(&w);
        let pitch = 6.5f32.to_radians();
        let g = Vec3::new(0.0, -9.81 * pitch.sin(), -9.81 * pitch.cos());
        for p in [STEEL, MINI, JUMBO] {
            let r = rad(p);
            let mut s = solver(g);
            let z = r / tilt.cos();
            let h = s
                .add_ball(Ball::new(p, Vec3::new(0.0, -0.02 + r + 0.002, z + 0.001)))
                .unwrap();
            let mut ev = BallEvents::new();
            for _ in 0..2000 {
                s.step(&geo, DT, &mut ev);
            }
            let si = impacts(&ev);
            ev.clear();
            let p0 = s.ball(h).unwrap().position;
            let e0 = mech(&s, g);
            let mut md = 0.0f32;
            let mut emax = 0.0f64;
            for _ in 0..10000 {
                s.step(&geo, DT, &mut ev);
                md = md.max(s.ball(h).unwrap().position.distance(p0));
                emax = emax.max(mech(&s, g) - e0);
            }
            println!(
                "vgroove half={half_angle} p={p:?}: settle_imp={si} drift={md:.3e} impacts={} dE+={emax:.3e} v={:?}",
                impacts(&ev),
                s.ball(h).unwrap().velocity
            );
        }
    }
}

#[test]
fn p_row_of_balls_against_post() {
    // Three balls in a row on a 6.5 deg slope, the lowest held by a post; and a
    // ball pinned between two balls in a corner.
    for &mesh in &[false, true] {
        let (w, n, d) = slope_world(6.5, mesh, 0.005);
        let geo = Geometry::new(&w);
        let r = rad(STEEL);
        let mut s = solver(g0());
        let mut hs = vec![];
        for k in 0..3 {
            let pos = n * (r + 1e-6) - d * (0.005 + r + 2.0 * r * k as f32 + 1e-5 * k as f32);
            hs.push(s.add_ball(Ball::new(STEEL, pos)).unwrap());
        }
        // a mini ball pinned between two steel balls side by side above row? keep simple
        let mut ev = BallEvents::new();
        for _ in 0..2000 {
            s.step(&geo, DT, &mut ev);
        }
        let si = impacts(&ev);
        ev.clear();
        let p0: Vec<Vec3> = hs.iter().map(|h| s.ball(*h).unwrap().position).collect();
        let mut md = vec![0.0f32; 3];
        let e0 = mech(&s, g0());
        let mut emax = 0.0f64;
        for _ in 0..10000 {
            s.step(&geo, DT, &mut ev);
            for k in 0..3 {
                md[k] = md[k].max(s.ball(hs[k]).unwrap().position.distance(p0[k]));
            }
            emax = emax.max(mech(&s, g0()) - e0);
        }
        println!(
            "row mesh={mesh}: settle_imp={si} drift={md:?} impacts={} dE+={emax:.3e}",
            impacts(&ev)
        );
    }
}

#[test]
fn p_rolling_along_wall_and_skimming() {
    // Flat floor + wall at y=0 (wall occupies y<0). Gravity has a small
    // component into the wall and a component along +x (rolling).
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cub(Vec3::new(0.0, 0.0, -0.05), Vec3::new(50.0, 2.0, 0.05), Quat::IDENTITY),
        layers::ALL,
        true,
    );
    b.add_collider(
        cub(Vec3::new(0.0, -0.05, 0.05), Vec3::new(50.0, 0.05, 0.05), Quat::IDENTITY),
        layers::ALL,
        true,
    );
    let w = b.build();
    let geo = Geometry::new(&w);
    for (gx, gy) in [(1.1f32, -0.3f32), (0.5, -0.05), (0.0, -1.1)] {
        let g = Vec3::new(gx, gy, -9.75);
        for p in [STEEL, MINI] {
            let r = rad(p);
            let mut s = solver(g);
            let h = s
                .add_ball(Ball::new(p, Vec3::new(-40.0, r + 1e-6, r + 1e-6)))
                .unwrap();
            let mut ev = BallEvents::new();
            let e0 = mech(&s, g);
            let mut emax = 0.0f64;
            let mut tot = 0;
            for t in 0..3000 {
                ev.clear();
                s.step(&geo, DT, &mut ev);
                if t > 50 {
                    tot += impacts(&ev);
                }
                emax = emax.max(mech(&s, g) - e0);
            }
            let bb = s.ball(h).unwrap();
            println!(
                "wall-roll g=({gx},{gy}) p={p:?}: impacts(after 50ms)={tot} dE+={emax:.3e} y-r={:.3e} z-r={:.3e} v={:?}",
                bb.position.y - r,
                bb.position.z - r,
                bb.velocity
            );
        }
    }
    // Skimming: fast ball parallel to the wall at various gaps, gravity on.
    for speed in [2.0f32, 10.0, 20.0, 30.0] {
        for gap in [0.0f32, 1e-6, 5e-6, 1.5e-5, 3e-5] {
            for &ang in &[0.0f32, 0.001, -0.001] {
                let r = rad(STEEL);
                let mut s = solver(g0());
                let mut ball = Ball::new(STEEL, Vec3::new(-40.0, r + gap, r));
                ball.velocity = Vec3::new(speed * ang.cos(), -speed * ang.sin(), 0.0);
                ball.angular_velocity = Vec3::new(0.0, speed / r, 0.0);
                let h = s.add_ball(ball).unwrap();
                let mut ev = BallEvents::new();
                let mut tot = 0;
                let e0 = ke(&s);
                for _ in 0..200 {
                    ev.clear();
                    s.step(&geo, DT, &mut ev);
                    tot += impacts(&ev);
                }
                let bb = s.ball(h).unwrap();
                println!(
                    "skim v={speed} gap={gap:e} ang={ang}: impacts={tot} vy={:.3e} vz={:.3e} z-r={:.2e} dKE={:.3e}",
                    bb.velocity.y,
                    bb.velocity.z,
                    bb.position.z - r,
                    ke(&s) - e0
                );
            }
        }
    }
}

#[test]
fn p_restitution_fade() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cub(Vec3::new(0.0, 0.0, -0.05), Vec3::new(2.0, 2.0, 0.05), Quat::IDENTITY),
        layers::ALL,
        true,
    );
    let w = b.build();
    let geo = Geometry::new(&w);
    let r = rad(STEEL);
    for gap in [1e-3f32, 1.5e-5, 2e-6, 0.0] {
        let mut line = String::new();
        let mut prev = None::<f32>;
        let mut maxjump = 0.0f32;
        let mut k = 0;
        while k <= 60 {
            let v = 0.03 + 0.0025 * k as f32;
            let mut s = solver(Vec3::ZERO);
            let mut ball = Ball::new(STEEL, Vec3::new(0.0, 0.0, r + gap));
            ball.velocity = Vec3::new(0.0, 0.0, -v);
            let h = s.add_ball(ball).unwrap();
            let mut ev = BallEvents::new();
            for _ in 0..40 {
                s.step(&geo, DT, &mut ev);
            }
            let vz = s.ball(h).unwrap().velocity.z;
            let e = vz / v;
            if let Some(pe) = prev {
                maxjump = maxjump.max((e - pe).abs());
            }
            prev = Some(e);
            if k % 4 == 0 {
                line += &format!("{v:.3}:{e:.3}[{}] ", impacts(&ev));
            }
            k += 1;
        }
        println!("fade gap={gap:e}: max step in e={maxjump:.3} | {line}");
    }
}

#[test]
fn p_oblique_spinning_energy_and_tunnelling() {
    // Zero gravity, ball in a closed box with high speeds + spins: KE never
    // increases tick to tick, ball never leaves the box.
    let mut b = StaticWorldBuilder::new();
    let h = Vec3::new(0.3, 0.2, 0.15);
    let t = 0.001; // 2 mm thick walls
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut c = Vec3::ZERO;
            c[axis] = sign * (h[axis] + t);
            let mut he = h + Vec3::splat(2.0 * t);
            he[axis] = t;
            b.add_collider(cub(c, he, Quat::IDENTITY), layers::ALL, true);
        }
    }
    let w = b.build();
    let geo = Geometry::new(&w);
    let mut seed = 12345u64;
    let mut rnd = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as f32) / (1u64 << 31) as f32 * 2.0 - 1.0
    };
    let mut worst_gain = 0.0f64;
    let mut escapes = 0;
    for trial in 0..60 {
        let p = [STEEL, MINI, JUMBO, POWER][trial % 4];
        let mut s = solver(Vec3::ZERO);
        let speed = 10.0 + 20.0 * (rnd() * 0.5 + 0.5);
        let dir = Vec3::new(rnd(), rnd(), rnd()).normalize();
        let mut ball = Ball::new(p, Vec3::new(rnd() * 0.1, rnd() * 0.1, rnd() * 0.05));
        ball.velocity = dir * speed;
        ball.angular_velocity = Vec3::new(rnd(), rnd(), rnd()) * 2000.0;
        let hh = s.add_ball(ball).unwrap();
        let mut ev = BallEvents::new();
        let mut e_prev = ke(&s);
        for _ in 0..300 {
            ev.clear();
            s.step(&geo, DT, &mut ev);
            let e = ke(&s);
            worst_gain = worst_gain.max((e - e_prev) / e_prev.max(1e-12));
            e_prev = e;
            let bp = s.ball(hh).unwrap().position;
            if bp.abs().cmpgt(h).any() {
                escapes += 1;
                println!("ESCAPE trial {trial} {bp:?}");
                break;
            }
        }
    }
    println!("box: worst relative KE gain per tick = {worst_gain:.3e}, escapes = {escapes}");

    // Multi-ball cluster in the box, gravity off, with friction: KE must not grow.
    let mut s = solver(Vec3::ZERO);
    for k in 0..8 {
        let p = [STEEL, MINI, JUMBO, POWER][k % 4];
        let mut ball = Ball::new(p, Vec3::new(-0.2 + 0.05 * k as f32, 0.02 * (k % 3) as f32, 0.0));
        ball.velocity = Vec3::new(rnd(), rnd(), rnd()) * 15.0;
        ball.angular_velocity = Vec3::new(rnd(), rnd(), rnd()) * 1000.0;
        s.add_ball(ball).unwrap();
    }
    let mut ev = BallEvents::new();
    let mut e_prev = ke(&s);
    let mut worst = 0.0f64;
    let mut esc = 0;
    for _ in 0..3000 {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        let e = ke(&s);
        worst = worst.max((e - e_prev) / e_prev.max(1e-12));
        e_prev = e;
        for (_, b) in s.balls() {
            if b.position.abs().cmpgt(h).any() {
                esc += 1;
            }
        }
    }
    println!("cluster: worst relative KE gain per tick = {worst:.3e}, escape-ticks = {esc}, final KE={e_prev:.3e}");
}

#[test]
fn p_thin_wall_tunnelling_sweep() {
    let mut fails = 0;
    let mut n = 0;
    for &mesh in &[false, true] {
        for &tilt_deg in &[0.0f32, 30.0, 60.0, 80.0] {
            let rot = Quat::from_rotation_z(tilt_deg.to_radians());
            let nrm = rot * Vec3::X;
            let mut b = StaticWorldBuilder::new();
            if mesh {
                let u = rot * Vec3::Y;
                let v = Vec3::Z;
                b.add_collider(
                    StaticShape::TriMesh {
                        vertices: vec![-u - v, u - v, u + v, -u + v],
                        indices: vec![[0, 1, 2], [0, 2, 3]],
                    },
                    layers::ALL,
                    true,
                );
            } else {
                b.add_collider(cub(Vec3::ZERO, Vec3::new(0.001, 1.0, 1.0), rot), layers::ALL, true);
            }
            let w = b.build();
            let geo = Geometry::new(&w);
            for p in [STEEL, MINI] {
                for speed in [10.0f32, 20.0, 30.0, 45.0] {
                    for ang in [0.0f32, 30.0, 60.0, 85.0] {
                        let a = ang.to_radians();
                        let side = (rot * Vec3::Y) * a.sin();
                        let dir = (nrm * a.cos() + side).normalize();
                        let mut s = solver(Vec3::ZERO);
                        let mut ball = Ball::new(p, -nrm * 0.1 - dir * 0.0 + Vec3::new(0.0, 0.0, 0.0));
                        ball.velocity = dir * speed;
                        ball.angular_velocity = Vec3::new(300.0, -500.0, 800.0);
                        let h = s.add_ball(ball).unwrap();
                        let mut ev = BallEvents::new();
                        for _ in 0..30 {
                            s.step(&geo, DT, &mut ev);
                        }
                        n += 1;
                        let x = s.ball(h).unwrap().position.dot(nrm);
                        if x > 0.0 {
                            fails += 1;
                            println!("TUNNEL mesh={mesh} tilt={tilt_deg} p={p:?} v={speed} ang={ang} x={x}");
                        }
                    }
                }
            }
        }
    }
    println!("thin wall sweep: {fails}/{n} tunnelled");
}

#[test]
fn p_criterion2_with_other_balls() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cub(Vec3::new(0.0, 0.0, -0.05), Vec3::new(2.0, 2.0, 0.05), Quat::IDENTITY),
        layers::ALL,
        true,
    );
    let w = b.build();
    let geo = Geometry::new(&w);
    let run = |p: ProfileId, on: bool, other: bool| -> Vec<(Vec3, Vec3, Vec3, Quat)> {
        let mut s = solver(g0());
        let m = s.add_magnet(Magnet {
            position: Vec3::new(0.0, 0.0, -0.005),
            k_coil: 2.0e-5,
            d_min: 0.01,
            range: 0.1,
            layer_mask: layers::ALL,
        });
        if on {
            s.set_magnet_power(m, 1.0);
        }
        let r = rad(p);
        let mut bl = Ball::new(p, Vec3::new(-0.2, 0.0, r));
        bl.velocity = Vec3::new(1.5, 0.05, 0.0);
        let h = s.add_ball(bl).unwrap();
        if other {
            // A steel ball crossing the magnet field elsewhere (different sub-steps).
            let mut o = Ball::new(STEEL, Vec3::new(0.0, -0.3, rad(STEEL)));
            o.velocity = Vec3::new(0.0, 2.95, 0.0);
            s.add_ball(o).unwrap();
            let mut o2 = Ball::new(STEEL, Vec3::new(0.05, 0.5, 0.05));
            o2.velocity = Vec3::new(0.0, -6.0, 0.0);
            s.add_ball(o2).unwrap();
        }
        let mut ev = BallEvents::new();
        let mut out = vec![];
        let mut mind = f32::INFINITY;
        for _ in 0..400 {
            s.step(&geo, DT, &mut ev);
            let bb = *s.ball(h).unwrap();
            for (hh, o) in s.balls() { if hh != h { mind = mind.min(o.position.distance(bb.position) - rad(o.profile) - rad(p)); } }
            out.push((bb.position, bb.velocity, bb.angular_velocity, bb.orientation));
        }
        if other { println!("c2 p={p:?} on={on}: min surface gap to other balls = {mind:e}"); }
        out
    };
    for p in [POWER, GLO] {
        for other in [false, true] {
            let a = run(p, false, other);
            let bb = run(p, true, other);
            let first = a.iter().zip(&bb).position(|(x, y)| (x.0, x.1, x.2) != (y.0, y.1, y.2));
            let firsto = a.iter().zip(&bb).position(|(x, y)| x.3 != y.3);
            let maxd = a.iter().zip(&bb).map(|(x, y)| x.0.distance(y.0)).fold(0.0f32, f32::max);
            println!("c2 p={p:?} other_balls={other}: first differing tick (pos/vel/w) = {first:?}, orientation = {firsto:?}, max pos diff = {maxd:e}");
            // also: did the other balls ever touch? (if they collide with p, a difference is legit)
        }
    }
    // Sanity: steel ball is affected.
    let a = run(STEEL, false, false);
    let bb = run(STEEL, true, false);
    println!(
        "c2 steel differs: {:?}",
        a.iter().zip(&bb).position(|(x, y)| x != y)
    );
}

#[test]
fn p_criterion5_sweep() {
    for &(slope, mesh) in &[(0.0f32, false), (6.5, false), (6.5, true)] {
        let (w, n, _d) = slope_world(slope, mesh, 0.0);
        let geo = Geometry::new(&w);
        let mut worst_end = 0.0f32;
        let mut worst_tail = 0.0f32;
        let mut cases = 0;
        for p in [STEEL, POWER, MINI, JUMBO] {
            for spin in [
                Vec3::new(300.0, 0.0, 0.0),
                Vec3::new(0.0, -500.0, 0.0),
                Vec3::new(0.0, 0.0, 400.0),
                Vec3::new(200.0, 200.0, 200.0),
                Vec3::new(-1000.0, 50.0, 10.0),
            ] {
                for vel in [Vec3::ZERO, Vec3::new(1.0, 0.5, 0.0), Vec3::new(-3.0, 0.0, -1.0)] {
                    for hgt in [0.0f32, 0.05] {
                        let r = rad(p);
                        let mut s = solver(g0());
                        let mut b = Ball::new(p, n * (r + hgt + 1e-6) + Vec3::new(0.0, 0.5, 0.0) * n.z);
                        b.angular_velocity = spin;
                        b.velocity = vel;
                        let h = s.add_ball(b).unwrap();
                        let mut ev = BallEvents::new();
                        let mut tail = 0.0f32;
                        let ticks = 1500;
                        for k in 0..ticks {
                            s.step(&geo, DT, &mut ev);
                            let bb = s.ball(h).unwrap();
                            let slip = bb.velocity + bb.angular_velocity.cross(-n * r);
                            if k > ticks - 300 {
                                tail = tail.max(slip.length());
                            }
                        }
                        let bb = s.ball(h).unwrap();
                        let slip = (bb.velocity + bb.angular_velocity.cross(-n * r)).length();
                        if tail > 1e-3 {
                            println!(
                                "c5 FAIL slope={slope} mesh={mesh} p={p:?} spin={spin} vel={vel} h={hgt}: tail={tail:.3e} end={slip:.3e} pos={:?} w={:?}",
                                bb.position, bb.angular_velocity
                            );
                        }
                        worst_end = worst_end.max(slip);
                        worst_tail = worst_tail.max(tail);
                        cases += 1;
                    }
                }
            }
        }
        println!("c5 slope={slope} mesh={mesh}: {cases} cases, worst tail slip={worst_tail:.3e}, worst end={worst_end:.3e}");
    }
}

#[test]
fn p_criterion4_sweep() {
    let w = StaticWorldBuilder::new().build();
    let geo = Geometry::new(&w);
    let mut seed = 99u64;
    let mut rnd = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as f32) / (1u64 << 31) as f32 * 2.0 - 1.0
    };
    let (mut wp, mut wl, mut we) = (0.0f64, 0.0f64, 0.0f64);
    for _ in 0..300 {
        let mut s = solver(Vec3::ZERO);
        let rm = rad(MINI);
        let rj = rad(JUMBO);
        let b_param = rnd() * (rm + rj) * 0.999;
        let speed = 0.06 + 30.0 * (rnd() * 0.5 + 0.5);
        let mut a = Ball::new(MINI, Vec3::new(-0.2, b_param, rnd() * 0.0));
        a.velocity = Vec3::new(speed, 0.0, 0.0);
        a.angular_velocity = Vec3::new(rnd(), rnd(), rnd()) * 3000.0;
        let mut j = Ball::new(JUMBO, Vec3::ZERO);
        j.velocity = Vec3::new(rnd(), rnd(), rnd()) * 2.0;
        j.angular_velocity = Vec3::new(rnd(), rnd(), rnd()) * 500.0;
        s.add_ball(a).unwrap();
        s.add_ball(j).unwrap();
        let mom = |s: &BallSolver| -> glam::DVec3 {
            s.balls().map(|(_, b)| b.velocity.as_dvec3() * mass(b.profile) as f64).sum()
        };
        let angm = |s: &BallSolver| -> glam::DVec3 {
            let mt: f64 = s.balls().map(|(_, b)| mass(b.profile) as f64).sum();
            let c: glam::DVec3 = s
                .balls()
                .map(|(_, b)| b.position.as_dvec3() * mass(b.profile) as f64)
                .sum::<glam::DVec3>()
                / mt;
            s.balls()
                .map(|(_, b)| {
                    let m = mass(b.profile) as f64;
                    (b.position.as_dvec3() - c).cross(b.velocity.as_dvec3() * m)
                        + b.angular_velocity.as_dvec3() * inertia(b.profile) as f64
                })
                .sum()
        };
        let (p0, l0, e0) = (mom(&s), angm(&s), ke(&s));
        let mut ev = BallEvents::new();
        let mut emax = e0;
        for _ in 0..40 {
            s.step(&geo, DT, &mut ev);
            let e = ke(&s);
            we = we.max((e - emax) / e0);
            emax = emax.max(e);
            wp = wp.max((mom(&s) - p0).length() / p0.length());
            wl = wl.max((angm(&s) - l0).length() / l0.length().max(1e-12));
        }
    }
    println!("c4 sweep: worst p rel={wp:.3e} (tol 1e-4), L rel={wl:.3e} (tol 1e-3), KE gain rel={we:.3e}");
}

fn straight(n: usize, len: f32, z: f32) -> CubicSpline {
    let pts: Vec<Vec3> = (0..n)
        .map(|i| Vec3::new(len * i as f32 / (n - 1) as f32, 0.0, z))
        .collect();
    CubicSpline::interpolate(&pts).unwrap()
}

#[test]
fn p_newton() {
    let w = StaticWorldBuilder::new().build();
    let geo = Geometry::new(&w);
    let r = rad(STEEL);
    for nb in [3usize, 5, 8] {
        for &g in &[Vec3::ZERO, g0()] {
            // path length 1 m, param 0..(n-1) with n=11 points → 0.1 m per t
            let path = straight(11, 1.0, 0.0);
            let ts: Vec<f32> = (0..nb).map(|i| (0.3 + 2.0 * r * i as f32) / 0.1).collect();
            let chain = NewtonChain::new(path, STEEL, &ts).unwrap();
            let mut s = solver(g);
            let ci = s.add_newton_chain(chain);
            s.newton_chain_mut(ci).strike(0, 1.0);
            let mut ev = BallEvents::new();
            s.step(&geo, DT, &mut ev);
            let a = s.newton_chain(ci).ball_speed(nb - 1);
            s.step(&geo, DT, &mut ev);
            let b2 = s.newton_chain(ci).ball_speed(nb - 1);
            let speeds: Vec<f32> = (0..nb).map(|i| s.newton_chain(ci).ball_speed(i)).collect();
            println!("newton strike n={nb} g={}: far speed tick1={a:.4} tick2={b2:.4} speeds={speeds:?}", g.z);
        }
        // free ball impact, oblique, zero g; energy accounting (chain KE counted as 1/2 m u^2 * 7/5? report both)
        for ang in [0.0f32, 20.0, 45.0] {
            let path = straight(11, 1.0, 0.0);
            let ts: Vec<f32> = (0..nb).map(|i| (0.3 + 2.0 * r * i as f32) / 0.1).collect();
            let chain = NewtonChain::new(path, STEEL, &ts).unwrap();
            let mut s = solver(Vec3::ZERO);
            let ci = s.add_newton_chain(chain);
            let a = ang.to_radians();
            let dir = Vec3::new(a.cos(), a.sin(), 0.0);
            let target = Vec3::new(0.3, 0.0, 0.0);
            let mut fb = Ball::new(STEEL, target - dir * 0.1);
            fb.velocity = dir * 2.0;
            let h = s.add_ball(fb).unwrap();
            let m = mass(STEEL) as f64;
            let ke0 = ke(&s);
            let mut ev = BallEvents::new();
            let mut hit_tick = None;
            let mut reach = None;
            let mut maxe = 0.0f64;
            for k in 0..200 {
                ev.clear();
                s.step(&geo, DT, &mut ev);
                if hit_tick.is_none() && impacts(&ev) > 0 {
                    hit_tick = Some(k);
                }
                if reach.is_none() && s.newton_chain(ci).ball_speed(nb - 1) != 0.0 {
                    reach = Some(k);
                }
                let chain_ke: f64 = (0..nb)
                    .map(|i| 0.5 * m * (s.newton_chain(ci).ball_speed(i) as f64).powi(2))
                    .sum();
                maxe = maxe.max((ke(&s) + chain_ke - ke0) / ke0);
            }
            println!(
                "newton free-ball n={nb} ang={ang}: impact tick={hit_tick:?} far moves tick={reach:?} max rel energy gain(translational chain KE)={maxe:.3e} free v={:?}",
                s.ball(h).unwrap().velocity
            );
        }
    }
}

#[test]
fn p_guide_climb() {
    // Quarter+ arc guide of radius R=0.15 m centred at (0, R), wall height 30 mm,
    // floor at z=0. Ball starts at the arc's start rolling along +x.
    for &(v0, mu_note) in &[(2.0f32, ""), (4.0, ""), (8.0, ""), (12.0, "")] {
        let _ = mu_note;
        let rw = 0.15f32;
        let pts: Vec<Vec3> = (0..=60)
            .map(|i| {
                let a = (-15.0f32 + 3.0 * i as f32).to_radians();
                Vec3::new(rw * a.sin(), rw - rw * a.cos(), 0.0)
            })
            .collect();
        let guide = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap();
        let mut b = StaticWorldBuilder::new();
        b.add_collider(
            cub(Vec3::new(0.0, 0.0, -0.05), Vec3::new(2.0, 2.0, 0.05), Quat::IDENTITY),
            layers::ALL,
            true,
        );
        let w = b.build();
        let guides = [guide];
        let geo = Geometry {
            world: &w,
            guides: &guides,
            wireforms: &[],
        };
        for (spin_on, label) in [(true, "rolling"), (false, "sliding,no-spin")] {
            let r = rad(STEEL);
            let mut s = solver(g0());
            let mut bl = Ball::new(STEEL, Vec3::new(0.0, r, r));
            bl.velocity = Vec3::new(v0, 0.0, 0.0);
            if spin_on {
                bl.angular_velocity = Vec3::new(0.0, v0 / r, 0.0);
            }
            let h = s.add_ball(bl).unwrap();
            let mut ev = BallEvents::new();
            let mut zmax = 0.0f32;
            let mut vzmax = 0.0f32;
            let mut phi_at = 0.0f32;
            let mut nimp = 0;
            let mut esc = false;
            let e0 = mech(&s, g0());
            let mut emax = 0.0f64;
            let mut bound_viol = 0.0f32;
            for _ in 0..200 {
                ev.clear();
                s.step(&geo, DT, &mut ev);
                nimp += impacts(&ev);
                let bb = *s.ball(h).unwrap();
                let phi = libm::atan2f(bb.velocity.y, bb.velocity.x);
                if bb.position.z - r > zmax {
                    zmax = bb.position.z - r;
                }
                if bb.velocity.z > vzmax {
                    vzmax = bb.velocity.z;
                    phi_at = phi;
                }
                // Coulomb bound: v_z <= mu*v*phi (mu = 0.15, steel-steel) while in the arc
                if phi > 0.0 && phi < 2.5 {
                    let bound = 0.15 * v0 * phi;
                    bound_viol = bound_viol.max(bb.velocity.z - bound);
                }
                emax = emax.max(mech(&s, g0()) - e0);
                let c = Vec3::new(0.0, rw, bb.position.z);
                if bb.position.distance(c) > rw + 0.001 {
                    esc = true;
                }
            }
            println!(
                "guide v0={v0} {label}: zmax={:.2} mm vzmax={vzmax:.3} at phi={phi_at:.2} (mu*v*phi={:.3}) excess over Coulomb bound={bound_viol:.3e} impacts={nimp} escaped={esc} dE+={emax:.3e}",
                zmax * 1000.0,
                0.15 * v0 * phi_at
            );
        }
    }
}

#[test]
fn p_wireform() {
    // Straight wireform rising 0.1 m over 1 m, gauge 20 mm; ball launched up it.
    let n = 21;
    let pts: Vec<Vec3> = (0..n)
        .map(|i| {
            let x = i as f32 / (n - 1) as f32;
            Vec3::new(x, 0.0, 0.1 * x)
        })
        .collect();
    let cl = CubicSpline::interpolate(&pts).unwrap();
    let wf = Wireform::new(cl, &vec![0.0; n], 0.02, Vec3::Z).unwrap();
    let w = StaticWorldBuilder::new().build();
    let wfs = [wf];
    let geo = Geometry {
        world: &w,
        guides: &[],
        wireforms: &wfs,
    };
    for p in [STEEL, MINI, JUMBO] {
        let r = rad(p);
        let hgt = (r * r - 0.01f32 * 0.01).sqrt();
        for v0 in [1.0f32, 4.0, 10.0, 20.0, 30.0] {
            for lat in [0.0f32, 0.3] {
                let mut s = solver(g0());
                let x0 = 0.05;
                let mut bl = Ball::new(p, Vec3::new(x0, 0.0, 0.1 * x0 + hgt + 1e-6));
                bl.velocity = Vec3::new(v0, lat, v0 * 0.1);
                let h = s.add_ball(bl).unwrap();
                let mut ev = BallEvents::new();
                let e0 = mech(&s, g0());
                let mut emax = 0.0f64;
                let mut minrel = f32::INFINITY;
                let mut nimp = 0;
                for _ in 0..300 {
                    ev.clear();
                    s.step(&geo, DT, &mut ev);
                    nimp += impacts(&ev);
                    let bb = s.ball(h).unwrap();
                    if bb.position.x > 0.02 && bb.position.x < 0.98 {
                        minrel = minrel.min(bb.position.z - 0.1 * bb.position.x - hgt);
                    }
                    emax = emax.max((mech(&s, g0()) - e0) / e0.abs().max(1e-9));
                }
                let bb = s.ball(h).unwrap();
                println!(
                    "wire p={p:?} v0={v0} lat={lat}: min height rel rails={:.3} mm impacts={nimp} dE+rel={emax:.3e} end={:?}",
                    minrel * 1000.0,
                    bb.position
                );
            }
        }
    }
}

#[test]
fn p_seam() {
    // Two flush cuboids meeting at x=0; ball rolls across.
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cub(Vec3::new(-1.0, 0.0, -0.05), Vec3::new(1.0, 1.0, 0.05), Quat::IDENTITY),
        layers::ALL,
        true,
    );
    b.add_collider(
        cub(Vec3::new(1.0, 0.0, -0.05), Vec3::new(1.0, 1.0, 0.05), Quat::IDENTITY),
        layers::ALL,
        true,
    );
    let w = b.build();
    let geo = Geometry::new(&w);
    for v0 in [1.0f32, 2.0, 4.0, 8.0, 12.0, 20.0] {
        let r = rad(STEEL);
        let mut s = solver(g0());
        let mut bl = Ball::new(STEEL, Vec3::new(-0.5, 0.0, r));
        bl.velocity = Vec3::new(v0, 0.0, 0.0);
        bl.angular_velocity = Vec3::new(0.0, v0 / r, 0.0);
        let h = s.add_ball(bl).unwrap();
        let mut ev = BallEvents::new();
        let mut zmax = 0.0f32;
        let mut vzmax = 0.0f32;
        let mut nimp = 0;
        for _ in 0..((1.0 / v0 / DT) as usize + 50) {
            ev.clear();
            s.step(&geo, DT, &mut ev);
            nimp += impacts(&ev);
            let bb = s.ball(h).unwrap();
            zmax = zmax.max(bb.position.z - r);
            vzmax = vzmax.max(bb.velocity.z);
        }
        println!(
            "seam v0={v0}: hop={:.1} um vz_max={vzmax:.4} ({:.2}% of v) impacts={nimp}",
            zmax * 1e6,
            100.0 * vzmax / v0
        );
    }
}

#[test]
fn p_magnet_cutoff_and_cap() {
    let w = StaticWorldBuilder::new().build();
    let geo = Geometry::new(&w);
    // Force inside d_min equal to cap, at d slightly below range nonzero, beyond zero.
    for d in [0.001f32, 0.005, 0.0099, 0.01, 0.02, 0.0499, 0.05, 0.0501] {
        let mut s = solver(Vec3::ZERO);
        let m = s.add_magnet(Magnet {
            position: Vec3::ZERO,
            k_coil: 1.0e-5,
            d_min: 0.01,
            range: 0.05,
            layer_mask: layers::ALL,
        });
        s.set_magnet_power(m, 1.0);
        let h = s.add_ball(Ball::new(STEEL, Vec3::new(d, 0.0, 0.0))).unwrap();
        let mut ev = BallEvents::new();
        s.step(&geo, DT, &mut ev);
        let dv = s.ball(h).unwrap().velocity.x;
        let c = profiles()[0].magnetic_coupling;
        let expect = -c * 1e-5 / d.max(0.01).powi(3) * DT / mass(STEEL);
        println!("magnet d={d}: dv={dv:.4e} expected(c={c})={:.4e}", if d <= 0.05 { expect } else { 0.0 });
    }
}

#[test]
fn p_low_speed_bounce_events() {
    // Drops from small heights: the number of reported impacts and the
    // settling, on a 6.5 deg mesh slope and flat cuboid.
    for &(slope, mesh) in &[(0.0f32, false), (6.5, true), (6.5, false)] {
        let (w, n, _) = slope_world(slope, mesh, 0.0);
        let geo = Geometry::new(&w);
        for hgt in [1e-5f32, 1e-4, 2e-4, 5e-4, 1e-3, 1e-2] {
            let r = rad(STEEL);
            let mut s = solver(g0());
            let h = s.add_ball(Ball::new(STEEL, n * (r + hgt))).unwrap();
            let mut ev = BallEvents::new();
            let mut last_imp_tick = 0;
            for k in 0..2000 {
                let before = impacts(&ev);
                s.step(&geo, DT, &mut ev);
                if impacts(&ev) > before {
                    last_imp_tick = k;
                }
            }
            let speeds: Vec<f32> = ev
                .as_slice()
                .iter()
                .filter_map(|e| match e {
                    BallEvent::Impact { normal_speed, .. } => Some(*normal_speed),
                    _ => None,
                })
                .collect();
            let bb = s.ball(h).unwrap();
            let height = bb.position.dot(n) - r;
            println!(
                "drop slope={slope} mesh={mesh} h={hgt:e}: impacts={} last at tick {last_imp_tick} speeds={:?} final height={height:.2e} dropped={}",
                speeds.len(),
                &speeds[..speeds.len().min(6)],
                ev.dropped()
            );
        }
    }
}

#[test]
fn p_pinned_between_balls() {
    // Mini ball pinned between two jumbo balls resting in a floor corner, gravity
    // pushing them all into the corner (tilted gravity). Check rest & events.
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cub(Vec3::new(0.0, 0.0, -0.05), Vec3::new(2.0, 2.0, 0.05), Quat::IDENTITY),
        layers::ALL,
        true,
    );
    b.add_collider(
        cub(Vec3::new(-0.05, 0.0, 0.05), Vec3::new(0.05, 2.0, 0.05), Quat::IDENTITY),
        layers::ALL,
        true,
    );
    let w = b.build();
    let geo = Geometry::new(&w);
    let pitch = 6.5f32.to_radians();
    let g = Vec3::new(-9.81 * pitch.sin(), 0.0, -9.81 * pitch.cos());
    let rj = rad(JUMBO);
    let rm = rad(MINI);
    let mut s = solver(g);
    let h1 = s.add_ball(Ball::new(JUMBO, Vec3::new(rj, 0.0, rj))).unwrap();
    // mini in the dip between jumbo 1 and jumbo 2, resting on floor
    let x2 = rj + 2.0 * rm + rj + 0.0;
    let hm = s.add_ball(Ball::new(MINI, Vec3::new(rj + rm + 0.0005, 0.0, rm))).unwrap();
    let h2 = s.add_ball(Ball::new(JUMBO, Vec3::new(x2 + 0.002, 0.0, rj))).unwrap();
    let mut ev = BallEvents::new();
    for _ in 0..3000 {
        s.step(&geo, DT, &mut ev);
    }
    let si = impacts(&ev);
    ev.clear();
    let p0: Vec<Vec3> = [h1, hm, h2].iter().map(|h| s.ball(*h).unwrap().position).collect();
    let e0 = mech(&s, g);
    let mut emax = 0.0f64;
    let mut md = [0.0f32; 3];
    for _ in 0..10000 {
        s.step(&geo, DT, &mut ev);
        for (k, h) in [h1, hm, h2].iter().enumerate() {
            md[k] = md[k].max(s.ball(*h).unwrap().position.distance(p0[k]));
        }
        emax = emax.max(mech(&s, g) - e0);
    }
    println!(
        "pinned: settle impacts={si} drift={md:?} impacts10s={} dE+={emax:.3e} pos={p0:?}",
        impacts(&ev)
    );
    let _ = ImpactTarget::Guide(0);
}

fn big_slope_world(deg: f32, mesh: bool) -> (StaticWorld, Vec3) {
    let th = deg.to_radians();
    let rot = Quat::from_rotation_x(th);
    let n = rot * Vec3::Z;
    let mut b = StaticWorldBuilder::new();
    if mesh {
        let u = rot * Vec3::X;
        let w = rot * Vec3::Y;
        let s = 60.0;
        b.add_collider(
            StaticShape::TriMesh {
                vertices: vec![-u * s - w * s, u * s - w * s, u * s + w * s, -u * s + w * s],
                indices: vec![[0, 1, 2], [0, 2, 3]],
            },
            layers::ALL,
            true,
        );
    } else {
        b.add_collider(cub(-n * 0.05, Vec3::new(60.0, 60.0, 0.05), rot), layers::ALL, true);
    }
    (b.build(), n)
}

#[test]
fn q_criterion5_big_floor() {
    for &(slope, mesh) in &[(0.0f32, false), (6.5, false), (6.5, true)] {
        let (w, n) = big_slope_world(slope, mesh);
        let geo = Geometry::new(&w);
        let mut worst_tail = 0.0f32;
        let mut cases = 0;
        let mut fails = 0;
        for p in [STEEL, POWER, MINI, JUMBO] {
            for spin in [
                Vec3::new(300.0, 0.0, 0.0),
                Vec3::new(0.0, -500.0, 0.0),
                Vec3::new(0.0, 0.0, 400.0),
                Vec3::new(200.0, 200.0, 200.0),
                Vec3::new(-1000.0, 50.0, 10.0),
            ] {
                for vel in [Vec3::ZERO, Vec3::new(1.0, 0.5, 0.0), Vec3::new(-3.0, 0.0, -1.0)] {
                    for hgt in [0.0f32, 0.05] {
                        let r = rad(p);
                        let mut s = solver(g0());
                        let mut b = Ball::new(p, n * (r + hgt + 1e-6));
                        b.angular_velocity = spin;
                        b.velocity = vel;
                        let h = s.add_ball(b).unwrap();
                        let mut ev = BallEvents::new();
                        let mut tail = 0.0f32;
                        let ticks = 4000;
                        for k in 0..ticks {
                            s.step(&geo, DT, &mut ev);
                            let bb = s.ball(h).unwrap();
                            let slip = bb.velocity + bb.angular_velocity.cross(-n * r);
                            if k >= ticks - 1000 {
                                tail = tail.max(slip.length());
                            }
                        }
                        if tail > 1e-3 {
                            fails += 1;
                            let bb = s.ball(h).unwrap();
                            println!("c5 FAIL slope={slope} mesh={mesh} p={p:?} spin={spin} vel={vel} h={hgt}: tail={tail:.3e} pos={:?} v={:?}", bb.position, bb.velocity);
                        }
                        worst_tail = worst_tail.max(tail);
                        cases += 1;
                    }
                }
            }
        }
        println!("c5 slope={slope} mesh={mesh}: {cases} cases, {fails} fail, worst slip over last 1 s of 4 s = {worst_tail:.3e}");
    }
}

#[test]
fn q_thin_wall_big() {
    let mut fails = 0;
    let mut n = 0;
    for &mesh in &[false, true] {
        for &tilt_deg in &[0.0f32, 30.0, 60.0, 80.0] {
            let rot = Quat::from_rotation_z(tilt_deg.to_radians());
            let nrm = rot * Vec3::X;
            let mut b = StaticWorldBuilder::new();
            if mesh {
                let u = rot * Vec3::Y * 20.0;
                let v = Vec3::Z * 20.0;
                b.add_collider(
                    StaticShape::TriMesh {
                        vertices: vec![-u - v, u - v, u + v, -u + v],
                        indices: vec![[0, 1, 2], [0, 2, 3]],
                    },
                    layers::ALL,
                    true,
                );
            } else {
                b.add_collider(cub(Vec3::ZERO, Vec3::new(0.001, 20.0, 20.0), rot), layers::ALL, true);
            }
            let w = b.build();
            let geo = Geometry::new(&w);
            for p in [STEEL, MINI, JUMBO] {
                for speed in [10.0f32, 20.0, 30.0, 45.0] {
                    for ang in [0.0f32, 30.0, 60.0, 85.0, 89.0] {
                        for off in [0.0f32, 0.0003, 0.0007] {
                            let a = ang.to_radians();
                            let side = (rot * Vec3::Y) * a.sin();
                            let dir = (nrm * a.cos() + side).normalize();
                            let mut s = solver(Vec3::ZERO);
                            let mut ball = Ball::new(p, -nrm * (0.03 + off));
                            ball.velocity = dir * speed;
                            ball.angular_velocity = Vec3::new(300.0, -500.0, 800.0);
                            let h = s.add_ball(ball).unwrap();
                            let mut ev = BallEvents::new();
                            for _ in 0..30 {
                                s.step(&geo, DT, &mut ev);
                            }
                            n += 1;
                            let x = s.ball(h).unwrap().position.dot(nrm);
                            if x > 0.0 {
                                fails += 1;
                                println!("TUNNEL mesh={mesh} tilt={tilt_deg} p={p:?} v={speed} ang={ang} x={x}");
                            }
                        }
                    }
                }
            }
        }
    }
    println!("thin wall big sweep: {fails}/{n} tunnelled");
}

#[test]
fn q_pinned_and_row_series() {
    // row of 3 on slope against a post
    let (w, n, d) = slope_world(6.5, false, 0.005);
    let geo = Geometry::new(&w);
    let r = rad(STEEL);
    let mut s = solver(g0());
    let mut hs = vec![];
    for k in 0..3 {
        let pos = n * (r + 1e-6) - d * (0.005 + r + 2.0 * r * k as f32 + 1e-5 * k as f32);
        hs.push(s.add_ball(Ball::new(STEEL, pos)).unwrap());
    }
    let mut ev = BallEvents::new();
    for _ in 0..2000 {
        s.step(&geo, DT, &mut ev);
    }
    let p0: Vec<Vec3> = hs.iter().map(|h| s.ball(*h).unwrap().position).collect();
    let mut last = p0.clone();
    for t in 0..10000 {
        s.step(&geo, DT, &mut ev);
        for k in 0..3 {
            let p = s.ball(hs[k]).unwrap().position;
            let jump = p.distance(last[k]);
            if jump > 5e-8 {
                println!("row t={t} ball{k} jump={jump:.3e} dir={:?} total={:.3e}", (p - last[k]) / jump, p.distance(p0[k]));
            }
            last[k] = p;
        }
    }
    // pinned
    let mut b = StaticWorldBuilder::new();
    b.add_collider(cub(Vec3::new(0.0, 0.0, -0.05), Vec3::new(2.0, 2.0, 0.05), Quat::IDENTITY), layers::ALL, true);
    b.add_collider(cub(Vec3::new(-0.05, 0.0, 0.05), Vec3::new(0.05, 2.0, 0.05), Quat::IDENTITY), layers::ALL, true);
    let w = b.build();
    let geo = Geometry::new(&w);
    let pitch = 6.5f32.to_radians();
    let g = Vec3::new(-9.81 * pitch.sin(), 0.0, -9.81 * pitch.cos());
    let rj = rad(JUMBO);
    let rm = rad(MINI);
    let mut s = solver(g);
    let h1 = s.add_ball(Ball::new(JUMBO, Vec3::new(rj, 0.0, rj))).unwrap();
    let x2 = rj + 2.0 * rm + rj;
    let hm = s.add_ball(Ball::new(MINI, Vec3::new(rj + rm + 0.0005, 0.0, rm))).unwrap();
    let h2 = s.add_ball(Ball::new(JUMBO, Vec3::new(x2 + 0.002, 0.0, rj))).unwrap();
    let hs = [h1, hm, h2];
    let mut ev = BallEvents::new();
    for _ in 0..3000 {
        s.step(&geo, DT, &mut ev);
    }
    let p0: Vec<Vec3> = hs.iter().map(|h| s.ball(*h).unwrap().position).collect();
    let e0 = mech(&s, g);
    for t in 0..10000 {
        s.step(&geo, DT, &mut ev);
        if t % 1000 == 999 {
            let d: Vec<f32> = (0..3).map(|k| s.ball(hs[k]).unwrap().position.distance(p0[k])).collect();
            let v: Vec<f32> = (0..3).map(|k| s.ball(hs[k]).unwrap().velocity.length()).collect();
            println!("pinned t={} drift={d:?} |v|={v:?} dE={:.3e}", t + 1, mech(&s, g) - e0);
        }
    }
    for k in 0..3 {
        let b = s.ball(hs[k]).unwrap();
        println!("pinned ball{k} pos={:?} w={:?}", b.position, b.angular_velocity);
    }
}

#[test]
fn r_post_series() {
    for &(p, deg, mesh) in &[(POWER, 6.5f32, false), (STEEL, 3.0, false), (MINI, 6.5, false), (STEEL, 6.5, false), (STEEL, 6.5, true)] {
        let (w, n, d) = slope_world(deg, mesh, 0.005);
        let geo = Geometry::new(&w);
        let r = rad(p);
        let mut s = solver(g0());
        let pos = n * r - d * (0.005 + r) + n * 1e-6 - d * 1e-6;
        let h = s.add_ball(Ball::new(p, pos)).unwrap();
        let mut ev = BallEvents::new();
        let mut last = pos;
        let mut jumps = 0;
        let mut maxtot = 0.0f32;
        for t in 0..11000 {
            s.step(&geo, DT, &mut ev);
            let q = s.ball(h).unwrap().position;
            let j = q.distance(last);
            if j > 1e-7 {
                jumps += 1;
                if jumps < 6 {
                    println!("post p={p:?} deg={deg} mesh={mesh} t={t} jump={j:.4e} dir={:?} gap_post={:.4e}", (q - last) / j, (q - n * q.dot(n)).length() - 0.005 - r);
                }
            }
            if t >= 1000 {
                maxtot = maxtot.max(q.distance(Vec3::ZERO));
            }
            last = q;
        }
        let q = s.ball(h).unwrap().position;
        println!("post p={p:?} deg={deg} mesh={mesh}: jumps>0.1um={jumps} final gap_post={:.4e} height={:.4e}", (q - n * q.dot(n)).length() - 0.005 - r, q.dot(n) - r);
    }
}

#[test]
fn r_mesh_slip_spike() {
    let (w, n) = big_slope_world(6.5, true);
    let geo = Geometry::new(&w);
    let r = rad(MINI);
    let mut s = solver(g0());
    let mut b = Ball::new(MINI, n * (r + 1e-6));
    b.angular_velocity = Vec3::new(300.0, 0.0, 0.0);
    b.velocity = Vec3::new(-3.0, 0.0, -1.0);
    let h = s.add_ball(b).unwrap();
    let mut ev = BallEvents::new();
    for k in 0..4000 {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        let bb = s.ball(h).unwrap();
        let slip = (bb.velocity + bb.angular_velocity.cross(-n * r)).length();
        if k > 2000 && slip > 1e-4 {
            let u = Quat::from_rotation_x(6.5f32.to_radians()).inverse() * bb.position;
            println!("mesh slip spike t={k} slip={slip:.3e} local pos={u:?} vn={:.3e} height={:.3e} ev={}", bb.velocity.dot(n), bb.position.dot(n) - r, ev.as_slice().len());
        }
    }
}

#[test]
fn s_flat_hover() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(cub(Vec3::new(0.0, 0.0, -0.05), Vec3::new(2.0, 2.0, 0.05), Quat::IDENTITY), layers::ALL, true);
    let w = b.build();
    let geo = Geometry::new(&w);
    for p in [STEEL, POWER, MINI, JUMBO] {
        for gap in [0.0f32, 3e-7, 6e-7, 9e-7, 1e-6, 3e-6, 1.5e-5, 1e-3, 3e-3] {
            for xy in [Vec3::ZERO, Vec3::new(0.3, -0.7, 0.0)] {
                let r = rad(p);
                let mut s = solver(g0());
                let h = s.add_ball(Ball::new(p, xy + Vec3::new(0.0, 0.0, r + gap))).unwrap();
                let mut ev = BallEvents::new();
                for _ in 0..1000 {
                    s.step(&geo, DT, &mut ev);
                }
                let p0 = s.ball(h).unwrap().position;
                let mut md = 0.0f32;
                let mut when = 0;
                for t in 0..10000 {
                    s.step(&geo, DT, &mut ev);
                    let d = s.ball(h).unwrap().position.distance(p0);
                    if d > md { md = d; when = t; }
                }
                if md > 1e-7 {
                    println!("flat-hover p={p:?} gap={gap:e} xy={xy}: height after settle={:.3e} drift10s={md:.4e} at t={when}", p0.z - r);
                }
            }
        }
    }
    println!("flat-hover done");
}

const SPEED: f32 = 6.7;
#[test]
fn t_c2_far_steel() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(cub(Vec3::new(0.0, 0.0, -0.05), Vec3::new(3.0, 3.0, 0.05), Quat::IDENTITY), layers::ALL, true);
    let w = b.build();
    let geo = Geometry::new(&w);
    let run = |p: ProfileId, on: bool| -> (Vec<(Vec3, Vec3, Vec3)>, f32) {
        let mut s = solver(g0());
        for pos in [Vec3::new(0.0, 0.0, -0.005), Vec3::new(1.0, 1.0, -0.005)] {
            let m = s.add_magnet(Magnet { position: pos, k_coil: 2.0e-5, d_min: 0.01, range: 0.1, layer_mask: layers::ALL });
            if on { s.set_magnet_power(m, 1.0); }
        }
        let r = rad(p);
        let mut bl = Ball::new(p, Vec3::new(-0.2, 0.0, r));
        bl.velocity = Vec3::new(1.5, 0.05, 0.0);
        let h = s.add_ball(bl).unwrap();
        // steel ball rolling past the far magnet (1 m away from the powerball's path)
        let mut o = Ball::new(STEEL, Vec3::new(1.0, 0.75, rad(STEEL)));
        o.velocity = Vec3::new(0.02, SPEED, 0.0);
        let ho = s.add_ball(o).unwrap();
        let mut ev = BallEvents::new();
        let mut out = vec![];
        let mut mind = f32::INFINITY;
        for _ in 0..400 {
            s.step(&geo, DT, &mut ev);
            let bb = s.ball(h).unwrap();
            mind = mind.min(bb.position.distance(s.ball(ho).unwrap().position));
            out.push((bb.position, bb.velocity, bb.angular_velocity));
        }
        (out, mind)
    };
    for p in [POWER, GLO] {
        let (a, m1) = run(p, false);
        println!("(steel speed {SPEED})");
        let (b2, m2) = run(p, true);
        let first = a.iter().zip(&b2).position(|(x, y)| x != y);
        let maxd = a.iter().zip(&b2).map(|(x, y)| x.0.distance(y.0)).fold(0.0f32, f32::max);
        println!("c2-far p={p:?}: first differing tick={first:?} max pos diff={maxd:e} min distance to steel ball={m1:.3}/{m2:.3} m");
    }
}

#[test]
fn u_rising_guide() {
    let w = StaticWorldBuilder::new().build();
    for (label, curved) in [("straight rising", false), ("curved rising", true)] {
        let pts: Vec<Vec3> = (0..=40)
            .map(|i| {
                let s = i as f32 / 40.0;
                if curved {
                    let a = s * 2.0;
                    Vec3::new(0.4 * libm::sinf(a), 0.4 - 0.4 * libm::cosf(a), 0.15 * s)
                } else {
                    Vec3::new(s, 0.0, 0.1 * s)
                }
            })
            .collect();
        let g = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.05).unwrap();
        println!("{label}: is_level={}", g.is_level());
        let guides = [g];
        let geo = Geometry { world: &w, guides: &guides, wireforms: &[] };
        for v0 in [0.5f32, 2.0, 5.0, 10.0] {
            let r = rad(STEEL);
            let mut s = solver(Vec3::ZERO);
            let (pos, vel) = if curved {
                (Vec3::new(0.0, r, 0.02 + 0.0), Vec3::new(v0, -0.02, 0.0375 * v0 / 0.4 * 0.4 / 1.0 * 0.0 + 0.15 / 0.8 * v0))
            } else {
                (Vec3::new(0.05, r, 0.005 + 0.02), Vec3::new(v0, -0.02, 0.1 * v0))
            };
            let mut b = Ball::new(STEEL, pos);
            b.velocity = vel;
            let h = s.add_ball(b).unwrap();
            let mut ev = BallEvents::new();
            let ticks = (0.3 / v0 / DT) as usize;
            let mut nimp = 0;
            for _ in 0..ticks {
                ev.clear();
                s.step(&geo, DT, &mut ev);
                nimp += impacts(&ev);
            }
            let bb = s.ball(h).unwrap();
            let travelled = bb.position.distance(pos);
            println!(
                "{label} v0={v0}: expected travel ~{:.3} m, got {travelled:.4} m, |v| {:.3} -> {:.3}, impacts={nimp}",
                vel.length() * ticks as f32 * DT,
                vel.length(),
                bb.velocity.length()
            );
        }
    }
}

#[test]
fn v_curved_guide_control() {
    let w = StaticWorldBuilder::new().build();
    for rise in [0.0f32, 0.001, 0.01, 0.15] {
        let pts: Vec<Vec3> = (0..=40)
            .map(|i| {
                let s = i as f32 / 40.0;
                let a = s * 2.0;
                Vec3::new(0.4 * libm::sinf(a), 0.4 - 0.4 * libm::cosf(a), rise * s)
            })
            .collect();
        let g = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.05).unwrap();
        let guides = [g];
        let geo = Geometry { world: &w, guides: &guides, wireforms: &[] };
        let r = rad(STEEL);
        let mut s = solver(Vec3::ZERO);
        let v0 = 5.0;
        let mut b = Ball::new(STEEL, Vec3::new(0.0, r, 0.02));
        b.velocity = Vec3::new(v0, 0.0, rise / 0.8 * v0);
        let h = s.add_ball(b).unwrap();
        let mut ev = BallEvents::new();
        let mut last = b.position;
        for t in 0..60 {
            ev.clear();
            s.step(&geo, DT, &mut ev);
            let bb = s.ball(h).unwrap();
            if t % 30 == 29 {
                println!("rise={rise} t={t} step={:.5} m pos={:?} |v|={:.3} vz={:.3}", bb.position.distance(last), bb.position, bb.velocity.length(), bb.velocity.z);
            }
            last = bb.position;
        }
    }
}
