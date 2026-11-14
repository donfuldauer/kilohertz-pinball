//! `HostCommandQueue` application.
//!
//! Owner: Subphase 2.5. Stub created by Subphase 1.1.
//!
//! Tick step 3 (PLAN.md §2.2): everything the script queued in step 2 (or in
//! `on_init`) is applied in list order, then the queue is cleared. Nothing
//! allocates.
//!
//! | list | effect |
//! |---|---|
//! | `coil_pulses (coil, ms)` | a flipper coil is energised for `ms` ticks from this tick; mech coils ([`MechSet::fire_coil`](pin_physics::mech::MechSet::fire_coil)); a gate's coil opens it for `ms` ticks; a haptic impulse |
//! | `coil_holds (coil, on)` | flipper hold, [`MechSet::hold_coil`](pin_physics::mech::MechSet::hold_coil), gate hold; a haptic impulse when switched on |
//! | `magnets (magnet, power)` | the solver magnet's power (clamped `0..=1`) |
//! | `zippers (flipper, on)` | [`FlipperSet::set_zipper`](pin_physics::flipper::FlipperSet::set_zipper) |
//! | `diverters (diverter, on)` | enables the diverter's `on` colliders and disables its `off` ones (or the reverse) |
//! | `ball_spawns (dispenser, profile)` | a ball of `profile` at the dispenser's exit with its exit velocity (layer 0); counted as failed when the slot arena is full or an id is unknown |
//! | `lamps`, `gi_tint` | the snapshot's lamp intensities (clamped `0..=1`) and GI tint |
//! | `scores (player, score)` | stored for [`Simulation::score`] (the host only stores what scripts report) |
//! | `persist_writes (key, value)` | `HostView::persist` |
//! | `display_scenes`, `sounds` | not consumed here (Subphase 6.0) |

use pin_physics::ball::Ball;

use crate::sim::{Simulation, set_bit};

fn unit(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

impl Simulation {
    /// Applies and clears the command queue (tick step 3).
    pub(crate) fn apply_commands(&mut self) {
        let tick = self.tick;
        for i in 0..self.cmd.coil_pulses.len() {
            let (coil, ms) = self.cmd.coil_pulses[i];
            let until = tick.saturating_add(u64::from(ms));
            for d in &mut self.drives {
                if d.coil == coil {
                    d.pulse_until = d.pulse_until.max(until);
                }
            }
            for g in &mut self.gates {
                if g.coil == Some(coil) {
                    g.pulse_until = g.pulse_until.max(until);
                }
            }
            self.mechs.fire_coil(coil, ms);
            if ms > 0 {
                self.push_haptic(coil.0);
            }
        }
        for i in 0..self.cmd.coil_holds.len() {
            let (coil, on) = self.cmd.coil_holds[i];
            for d in &mut self.drives {
                if d.coil == coil {
                    d.held = on;
                    if !on {
                        d.pulse_until = 0;
                    }
                }
            }
            for g in &mut self.gates {
                if g.coil == Some(coil) {
                    g.held = on;
                    if !on {
                        g.pulse_until = 0;
                    }
                }
            }
            self.mechs.hold_coil(coil, on);
            if on {
                self.push_haptic(coil.0);
            }
        }
        for &(magnet, power) in &self.cmd.magnets {
            if let Some(&(_, sid)) = self.magnets.iter().find(|(m, _)| *m == magnet) {
                self.solver.set_magnet_power(sid, power);
            }
        }
        for &(flipper, on) in &self.cmd.zippers {
            self.flippers.set_zipper(flipper, on);
        }
        for &(diverter, on) in &self.cmd.diverters {
            for d in &mut self.diverters {
                if d.id == diverter && d.active != on {
                    d.active = on;
                    for &c in &d.on {
                        self.world.set_enabled(c, on);
                    }
                    for &c in &d.off {
                        self.world.set_enabled(c, !on);
                    }
                }
            }
        }
        for i in 0..self.cmd.ball_spawns.len() {
            let (dispenser, profile) = self.cmd.ball_spawns[i];
            let exit = self
                .dispensers
                .iter()
                .find(|(d, _, _)| *d == dispenser)
                .map(|&(_, p, v)| (p, v));
            let added = exit.and_then(|(p, v)| {
                let mut ball = Ball::new(profile, p);
                ball.velocity = v;
                self.solver.add_ball(ball)
            });
            match added {
                Some(h) => {
                    self.adopt(h);
                }
                None => self.counters.spawns_failed = self.counters.spawns_failed.saturating_add(1),
            }
        }
        for &(lamp, v) in &self.cmd.lamps {
            if let Some(l) = self.lamps.get_mut(lamp.index()) {
                *l = unit(v);
            }
        }
        if let Some(t) = self.cmd.gi_tint {
            self.gi = t.map(unit);
        }
        for &(player, score) in &self.cmd.scores {
            self.scores[usize::from(player)] = score;
            set_bit(&mut self.scores_reported, usize::from(player), true);
        }
        for &(key, value) in &self.cmd.persist_writes {
            if let Some(p) = self.view.persist.get_mut(key.index()) {
                *p = value;
            }
        }
        self.cmd.clear();
    }
}
