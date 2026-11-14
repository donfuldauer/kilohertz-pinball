//! GR-1 temporary investigation (deleted after use).
#[path = "spec_2_7_common.rs"]
mod common;
use common::*;
use glam::Vec3;
use pin_core::{BallProfile, ProfileId};
use pin_physics::ball::{Ball, BallEvents, BallHandle, ExternalForce, ForceContext, Geometry};
use std::sync::{Arc, atomic::{AtomicU32, Ordering}};

struct Count(Arc<AtomicU32>);
impl ExternalForce for Count {
    fn force_in(&mut self, _c: &ForceContext<'_>, _h: BallHandle, _b: &Ball, _p: &BallProfile, _dt: f32) -> Vec3 {
        self.0.fetch_add(1, Ordering::Relaxed);
        Vec3::splat(-0.0)
    }
}

fn run(p: &BallProfile, v: Vec3, w: Vec3, ticks: usize, magnus: bool) -> (f64, u32) {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let profiles = [p.clone()];
    let mut s = solver(&profiles, Vec3::ZERO);
    if magnus { let _ = register(&mut s); }
    let c = Arc::new(AtomicU32::new(0));
    s.add_external_force(Box::new(Count(c.clone())));
    let mut b = Ball::new(ProfileId(0), Vec3::new(0.0, 0.0, 1.0));
    b.velocity = v;
    b.angular_velocity = w;
    let h = s.add_ball(b).unwrap();
    let mut ev = BallEvents::new();
    for _ in 0..ticks { ev.clear(); s.step(&geo, DT, &mut ev); }
    let k0 = v.as_dvec3().length_squared();
    let k1 = s.ball(h).unwrap().velocity.as_dvec3().length_squared();
    ((k1 - k0) / k0, c.load(Ordering::Relaxed))
}

#[test]
#[ignore]
fn gr1_magnus_ke() {
    let mut p = builtin()[0].clone();
    p.magnus_coeff = 2.7939423e-6;
    let v = Vec3::new(-2.2705288, 1.0387479, 2.4917116);
    let w = Vec3::new(-171.76329, 125.194756, 150.64848);
    let eps = f64::from(f32::EPSILON);
    let (r, n) = run(&p, v, w, 200, true);
    let (r0, n0) = run(&p, v, w, 200, false);
    let phi_sub = f64::from(p.magnus_coeff / p.mass_kg * w.length()) * 1e-3 / f64::from(n) * 200.0;
    println!("failing input: mass {} radius {} rel {r:e} substeps {n} ({}/tick) rel/(ticks eps) {} rel/(substeps eps) {}; no magnus: rel {r0:e} substeps {n0}; phi per substep {phi_sub:e}, |dv|/ulp(|v|)~{}",
        p.mass_kg, p.radius_m(), n / 200, r / (200.0 * eps), r / (f64::from(n) * eps), phi_sub * f64::from(v.length()) / (f64::from(v.length()) * eps));
    // random sweep like the spec (gravity off)
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut rnd = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed >> 11) as f64 / (1u64 << 53) as f64 };
    let (mut worst_t, mut worst_s, mut fails) = (0.0f64, 0.0f64, 0);
    let total = 20000;
    for _ in 0..total {
        let which = (rnd() * 7.0) as usize;
        let mut p = builtin()[which].clone();
        p.magnus_coeff = (1.0e-6 + rnd() * 4.9e-5) as f32;
        let v = Vec3::new((rnd()*8.0-4.0) as f32, (rnd()*8.0-4.0) as f32, (rnd()*8.0-4.0) as f32);
        let w = Vec3::new((rnd()*600.0-300.0) as f32, (rnd()*600.0-300.0) as f32, (rnd()*600.0-300.0) as f32);
        if v.length() <= 0.2 { continue; }
        let (r, n) = run(&p, v, w, 200, true);
        let wc = f64::from(p.magnus_coeff) / f64::from(p.mass_kg) * f64::from(w.length()) * 1e-3;
        let quad = 2.0 * 200.0 * wc * wc;
        let spec_bound = quad + 200.0 * eps;
        if r.abs() > spec_bound { fails += 1; }
        worst_t = worst_t.max((r.abs() - quad) / (200.0 * eps));
        worst_s = worst_s.max((r.abs() - quad) / (f64::from(n) * eps));
    }
    println!("sweep {total}: spec-bound failures {fails}; worst (|rel|-quad)/(ticks*eps) {worst_t}; worst (|rel|-quad)/(substeps*eps) {worst_s}");
}

#[test]
#[ignore]
fn gr1_magnus_ideal() {
    use glam::DVec3;
    let mut p = builtin()[0].clone();
    p.magnus_coeff = 2.7939423e-6;
    let v0 = Vec3::new(-2.2705288, 1.0387479, 2.4917116);
    let w = Vec3::new(-171.76329, 125.194756, 150.64848);
    let wd = w.as_dvec3();
    let phi = f64::from(p.magnus_coeff) / f64::from(p.mass_kg) * wd.length() * 5.0e-4;
    let ax = wd.normalize();
    let mut v = v0;
    for _ in 0..400 {
        let vd = v.as_dvec3();
        let par = ax * ax.dot(vd);
        let rot = par + (vd - par) * phi.cos() + ax.cross(vd) * phi.sin();
        let dv: DVec3 = rot - vd;
        v += dv.as_vec3();
    }
    let k0 = v0.as_dvec3().length_squared();
    let rel = (v.as_dvec3().length_squared() - k0) / k0;
    println!("ideal f64 rotation increment, f32 add: rel {rel:e} = {} substep-eps", rel / (400.0 * f64::from(f32::EPSILON)));
}

#[path = "mech_util.rs"]
mod mutil;

#[test]
#[ignore]
fn gr1_plunger_old_setup() {
    use pin_core::schema::table::MechKind;
    use pin_physics::mech::MechSpec;
    let launch = |depth: f32| {
        let spec = MechSpec::new(MechKind::Plunger, mutil::v(0.0, -0.3, 0.0));
        let mut t = mutil::Table::new(&[spec], mutil::world(&[]), 6.5);
        let h = t.ball(0.0, -0.3 + 0.006 + mutil::radius(mutil::STEEL) + 0.0005, Vec3::ZERO);
        for i in 0..500 {
            t.mechs.set_plunger(if i >= 200 { depth * ((i - 200) as f32 / 200.0).min(1.0) } else { 0.0 });
            t.tick();
        }
        t.mechs.set_plunger(0.0);
        let mut peak = 0.0f32;
        for _ in 0..60 { t.tick(); peak = peak.max(t.get(h).velocity.y); }
        peak
    };
    for d in [0.7304841f32, 0.75067747] {
        let p = launch(d);
        println!("old setup depth {d}: launch {p} (bits {:08x})", p.to_bits());
    }
}
