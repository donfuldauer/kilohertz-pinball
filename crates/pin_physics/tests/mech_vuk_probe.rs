//! VUK robustness: capture/eject with radii around the ball radius and any
//! exit parameters never blows a ball up (2.4 fix round 1).

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::mech::MechSpec;
use proptest::prelude::*;
use util::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn vuk_never_blows_a_ball_up(radius_mm in 2.0f32..60.0, eject in 0.0f32..20.0, kick in 0.0f32..20.0,
                                 ox in -400.0f32..400.0, oy in -400.0f32..400.0, oz in -50.0f32..400.0,
                                 exit_layer in 0u8..8, pitch in 0.0f32..7.0,
                                 balls in proptest::collection::vec((-0.06f32..0.06, -0.06f32..0.06, -3.0f32..3.0, -3.0f32..3.0), 1..7),
                                 fire_every in 20u32..300) {
        let mut spec = MechSpec::new(MechKind::Vuk, Vec3::ZERO).with_coil(CoilId(1)).with_switch(SwitchId(1));
        spec.params.radius = Some(radius_mm);
        spec.params.eject_speed_m_s = Some(eject);
        spec.params.kick_speed_m_s = Some(kick);
        spec.params.exit_offset = Some([ox, oy, oz]);
        spec.params.exit_layer = Some(exit_layer);
        let mut t = Table::new(&[spec], world(&[]), pitch);
        let mut hs = Vec::new();
        for (x, y, vx, vy) in balls {
            if let Some(h) = t.solver.add_ball({
                let mut b = pin_physics::ball::Ball::new(STEEL, v(x, y, radius(STEEL)));
                b.velocity = v(vx, vy, 0.0);
                b
            }) { hs.push(h); }
        }
        for i in 0..600u32 {
            if i % fire_every == fire_every - 1 { t.mechs.fire_coil(CoilId(1), 10); }
            t.tick();
            for &h in &hs {
                let b = t.get(h);
                prop_assert!(b.velocity.length() < 40.0, "tick {}: speed {} at {:?}", i, b.velocity.length(), b.position);
            }
        }
    }
}

/// Regression (2.4 fix round 1): a random-parameter run drove a ball to
/// 1e9 m/s through a VUK; absurd speeds and sizes are now rejected at
/// construction, and the accepted range stays bounded (proptest above).
#[test]
fn absurd_parameters_are_rejected() {
    use pin_core::PhysicsDefaults;
    use pin_physics::mech::{MechError, MechSet};
    let d = PhysicsDefaults::builtin();
    let cases: [(MechKind, fn(&mut MechSpec), &str); 8] = [
        (
            MechKind::Vuk,
            |s| s.params.eject_speed_m_s = Some(1.0e9),
            "eject_speed_m_s",
        ),
        (
            MechKind::Vuk,
            |s| s.params.kick_speed_m_s = Some(50.0),
            "kick_speed_m_s",
        ),
        (
            MechKind::Vuk,
            |s| s.params.exit_offset = Some([0.0, 0.0, 5.0e3]),
            "exit_offset",
        ),
        (
            MechKind::Bumper,
            |s| s.params.kick_speed_m_s = Some(1.0e6),
            "kick_speed_m_s",
        ),
        (
            MechKind::Sling,
            |s| s.params.force_n = Some(1.0e6),
            "force_n",
        ),
        (
            MechKind::Plunger,
            |s| s.params.spring_k_n_m = Some(1.0e9),
            "spring_k_n_m",
        ),
        (
            MechKind::DropTarget,
            |s| s.params.length = Some(2000.0),
            "length",
        ),
        (
            MechKind::Spinner,
            |s| s.params.radius = Some(900.0),
            "radius",
        ),
    ];
    for (kind, f, name) in cases {
        let mut s = MechSpec::new(kind, Vec3::ZERO);
        f(&mut s);
        assert_eq!(
            MechSet::new(&[s], &d).err(),
            Some(MechError::BadParameter { mech: 0, name })
        );
    }
}
