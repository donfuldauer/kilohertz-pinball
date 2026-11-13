//! Scratch exploration (deleted before hand-off).
#[path = "mech_util.rs"]
mod util;
use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::mech::MechSpec;
use util::*;

fn sling(force: f32, stroke: f32, mass: f32, speed: f32, gap: f32, delay: u32) -> f32 {
    let mut spec = MechSpec::new(MechKind::Sling, Vec3::ZERO).with_coil(CoilId(1)).with_switch(SwitchId(1));
    spec.params.angle_deg = Some(0.0);
    spec.params.force_n = Some(force);
    spec.params.stroke = Some(stroke);
    spec.params.mass_g = Some(mass);
    let mut t = Table::new(&[spec], world(&[]), 0.0);
    let h = t.ball(0.005 + radius(STEEL) + gap, 0.0, v(-speed, 0.0, 0.0));
    let mut fire_at = None;
    for i in 0..400u32 {
        t.tick();
        if fire_at.is_none() && t.closed(1) { fire_at = Some(i + delay); }
        if fire_at == Some(i) { t.mechs.fire_coil(CoilId(1), 30); }
        if fire_at.is_some_and(|f| i == f + 40) { break; }
    }
    t.get(h).velocity.truncate().length()
}

#[test]
#[ignore]
fn explore_sling() {
    for (f, s, m) in [(60.0, 12.0, 60.0), (60.0, 14.0, 60.0), (70.0, 12.0, 60.0), (50.0, 14.0, 50.0), (60.0, 15.0, 50.0)] {
        let (mut lo, mut hi) = (f32::INFINITY, 0.0f32);
        for k in 1..=35 {
            let sp = k as f32 * 0.1;
            for g in 0..8 {
                for d in 0..5 {
                    let e = sling(f, s, m, sp, 0.002 + g as f32 * 0.00037, d);
                    if e < lo { println!("  low {e:.3} at v {sp:.1} g {g} d {d}"); }
                    lo = lo.min(e); hi = hi.max(e);
                }
            }
        }
        println!("F {f} stroke {s} mass {m}: exit {lo:.3}..{hi:.3}");
    }
}
