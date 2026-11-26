//! The two read-only accessors the frame snapshot's dynamic mesh slots read
//! (#103, REQ-VIS-002, SPEC Appendix A "Dynamic mesh slot convention"):
//! [`MechSet::drop_target_offset_m`] and [`MechSet::spinner_pivot_axis`].

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::mech::MechSpec;
use util::*;

const RESET: CoilId = CoilId(12);

fn drop_spec(height_mm: Option<f32>) -> MechSpec {
    let mut s = MechSpec::new(MechKind::DropTarget, Vec3::ZERO)
        .with_coil(RESET)
        .with_switch(SwitchId(60));
    s.params.angle_deg = Some(0.0);
    s.params.height = height_mm;
    s
}

/// REQ-VIS-002: `None` for every other kind and for a bad index.
#[test]
fn accessors_are_none_for_other_kinds_and_bad_indices() {
    let kinds = [
        MechKind::Bumper,
        MechKind::Sling,
        MechKind::Vuk,
        MechKind::Plunger,
        MechKind::Kickback,
        MechKind::Diverter,
        MechKind::NewtonBall,
        MechKind::Gate,
    ];
    let mut specs: Vec<MechSpec> = kinds
        .iter()
        .enumerate()
        .map(|(i, &k)| MechSpec::new(k, v(0.05 * i as f32 - 0.2, 0.2, 0.0)))
        .collect();
    specs.push(drop_spec(None));
    specs.push(MechSpec::new(MechKind::Spinner, v(0.2, -0.2, 0.0)));
    let t = Table::new(&specs, world(&[]), 0.0);
    for (i, k) in kinds.iter().enumerate() {
        assert_eq!(t.mechs.drop_target_offset_m(i), None, "{k:?}");
        assert_eq!(t.mechs.spinner_pivot_axis(i), None, "{k:?}");
    }
    let (d, s) = (kinds.len(), kinds.len() + 1);
    assert!(t.mechs.drop_target_offset_m(d).is_some());
    assert_eq!(t.mechs.spinner_pivot_axis(d), None);
    assert!(t.mechs.spinner_pivot_axis(s).is_some());
    assert_eq!(t.mechs.drop_target_offset_m(s), None);
    for bad in [s + 1, 1000, usize::MAX] {
        assert_eq!(t.mechs.drop_target_offset_m(bad), None);
        assert_eq!(t.mechs.spinner_pivot_axis(bad), None);
    }
}

/// REQ-VIS-002 (drop target slot): 0 when up, −height when down (not the
/// hidden depth), monotone non-decreasing while the reset raises it, back
/// to 0 when up; for the default height and an override.
#[test]
fn drop_offset_is_zero_up_minus_height_down_and_monotone_rising() {
    for (h_mm, height) in [(None, 0.032f32), (Some(30.0), 0.030)] {
        let mut t = Table::new(&[drop_spec(h_mm)], world(&[]), 0.0);
        assert_eq!(t.mechs.drop_target_offset_m(0), Some(0.0));
        t.ball(0.08, 0.0, v(-1.2, 0.0, 0.0));
        let mut dropped_at = None;
        for k in 0..400 {
            t.tick();
            let off = t.mechs.drop_target_offset_m(0).unwrap();
            if t.mechs.drop_target_down(0).unwrap() {
                dropped_at.get_or_insert(k);
                assert!((off + height).abs() <= 1e-6, "down: {off} vs -{height}");
            } else {
                assert_eq!(off, 0.0);
            }
        }
        assert!(dropped_at.is_some(), "the hit dropped the target");
        let balls: Vec<_> = t.solver.balls().map(|(h, _)| h).collect();
        for h in balls {
            t.solver.remove_ball(h);
        }
        t.mechs.fire_coil(RESET, 200);
        let mut prev = t.mechs.drop_target_offset_m(0).unwrap();
        let mut rose = false;
        for _ in 0..1000 {
            t.tick();
            let off = t.mechs.drop_target_offset_m(0).unwrap();
            assert!(off >= -height - 1e-7 && off <= 0.0, "{off}");
            assert!(off >= prev, "monotone rise: {prev} -> {off}");
            if off > -height && off < 0.0 {
                rose = true;
            }
            prev = off;
        }
        assert!(rose, "seen strictly between -height and 0");
        assert!(!t.mechs.drop_target_down(0).unwrap());
        assert_eq!(t.mechs.drop_target_offset_m(0), Some(0.0));
    }
}

/// REQ-VIS-002 (spinner slot): pivot = position + z·pivot_height (default
/// 42 mm, or the `height` override), axis unit and equal to −(z × u).
#[test]
fn spinner_pivot_is_lifted_position_and_axis_is_minus_z_cross_u() {
    for (deg, h_mm, pivot_h) in [
        (None, None, 0.042f32),
        (Some(0.0), Some(50.0), 0.050),
        (Some(37.0), None, 0.042),
        (Some(-120.0), Some(30.0), 0.030),
    ] {
        let p = v(0.1, -0.05, 0.002);
        let mut s = MechSpec::new(MechKind::Spinner, p);
        s.params.angle_deg = deg;
        s.params.height = h_mm;
        let t = Table::new(&[s], world(&[]), 0.0);
        let (pivot, axis) = t.mechs.spinner_pivot_axis(0).unwrap();
        assert!(
            (pivot - (p + Vec3::Z * pivot_h)).length() <= 1e-6,
            "{pivot}"
        );
        let a = deg.unwrap_or(90.0).to_radians();
        let u = Vec3::new(libm::cosf(a), libm::sinf(a), 0.0);
        assert!((axis.length() - 1.0).abs() <= 1e-6);
        assert!((axis - (-Vec3::Z.cross(u))).length() <= 1e-6, "{axis}");
    }
}
