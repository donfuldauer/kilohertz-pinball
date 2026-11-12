#[path = "ball_util.rs"]
mod util;
use glam::Vec3;
use pin_physics::ball::*;
use util::*;

#[test]
fn dbg_stack() {
    let r = radius(STEEL);
    let world = box_world(v(3.0 * r + 0.0005, r + 0.0005, 0.2));
    let geo = Geometry::new(&world);
    let mut s = solver();
    for i in 0..3 {
        s.add_ball(Ball::new(STEEL, v((i as f32 - 1.0) * 2.0 * r, 0.0, -0.2 + r))).unwrap();
    }
    s.add_ball(Ball::new(STEEL, v(r, 0.0, -0.2 + 3.0 * r + 0.01))).unwrap();
    let mut ev = BallEvents::new();
    for t in 0..3000 {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        if t % 100 == 0 || (t > 40 && t < 70) {
            println!("t{t} ke {:.3e} drop {}", kinetic_energy(&s), ev.dropped());
            for (h, b) in s.balls() { println!("   {} p {:?} v {:?} w {:?}", h.0, b.position - v(0.0,0.0,-0.2), b.velocity, b.angular_velocity); }
        }
    }
}
