#[path = "ball_util.rs"]
mod util;
use pin_physics::ball::*;
use pin_physics::world::*;
use util::*;

#[test]
fn dbg_guide() {
    let world = floor_world();
    let radius_g = 0.150f32;
    let step = 2.0 * radius_g * libm::sinf(2.5f32.to_radians());
    let mut pts = Vec::new();
    for k in (1..=20).rev() { pts.push(v(radius_g, -step * k as f32, 0.0)); }
    for k in 0..=36 { let a = (5.0 * k as f32).to_radians(); pts.push(v(radius_g * libm::cosf(a), radius_g * libm::sinf(a), 0.0)); }
    for k in 1..=20 { pts.push(v(-radius_g, -step * k as f32, 0.0)); }
    let guides = [GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap()];
    let geo = Geometry { world: &world, guides: &guides, wireforms: &[] };
    for speed in [1.0f32, 4.0, 8.0] {
        let r = radius(STEEL);
        let mut s = solver();
        let mut b = ball_at(STEEL, v(radius_g - r - 1e-4, -0.2, r), v(0.0, speed, 0.0));
        b.angular_velocity = v(-speed / r, 0.0, 0.0);
        let h = s.add_ball(b).unwrap();
        let mut ev = BallEvents::new();
        for t in 0..700 {
            ev.clear();
            s.step(&geo, DT, &mut ev);
            let b = s.ball(h).unwrap();
            let gap = guides[0].closest_point(b.position, None).distance - r;
            if t % 10 == 0 || !ev.as_slice().is_empty() || ev.dropped() > 0 {
                println!("s{speed} t{t} p {:?} v {:?} |v| {} w {:?} gap {:.2e} ev {:?} drop {}", b.position, b.velocity, b.velocity.length(), b.angular_velocity, gap, ev.as_slice(), ev.dropped());
            }
            if b.position.y < -0.25 { break; }
        }
    }
}
