//! Subphase 5.5, verification criterion 1: a flipper on a 90° (vertical)
//! plane produces the shot speed predicted from the horizontal case once
//! gravity along the plane is accounted for; and the staging modes actuate
//! the vertical flipper on the expected tick.
//!
//! No solver code knows about the backbox: the same flipper rig is built in
//! two frames ([`pitched_frame`]: the 6.5° table; [`vertical_frame`]: the
//! backbox plane) and stepped with the existing solver.
//!
//! # The prediction
//! A ball cradled at rest between the lowered bat and a post (the "cradle":
//! its rest position depends only on the *direction* of in-plane gravity,
//! which is the same "down the playfield" in both frames, so both runs start
//! from the same point on the bat) is fired. In plane coordinates (`u`
//! across, `w` up the playfield), the ball's acceleration from everything but
//! the bat is
//! * horizontal table: `a_h = −g·sin 6.5° ŵ − μ·g·cos 6.5° ŝ` (gravity along
//!   the plane plus sliding friction on the playfield, `ŝ` the sliding
//!   direction, `μ` the steel–wood pair: the ball leaves a flipper sliding,
//!   and sliding lasts `2v/(7μg)` ≈ 0.4 s, longer than the window);
//! * vertical plane: `a_v = −g ŵ` (full gravity along the plane, none along
//!   the normal, so no playfield reaction and no friction).
//!
//! The bat drives the ball along the bat normal (a kinematic contact: the
//! ball's normal velocity is the bat surface's), so of `Δa = a_v − a_h` the
//! part along the bat `t̂` and the friction term reach the exit velocity
//! over the contact time `T_c`:
//!
//! `v_v,pred = v_h + [(Δg·t̂) t̂ + μ g cos 6.5° ŝ_h] · T_c`, with
//! `Δg = −g(1 − sin 6.5°) ŵ`.
//!
//! # Tolerance (derived, per shot)
//! * the bat-normal part of `Δg` may be absorbed by the contact anywhere
//!   between fully and not at all: `|Δg − (Δg·t̂) t̂| · T_c`;
//! * the ball's extra weight loads the bat: relative bat-speed change at most
//!   `m·|Δg|·r / τ_power` (`r` the contact radius), times `|v_h|`;
//! * separation is detected to one tick: `|Δa| · dt`.
//!
//! After separation the vertical ball is in free flight with exactly `−g ŵ`
//! (checked to 1 mm/s over 20 ms), so the plane is vertical in the solver's
//! eyes: full `g` along it and no normal force.

#[path = "backbox_util.rs"]
mod util;

use glam::{Quat, Vec3};
use pin_core::{CabinetAxis, CabinetInput, FlipperStagingMode, Material, PhysicsDefaults};
use pin_physics::world::{StaticShape, StaticWorldBuilder, layers};
use util::*;

/// Post radius of the cradle, m.
const POST_R: f32 = 0.003;
/// Ticks the cradled ball settles before the flip.
const SETTLE: u32 = 300;
/// Separation: the ball surface is this far from the bat surface, m.
const SEPARATED: f32 = 1.0e-3;

/// One shot: exit velocity (plane coordinates), contact time, the bat
/// direction at separation (plane coordinates), contact radius, and the
/// velocities over the free flight after separation.
struct Shot {
    exit: Vec3,
    contact_s: f32,
    bat_dir: Vec3,
    radius: f32,
    flight: Vec<Vec3>,
}

/// The cradle: ball resting on the lowered bat at `fraction` of its length,
/// held from rolling down the bat by a post. `torque` scales the coil.
fn shot(frame: Quat, fraction: f32, torque: f32, layer: u8) -> Shot {
    let mut cfg = flipper_config(frame, Vec3::ZERO, layer, dedicated());
    cfg.power_torque_nm *= torque;
    let dir = bat_dir(-30.0);
    let n = v(-dir.y, dir.x, 0.0);
    let radius = BASE_R + (TIP_R - BASE_R) * fraction;
    let c = dir * (BAT_LEN * fraction) + n * (radius + BALL_R + 2.0e-5);
    let post = c + dir * (BALL_R + POST_R + 2.0e-5);
    let mask = layers::layer_bit(layer);
    let mut b = StaticWorldBuilder::new();
    b.add_collider(plane_slab(frame, Vec3::ZERO, 2.0, 2.0), mask, true);
    b.add_collider(
        StaticShape::Cylinder {
            base: frame * post,
            axis: frame * v(0.0, 0.0, 0.03),
            radius: POST_R,
        },
        mask,
        true,
    );
    let mut rig = Rig::new(&[cfg], b.build());
    let h = rig.add_ball(frame * (c + v(0.0, 0.0, BALL_R)), Vec3::ZERO, layer);
    for _ in 0..SETTLE {
        rig.step();
    }
    assert!(
        rig.ball(h).velocity.length() < 1.0e-3,
        "the cradle holds the ball"
    );
    let inv = frame.inverse();
    let normal = frame * Vec3::Z;
    rig.flippers.set_energised(BB_FLIPPER, true);
    for t in 1..=200u32 {
        rig.step();
        let b = rig.ball(h);
        if bat_gap(&rig.flippers, BB_FLIPPER, normal, b.position) > SEPARATED {
            let bat = inv * rig.flippers.bat_direction(BB_FLIPPER).unwrap();
            let exit = inv * b.velocity;
            let mut flight = vec![exit];
            for _ in 0..20 {
                rig.step();
                flight.push(inv * rig.ball(h).velocity);
            }
            return Shot {
                exit,
                contact_s: t as f32 * DT,
                bat_dir: v(bat.x, bat.y, 0.0).normalize(),
                radius: (c.length()).max(BAT_LEN * fraction),
                flight,
            };
        }
    }
    panic!("fraction {fraction}, torque {torque}: the ball never left the bat");
}

fn steel_wood_friction() -> f32 {
    let d = PhysicsDefaults::builtin();
    pin_core::materials::lookup_pair(&d.material_pairs, Material::Steel, Material::Wood)
        .expect("steel-wood pair")
        .friction
}

#[test]
fn frames_put_full_gravity_along_the_vertical_plane() {
    let g_h = gravity_along(pitched_frame(PITCH_DEG));
    let g_v = gravity_along(vertical_frame());
    assert!((g_h - G * libm::sinf(PITCH_DEG.to_radians())).abs() < 1e-5);
    assert!(
        (g_v - G).abs() < 1e-5,
        "full g along the backbox plane: {g_v}"
    );
    // The backbox plane normal is horizontal.
    let n = vertical_frame() * Vec3::Z;
    assert!(n.z.abs() < 1e-6 && (n.y + 1.0).abs() < 1e-6);
}

#[test]
fn vertical_flipper_shot_speed_matches_the_horizontal_prediction() {
    let p = PITCH_DEG.to_radians();
    let mu = steel_wood_friction();
    let mass = pin_core::builtin_profile("standard_steel")
        .expect("steel")
        .mass_kg;
    let tau = PhysicsDefaults::builtin()
        .resolve(None)
        .flipper_power_torque;
    let dg = v(0.0, -G * (1.0 - libm::sinf(p)), 0.0);
    let mut worst = 0.0f32;
    let mut n = 0;
    for torque in [0.7f32, 1.0, 1.5] {
        for fraction in [0.2f32, 0.4, 0.6, 0.8] {
            let h = shot(pitched_frame(PITCH_DEG), fraction, torque, 0);
            let vt = shot(vertical_frame(), fraction, torque, BACKBOX_LAYER);
            let tc = vt.contact_s;
            let t_hat = vt.bat_dir;
            let s_hat = h.exit.normalize();
            let along = t_hat * dg.dot(t_hat);
            let pred = h.exit + (along + s_hat * (mu * G * libm::cosf(p))) * tc;
            let tol = (dg - along).length() * tc
                + h.exit.length() * mass * dg.length() * vt.radius / (tau * torque)
                + (dg.length() + mu * G) * DT;
            let err = (vt.exit - pred).length();
            println!(
                "fraction {fraction:.1}, coil x{torque:.1}: v_h {:.3} m/s, v_v {:.3} m/s, predicted {:.3} m/s, |error| {:.3} <= tol {:.3} (T_c {:.0} ms)",
                h.exit.length(),
                vt.exit.length(),
                pred.length(),
                err,
                tol,
                tc * 1e3
            );
            assert!(
                err <= tol,
                "fraction {fraction}, torque {torque}: |v_v - v_pred| = {err} > {tol}"
            );
            assert!(
                vt.exit.is_finite() && vt.exit.z.abs() < 1e-3,
                "stays in the plane"
            );
            // Free flight on the vertical plane: exactly −g along the plane.
            for (k, w) in vt.flight.iter().enumerate() {
                let want = vt.exit + v(0.0, -G * k as f32 * DT, 0.0);
                assert!(
                    (*w - want).length() < 1.0e-3,
                    "free flight tick {k}: {w:?} vs {want:?}"
                );
            }
            // The shot is fast: the gravity change is a correction, not the shot.
            assert!(vt.exit.length() > 2.0);
            worst = worst.max(err / tol);
            n += 1;
        }
    }
    println!("{n} shots, worst error/tolerance {worst:.2}");
}

/// A vertical flipper with `mode` on the backbox layer; returns the first
/// tick its bat moved after being energised at tick `on`.
fn first_move(mode: FlipperStagingMode, on: u64) -> (u64, bool) {
    let frame = vertical_frame();
    let cfg = flipper_config(frame, v(0.0, 0.0, 0.0), BACKBOX_LAYER, mode);
    let mut rig = Rig::new(&[cfg], playfield_world(frame, layers::BACKBOX));
    let rest = rig.flippers.angle_deg(BB_FLIPPER);
    for t in 0..100u64 {
        if t == on {
            rig.flippers.set_energised(BB_FLIPPER, true);
        }
        rig.step();
        if rig.flippers.angle_deg(BB_FLIPPER) != rest {
            return (t, rig.flippers.is_energised(BB_FLIPPER));
        }
    }
    panic!("the bat never moved");
}

#[test]
fn every_staging_mode_actuates_the_vertical_flipper_on_its_tick() {
    let modes = [
        FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
        FlipperStagingMode::ParallelLinked(CabinetInput::RightFlipper),
        FlipperStagingMode::DualStageThreshold {
            axis: CabinetAxis::LeftFlipper,
            lower_threshold: 0.30,
            upper_threshold: 0.85,
        },
    ];
    for mode in modes {
        for on in [0u64, 7, 33] {
            let (t, energised) = first_move(mode, on);
            assert_eq!(t, on, "{mode:?}: the bat moves in the energised tick");
            assert!(energised);
        }
    }
}

#[test]
fn the_vertical_bat_rests_and_returns_under_full_gravity() {
    // The return spring holds the bat at rest against full gravity on the
    // vertical plane (the bat has no weight in the solver; this guards that
    // gravity changes nothing for an idle bat), and a released bat returns.
    let frame = vertical_frame();
    let cfg = flipper_config(frame, Vec3::ZERO, BACKBOX_LAYER, dedicated());
    let mut rig = Rig::new(&[cfg], playfield_world(frame, layers::BACKBOX));
    for _ in 0..200 {
        rig.step();
    }
    assert_eq!(rig.flippers.angle_deg(BB_FLIPPER), -30.0);
    rig.flippers.set_energised(BB_FLIPPER, true);
    for _ in 0..80 {
        rig.step();
    }
    assert!((rig.flippers.angle_deg(BB_FLIPPER) - 20.0).abs() < 1e-3);
    rig.flippers.set_energised(BB_FLIPPER, false);
    for _ in 0..400 {
        rig.step();
    }
    assert_eq!(rig.flippers.angle_deg(BB_FLIPPER), -30.0);
}
