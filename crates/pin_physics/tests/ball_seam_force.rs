//! Seam task 2.2-S, force context: `ExternalForce::force_in` sees the
//! solver's magnets (with remanence τ) and the sub-step time; hooks are told
//! when balls are added and removed; a hook returning exactly zero is
//! bit-identical to no hook. A prototype of the PLAN.md §3.2 remanence model
//! shows the seam is enough for Subphase 2.7.

#[path = "ball_util.rs"]
mod util;

use std::sync::{Arc, Mutex};

use glam::Vec3;
use pin_core::BallProfile;
use pin_core::ids::MagnetId;
use pin_physics::ball::{
    Ball, BallEvents, BallHandle, ExternalForce, ForceContext, Geometry, MAX_BALLS, Magnet,
};
use pin_physics::world::layers;
use util::*;

#[derive(Clone, Debug, PartialEq)]
struct Seen {
    handle: u8,
    step_index: u64,
    tick_dt: f32,
    tick_time: f32,
    sim_time: f64,
    substep: u32,
    substeps: u32,
    dt: f32,
    magnets: usize,
    power0: f32,
    tau0: f32,
    in_range0: bool,
}

#[derive(Default)]
struct Log {
    seen: Vec<Seen>,
    lifecycle: Vec<(char, u8)>,
}

/// Records everything it is given; returns zero.
struct Probe(Arc<Mutex<Log>>);

impl ExternalForce for Probe {
    fn force_in(
        &mut self,
        ctx: &ForceContext<'_>,
        handle: BallHandle,
        ball: &Ball,
        _profile: &BallProfile,
        dt: f32,
    ) -> Vec3 {
        let m0 = ctx.magnets.first();
        self.0.lock().unwrap().seen.push(Seen {
            handle: handle.0,
            step_index: ctx.step_index,
            tick_dt: ctx.tick_dt,
            tick_time: ctx.tick_time,
            sim_time: ctx.sim_time,
            substep: ctx.substep,
            substeps: ctx.substeps,
            dt,
            magnets: ctx.magnets.len(),
            power0: m0.map_or(-1.0, |m| m.power),
            tau0: m0.map_or(-1.0, |m| m.remanence_tau_s),
            in_range0: m0.is_some_and(|m| m.in_range(ball.position, ball.layer)),
        });
        Vec3::ZERO
    }
    fn ball_added(&mut self, handle: BallHandle, _ball: &Ball) {
        self.0.lock().unwrap().lifecycle.push(('+', handle.0));
    }
    fn ball_removed(&mut self, handle: BallHandle) {
        self.0.lock().unwrap().lifecycle.push(('-', handle.0));
    }
}

fn magnet_at(position: Vec3) -> Magnet {
    Magnet {
        position,
        k_coil: 2.0e-5,
        d_min: 0.01,
        range: 0.08,
        layer_mask: layers::ALL,
    }
}

#[test]
fn context_shows_magnets_and_sub_step_time() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let log = Arc::new(Mutex::new(Log::default()));
    let id = s.add_magnet(magnet_at(v(0.0, 0.0, -0.005)));
    s.add_magnet(magnet_at(v(1.0, 0.0, -0.005)));
    s.set_magnet_power(id, 0.75);
    assert!((s.magnets()[0].remanence_tau_s - 0.060).abs() < 1e-7);
    s.set_magnet_remanence_tau(id, 0.025);
    s.set_magnet_remanence_tau(id, f32::NAN); // ignored
    s.set_magnet_remanence_tau(id, -1.0); // ignored
    s.set_magnet_remanence_tau(MagnetId(9), 1.0); // unknown: ignored
    assert_eq!(s.magnets()[0].remanence_tau_s, 0.025);
    assert_eq!(s.magnets()[1].remanence_tau_s, s.config().magnet_remanence_tau_s);
    s.add_external_force(Box::new(Probe(log.clone())));
    let r = radius(STEEL);
    // One fast ball (several sub-steps), one in the magnet's range.
    s.add_ball(ball_at(STEEL, v(-0.3, 0.0, r), v(9.0, 0.0, 0.0)))
        .unwrap();
    s.add_ball(ball_at(MINI, v(0.03, 0.2, radius(MINI)), Vec3::ZERO))
        .unwrap();
    s.add_ball(ball_at(MINI, v(0.0, 0.05, radius(MINI)), Vec3::ZERO))
        .unwrap();
    let mut ev = BallEvents::new();
    for _ in 0..20 {
        s.step(&geo, DT, &mut ev);
    }
    assert_eq!(s.step_count(), 20);
    assert!((s.sim_time() - 0.020).abs() < 1e-9);
    let log = log.lock().unwrap();
    assert!(!log.seen.is_empty());
    for e in &log.seen {
        assert_eq!(e.magnets, 2);
        assert_eq!(e.power0, 0.75);
        assert_eq!(e.tau0, 0.025);
        assert_eq!(e.tick_dt, DT);
        assert!(e.substep < e.substeps);
        assert_eq!(e.dt, DT / e.substeps as f32);
        assert_eq!(e.tick_time, e.substep as f32 * e.dt);
        let expect = e.step_index as f64 * f64::from(DT) + f64::from(e.tick_time);
        assert!((e.sim_time - expect).abs() < 1e-9, "{e:?}");
    }
    // Once per sub-step per ball, in handle order within a boundary time.
    for step in 0..20u64 {
        for h in 0..3u8 {
            let calls: Vec<&Seen> = log
                .seen
                .iter()
                .filter(|e| e.step_index == step && e.handle == h)
                .collect();
            assert_eq!(calls.len() as u32, calls[0].substeps, "step {step} ball {h}");
            for (k, c) in calls.iter().enumerate() {
                assert_eq!(c.substep, k as u32);
            }
        }
    }
    assert!(log.seen.iter().any(|e| e.substeps > 1));
    // Range test as the solver applies it.
    assert!(log.seen.iter().any(|e| e.handle == 2 && e.in_range0));
    assert!(log.seen.iter().all(|e| e.handle != 1 || !e.in_range0));
    // sim_time never goes back for one ball.
    for h in 0..3u8 {
        let t: Vec<f64> = log
            .seen
            .iter()
            .filter(|e| e.handle == h)
            .map(|e| e.sim_time)
            .collect();
        assert!(t.windows(2).all(|w| w[1] > w[0]));
    }
}

#[test]
fn magnet_state_helpers_match_the_solver_force() {
    let s = {
        let mut s = solver();
        let id = s.add_magnet(Magnet {
            layer_mask: layers::layer_bit(1),
            ..magnet_at(v(0.0, 0.0, 0.0))
        });
        s.set_magnet_power(id, 1.0);
        s
    };
    let m = s.magnets()[0];
    assert!(m.in_range(v(0.05, 0.0, 0.0), 1));
    assert!(!m.in_range(v(0.05, 0.0, 0.0), 0)); // wrong layer
    assert!(!m.in_range(v(0.09, 0.0, 0.0), 1)); // out of range
    assert!(!m.in_range(v(0.0, 0.0, 0.0), 1)); // at the pole
    // Inside d_min the pull stops growing; beyond the range it is zero.
    let near = m.pull(v(0.005, 0.0, 0.0));
    assert!((near.x + 2.0e-5 * 0.005 / (1.0e-6 * 0.005)).abs() < 1e-3);
    let far = m.pull(v(0.04, 0.0, 0.0));
    assert!((far.x + 2.0e-5 / (0.04 * 0.04 * 0.04)).abs() < 1e-3);
    assert_eq!(m.pull(v(0.1, 0.0, 0.0)), Vec3::ZERO);
    assert_eq!(m.pull(Vec3::ZERO), Vec3::ZERO);
}

#[test]
fn hooks_follow_ball_lifetimes() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let r = radius(STEEL);
    let a = s.add_ball(ball_at(STEEL, v(0.0, 0.0, r), Vec3::ZERO)).unwrap();
    let b = s
        .add_ball(ball_at(STEEL, v(0.1, 0.0, r), Vec3::ZERO))
        .unwrap();
    let log = Arc::new(Mutex::new(Log::default()));
    // Registered late: told about the balls already there.
    s.add_external_force(Box::new(Probe(log.clone())));
    assert_eq!(log.lock().unwrap().lifecycle, vec![('+', 0), ('+', 1)]);
    let mut ev = BallEvents::new();
    s.step(&geo, DT, &mut ev);
    assert_eq!(s.remove_ball(a).map(|b| b.profile), Some(STEEL));
    assert!(s.remove_ball(a).is_none()); // nothing to remove: no call
    let c = s
        .add_ball(ball_at(MINI, v(0.2, 0.0, radius(MINI)), Vec3::ZERO))
        .unwrap();
    assert_eq!(c, a); // the handle is reused...
    s.step(&geo, DT, &mut ev);
    s.remove_ball(b);
    // ...and the hook saw the removal before the new ball.
    assert_eq!(
        log.lock().unwrap().lifecycle,
        vec![('+', 0), ('+', 1), ('-', 0), ('+', 0), ('-', 1)]
    );
}

/// PLAN.md §3.2 remanence as a 2.7 hook would write it: per-ball M that
/// rises toward P inside a magnet's range and decays with τ otherwise;
/// residual pull `c·ρ·M·K·d̂/max(d³, d_min³)`.
struct Remanence {
    rho: f32,
    m: [f32; MAX_BALLS],
}

impl ExternalForce for Remanence {
    fn force_in(
        &mut self,
        ctx: &ForceContext<'_>,
        handle: BallHandle,
        ball: &Ball,
        profile: &BallProfile,
        dt: f32,
    ) -> Vec3 {
        let m = &mut self.m[usize::from(handle.0)];
        let mut f = Vec3::ZERO;
        for ms in ctx.magnets {
            let decay = libm::expf(-dt / ms.remanence_tau_s);
            if ms.in_range(ball.position, ball.layer) {
                *m = ms.power + (*m - ms.power) * decay;
            } else {
                *m *= decay;
            }
            f += ms.pull(ball.position) * (profile.magnetic_coupling * self.rho * *m);
        }
        f
    }
    fn ball_added(&mut self, handle: BallHandle, _ball: &Ball) {
        self.m[usize::from(handle.0)] = 0.0;
    }
}

/// A steel ball held on a magnet for 300 ms, then the magnet is switched
/// off and the ball pushed away at 0.25 m/s. Returns the ball's distance
/// from the pole after 40 ms and the final state.
fn release(hook: Option<f32>) -> (f32, Ball) {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let pole = v(0.0, 0.0, -0.005);
    let id = s.add_magnet(magnet_at(pole));
    s.set_magnet_power(id, 1.0);
    if let Some(rho) = hook {
        s.add_external_force(Box::new(Remanence {
            rho,
            m: [0.7; MAX_BALLS], // stale values: ball_added must reset them
        }));
    }
    let r = radius(STEEL);
    let h = s
        .add_ball(ball_at(STEEL, v(0.02, 0.0, r), Vec3::ZERO))
        .unwrap();
    let mut ev = BallEvents::new();
    for _ in 0..300 {
        s.step(&geo, DT, &mut ev);
    }
    s.set_magnet_power(id, 0.0);
    s.apply_impulse(h, v(0.25 * 0.08, 0.0, 0.0));
    for _ in 0..40 {
        s.step(&geo, DT, &mut ev);
    }
    let b = *s.ball(h).unwrap();
    (b.position.distance(pole), b)
}

#[test]
fn remanence_prototype_lingers_and_zero_remanence_is_bit_identical() {
    let (free, base) = release(None);
    let (zero, zero_state) = release(Some(0.0));
    let (lingering, _) = release(Some(0.5));
    // ρ = 0 (and a registered hook) is bit-identical to no hook at all.
    assert_eq!(zero.to_bits(), free.to_bits());
    for (x, y) in [
        (zero_state.position, base.position),
        (zero_state.velocity, base.velocity),
        (zero_state.angular_velocity, base.angular_velocity),
    ] {
        assert_eq!(x.to_array().map(f32::to_bits), y.to_array().map(f32::to_bits));
    }
    // With remanence the ball leaves measurably later.
    assert!(lingering < free - 1.0e-3, "{lingering} vs {free}");
}
