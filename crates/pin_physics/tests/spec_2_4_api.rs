//! Subphase 2.4 spec tests: the fixed public API (`MechSet::new`, coils,
//! switches and edges, per-tick hooks), plus robustness properties.

#[path = "spec_2_4_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::{MechDef, MechKind, MechParams};
use pin_core::script::SwitchEdge;
use pin_physics::ball::{BallEvents, Geometry, MovingBodies};
use pin_physics::mech::{MAX_MECHS, MechError, MechSet, MechSpec, TiltBob, TiltBobParams};
use proptest::prelude::*;

#[test]
fn fixed_api_signatures() {
    // The names and shapes the brief fixes.
    let _: fn(&mut MechSet, CoilId, u16) = MechSet::fire_coil;
    let _: fn(&mut MechSet, CoilId, bool) = MechSet::hold_coil;
    let _: fn(&mut MechSet, f32) = MechSet::set_plunger;
    let _: fn(&mut MechSet, Vec3) = MechSet::nudge;
    let _: fn(&MechSet, &mut [u64; 4]) = MechSet::switch_states;
    let _: fn(&MechSet, usize) -> u64 = MechSet::spinner_pulses;
    let _: fn(&MechSet) -> Vec3 = MechSet::tilt_bob_offset;
    let _: fn(&MechSet) -> bool = MechSet::tilt_closed;
    let _: fn(&MechSet) -> &[(SwitchId, SwitchEdge)] = MechSet::edges;
    let _: fn(TiltBobParams) -> TiltBob = TiltBob::new;
    let r: Result<MechSet, MechError> = MechSet::new(&[], &defaults());
    let set = r.unwrap();
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);
    fn is_bodies<T: MovingBodies + Send>(_: &T) {}
    is_bodies(&set);
    fn is_error<E: std::error::Error>() {}
    is_error::<MechError>();
}

#[test]
fn every_kind_is_accepted_and_indexed_in_input_order() {
    let kinds = [
        MechKind::Bumper,
        MechKind::Sling,
        MechKind::DropTarget,
        MechKind::Spinner,
        MechKind::Vuk,
        MechKind::Plunger,
        MechKind::Kickback,
        MechKind::Diverter,
        MechKind::NewtonBall,
        MechKind::Gate,
    ];
    let specs: Vec<MechSpec> = kinds
        .iter()
        .enumerate()
        .map(|(i, k)| spec(*k, Vec3::new(0.1 * i as f32, 0.0, 0.0)))
        .collect();
    let set = MechSet::new(&specs, &defaults()).unwrap();
    assert_eq!(set.len(), kinds.len());
    assert_eq!(set.body_count(), kinds.len());
    for (i, k) in kinds.iter().enumerate() {
        assert_eq!(set.kind(i), Some(*k));
        assert_eq!(set.spinner_state(i).is_some(), *k == MechKind::Spinner);
        assert_eq!(
            set.drop_target_down(i).is_some(),
            *k == MechKind::DropTarget
        );
        assert_eq!(
            set.slider_state(i).is_some(),
            matches!(k, MechKind::Sling | MechKind::Kickback | MechKind::Plunger)
        );
        assert_eq!(set.spinner_pulses(i), 0);
        assert!(!set.mech_switch_closed(i));
    }
    assert_eq!(set.kind(kinds.len()), None);
    assert_eq!(set.spinner_pulses(999), 0);
    assert_eq!(set.vuk_ball(999), None);
    assert!(!set.mech_switch_closed(999));
}

#[test]
fn new_rejects_bad_input() {
    let d = defaults();
    let bad_pos = spec(MechKind::Bumper, Vec3::new(f32::NAN, 0.0, 0.0));
    assert!(MechSet::new(&[spec(MechKind::Gate, Vec3::ZERO), bad_pos], &d).is_err());
    let inf_pos = spec(MechKind::Spinner, Vec3::new(0.0, f32::INFINITY, 0.0));
    assert!(MechSet::new(&[inf_pos], &d).is_err());

    let with = |kind: MechKind, f: &dyn Fn(&mut MechParams)| {
        let mut p = params();
        f(&mut p);
        MechSet::new(&[spec(kind, Vec3::ZERO).with_params(p)], &d)
    };
    assert!(with(MechKind::Bumper, &|p| p.radius = Some(-5.0)).is_err());
    assert!(with(MechKind::Bumper, &|p| p.radius = Some(0.0)).is_err());
    assert!(with(MechKind::Bumper, &|p| p.kick_speed_m_s = Some(f32::NAN)).is_err());
    assert!(with(MechKind::Bumper, &|p| p.kick_speed_m_s = Some(-1.0)).is_err());
    assert!(with(MechKind::Bumper, &|p| p.layer = Some(8)).is_err());
    assert!(with(MechKind::Sling, &|p| p.mass_g = Some(0.0)).is_err());
    assert!(with(MechKind::Sling, &|p| p.force_n = Some(f32::INFINITY)).is_err());
    assert!(with(MechKind::Kickback, &|p| p.stroke = Some(-1.0)).is_err());
    assert!(
        with(MechKind::DropTarget, &|p| p.threshold_speed_m_s =
            Some(-0.1))
        .is_err()
    );
    assert!(with(MechKind::Spinner, &|p| p.inertia_kg_m2 = Some(0.0)).is_err());
    assert!(with(MechKind::Spinner, &|p| p.damping_per_s = Some(-1.0)).is_err());
    assert!(with(MechKind::Vuk, &|p| p.exit_layer = Some(9)).is_err());
    assert!(with(MechKind::Plunger, &|p| p.auto_pull = Some(1.5)).is_err());
    assert!(with(MechKind::Plunger, &|p| p.spring_k_n_m = Some(-10.0)).is_err());
    // Valid overrides are fine, and the error is printable.
    assert!(with(MechKind::Bumper, &|p| p.radius = Some(28.0)).is_ok());
    let e = with(MechKind::Bumper, &|p| p.radius = Some(-5.0))
        .err()
        .unwrap();
    assert!(!e.to_string().is_empty());
    assert!(format!("{e:?}").contains("radius"), "{e:?}");

    let many = vec![spec(MechKind::Gate, Vec3::ZERO); MAX_MECHS + 1];
    assert!(MechSet::new(&many, &d).is_err());
    let max = vec![spec(MechKind::Gate, Vec3::ZERO); MAX_MECHS];
    assert_eq!(MechSet::new(&max, &d).unwrap().len(), MAX_MECHS);
}

#[test]
fn spec_from_manifest_entry_converts_units() {
    let def: MechDef = toml_def();
    let s = MechSpec::from_def(&def, Some(CoilId(4)), Some(SwitchId(77)));
    assert_eq!(s.kind, MechKind::Bumper);
    assert_eq!(s.coil, Some(CoilId(4)));
    assert_eq!(s.switch, Some(SwitchId(77)));
    assert!((s.position - Vec3::new(0.260, 0.820, 0.0)).length() < 1.0e-6);
    assert_eq!(s.params, def.params());
    assert_eq!(s.params.radius, Some(28.0));
}

fn toml_def() -> MechDef {
    MechDef {
        id: "bumper_1".into(),
        kind: MechKind::Bumper,
        coil: Some("c".into()),
        switch: Some("s".into()),
        position: Some([260.0, 820.0, 0.0]),
        angle_deg: None,
        layer: None,
        radius: Some(28.0),
        length: None,
        height: None,
        thickness: None,
        kick_speed_m_s: Some(2.0),
        kick_reach: None,
        mass_g: None,
        force_n: None,
        stroke: None,
        spring_k_n_m: None,
        threshold_speed_m_s: None,
        inertia_kg_m2: None,
        damping_per_s: None,
        eject_speed_m_s: None,
        exit_layer: None,
        exit_offset: None,
        auto_pull: None,
    }
}

#[test]
fn radius_override_is_in_millimetres() {
    // A 20 mm ring: a ball 25 mm + r from the axis does not touch; one at
    // 20 mm + r does.
    let mut p = params();
    p.radius = Some(20.0);
    let s = spec(MechKind::Bumper, Vec3::ZERO)
        .with_switch(SwitchId(1))
        .with_params(p);
    let mut rig = Rig::on_floor(&[s]);
    let far = rig.add(steel_ball(Vec3::new(0.025 + R, 0.0, R), Vec3::ZERO));
    rig.run(5);
    assert!(!rig.mechs.mech_switch_closed(0));
    rig.solver.remove_ball(far);
    rig.add(steel_ball(
        Vec3::new(0.020 + R + 2.0e-5, 0.0, R),
        Vec3::ZERO,
    ));
    rig.run(5);
    assert!(rig.mechs.mech_switch_closed(0));
}

#[test]
fn coil_pulses_and_holds() {
    let c = CoilId(3);
    let other = CoilId(9);
    let mut rig = Rig::in_space(&[spec(MechKind::Bumper, Vec3::ZERO).with_coil(c)]);
    assert!(!rig.mechs.coil_on(c));
    // Unknown coils are ignored.
    rig.mechs.fire_coil(other, 50);
    rig.mechs.hold_coil(other, true);
    assert!(!rig.mechs.coil_on(other));
    // 0 ms does nothing.
    rig.mechs.fire_coil(c, 0);
    assert!(!rig.mechs.coil_on(c));
    // A 12 ms pulse is on for 12 ticks.
    rig.mechs.fire_coil(c, 12);
    let mut on_ticks = 0;
    for _ in 0..40 {
        if rig.mechs.coil_on(c) {
            on_ticks += 1;
        }
        rig.step();
    }
    assert_eq!(on_ticks, 12);
    // A longer running pulse is kept when a shorter one is fired.
    rig.mechs.fire_coil(c, 20);
    rig.run(5);
    rig.mechs.fire_coil(c, 3);
    let mut on_ticks = 5;
    for _ in 0..40 {
        if rig.mechs.coil_on(c) {
            on_ticks += 1;
        }
        rig.step();
    }
    assert_eq!(on_ticks, 20);
    // Hold: on until released; releasing ends a running pulse too.
    rig.mechs.hold_coil(c, true);
    rig.run(500);
    assert!(rig.mechs.coil_on(c));
    rig.mechs.fire_coil(c, 100);
    rig.mechs.hold_coil(c, false);
    assert!(!rig.mechs.coil_on(c));
    rig.run(2);
    assert!(!rig.mechs.coil_on(c));
}

#[test]
fn switch_states_only_touch_own_bits_and_edges_are_sorted() {
    // Three bumpers with switches in three different words, given in
    // descending order, each touched by a resting ball.
    let sw = [SwitchId(200), SwitchId(70), SwitchId(3)];
    let specs: Vec<MechSpec> = sw
        .iter()
        .enumerate()
        .map(|(i, s)| spec(MechKind::Bumper, Vec3::new(0.2 * i as f32, 0.0, 0.0)).with_switch(*s))
        .collect();
    let mut rig = Rig::on_floor(&specs);
    let r = defaults().mechs.bumper.radius;
    for i in 0..3 {
        rig.add(steel_ball(
            Vec3::new(0.2 * i as f32 + r + R + 2.0e-5, 0.0, R),
            Vec3::ZERO,
        ));
    }
    rig.step();
    let edges = rig.mechs.edges().to_vec();
    assert_eq!(
        edges,
        vec![
            (SwitchId(3), SwitchEdge::Closed),
            (SwitchId(70), SwitchEdge::Closed),
            (SwitchId(200), SwitchEdge::Closed)
        ],
        "edges must be in ascending switch order"
    );
    rig.step();
    assert!(rig.mechs.edges().is_empty(), "edges are per tick");
    // Foreign bits survive; own bits are set.
    let mut out = [u64::MAX, 0, 1 << 5, 0];
    rig.mechs.switch_states(&mut out);
    assert_eq!(out[0], u64::MAX);
    assert_eq!(out[1], 1 << (70 - 64));
    assert_eq!(out[2], 1 << 5);
    assert_eq!(out[3], 1 << (200 - 192));
    // Own bits are cleared when the switch is open.
    let mut rig = Rig::on_floor(&specs);
    rig.step();
    let mut out = [u64::MAX; 4];
    rig.mechs.switch_states(&mut out);
    for s in sw {
        assert!(!bit(&out, s), "open switch {s:?} left set");
    }
    assert_eq!(out.iter().map(|w| w.count_zeros()).sum::<u32>(), 3);
}

/// `MechSet::step` is exactly pre_step + step_with_bodies + post_step.
#[test]
fn step_equals_the_documented_hook_sequence() {
    let mut a = busy_rig();
    let mut b = busy_rig();
    b.mechs.set_body_set(0);
    assert_eq!(b.mechs.body_set(), 0);
    for t in 0..2500 {
        busy_drive(&mut a, t);
        busy_drive(&mut b, t);
        a.step();
        {
            b.events.clear();
            let geo = Geometry::new(&b.world);
            b.mechs.pre_step(&mut b.solver, DT);
            b.solver
                .step_with_bodies(&geo, &mut [&mut b.mechs], DT, &mut b.events);
            b.mechs.post_step(&mut b.solver, b.events.as_slice(), DT);
            b.tick += 1;
        }
        let mut ha = Hash::new();
        let mut hb = Hash::new();
        busy_hash(&a, &mut ha);
        busy_hash(&b, &mut hb);
        assert_eq!(ha.0, hb.0, "diverged at tick {t}");
    }
}

/// A do-nothing body set standing in for the flippers (set 0).
struct NoBodies;
impl MovingBodies for NoBodies {
    fn body_count(&self) -> usize {
        0
    }
    fn begin_tick(&mut self, _dt: f32) {}
    fn end_tick(&mut self, _dt: f32) {}
    fn layer_mask(&self, _body: usize) -> u8 {
        0
    }
    fn material(&self, _body: usize) -> pin_core::Material {
        pin_core::Material::Rubber
    }
    fn swept_bounds(&self, _body: usize) -> (Vec3, Vec3) {
        (Vec3::ZERO, Vec3::ZERO)
    }
    fn max_surface_speed(&self, _body: usize) -> f32 {
        0.0
    }
    fn closest_point(&self, _b: usize, _t: f32, _p: Vec3) -> Option<pin_physics::ball::BodyPoint> {
        None
    }
    fn velocity_at(&self, _b: usize, _t: f32, _p: Vec3) -> Vec3 {
        Vec3::ZERO
    }
    fn inverse_mass_at(&self, _b: usize, _t: f32, _p: Vec3, _d: Vec3) -> f32 {
        0.0
    }
    fn apply_impulse(&mut self, _b: usize, _t: f32, _p: Vec3, _i: Vec3) {}
}

/// In the engine order (flippers set 0, mechs set 1) the drop targets still
/// see their impacts.
#[test]
fn mechs_work_as_body_set_one() {
    let mut p = params();
    p.angle_deg = Some(270.0);
    let specs = [spec(MechKind::DropTarget, Vec3::new(0.0, 0.1, 0.0))
        .with_switch(SwitchId(8))
        .with_params(p)];
    let world = empty_world();
    let geo = Geometry::new(&world);
    let t = defaults().mechs.drop_target.thickness;
    let start = Vec3::new(0.0, 0.1 - 0.5 * t - R - 0.005, R);
    // Through MechSet::step with a flipper set.
    let mut mechs = MechSet::new(&specs, &defaults()).unwrap();
    let mut solver = solver_with_gravity(Vec3::ZERO);
    solver.add_ball(steel_ball(start, Vec3::new(0.0, 1.5, 0.0)));
    let mut flippers = NoBodies;
    let mut ev = BallEvents::new();
    for _ in 0..30 {
        ev.clear();
        mechs.step(&mut solver, &geo, Some(&mut flippers), DT, &mut ev);
    }
    assert_eq!(mechs.body_set(), 1);
    assert_eq!(mechs.drop_target_down(0), Some(true));
    // Through the hooks with the default body set.
    let mut mechs = MechSet::new(&specs, &defaults()).unwrap();
    assert_eq!(mechs.body_set(), pin_physics::mech::DEFAULT_BODY_SET);
    assert_eq!(pin_physics::mech::DEFAULT_BODY_SET, 1);
    let mut solver = solver_with_gravity(Vec3::ZERO);
    solver.add_ball(steel_ball(start, Vec3::new(0.0, 1.5, 0.0)));
    for _ in 0..30 {
        ev.clear();
        mechs.pre_step(&mut solver, DT);
        solver.step_with_bodies(&geo, &mut [&mut flippers, &mut mechs], DT, &mut ev);
        mechs.post_step(&mut solver, ev.as_slice(), DT);
    }
    assert_eq!(mechs.drop_target_down(0), Some(true));
    assert!(mechs.mech_switch_closed(0));
}

#[test]
fn unsimulated_kinds_have_no_surface() {
    let specs = [
        spec(MechKind::Diverter, Vec3::new(0.0, 0.05, 0.0)),
        spec(MechKind::Gate, Vec3::new(0.0, 0.10, 0.0)),
        spec(MechKind::NewtonBall, Vec3::new(0.0, 0.15, 0.0)),
    ];
    let mut rig = Rig::in_space(&specs);
    let h = rig.add(steel_ball(Vec3::new(0.0, 0.0, R), Vec3::new(0.0, 1.0, 0.0)));
    rig.run(250);
    assert!((rig.ball(h).velocity - Vec3::new(0.0, 1.0, 0.0)).length() < 1.0e-6);
    assert!(rig.mechs.edges().is_empty());
}

#[test]
fn an_idle_set_leaves_the_balls_bit_identical_to_no_mechs() {
    // Mechs far away from the balls change nothing about the ball motion.
    let run = |with_mechs: bool| {
        let far = Vec3::new(1.5, 1.5, 0.0);
        let specs: Vec<MechSpec> = if with_mechs {
            busy_specs()
                .into_iter()
                .map(|mut s| {
                    s.position += far;
                    s
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut rig = Rig::on_floor(&specs);
        let a = rig.add(steel_ball(
            Vec3::new(0.0, 0.0, R + 0.05),
            Vec3::new(0.3, 0.2, 0.0),
        ));
        let b = rig.add(steel_ball(
            Vec3::new(0.2, 0.01, R),
            Vec3::new(-1.0, 0.0, 0.0),
        ));
        rig.run(1500);
        (state_bits(&rig.ball(a)), state_bits(&rig.ball(b)))
    };
    assert_eq!(run(true), run(false));
}

fn arb_opt(lo: f32, hi: f32) -> impl Strategy<Value = Option<f32>> {
    prop_oneof![
        3 => Just(None),
        6 => (lo..hi).prop_map(Some),
        1 => prop_oneof![Just(f32::NAN), Just(f32::INFINITY), Just(-1.0f32), Just(0.0f32), Just(1.0e9f32)].prop_map(Some),
    ]
}

fn arb_kind() -> impl Strategy<Value = MechKind> {
    prop_oneof![
        Just(MechKind::Bumper),
        Just(MechKind::Sling),
        Just(MechKind::DropTarget),
        Just(MechKind::Spinner),
        Just(MechKind::Vuk),
        Just(MechKind::Plunger),
        Just(MechKind::Kickback),
        Just(MechKind::Gate),
    ]
}

prop_compose! {
    fn arb_params()(
        angle_deg in arb_opt(-720.0, 720.0),
        radius in arb_opt(5.0, 60.0),
        length in arb_opt(5.0, 150.0),
        height in arb_opt(5.0, 60.0),
        thickness in arb_opt(1.0, 20.0),
        kick_speed_m_s in arb_opt(0.0, 6.0),
        kick_reach in arb_opt(0.0, 20.0),
        mass_g in arb_opt(5.0, 300.0),
        force_n in arb_opt(0.0, 200.0),
        stroke in arb_opt(1.0, 80.0),
        spring_k_n_m in arb_opt(0.0, 4000.0),
        threshold_speed_m_s in arb_opt(0.0, 3.0),
        inertia_kg_m2 in arb_opt(1.0e-7, 1.0e-5),
        damping_per_s in arb_opt(0.0, 10.0),
        eject_speed_m_s in arb_opt(0.0, 4.0),
        auto_pull in arb_opt(0.0, 1.0),
        layer in prop::option::of(0u8..10),
        exit_layer in prop::option::of(0u8..10),
    ) -> MechParams {
        MechParams {
            angle_deg, layer, radius, length, height, thickness, kick_speed_m_s, kick_reach,
            mass_g, force_n, stroke, spring_k_n_m, threshold_speed_m_s, inertia_kg_m2,
            damping_per_s, eject_speed_m_s, exit_layer, exit_offset: None, auto_pull,
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// `MechSet::new` never panics; an accepted set steps a ball through and
    /// around the mech with every coil exercised, staying finite.
    #[test]
    fn prop_random_parameters_never_panic_or_blow_up(
        kind in arb_kind(),
        p in arb_params(),
        vx in -3.0f32..3.0,
        vy in -3.0f32..3.0,
        off in -0.03f32..0.03,
        depth in 0.0f32..1.0,
    ) {
        let s = spec(kind, Vec3::new(0.0, 0.0, 0.0))
            .with_coil(CoilId(1))
            .with_switch(SwitchId(2))
            .with_params(p);
        let Ok(mechs) = MechSet::new(&[s], &defaults()) else { return Ok(()); };
        let mut rig = Rig::on_floor(&[]);
        rig.mechs = mechs;
        // A plunger is only approached from its tip side (the rod and its
        // housing are behind it): with the default direction +y, from y > 0.
        // (Observation reported separately: a ball *behind* the tip is hit by
        // the kinematic pull-back, which moves at depth·stroke per tick.)
        let vy = if kind == MechKind::Plunger && p.angle_deg.is_none() { -vy.abs() } else { vy };
        prop_assume!(kind != MechKind::Plunger || p.angle_deg.is_none());
        let v = Vec3::new(vx, vy, 0.0);
        let dir = if v.length() > 1.0e-3 { v.normalize() } else { Vec3::X };
        let side = Vec3::new(-dir.y, dir.x, 0.0);
        let h = rig.add(steel_ball(-dir * 0.12 + side * off + Vec3::new(0.0, 0.0, R + 1.0e-4), v));
        for t in 0..600u32 {
            if t == 100 { rig.mechs.fire_coil(CoilId(1), 30); }
            if t == 250 { rig.mechs.hold_coil(CoilId(1), true); rig.mechs.set_plunger(depth); }
            if t == 400 { rig.mechs.hold_coil(CoilId(1), false); rig.mechs.set_plunger(0.0); }
            if t == 300 { rig.mechs.nudge(Vec3::new(0.2, -0.1, 0.0)); }
            rig.step();
            let b = rig.ball(h);
            prop_assert!(is_finite_ball(&b), "tick {}: {:?}", t, b);
            prop_assert!(b.velocity.length() < 60.0, "tick {}: speed {}", t, b.velocity.length());
            if let Some((x, xv)) = rig.mechs.slider_state(0) { prop_assert!(x.is_finite() && xv.is_finite()); }
            if let Some((a, w)) = rig.mechs.spinner_state(0) { prop_assert!(a.is_finite() && w.is_finite()); }
        }
    }
}
