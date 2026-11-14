//! `Simulation::step_1ms()` and the fixed tick order (PLAN.md §2.2).
//!
//! Owner: Subphase 2.5. Stub created by Subphase 1.1.
//!
//! See [`Simulation`] for the six steps. Everything here runs on the sim
//! thread: no heap allocation, no locks, no I/O (D5), fixed iteration
//! order (switches ascending, balls by slot, flippers by body index).
//!
//! Switch states are composed from three sources each tick: the cabinet
//! buttons (step 1), the physics (sensors, mechs, flipper EOS switches,
//! gates and Newton balls; step 5) and the test API's forced switches
//! ([`Simulation::set_switch`]), which override both. Every change of the
//! composed state is an edge, in ascending switch order.

use glam::Vec3;
use pin_core::input::{CabinetInput, HapticsTelemetryEvent, InputEvent};
use pin_core::script::MAX_SWITCHES;
use pin_core::sim::SimStep;
use pin_core::snapshot::{BallState, FlipperState, FrameStateSnapshot, MAX_BALLS};
use pin_core::{CabinetAxis, HostCtx, SwitchEdge, TableScriptBackend};
use pin_physics::ball::{BallHandle as PhysHandle, Geometry};
use pin_physics::world::layers;

use crate::sim::{PendingEdge, Script, Simulation, bit, set_bit};

/// Seconds per tick.
pub const DT: f32 = 0.001;

/// Cabinet velocity change per tick at full nudge-axis deflection, m/s
/// (30 m/s² ≈ 3 g of cabinet acceleration for one tick).
pub const NUDGE_DV_PER_TICK: f32 = 0.03;

/// Speed (m/s) above which a captive Newton ball closes its mech's switch.
pub const NEWTON_SWITCH_SPEED: f32 = 0.05;

impl Simulation {
    /// One tick, numbered `tick` (the next is `tick + 1`), with the input
    /// events whose timestamp is ≤ the tick's end. Writes `self.snapshot`.
    pub(crate) fn step_tick(&mut self, tick: u64, inputs: &[InputEvent]) {
        self.tick = tick;
        self.view.tick = tick;
        self.haptics.clear();

        // 1. Inputs → cabinet switches, staging, plunger, nudge.
        for ev in inputs {
            if self.ingest.push(*ev).is_err() {
                self.counters.inputs_dropped = self.counters.inputs_dropped.saturating_add(1);
            }
        }
        let mut frame = self.ingest.advance(tick);
        let mut cab = [0u64; 4];
        for c in CabinetInput::ALL {
            let n = self.cabinet[c.index()].index();
            if bit(&self.forced_mask, n) {
                let b = 1u8 << c.index();
                if bit(&self.forced_val, n) {
                    frame.buttons |= b;
                } else {
                    frame.buttons &= !b;
                }
            }
            if frame.pressed(c) {
                set_bit(&mut cab, n, true);
            }
        }
        self.cab_bits = cab;
        self.router_out.clear();
        self.router.update(&frame, &mut self.router_out);
        for i in 0..self.router_out.len() {
            let (id, on) = self.router_out[i];
            if let Some(d) = self.drives.iter_mut().find(|d| d.id == id) {
                d.staged = on;
            }
        }
        self.mechs.set_plunger(frame.axis(CabinetAxis::Plunger));
        let nudge = Vec3::new(
            frame.axis(CabinetAxis::NudgeX),
            frame.axis(CabinetAxis::NudgeY),
            0.0,
        );
        if nudge != Vec3::ZERO {
            self.mechs.nudge(nudge * NUDGE_DV_PER_TICK);
        }
        self.update_switches(tick);

        // 2. Switch edges to the script (and Thing Flips), then on_tick.
        self.view.balls_in_play = self.solver.ball_count().min(255) as u8;
        self.deliver_edges();

        // 3. Commands.
        self.apply_commands();

        // 4. Physics.
        self.update_flipper_drive();
        self.update_gates();
        self.step_physics(tick);

        // 5. Switch states, edges, haptics (collected in steps 3-4).
        self.collect_physics_bits();
        self.update_switches(tick);

        // 6. Snapshot.
        self.publish_snapshot();
    }

    fn deliver_edges(&mut self) {
        let mut ctx = HostCtx {
            view: &self.view,
            rng: &mut self.rng,
            cmd: &mut self.cmd,
        };
        let script: &mut dyn TableScriptBackend = match &mut self.script {
            Script::Lua(b) => b.as_mut(),
            Script::Wasm(b) => b.as_mut(),
        };
        for e in &self.pending {
            self.flippers
                .on_switch_edge(e.switch, e.edge == SwitchEdge::Closed, e.tick);
            script.on_switch_edge(e.switch, e.edge, &mut ctx);
        }
        self.pending.clear();
        script.on_tick(&mut ctx);
    }

    fn step_physics(&mut self, tick: u64) {
        for (h, b) in self.solver.balls() {
            self.prev_positions[usize::from(h.0)] = b.position;
        }
        self.flippers.step_tick(tick);
        self.events.clear();
        let geo = Geometry {
            world: &self.world,
            guides: &self.guides,
            wireforms: &self.wireforms,
        };
        self.mechs.step(
            &mut self.solver,
            &geo,
            Some(&mut self.flippers),
            DT,
            &mut self.events,
        );
        self.counters.ball_events_dropped = self
            .counters
            .ball_events_dropped
            .saturating_add(self.events.dropped());

        // Layer transitions, then drains and escapes, by slot.
        for slot in 0..MAX_BALLS {
            let h = PhysHandle(slot as u8);
            let Some(b) = self.solver.ball(h).copied() else {
                continue;
            };
            let layer = self
                .transitions
                .update(b.layer, self.prev_positions[slot], b.position);
            if layer != b.layer
                && let Some(m) = self.solver.ball_mut(h)
            {
                m.layer = layer;
            }
            let p = b.position;
            let drained = self.troughs.iter().any(|z| {
                z.layer_mask & layers::layer_bit(layer) != 0 && p.distance(z.center) <= z.radius
            });
            let escaped = !(p.is_finite() && b.velocity.is_finite())
                || p.cmplt(self.bounds.0).any()
                || p.cmpgt(self.bounds.1).any();
            if drained {
                self.counters.balls_drained = self.counters.balls_drained.saturating_add(1);
                self.remove_slot(slot);
            } else if escaped {
                self.counters.balls_escaped = self.counters.balls_escaped.saturating_add(1);
                self.remove_slot(slot);
            }
        }
    }

    /// Energises each flipper from its staging button, script hold and
    /// script pulse; a rising edge is a haptic impulse.
    pub(crate) fn update_flipper_drive(&mut self) {
        let tick = self.tick;
        for i in 0..self.drives.len() {
            let d = &mut self.drives[i];
            let on = d.staged || d.held || tick < d.pulse_until;
            let rising = on && !d.energised;
            d.energised = on;
            let (id, coil) = (d.id, d.coil);
            self.flippers.set_energised(id, on);
            if rising {
                self.push_haptic(coil.0);
            }
        }
    }

    /// Opens or closes each gate from its coil.
    pub(crate) fn update_gates(&mut self) {
        let tick = self.tick;
        for g in &mut self.gates {
            let open = g.held || tick < g.pulse_until;
            if open != g.open {
                g.open = open;
                for &c in &g.colliders {
                    self.world.set_enabled(c, !open);
                }
            }
        }
    }

    pub(crate) fn push_haptic(&mut self, source: u16) {
        let e = HapticsTelemetryEvent {
            tick: self.tick,
            source,
            strength: 1.0,
        };
        if self.haptics.try_push(e).is_err() {
            self.counters.haptics_dropped = self.counters.haptics_dropped.saturating_add(1);
        }
    }

    /// Recomputes the physics switch bits: sensors, mechs and tilt bob,
    /// flipper EOS switches, gates, Newton balls.
    pub(crate) fn collect_physics_bits(&mut self) {
        let mut bits = self.phys_bits;
        self.solver.sensor_switches(&mut bits);
        self.mechs.switch_states(&mut bits);
        for d in &self.drives {
            set_bit(&mut bits, d.eos_switch.index(), self.flippers.eos_closed(d.id));
        }
        for g in &self.gates {
            if let Some(s) = g.switch {
                set_bit(&mut bits, s.index(), g.open);
            }
        }
        for n in &self.newtons {
            if let Some(s) = n.switch {
                let chain = self.solver.newton_chain(n.chain);
                let moving = (0..chain.len()).any(|i| chain.ball_speed(i).abs() > NEWTON_SWITCH_SPEED);
                set_bit(&mut bits, s.index(), moving);
            }
        }
        self.phys_bits = bits;
    }

    /// The switch states: (cabinet | physics), then the forced switches.
    pub(crate) fn compose_switches(&self) -> [u64; 4] {
        let mut out = [0u64; 4];
        for (i, o) in out.iter_mut().enumerate() {
            let raw = self.cab_bits[i] | self.phys_bits[i];
            *o = (raw & !self.forced_mask[i]) | (self.forced_val[i] & self.forced_mask[i]);
        }
        out
    }

    /// Applies the composed switch states to the script's view; every
    /// change is an edge (ascending switch number) queued for the script,
    /// and every closing edge counts as a hit.
    pub(crate) fn update_switches(&mut self, tick: u64) {
        let new = self.compose_switches();
        for w in 0..4 {
            let mut diff = new[w] ^ self.view.switches[w];
            while diff != 0 {
                let b = diff.trailing_zeros() as usize;
                diff &= diff - 1;
                let n = w * 64 + b;
                if n >= MAX_SWITCHES {
                    continue;
                }
                let closed = new[w] & (1u64 << b) != 0;
                if closed {
                    self.switch_hits[n] = self.switch_hits[n].saturating_add(1);
                }
                let e = PendingEdge {
                    switch: pin_core::SwitchId(n as u16),
                    edge: if closed {
                        SwitchEdge::Closed
                    } else {
                        SwitchEdge::Opened
                    },
                    tick,
                };
                if self.pending.try_push(e).is_err() {
                    self.counters.edges_dropped = self.counters.edges_dropped.saturating_add(1);
                }
            }
        }
        self.view.switches = new;
    }

    /// Step 6: the frame snapshot.
    pub(crate) fn publish_snapshot(&mut self) {
        let s = &mut self.snapshot;
        s.tick = self.tick;
        s.ball_count = 0;
        for (_, b) in self.solver.balls() {
            s.push_ball(BallState::from(b));
        }
        s.flipper_count = 0;
        for d in &self.drives {
            s.push_flipper(FlipperState {
                id: d.id,
                angle_rad: self.flippers.relative_angle_rad(d.id),
                angular_velocity_rad_s: self.flippers.angular_velocity(d.id),
                zipper_offset_m: self.flippers.base_offset_mm(d.id) * 1.0e-3,
            });
        }
        s.dynamic_mesh_count = 0;
        s.lamps = self.lamps;
        s.gi = self.gi;
    }
}

impl SimStep for Simulation {
    /// Runs tick `tick` (normally the last tick + 1) with `inputs` and
    /// copies the new snapshot into `out`.
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        self.step_tick(tick, inputs);
        *out = self.snapshot;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_bits_helpers() {
        let mut b = [0u64; 4];
        set_bit(&mut b, 200, true);
        assert!(bit(&b, 200));
        set_bit(&mut b, 300, true);
        assert!(!bit(&b, 300));
        set_bit(&mut b, 200, false);
        assert_eq!(b, [0; 4]);
    }
}
