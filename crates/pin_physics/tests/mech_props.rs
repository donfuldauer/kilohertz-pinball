//! Property tests of the Subphase 2.4 mechs: construction never panics on
//! any parameters, body geometry and seam obligations, spinner spin-down,
//! tilt-bob bounds and threshold monotonicity, bumper symmetry, plunger
//! monotonicity.

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::PhysicsDefaults;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::MechParams;
use pin_core::schema::table::MechKind;
use pin_physics::ball::MovingBodies;
use pin_physics::mech::{MechSet, MechSpec, TiltBob, TiltBobParams};
use proptest::prelude::*;
use util::*;

const KINDS: [MechKind; 10] = [
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

fn any_f32() -> impl Strategy<Value = Option<f32>> {
    prop_oneof![
        Just(None),
        (-1.0e3f32..1.0e3).prop_map(Some),
        (0.0f32..100.0).prop_map(Some),
        Just(Some(f32::NAN)),
        Just(Some(f32::INFINITY)),
        Just(Some(0.0)),
    ]
}

prop_compose! {
    fn params()(a in any_f32(), b in any_f32(), c in any_f32(), d in any_f32(),
                e in any_f32(), f in any_f32(), g in any_f32(), h in any_f32(),
                layer in proptest::option::of(0u8..12), exit in proptest::option::of(0u8..12))
        -> MechParams {
        MechParams {
            angle_deg: a, layer, radius: b, length: c, height: d, thickness: e,
            kick_speed_m_s: f, kick_reach: g, mass_g: h, force_n: a, stroke: b,
            spring_k_n_m: c, threshold_speed_m_s: d, inertia_kg_m2: e, damping_per_s: f,
            eject_speed_m_s: g, exit_layer: exit, exit_offset: h.map(|x| [x, 0.0, x]),
            auto_pull: a,
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn construction_never_panics_and_valid_sets_run(kind in 0usize..10, p in params(),
                                                    x in -0.2f32..0.2, y in -0.2f32..0.2,
                                                    vx in -3.0f32..3.0, vy in -3.0f32..3.0) {
        let spec = MechSpec::new(KINDS[kind], v(x, y, 0.0))
            .with_coil(CoilId(1))
            .with_switch(SwitchId(2))
            .with_params(p);
        if let Ok(set) = MechSet::new(&[spec], &PhysicsDefaults::builtin()) {
            let mut t = Table { mechs: set, ..Table::new(&[], world(&[]), 6.5) };
            let h = t.ball(0.0, 0.0, v(vx, vy, 0.0));
            for i in 0..150 {
                if i == 20 { t.mechs.fire_coil(CoilId(1), 30); }
                if i == 60 { t.mechs.set_plunger(0.7); }
                if i == 100 { t.mechs.set_plunger(0.0); }
                t.tick();
            }
            let b = t.get(h);
            prop_assert!(b.position.is_finite() && b.velocity.is_finite());
        }
    }

    #[test]
    fn non_finite_positions_are_rejected(kind in 0usize..10) {
        let spec = MechSpec::new(KINDS[kind], v(f32::NAN, 0.0, 0.0));
        prop_assert!(MechSet::new(&[spec], &PhysicsDefaults::builtin()).is_err());
    }

    #[test]
    fn closest_point_is_consistent(kind in prop_oneof![Just(1usize), Just(2), Just(3), Just(5), Just(0)],
                                   angle in -180.0f32..180.0,
                                   px in -0.06f32..0.06, py in -0.06f32..0.06, pz in -0.02f32..0.08) {
        let mut spec = MechSpec::new(KINDS[kind], Vec3::ZERO);
        spec.params.angle_deg = Some(angle);
        let mut set = MechSet::new(&[spec], &PhysicsDefaults::builtin()).unwrap();
        set.begin_tick(DT);
        let p = v(px, py, pz);
        let c = set.closest_point(0, 0.0, p).unwrap();
        prop_assert!((c.normal.length() - 1.0).abs() < 1e-4);
        prop_assert!(((p - c.point).length() - c.distance.abs()).abs() < 1e-5);
        prop_assert!((c.point + c.normal * c.distance - p).length() < 1e-5);
        // A point pushed out along the normal is farther away.
        let q = c.point + c.normal * 0.01;
        let d = set.closest_point(0, 0.0, q).unwrap().distance;
        prop_assert!((d - 0.01).abs() < 1e-4, "{d}");
        set.end_tick(DT);
    }

    #[test]
    fn slider_obligations_hold(kind in prop_oneof![Just(MechKind::Sling), Just(MechKind::Kickback), Just(MechKind::Plunger)],
                               ticks in 0usize..40, coil in any::<bool>(), pull in 0.0f32..1.0,
                               j in -0.05f32..0.05, t in 0.0f32..1.0e-3) {
        let spec = MechSpec::new(kind, Vec3::ZERO).with_coil(CoilId(1));
        let mut set = MechSet::new(&[spec], &PhysicsDefaults::builtin()).unwrap();
        set.hold_coil(CoilId(1), coil);
        set.set_plunger(pull);
        for _ in 0..ticks {
            set.begin_tick(DT);
            set.end_tick(DT);
        }
        if kind == MechKind::Plunger { set.set_plunger(0.0); }
        set.begin_tick(DT);
        let p = v(0.0, 0.01, 0.01);
        set.apply_impulse(0, t, p, Vec3::Y * j);
        let (lo, hi) = if kind == MechKind::Plunger { (-0.05, 0.0) } else {
            (0.0, if kind == MechKind::Sling { 0.012 } else { 0.020 })
        };
        for k in 0..=10 {
            let s = t + (DT - t) * k as f32 / 10.0;
            let vel = set.velocity_at(0, s, p).y;
            for d in [Vec3::Y, -Vec3::Y, Vec3::X] {
                let w = set.inverse_mass_at(0, s, p, d);
                prop_assert!(w.is_finite() && w >= 0.0);
            }
            prop_assert!(vel.abs() <= set.max_surface_speed(0) + 1e-6);
            let c = set.closest_point(0, s, p).unwrap();
            prop_assert!(c.distance.is_finite());
            let _ = (lo, hi);
        }
        set.end_tick(DT);
        let (x, vel) = set.slider_state(0).unwrap();
        prop_assert!(x >= lo - 1e-7 && x <= hi + 1e-7);
        // Engaged on a stop: the velocity is 0 or moving away.
        if x <= lo { prop_assert!(vel >= 0.0); }
        if x >= hi { prop_assert!(vel <= 0.0); }
    }

    #[test]
    fn spinner_total_rotation_and_pulses(w0 in 1.0f32..200.0, d in 0.5f32..3.0, sign in any::<bool>()) {
        let w0 = if sign { w0 } else { -w0 };
        let mut spec = MechSpec::new(MechKind::Spinner, Vec3::ZERO);
        spec.params.damping_per_s = Some(d);
        let mut set = MechSet::new(&[spec], &PhysicsDefaults::builtin()).unwrap();
        set.set_spinner_speed(0, w0);
        for _ in 0..100_000 {
            set.begin_tick(DT);
            set.end_tick(DT);
            if set.spinner_state(0).unwrap().1 == 0.0 { break; }
        }
        let (angle, w) = set.spinner_state(0).unwrap();
        prop_assert_eq!(w, 0.0);
        let total = w0 / d;
        prop_assert!((angle - total).abs() <= 2e-3 * total.abs() + 1e-3, "{} vs {}", angle, total);
        let floor = (total.abs() / core::f32::consts::PI).floor() as i64;
        prop_assert!((set.spinner_pulses(0) as i64 - floor).abs() <= 1);
    }

    #[test]
    fn tilt_bob_stays_inside_its_ring(nudges in proptest::collection::vec((-0.5f32..0.5, -0.5f32..0.5, -0.5f32..0.5, 1u32..300), 1..6)) {
        let mut bob = TiltBob::new(TiltBobParams::default());
        let gap = bob.params().gap;
        for (x, y, z, n) in nudges {
            bob.nudge(v(x, y, z));
            for _ in 0..n {
                bob.step(DT);
                prop_assert!(bob.offset().length() <= gap + 1e-5);
                prop_assert!(bob.velocity().is_finite());
            }
        }
    }

    #[test]
    fn tilt_closure_is_monotone_in_nudge(a in 0.0f32..core::f32::consts::TAU, m1 in 0.0f32..0.3, m2 in 0.0f32..0.3) {
        let (lo, hi) = if m1 < m2 { (m1, m2) } else { (m2, m1) };
        let dir = v(libm::cosf(a), libm::sinf(a), 0.0);
        let closes = |m: f32| {
            let mut bob = TiltBob::new(TiltBobParams::default());
            bob.nudge(dir * m);
            (0..1500).any(|_| { bob.step(DT); bob.closed() })
        };
        prop_assert!(!closes(lo) || closes(hi), "{} closes, {} does not", lo, hi);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn bumper_exit_is_symmetric(a in 0.0f32..core::f32::consts::TAU, speed in 0.2f32..2.0) {
        let spec = MechSpec::new(MechKind::Bumper, Vec3::ZERO)
            .with_coil(CoilId(1))
            .with_switch(SwitchId(1));
        let mut t = Table::new(&[spec], world(&[]), 0.0);
        let dir = v(libm::cosf(a), libm::sinf(a), 0.0);
        let h = t.ball(dir.x * 0.1, dir.y * 0.1, -dir * speed);
        let mut fired = None;
        for i in 0..600 {
            t.tick();
            if fired.is_none() && t.closed(1) {
                t.mechs.fire_coil(CoilId(1), 20);
                fired = Some(i);
            }
            // Exit speed 30 ms after the fire (before rolling friction).
            if fired.is_some_and(|f| i == f + 30) {
                break;
            }
        }
        prop_assert!(fired.is_some());
        let b = t.get(h);
        let out = b.velocity.truncate().length();
        prop_assert!((1.5..=4.0).contains(&out), "{}", out);
        // Leaves along the line it came in (radially).
        let n = b.position.truncate().normalize();
        prop_assert!(n.dot(dir.truncate()) > 0.99);
    }

    #[test]
    fn plunger_launch_is_monotone(d1 in 0.05f32..1.0, d2 in 0.05f32..1.0) {
        prop_assume!((d1 - d2).abs() > 0.02);
        let launch = |depth: f32| {
            let spec = MechSpec::new(MechKind::Plunger, v(0.0, -0.3, 0.0));
            let mut t = Table::new(&[spec], world(&[]), 6.5);
            let h = t.ball(0.0, -0.3 + 0.006 + radius(STEEL) + 0.0005, Vec3::ZERO);
            for i in 0..500 {
                t.mechs.set_plunger(if i >= 200 { depth * ((i - 200) as f32 / 200.0).min(1.0) } else { 0.0 });
                t.tick();
            }
            t.mechs.set_plunger(0.0);
            let mut peak = 0.0f32;
            for _ in 0..60 { t.tick(); peak = peak.max(t.get(h).velocity.y); }
            peak
        };
        let (lo, hi) = if d1 < d2 { (d1, d2) } else { (d2, d1) };
        prop_assert!(launch(lo) < launch(hi));
    }
}
