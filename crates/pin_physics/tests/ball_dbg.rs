#[path = "ball_util.rs"]
mod util;
use glam::Vec3;
use pin_physics::world::*;
use util::*;
#[test]
fn dbg_tri() {
    let rot = glam::Quat::from_rotation_x(6.5f32.to_radians());
    let n = rot * Vec3::Z;
    let (u, w) = (rot * Vec3::X * 60.0, rot * Vec3::Y * 60.0);
    let mut b = StaticWorldBuilder::new();
    b.add_collider(StaticShape::TriMesh { vertices: vec![-u - w, u - w, u + w, w - u], indices: vec![[0, 1, 2], [0, 2, 3]] }, layers::ALL, true);
    let world = b.build();
    let mut out = arrayvec::ArrayVec::new();
    for k in (0..2000).step_by(97) {
        for dk in 0..2 {
            let k = k + dk;
            let on_plane = rot * v(-1.0 + 0.001 * k as f32, 0.7 - 0.0007 * k as f32, 0.0);
            let depth = 3.0e-7 + 1.0e-9 * k as f32;
            let c = on_plane + n * (0.01 - depth);
            world.sphere_contacts(c, 0.01, layers::ALL, &mut out);
            let exact = (c.as_dvec3()).dot(n.as_dvec3());
            println!("k{k} n {} depth {:.4e} want {:.4e} err {:.3e} | centre height f64 {:.9} normal {:?}", out.len(), out[0].depth, depth, out[0].depth - depth, exact, out[0].normal - n);
        }
    }
}
