#[path = "ball_util.rs"]
mod util;
use glam::{Quat, Vec3};
use pin_core::ids::ProfileId;
use pin_physics::ball::*;
use pin_physics::world::*;
use util::*;

fn slope_world(deg: f32, mesh: bool) -> (StaticWorld, Vec3, Vec3) {
    let rot = Quat::from_rotation_x(deg.to_radians());
    let n = rot * Vec3::Z;
    let mut b = StaticWorldBuilder::new();
    if mesh {
        let (u, w, s) = (rot * Vec3::X, rot * Vec3::Y, 2.0);
        b.add_collider(StaticShape::TriMesh { vertices: vec![-u * s - w * s, u * s - w * s, u * s + w * s, -u * s + w * s], indices: vec![[0, 1, 2], [0, 2, 3]] }, layers::ALL, true);
    } else {
        b.add_collider(StaticShape::Cuboid { center: -n * 0.05, half_extents: v(2.0, 2.0, 0.05), rotation: rot }, layers::ALL, true);
    }
    b.add_collider(StaticShape::Cylinder { base: -n * 0.005, axis: n * 0.04, radius: 0.005 }, layers::ALL, true);
    let g = v(0.0, 0.0, -9.81);
    let gt = g - n * g.dot(n);
    (b.build(), n, gt.normalize())
}

#[test]
fn dbg_rest() {
    for (p, deg, mesh) in [(POWERBALL, 6.5f32, false), (STEEL, 3.0, false), (MINI, 6.5, false), (STEEL, 6.5, true)] {
        let (w, n, d) = slope_world(deg, mesh);
        let geo = Geometry::new(&w);
        let r = radius(p);
        let mut s = solver();
        let h = s.add_ball(Ball::new(p, n * r - d * (0.005 + r) + n * 1e-6 - d * 1e-6)).unwrap();
        let mut ev = BallEvents::new();
        for _ in 0..1000 { s.step(&geo, DT, &mut ev); }
        let p0 = s.ball(h).unwrap().position;
        let (mut maxd, mut maxv, mut jumps) = (0.0f32, 0.0f32, 0);
        let mut last = p0;
        for t in 0..10000 { s.step(&geo, DT, &mut ev); let b = s.ball(h).unwrap(); if b.position != last && jumps < 8 { let q=b.position; println!("   t{t} move {:?} floor gap {:.3e} post gap {:.3e} v {:?}", q-last, q.dot(n)-r, (q - n*q.dot(n)).length()-0.005-r, b.velocity); } maxd = maxd.max(b.position.distance(p0)); maxv = maxv.max(b.velocity.length()); if b.position != last { jumps += 1; } last = b.position; }
        println!("post {p:?} deg {deg} mesh {mesh}: drift {maxd:.3e} maxv {maxv:.3e} moved ticks {jumps}");
    }
    // row
    for n_balls in [2usize, 3, 5] {
        let (w, n, d) = slope_world(6.5, false);
        let geo = Geometry::new(&w);
        let r = radius(STEEL);
        let mut s = solver();
        let hs: Vec<_> = (0..n_balls).map(|k| s.add_ball(Ball::new(STEEL, n * (r + 1e-6) - d * (0.005 + r + 2.0 * r * k as f32 + 1e-5 * k as f32))).unwrap()).collect();
        let mut ev = BallEvents::new();
        for _ in 0..2000 { s.step(&geo, DT, &mut ev); }
        let p0: Vec<Vec3> = hs.iter().map(|h| s.ball(*h).unwrap().position).collect();
        let (mut maxd, mut maxv, mut jump) = (0.0f32, 0.0f32, 0.0f32);
        let mut last = p0.clone();
        for _ in 0..10000 { s.step(&geo, DT, &mut ev); for (k, h) in hs.iter().enumerate() { let b = s.ball(*h).unwrap(); maxd = maxd.max(b.position.distance(p0[k])); maxv = maxv.max(b.velocity.length()); jump = jump.max(b.position.distance(last[k])); last[k] = b.position; } }
        println!("row {n_balls}: drift {maxd:.3e} maxv {maxv:.3e} max jump/tick {jump:.3e}");
    }
    // pinned
    let mut b = StaticWorldBuilder::new();
    b.add_collider(cuboid(v(0.0, 0.0, -0.05), v(2.0, 2.0, 0.05)), layers::ALL, true);
    b.add_collider(cuboid(v(-0.05, 0.0, 0.05), v(0.05, 2.0, 0.05)), layers::ALL, true);
    let w = b.build();
    let geo = Geometry::new(&w);
    let pitch = 6.5f32.to_radians();
    let mut cfg = config();
    cfg.gravity = v(-9.81 * pitch.sin(), 0.0, -9.81 * pitch.cos());
    let mut s = BallSolver::new(cfg, &profiles());
    let (rj, rm) = (radius(JUMBO), radius(MINI));
    let hs = [
        s.add_ball(Ball::new(JUMBO, v(rj, 0.0, rj))).unwrap(),
        s.add_ball(Ball::new(MINI, v(rj + rm + 0.0005, 0.0, rm))).unwrap(),
        s.add_ball(Ball::new(JUMBO, v(2.0 * rj + 2.0 * rm + 0.002, 0.0, rj))).unwrap(),
    ];
    let mut ev = BallEvents::new();
    for _ in 0..3000 { s.step(&geo, DT, &mut ev); }
    let p0: Vec<Vec3> = hs.iter().map(|h| s.ball(*h).unwrap().position).collect();
    let (mut maxd, mut maxv) = ([0.0f32; 3], [0.0f32; 3]);
    for _ in 0..10000 { s.step(&geo, DT, &mut ev); for k in 0..3 { let b = s.ball(hs[k]).unwrap(); maxd[k] = maxd[k].max(b.position.distance(p0[k])); maxv[k] = maxv[k].max(b.velocity.length()); } }
    println!("pinned: drift {maxd:?} maxv {maxv:?}");
    for k in 0..3 { let b = s.ball(hs[k]).unwrap(); println!("  {k} p {:?} v {:?} w {:?}", b.position, b.velocity, b.angular_velocity); }
    let _ = ProfileId(0);
}
