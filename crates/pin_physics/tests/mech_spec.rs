//! `MechSpec`/`MechSet::new` input handling (CR-004 parameters, units,
//! errors) and the coil/accessor API of Subphase 2.4.

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_core::{PhysicsDefaults, TableManifest};
use pin_physics::ball::MovingBodies;
use pin_physics::mech::{
    DEFAULT_BODY_SET, MAX_MECHS, MechError, MechSet, MechSpec, TiltBob, TiltBobParams,
};
use util::*;

fn fixture() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../pin_core/fixtures/table.toml"
    ))
    .unwrap()
}

#[test]
fn specs_from_the_appendix_a_fixture() {
    let t = TableManifest::from_toml_str(&fixture()).unwrap();
    let specs: Vec<MechSpec> = t
        .mechs
        .iter()
        .enumerate()
        .map(|(i, m)| MechSpec::from_def(m, Some(CoilId(i as u16)), Some(SwitchId(i as u16))))
        .collect();
    assert!(!specs.is_empty());
    assert_eq!(specs[0].kind, MechKind::Bumper);
    assert!((specs[0].position - v(0.26, 0.82, 0.0)).length() < 1e-6);
    let set = MechSet::new(&specs, &PhysicsDefaults::builtin()).unwrap();
    assert_eq!(set.len(), specs.len());
    assert_eq!(set.kind(0), Some(MechKind::Bumper));
    assert_eq!(set.kind(99), None);
}

#[test]
fn table_overrides_use_table_units() {
    let text = fixture().replacen(
        "kind = \"bumper\"",
        "radius = 40.0\nkick_speed_m_s = 2.0\nkind = \"bumper\"",
        1,
    );
    let t = TableManifest::from_toml_str(&text).unwrap();
    let def = &t.mechs[0];
    assert_eq!(def.params().radius, Some(40.0));
    let mut spec = MechSpec::from_def(def, Some(CoilId(1)), None);
    spec.position = Vec3::ZERO;
    let mut set = MechSet::new(&[spec], &PhysicsDefaults::builtin()).unwrap();
    set.begin_tick(DT);
    // 40 mm ring radius.
    let c = set.closest_point(0, 0.0, v(0.1, 0.0, 0.01)).unwrap();
    assert!((c.distance - 0.06).abs() < 1e-6);
    set.end_tick(DT);
    // No position: the origin.
    let mut bare = def.clone();
    bare.position = None;
    assert_eq!(MechSpec::from_def(&bare, None, None).position, Vec3::ZERO);
}

#[test]
fn bad_parameters_are_named() {
    let d = PhysicsDefaults::builtin();
    type Case = (MechKind, fn(&mut MechSpec), &'static str);
    let cases: [Case; 9] = [
        (MechKind::Bumper, |s| s.params.radius = Some(-1.0), "radius"),
        (MechKind::Sling, |s| s.params.mass_g = Some(0.0), "mass_g"),
        (
            MechKind::Kickback,
            |s| s.params.force_n = Some(f32::NAN),
            "force_n",
        ),
        (
            MechKind::Plunger,
            |s| s.params.auto_pull = Some(1.5),
            "auto_pull",
        ),
        (
            MechKind::DropTarget,
            |s| s.params.threshold_speed_m_s = Some(-0.1),
            "threshold_speed_m_s",
        ),
        (
            MechKind::Spinner,
            |s| s.params.inertia_kg_m2 = Some(0.0),
            "inertia_kg_m2",
        ),
        (
            MechKind::Vuk,
            |s| s.params.exit_layer = Some(8),
            "exit_layer",
        ),
        (
            MechKind::Vuk,
            |s| s.params.exit_offset = Some([f32::NAN, 0.0, 0.0]),
            "exit_offset",
        ),
        (
            MechKind::Gate,
            |s| s.params.angle_deg = Some(f32::INFINITY),
            "angle_deg",
        ),
    ];
    for (kind, f, name) in cases {
        let mut s = MechSpec::new(kind, Vec3::ZERO);
        f(&mut s);
        let e = MechSet::new(&[MechSpec::new(MechKind::Gate, Vec3::ZERO), s], &d)
            .err()
            .unwrap();
        assert_eq!(e, MechError::BadParameter { mech: 1, name });
        assert!(e.to_string().contains(name));
    }
    let mut s = MechSpec::new(MechKind::Bumper, Vec3::ZERO);
    s.params.layer = Some(8);
    assert!(MechSet::new(&[s], &d).is_err());
    let e = MechSet::new(&[MechSpec::new(MechKind::Bumper, Vec3::NAN)], &d)
        .err()
        .unwrap();
    assert_eq!(e, MechError::BadPosition { mech: 0 });
    assert!(e.to_string().contains("position"));
    let many = vec![MechSpec::new(MechKind::Gate, Vec3::ZERO); MAX_MECHS + 1];
    let e = MechSet::new(&many, &d).err().unwrap();
    assert_eq!(e, MechError::TooMany);
    assert!(!e.to_string().is_empty());
}

#[test]
fn coils_and_accessors() {
    let specs = [
        MechSpec::new(MechKind::Sling, Vec3::ZERO).with_coil(CoilId(1)),
        MechSpec::new(MechKind::Vuk, v(0.2, 0.0, 0.0)).with_coil(CoilId(2)),
        MechSpec::new(MechKind::Gate, v(0.3, 0.0, 0.0)),
    ];
    let mut set = MechSet::new(&specs, &PhysicsDefaults::builtin()).unwrap();
    assert!(!set.is_empty());
    assert_eq!(set.body_count(), 3);
    assert_eq!(set.body_set(), DEFAULT_BODY_SET);
    set.set_body_set(4);
    assert_eq!(set.body_set(), 4);
    assert!(!set.coil_on(CoilId(1)));
    set.fire_coil(CoilId(1), 0);
    assert!(!set.coil_on(CoilId(1)));
    set.fire_coil(CoilId(1), 5);
    assert!(set.coil_on(CoilId(1)));
    set.hold_coil(CoilId(1), false);
    assert!(!set.coil_on(CoilId(1)));
    set.hold_coil(CoilId(1), true);
    assert!(set.coil_on(CoilId(1)));
    set.fire_coil(CoilId(77), 10);
    assert!(!set.coil_on(CoilId(77)));
    // Kinds without the asked state.
    assert_eq!(set.spinner_state(0), None);
    assert_eq!(set.spinner_pulses(0), 0);
    assert_eq!(set.slider_state(1), None);
    assert_eq!(set.drop_target_down(0), None);
    assert_eq!(set.vuk_ball(0), None);
    assert_eq!(set.vuk_ball(1), None);
    assert!(!set.mech_switch_closed(9));
    set.set_spinner_speed(0, 3.0);
    // Inert and colliderless kinds have no surface.
    set.begin_tick(DT);
    assert!(set.closest_point(1, 0.0, Vec3::ZERO).is_none());
    assert!(set.closest_point(2, 0.0, Vec3::ZERO).is_none());
    assert_eq!(set.max_surface_speed(2), 0.0);
    assert_eq!(set.velocity_at(2, 0.0, Vec3::ZERO), Vec3::ZERO);
    assert_eq!(set.inverse_mass_at(2, 0.0, Vec3::ZERO, Vec3::X), 0.0);
    let _ = set.swept_bounds(1);
    set.apply_impulse(2, 0.0, Vec3::ZERO, Vec3::X);
    set.end_tick(DT);
    // Plunger input is clamped.
    set.set_plunger(f32::NAN);
    set.set_plunger(5.0);
    set.nudge(Vec3::NAN);
    // A replaced tilt bob.
    set.set_tilt_bob(TiltBob::new(TiltBobParams {
        gap: 0.03,
        ..TiltBobParams::default()
    }));
    assert_eq!(set.tilt_bob().params().gap, 0.03);
    // Switch states leave foreign bits alone.
    let mut s = [u64::MAX; 4];
    set.switch_states(&mut s);
    assert_eq!(s, [u64::MAX; 4]);
    set.set_tilt_switch(Some(SwitchId(200)));
    set.set_tilt_switch(Some(SwitchId(201)));
    let mut s = [u64::MAX; 4];
    set.switch_states(&mut s);
    assert_eq!(s[3] & (1 << (201 - 192)), 0, "tilt open");
    assert_ne!(s[3] & (1 << (200 - 192)), 0, "old tilt bit no longer owned");
    // A non-positive dt does nothing.
    let mut sol = solver(0.0);
    set.pre_step(&mut sol, 0.0);
    set.post_step(&mut sol, &[], f32::NAN);
}
