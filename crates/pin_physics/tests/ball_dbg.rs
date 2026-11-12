#[path = "ball_util.rs"]
mod util;
use pin_physics::ball::*;
use pin_physics::world::*;
use util::*;

#[test]
fn dbg_groove() {
    let rot = |a: f32| glam::Quat::from_rotation_y(a);
    let mut b = StaticWorldBuilder::new();
    for a in [0.6f32, -0.6] {
        b.add_collider(StaticShape::Cuboid { center: rot(a) * v(0.0, 0.0, -0.05), half_extents: v(0.5, 0.5, 0.05), rotation: rot(a) }, layers::ALL, true);
    }
    let world = b.build();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let h = s.add_ball(Ball::new(STEEL, v(0.004, 0.0, 0.05))).unwrap();
    let mut ev = BallEvents::new();
    for t in 0..4000 {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        let b = s.ball(h).unwrap();
        if t % 200 == 0 || (t >= 3000 && t < 3012) {
            println!("t{t} p {:?} v {:?} w {:?} ev {:?}", b.position, b.velocity, b.angular_velocity, ev.as_slice());
        }
    }
}
