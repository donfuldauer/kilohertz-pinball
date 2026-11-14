//! Physics debug data and state injection helpers.
//!
//! Owner: Subphase 2.5. Stub created by Subphase 1.1.
//!
//! Read and write access to solver, flipper and mech state for the headless
//! simulation's test API (`pin_sim`), state-injection tests and the debug HUD.
//! Nothing here changes how the solver behaves: injection goes through the
//! same public entry points a table uses ([`BallSolver::add_ball`],
//! [`BallSolver::ball_mut`]) and every reader only copies state out.
//!
//! * [`ball_state`] / [`set_ball_state`] / [`spawn_ball`]: a ball's full
//!   state as a [`BallState`] (position, orientation, velocity, angular
//!   velocity, profile, layer).
//! * [`SolverDebug`], [`FlipperDebug`], [`MechDebug`]: plain-data views,
//!   filled without allocating ([`flipper_debug`], [`mech_debug`]).
//!
//! Every function is allocation-free, so it may run on the sim thread.

use arrayvec::ArrayVec;
use glam::{Quat, Vec3};
use pin_core::ids::{FlipperId, ProfileId};
use pin_core::schema::table::MechKind;
use pin_core::snapshot::{BallState, MAX_FLIPPERS};

use crate::ball::{Ball, BallHandle, BallSolver};
use crate::flipper::FlipperSet;
use crate::mech::MechSet;

/// The full state of ball `h` (`None` for an empty slot).
#[must_use]
pub fn ball_state(solver: &BallSolver, h: BallHandle) -> Option<BallState> {
    solver.ball(h).map(BallState::from)
}

/// Builds a solver [`Ball`] from a [`BallState`] (the orientation is taken
/// as given, `[x, y, z, w]`).
#[must_use]
pub fn ball_from_state(state: &BallState) -> Ball {
    let [x, y, z, w] = state.orientation;
    Ball {
        position: Vec3::from_array(state.position_m),
        velocity: Vec3::from_array(state.velocity_m_s),
        angular_velocity: Vec3::from_array(state.angular_velocity_rad_s),
        orientation: Quat::from_xyzw(x, y, z, w),
        profile: state.profile,
        layer: state.layer,
    }
}

/// Overwrites every field of ball `h` with `state` (a teleport, as
/// [`BallSolver::ball_mut`] documents). Returns `false`, changing nothing,
/// for an empty slot, a non-finite state, a layer above 7, or a profile the
/// solver does not know.
pub fn set_ball_state(solver: &mut BallSolver, h: BallHandle, state: &BallState) -> bool {
    if !state_is_valid(solver, state) {
        return false;
    }
    let ball = ball_from_state(state);
    match solver.ball_mut(h) {
        Some(b) => {
            *b = ball;
            true
        }
        None => false,
    }
}

/// Adds a ball of `profile` on `layer` at `position_m` with `velocity_m_s`
/// (no spin, identity orientation) in the lowest free slot. `None` when the
/// solver is full, the profile is unknown, the layer is above 7 or an input
/// is not finite.
pub fn spawn_ball(
    solver: &mut BallSolver,
    profile: ProfileId,
    layer: u8,
    position_m: [f32; 3],
    velocity_m_s: [f32; 3],
) -> Option<BallHandle> {
    let state = BallState {
        position_m,
        velocity_m_s,
        profile,
        layer,
        ..BallState::default()
    };
    if !state_is_valid(solver, &state) {
        return None;
    }
    solver.add_ball(ball_from_state(&state))
}

fn state_is_valid(solver: &BallSolver, s: &BallState) -> bool {
    let finite = s
        .position_m
        .iter()
        .chain(&s.velocity_m_s)
        .chain(&s.angular_velocity_rad_s)
        .chain(&s.orientation)
        .all(|v| v.is_finite());
    let profile_ok = solver
        .profiles()
        .get(s.profile.index())
        .is_some_and(|p| p.mass_kg.is_finite() && p.mass_kg > 0.0 && p.radius_m() > 0.0);
    finite && s.layer <= 7 && profile_ok
}

/// Solver-wide counters for the debug HUD.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SolverDebug {
    /// Balls on the table.
    pub ball_count: usize,
    /// Completed steps.
    pub step_count: u64,
    /// Simulated time of the completed steps, seconds.
    pub sim_time_s: f64,
    /// Gravity, m/s².
    pub gravity: Vec3,
    /// Number of magnets.
    pub magnet_count: usize,
    /// Number of Newton chains.
    pub newton_chain_count: usize,
}

/// Reads the solver counters.
#[must_use]
pub fn solver_debug(solver: &BallSolver) -> SolverDebug {
    SolverDebug {
        ball_count: solver.ball_count(),
        step_count: solver.step_count(),
        sim_time_s: solver.sim_time(),
        gravity: solver.config().gravity,
        magnet_count: solver.magnets().len(),
        newton_chain_count: solver.newton_chain_count(),
    }
}

/// One flipper's state.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FlipperDebug {
    /// The flipper.
    pub id: FlipperId,
    /// Angle relative to rest, radians (signed like the sweep).
    pub angle_rad: f32,
    /// Angular velocity, rad/s (signed like the sweep).
    pub angular_velocity_rad_s: f32,
    /// Coil commanded on (button, script hold or Thing Flip).
    pub energised: bool,
    /// EOS switch closed.
    pub eos_closed: bool,
    /// Coil torque now applied, N·m.
    pub coil_torque_nm: f32,
    /// Zipper slide offset, metres.
    pub zipper_offset_m: f32,
}

/// Every flipper of `set` in body order.
#[must_use]
pub fn flipper_debug(set: &FlipperSet) -> ArrayVec<FlipperDebug, MAX_FLIPPERS> {
    let mut out = ArrayVec::new();
    for body in 0..set.len() {
        if let Some(id) = set.id_of(body) {
            out.push(FlipperDebug {
                id,
                angle_rad: set.relative_angle_rad(id),
                angular_velocity_rad_s: set.angular_velocity(id),
                energised: set.is_energised(id),
                eos_closed: set.eos_closed(id),
                coil_torque_nm: set.coil_torque_nm(id),
                zipper_offset_m: set.base_offset_mm(id) * 1.0e-3,
            });
        }
    }
    out
}

/// One mech's state (fields that do not apply to its kind are `None`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MechDebug {
    /// Kind.
    pub kind: MechKind,
    /// Its switch was closed after the last tick.
    pub switch_closed: bool,
    /// Sling, kickback or plunger: displacement (m) and velocity (m/s).
    pub slider: Option<(f32, f32)>,
    /// Spinner: angle (rad) and angular speed (rad/s).
    pub spinner: Option<(f32, f32)>,
    /// Spinner pulses so far (0 for other kinds).
    pub spinner_pulses: u64,
    /// Drop target: down (or rising).
    pub drop_down: Option<bool>,
    /// VUK: the ball it holds.
    pub vuk_ball: Option<BallHandle>,
}

/// Mech `index` of `mechs` (`None` past the end).
#[must_use]
pub fn mech_debug(mechs: &MechSet, index: usize) -> Option<MechDebug> {
    Some(MechDebug {
        kind: mechs.kind(index)?,
        switch_closed: mechs.mech_switch_closed(index),
        slider: mechs.slider_state(index),
        spinner: mechs.spinner_state(index),
        spinner_pulses: mechs.spinner_pulses(index),
        drop_down: mechs.drop_target_down(index),
        vuk_ball: mechs.vuk_ball(index),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ball::BallSolverConfig;
    use crate::mech::MechSpec;
    use pin_core::{FlipperConfig, PhysicsDefaults, builtin_profile};

    fn solver() -> BallSolver {
        let cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
        let profiles = [builtin_profile("standard_steel").unwrap()];
        BallSolver::new(cfg, &profiles)
    }

    #[test]
    fn spawn_set_and_read_round_trip_exact_bits() {
        let mut s = solver();
        let h = spawn_ball(&mut s, ProfileId(0), 2, [0.1, 0.2, 0.3], [1.0, -2.0, 0.5]).unwrap();
        let b = ball_state(&s, h).unwrap();
        assert_eq!(b.position_m, [0.1, 0.2, 0.3]);
        assert_eq!(b.velocity_m_s, [1.0, -2.0, 0.5]);
        assert_eq!(b.layer, 2);
        let target = BallState {
            position_m: [0.5, 0.25, 0.0135],
            orientation: [0.0, 0.0, 0.70710677, 0.70710677],
            velocity_m_s: [-0.0, 3.0, 1.0e-7],
            angular_velocity_rad_s: [10.0, -20.0, 30.0],
            profile: ProfileId(0),
            layer: 1,
        };
        assert!(set_ball_state(&mut s, h, &target));
        let got = ball_state(&s, h).unwrap();
        assert_eq!(got, target);
        assert_eq!(got.velocity_m_s[0].to_bits(), (-0.0f32).to_bits());
        assert_eq!(ball_from_state(&got), *s.ball(h).unwrap());
    }

    #[test]
    fn invalid_injection_is_refused() {
        let mut s = solver();
        assert!(spawn_ball(&mut s, ProfileId(3), 0, [0.0; 3], [0.0; 3]).is_none());
        assert!(spawn_ball(&mut s, ProfileId(0), 8, [0.0; 3], [0.0; 3]).is_none());
        assert!(spawn_ball(&mut s, ProfileId(0), 0, [f32::NAN, 0.0, 0.0], [0.0; 3]).is_none());
        let h = spawn_ball(&mut s, ProfileId(0), 0, [0.0; 3], [0.0; 3]).unwrap();
        let bad = BallState {
            velocity_m_s: [f32::INFINITY, 0.0, 0.0],
            ..BallState::default()
        };
        assert!(!set_ball_state(&mut s, h, &bad));
        assert!(!set_ball_state(&mut s, BallHandle(9), &BallState::default()));
        assert_eq!(ball_state(&s, BallHandle(9)), None);
        for _ in 1..crate::ball::MAX_BALLS {
            assert!(spawn_ball(&mut s, ProfileId(0), 0, [0.0; 3], [0.0; 3]).is_some());
        }
        assert!(spawn_ball(&mut s, ProfileId(0), 0, [0.0; 3], [0.0; 3]).is_none());
        let d = solver_debug(&s);
        assert_eq!(d.ball_count, crate::ball::MAX_BALLS);
        assert_eq!(d.step_count, 0);
        assert_eq!(d.magnet_count, 0);
        assert_eq!(d.newton_chain_count, 0);
        assert_eq!(d.gravity, Vec3::new(0.0, 0.0, -9.81));
    }

    #[test]
    fn flipper_and_mech_views() {
        let cfg = FlipperConfig {
            id: FlipperId(3),
            coil: pin_core::CoilId(1),
            eos_switch: pin_core::SwitchId(10),
            collision_layer: 0,
            base_position_mm: [100.0, 100.0, 0.0],
            surface_normal: [0.0, 0.0, 1.0],
            rest_angle_deg: -30.0,
            sweep_angle_deg: 50.0,
            eos_angle_fraction: 0.92,
            shape: pin_core::FlipperShape::StraightCapsule {
                length_mm: 76.0,
                base_radius_mm: 11.0,
                tip_radius_mm: 6.5,
            },
            inertia_kg_m2: 1.0e-4,
            power_torque_nm: 1.2,
            hold_torque_nm: 0.15,
            return_spring_torque_nm: 0.03,
            stop_restitution: 0.15,
            rubber_restitution: 0.65,
            rubber_damping: 0.35,
            staging: pin_core::FlipperStagingMode::DedicatedButton(
                pin_core::CabinetInput::LeftFlipper,
            ),
            zipper_slide_axis_mm: None,
            zipper_slide_speed_mm_s: 0.0,
        };
        let mut set = FlipperSet::new(&[cfg]).unwrap();
        let v = flipper_debug(&set);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, FlipperId(3));
        assert_eq!(v[0].angle_rad, 0.0);
        assert!(v[0].eos_closed && !v[0].energised);
        set.set_energised(FlipperId(3), true);
        for _ in 0..40 {
            set.advance(0.001);
        }
        let v = flipper_debug(&set);
        assert!(v[0].energised && !v[0].eos_closed);
        assert!(v[0].angle_rad > 0.8, "{}", v[0].angle_rad);
        assert!(v[0].coil_torque_nm > 0.0);
        assert_eq!(v[0].zipper_offset_m, 0.0);

        let d = PhysicsDefaults::builtin();
        let specs = [
            MechSpec::new(MechKind::Bumper, Vec3::new(0.2, 0.3, 0.0)),
            MechSpec::new(MechKind::Spinner, Vec3::new(0.3, 0.3, 0.0)),
            MechSpec::new(MechKind::Sling, Vec3::new(0.1, 0.3, 0.0)),
        ];
        let mechs = MechSet::new(&specs, &d).unwrap();
        let b = mech_debug(&mechs, 0).unwrap();
        assert_eq!(b.kind, MechKind::Bumper);
        assert!(!b.switch_closed && b.slider.is_none() && b.spinner.is_none());
        assert!(mech_debug(&mechs, 1).unwrap().spinner.is_some());
        assert_eq!(mech_debug(&mechs, 1).unwrap().spinner_pulses, 0);
        assert!(mech_debug(&mechs, 2).unwrap().slider.is_some());
        assert_eq!(mech_debug(&mechs, 2).unwrap().drop_down, None);
        assert_eq!(mech_debug(&mechs, 2).unwrap().vuk_ball, None);
        assert!(mech_debug(&mechs, 3).is_none());
    }
}
