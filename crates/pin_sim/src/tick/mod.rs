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
use pin_core::snapshot::{
    BallState, FlipperState, FrameStateSnapshot, IDENTITY_QUAT, MAX_BALLS, MAX_DYNAMIC_MESHES,
    MeshTransform, NEWTON_SWING_RANGE_RAD,
};
use pin_core::{CabinetAxis, HostCtx, SwitchEdge, TableScriptBackend};
use pin_physics::ball::{BallHandle as PhysHandle, Geometry};
use pin_physics::world::layers;

use crate::alloc_guard::ExemptScope;
use crate::sim::{PendingEdge, Script, Simulation, SlotPlan, SlotSource, bit, set_bit};

/// Seconds per tick.
pub const DT: f32 = 0.001;

/// Cabinet velocity change per tick at full nudge-axis deflection, m/s
/// (30 m/s² ≈ 3 g of cabinet acceleration for one tick).
pub const NUDGE_DV_PER_TICK: f32 = 0.03;

/// Speed (m/s) above which a captive Newton ball closes its mech's switch.
pub const NEWTON_SWITCH_SPEED: f32 = 0.05;

/// Angle (rad) about its hinge axis that an open gate's dynamic mesh slot
/// publishes (SPEC Appendix A, REQ-VIS-002); a closed gate publishes 0.
/// The gate's colliders toggle instantly with its coil, so the published
/// angle steps between the two values (DECISIONS.md "#103 — gate and
/// diverter slot angles").
pub const GATE_OPEN_ANGLE_RAD: f32 = core::f32::consts::FRAC_PI_3;

/// Angle (rad) about its hinge axis that an active diverter's dynamic mesh
/// slot publishes (SPEC Appendix A, REQ-VIS-002); an inactive diverter (or
/// one without a `[[diverters]]` binding) publishes 0. Instantaneous like
/// the diverter's colliders.
pub const DIVERTER_ON_ANGLE_RAD: f32 = core::f32::consts::FRAC_PI_6;

/// The swing angle (rad) of a newton_ball's cradle slot for the captive
/// ball's signed displacement `offset_m` along its path direction from rest
/// on a path of `path_length_m`: `offset / length · NEWTON_SWING_RANGE_RAD`,
/// clamped to `±NEWTON_SWING_RANGE_RAD` (0 at rest, the full range at the
/// path's end; monotone; DECISIONS.md "#103 — Newton ball swing mapping").
/// A non-finite input or a non-positive length gives 0.
#[must_use]
pub fn newton_swing_rad(offset_m: f32, path_length_m: f32) -> f32 {
    if !(offset_m.is_finite() && path_length_m.is_finite() && path_length_m > 0.0) {
        return 0.0;
    }
    let s = offset_m / path_length_m * NEWTON_SWING_RANGE_RAD;
    pin_core::fmath::max_f32(
        pin_core::fmath::min_f32(s, NEWTON_SWING_RANGE_RAD),
        -NEWTON_SWING_RANGE_RAD,
    )
}

/// Unit quaternion `[x, y, z, w]` of a right-handed rotation by `angle_rad`
/// about `axis` (normalised here). The half-angle sine and cosine are
/// `libm` `f64` functions (deterministic on every target, and accurate for
/// a spinner's large unwrapped angles), the result is normalised in `f64`
/// and rounded once to `f32`. A non-finite angle or axis, or a zero axis,
/// gives the identity.
#[must_use]
pub fn axis_angle_quat(axis: Vec3, angle_rad: f32) -> [f32; 4] {
    let (ax, ay, az) = (f64::from(axis.x), f64::from(axis.y), f64::from(axis.z));
    let len = libm::sqrt((ax * ax + ay * ay) + az * az);
    if !(angle_rad.is_finite() && len.is_finite() && len > 0.0) {
        return IDENTITY_QUAT;
    }
    let half = 0.5 * f64::from(angle_rad);
    let k = libm::sin(half) / len;
    let (x, y, z, w) = (ax * k, ay * k, az * k, libm::cos(half));
    let n = libm::sqrt(((x * x + y * y) + z * z) + w * w);
    let q = [
        (x / n) as f32,
        (y / n) as f32,
        (z / n) as f32,
        (w / n) as f32,
    ];
    if q.iter().all(|c| c.is_finite()) {
        q
    } else {
        IDENTITY_QUAT
    }
}

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
        // `cab_bits` keeps the unforced button states (forced switches are
        // applied in `compose_switches`, so a release takes effect at once);
        // the staging frame sees the forced ones.
        let mut cab = [0u64; 4];
        for c in CabinetInput::ALL {
            let n = self.cabinet[c.index()].index();
            if frame.pressed(c) {
                set_bit(&mut cab, n, true);
            }
            if bit(&self.forced_mask, n) {
                let b = 1u8 << c.index();
                if bit(&self.forced_val, n) {
                    frame.buttons |= b;
                } else {
                    frame.buttons &= !b;
                }
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
        self.apply_commands(tick);

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
        // Rule 4: the Lua VM's arena allocations are exempt from the guard;
        // the scope covers the script calls only, never engine code.
        let lua = matches!(self.script, Script::Lua(_));
        // The previous tick's queue (kept for `last_commands`) is dropped
        // here, before the first script call; clearing keeps the capacity.
        self.cmd.clear();
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
            let _exempt = lua.then(ExemptScope::enter);
            script.on_switch_edge(e.switch, e.edge, &mut ctx);
        }
        self.pending.clear();
        let _exempt = lua.then(ExemptScope::enter);
        script.on_tick(&mut ctx);
    }

    fn step_physics(&mut self, tick: u64) {
        for (h, b) in self.solver.balls() {
            self.prev_positions[usize::from(h.0)] = b.position;
        }
        self.flippers.step_tick(tick);
        self.collect_flipper_haptics();
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

        // Layer transitions, then drains and escapes, by slot. The sensors
        // were sampled in `mechs.step`, before the transitions; a drained
        // ball's trough switches are recorded with its final layer so that
        // the trough closes this tick even when the ball changed layer.
        self.drain_bits = [0; 4];
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
            let mut drained = false;
            for z in &self.troughs {
                if z.layer_mask & layers::layer_bit(layer) != 0 && p.distance(z.center) <= z.radius
                {
                    drained = true;
                    set_bit(&mut self.drain_bits, z.switch.index(), true);
                }
            }
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
    /// script pulse (the coil command; a Thing Flip adds to it in
    /// `FlipperSet::step_tick`).
    pub(crate) fn update_flipper_drive(&mut self) {
        let tick = self.tick;
        for d in &self.drives {
            let on = d.staged || d.held || tick < d.pulse_until;
            self.flippers.set_energised(d.id, on);
        }
    }

    /// A rising edge of a flipper's energised state, as the physics sees it
    /// (coil command or automatic Thing Flip), is a haptic impulse. Runs
    /// after `FlipperSet::step_tick`, so automatic flips count too.
    pub(crate) fn collect_flipper_haptics(&mut self) {
        for i in 0..self.drives.len() {
            let d = &mut self.drives[i];
            let on = self.flippers.is_energised(d.id);
            let rising = on && !d.energised;
            d.energised = on;
            let coil = d.coil;
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
        // Trough switches follow the drains of this tick's physics step
        // (final layers), not the sensors sampled before the transitions.
        for z in &self.troughs {
            set_bit(&mut bits, z.switch.index(), false);
        }
        for (b, d) in bits.iter_mut().zip(self.drain_bits) {
            *b |= d;
        }
        self.mechs.switch_states(&mut bits);
        for d in &self.drives {
            set_bit(
                &mut bits,
                d.eos_switch.index(),
                self.flippers.eos_closed(d.id),
            );
        }
        for g in &self.gates {
            if let Some(s) = g.switch {
                set_bit(&mut bits, s.index(), g.open);
            }
        }
        for n in &self.newtons {
            if let Some(s) = n.switch {
                let chain = self.solver.newton_chain(n.chain);
                let moving =
                    (0..chain.len()).any(|i| chain.ball_speed(i).abs() > NEWTON_SWITCH_SPEED);
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
        for (w, &word) in new.iter().enumerate() {
            let mut diff = word ^ self.view.switches[w];
            while diff != 0 {
                let b = diff.trailing_zeros() as usize;
                diff &= diff - 1;
                let n = w * 64 + b;
                if n >= MAX_SWITCHES {
                    continue;
                }
                let closed = word & (1u64 << b) != 0;
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

    /// Step 6: the frame snapshot: the tick, the balls, the flippers, the
    /// lamps and GI, and one dynamic mesh slot per `[[mechs]]` entry in
    /// `table.toml` order (`dynamic_mesh_count = min(mechs, 64)`; SPEC
    /// Appendix A "Dynamic mesh slot convention", REQ-VIS-002). Each slot
    /// is rewritten every tick from the load-time slot plan:
    ///
    /// * drop target: `[0, 0, MechSet::drop_target_offset_m]` (0 up,
    ///   `−height` down), identity;
    /// * sling, kickback, plunger: `u · MechSet::slider_state.0` (plunger
    ///   negative = pulled back), identity;
    /// * spinner: the pivot, rotated by the spinner angle about its axis;
    /// * gate, diverter: the hinge (mech position), rotated about
    ///   `−(z × u)` by [`GATE_OPEN_ANGLE_RAD`] / [`DIVERTER_ON_ANGLE_RAD`]
    ///   while open / active, else identity;
    /// * newton_ball: the mech position, rotated about `−(z × u)` by
    ///   [`newton_swing_rad`] of the captive ball's displacement;
    /// * bumper (ring kick not modelled) and VUK: zero offset, identity.
    ///
    /// Balls: captive Newton balls are published as ordinary `balls`
    /// entries and take no extra slots (SPEC Appendix A). The order is
    /// fixed: the free balls first (ascending solver handle), then the
    /// captive balls in `[[mechs]]` order and, within a chain, by ball
    /// index. A captive ball's entry is its path position, its velocity the
    /// unit path tangent times its path speed, the chain's profile, the
    /// mech's `layer`, identity orientation and zero angular velocity
    /// (captive spin is not modelled); a captive ball with a non-finite
    /// position or velocity is skipped. The array holds at most
    /// `MAX_BALLS` entries: captive balls past it are dropped and counted
    /// in [`SimCounters::captive_balls_unpublished`](crate::SimCounters).
    /// [`Simulation::balls_in_play`] (and the script's `balls_in_play`)
    /// stays free-only, because the rules end a ball when it reaches 0, so
    /// `snapshot.balls()[..balls_in_play()]` are exactly the free balls.
    ///
    /// Reads only; writes nothing but `self.snapshot` and that counter,
    /// never allocates, and never publishes a non-finite value.
    pub(crate) fn publish_snapshot(&mut self) {
        let s = &mut self.snapshot;
        s.tick = self.tick;
        s.ball_count = 0;
        for (_, b) in self.solver.balls() {
            s.push_ball(BallState::from(b));
        }
        // Captive Newton balls: ordinary entries after the free balls.
        let mut unpublished = 0u32;
        for n in &self.newtons {
            if n.chain >= self.solver.newton_chain_count() {
                continue;
            }
            let c = self.solver.newton_chain(n.chain);
            for i in 0..c.len() {
                let position = c.ball_position(i);
                let velocity = c.path().tangent(c.ball_t(i)).normalize_or_zero() * c.ball_speed(i);
                if !position.is_finite() || !velocity.is_finite() {
                    continue;
                }
                let ball = BallState {
                    position_m: position.to_array(),
                    orientation: IDENTITY_QUAT,
                    velocity_m_s: velocity.to_array(),
                    angular_velocity_rad_s: [0.0; 3],
                    profile: c.profile(),
                    layer: n.layer,
                };
                if !s.push_ball(ball) {
                    unpublished = unpublished.saturating_add(1);
                }
            }
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
        s.lamps = self.lamps;
        s.gi = self.gi;
        let n = self.slots.len().min(MAX_DYNAMIC_MESHES);
        for i in 0..n {
            let t = self.slot_transform(i, &self.slots[i]);
            self.snapshot.dynamic_meshes[i] = t;
        }
        self.snapshot.dynamic_mesh_count = n as u8;
        self.counters.captive_balls_unpublished = self
            .counters
            .captive_balls_unpublished
            .saturating_add(unpublished);
    }
}

impl Simulation {
    /// The dynamic mesh slot of mech `index` (see `publish_snapshot`).
    fn slot_transform(&self, index: usize, plan: &SlotPlan) -> MeshTransform {
        let (position, orientation) = match plan.source {
            SlotSource::Fixed => (plan.position, IDENTITY_QUAT),
            SlotSource::Drop => {
                let z = self.mechs.drop_target_offset_m(index).unwrap_or(0.0);
                (Vec3::new(0.0, 0.0, z), IDENTITY_QUAT)
            }
            SlotSource::Slider => {
                let x = self.mechs.slider_state(index).map_or(0.0, |(x, _)| x);
                (plan.dir * x, IDENTITY_QUAT)
            }
            SlotSource::Spinner => {
                let a = self.mechs.spinner_state(index).map_or(0.0, |(a, _)| a);
                (plan.position, axis_angle_quat(plan.axis, a))
            }
            SlotSource::Gate(g) => {
                let open = self.gates.get(g).is_some_and(|g| g.open);
                let a = if open { GATE_OPEN_ANGLE_RAD } else { 0.0 };
                (plan.position, axis_angle_quat(plan.axis, a))
            }
            SlotSource::Diverter(d) => {
                let on = self.diverters.get(d).is_some_and(|d| d.active);
                let a = if on { DIVERTER_ON_ANGLE_RAD } else { 0.0 };
                (plan.position, axis_angle_quat(plan.axis, a))
            }
            SlotSource::Newton(k) => {
                let swing = self
                    .newtons
                    .get(k)
                    .filter(|b| b.chain < self.solver.newton_chain_count())
                    .map(|b| self.solver.newton_chain(b.chain))
                    .filter(|c| !c.is_empty())
                    .map_or(0.0, |c| {
                        let offset = (c.ball_position(0) - plan.path_start).dot(plan.dir);
                        newton_swing_rad(offset, plan.path_length)
                    });
                (plan.position, axis_angle_quat(plan.axis, swing))
            }
        };
        let position = if position.is_finite() {
            position
        } else {
            Vec3::ZERO
        };
        MeshTransform {
            position_m: position.to_array(),
            orientation,
        }
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

    /// Rotation angle (rad, `(−π, π]`) of unit quaternion `q` about `axis`.
    fn recovered_angle(q: [f32; 4], axis: Vec3) -> f64 {
        let v = [f64::from(q[0]), f64::from(q[1]), f64::from(q[2])];
        let s = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        let dot = v[0] * f64::from(axis.x) + v[1] * f64::from(axis.y) + v[2] * f64::from(axis.z);
        let a = 2.0 * libm::atan2(s, f64::from(q[3]));
        let a = if dot < 0.0 { -a } else { a };
        wrap(a)
    }

    fn wrap(a: f64) -> f64 {
        let t = core::f64::consts::TAU;
        let r = a - t * libm::floor(a / t);
        if r > core::f64::consts::PI { r - t } else { r }
    }

    fn norm(q: [f32; 4]) -> f64 {
        q.iter()
            .map(|&c| f64::from(c) * f64::from(c))
            .sum::<f64>()
            .sqrt()
    }

    /// REQ-VIS-002 (spinner, gate, diverter, newton slots): angle 0 is the
    /// exact identity; a non-finite angle or a zero / non-finite axis is
    /// the identity.
    #[test]
    fn axis_angle_zero_and_degenerate_inputs_are_identity() {
        for axis in [Vec3::X, Vec3::Y, Vec3::new(0.6, -0.8, 0.0)] {
            assert_eq!(axis_angle_quat(axis, 0.0), IDENTITY_QUAT);
            assert_eq!(axis_angle_quat(axis, f32::NAN), IDENTITY_QUAT);
            assert_eq!(axis_angle_quat(axis, f32::INFINITY), IDENTITY_QUAT);
        }
        assert_eq!(axis_angle_quat(Vec3::ZERO, 1.0), IDENTITY_QUAT);
        assert_eq!(
            axis_angle_quat(Vec3::new(f32::NAN, 0.0, 0.0), 1.0),
            IDENTITY_QUAT
        );
        assert_eq!(
            axis_angle_quat(Vec3::new(f32::INFINITY, 0.0, 0.0), 1.0),
            IDENTITY_QUAT
        );
    }

    /// REQ-VIS-002 (spinner slot): a large unwrapped spinner angle is
    /// encoded within 1e-5 (modulo 2π), and the rotation sense is
    /// right-handed (−z rotated by +90° about −(z × y) = +x is +y).
    #[test]
    fn axis_angle_large_angles_and_sense() {
        let axis = Vec3::X;
        for a in [100.0f32, -250.5, 1234.567, core::f32::consts::TAU] {
            let q = axis_angle_quat(axis, a);
            let err = wrap(recovered_angle(q, axis) - f64::from(a)).abs();
            assert!(err <= 1e-5, "{a}: {err}");
        }
        let q = axis_angle_quat(Vec3::X, core::f32::consts::FRAC_PI_2);
        let r = pin_physics::quat_ops::mul_vec3(glam::Quat::from_array(q), -Vec3::Z);
        assert!((r - Vec3::Y).length() < 1e-6, "{r}");
    }

    /// REQ-VIS-002 (newton_ball slot): 0 at rest, the full range at the
    /// path end, clamped, monotone, 0 for degenerate inputs.
    #[test]
    fn newton_swing_mapping() {
        assert_eq!(newton_swing_rad(0.0, 0.05), 0.0);
        assert!((newton_swing_rad(0.05, 0.05) - NEWTON_SWING_RANGE_RAD).abs() < 1e-6);
        assert_eq!(newton_swing_rad(1.0, 0.05), NEWTON_SWING_RANGE_RAD);
        assert_eq!(newton_swing_rad(-1.0, 0.05), -NEWTON_SWING_RANGE_RAD);
        assert_eq!(newton_swing_rad(f32::NAN, 0.05), 0.0);
        assert_eq!(newton_swing_rad(0.01, 0.0), 0.0);
        assert_eq!(newton_swing_rad(0.01, f32::INFINITY), 0.0);
        let mut prev = f32::NEG_INFINITY;
        for k in -100..=100 {
            let s = newton_swing_rad(k as f32 * 1.0e-3, 0.05);
            assert!(s >= prev);
            prev = s;
        }
    }

    proptest::proptest! {
        /// REQ-VIS-002: the axis-angle quaternion is unit length within
        /// 1e-6 and round-trips the angle within 1e-5 for angles in
        /// [−π, π] about any non-degenerate axis.
        #[test]
        fn axis_angle_unit_and_round_trip(
            x in -1.0f32..1.0, y in -1.0f32..1.0, z in -1.0f32..1.0,
            a in -core::f32::consts::PI..core::f32::consts::PI,
        ) {
            let axis = Vec3::new(x, y, z);
            proptest::prop_assume!(axis.length() > 1.0e-3);
            let q = axis_angle_quat(axis, a);
            proptest::prop_assert!(q.iter().all(|c| c.is_finite()));
            proptest::prop_assert!((norm(q) - 1.0).abs() <= 1.0e-6);
            let err = wrap(recovered_angle(q, axis.normalize()) - f64::from(a)).abs();
            proptest::prop_assert!(err <= 1.0e-5, "angle {} err {}", a, err);
        }

        /// REQ-VIS-002 (newton_ball slot): the swing is finite and within
        /// ±NEWTON_SWING_RANGE_RAD for any input.
        #[test]
        fn newton_swing_in_range(o in proptest::num::f32::ANY, l in proptest::num::f32::ANY) {
            let s = newton_swing_rad(o, l);
            proptest::prop_assert!(s.is_finite());
            proptest::prop_assert!(s.abs() <= NEWTON_SWING_RANGE_RAD);
        }
    }

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

    /// A copy of the 2.5 replay fixture whose rules emit different commands
    /// on consecutive ticks; removed on drop.
    struct Bundle(std::path::PathBuf);

    impl Drop for Bundle {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const RULES: &str = r#"
local t = 0
function on_init()
  pinball.set_score(1, 7)
end
function on_switch(id, closed)
  if closed and id == pinball.sw.bumper_ring then
    pinball.set_score(1, 1000)
  end
end
function on_tick()
  t = t + 1
  if t % 2 == 1 then
    pinball.set_score(1, t)
  else
    pinball.set_lamp(pinball.lamp.blink_lamp, 1)
  end
end
"#;

    fn bundle() -> Bundle {
        let from =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/replay_table");
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pin_sim_last_commands_{}_{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let guard = Bundle(dir);
        for e in std::fs::read_dir(&from).unwrap() {
            let p = e.unwrap().path();
            std::fs::copy(&p, guard.0.join(p.file_name().unwrap())).unwrap();
        }
        std::fs::write(guard.0.join("rules.lua"), RULES).unwrap();
        guard
    }

    #[test]
    fn last_commands_hold_the_last_tick_queue_only() {
        let b = bundle();
        let mut sim = Simulation::load(&b.0, crate::SimConfig::default()).expect("load");
        let blink = pin_core::ids::LampId(
            sim.ids()
                .get(pin_assets::IdKind::Lamp, "blink_lamp")
                .expect("blink_lamp"),
        );
        // After load: on_init's commands (applied, and still visible).
        assert_eq!(sim.last_commands().scores.as_slice(), &[(0, 7)]);
        assert!(sim.last_commands().lamps.is_empty());
        assert_eq!(sim.score(0), Some(7));
        let mut out = Box::new(FrameStateSnapshot::default());
        // Tick 1: on_tick's score only (on_init's report is gone).
        sim.step_1ms(1, &[], &mut out);
        assert_eq!(sim.last_commands().scores.as_slice(), &[(0, 1)]);
        assert!(sim.last_commands().lamps.is_empty());
        // Tick 2: the lamp only; tick 1's score is not carried over.
        sim.step_1ms(2, &[], &mut out);
        assert!(sim.last_commands().scores.is_empty());
        assert_eq!(sim.last_commands().lamps.as_slice(), &[(blink, 1.0)]);
        // Tick 3: an edge first, then on_tick, in queue order.
        sim.set_switch("bumper_ring", true).unwrap();
        sim.step_1ms(3, &[], &mut out);
        assert_eq!(sim.last_commands().scores.as_slice(), &[(0, 1000), (0, 3)]);
        assert!(sim.last_commands().lamps.is_empty());
        assert_eq!(sim.score(0), Some(3), "the queue was applied in order");
        // Reading the queue changes nothing: a twin run without reads
        // reaches the same state hash.
        let mut twin = Simulation::load(&b.0, crate::SimConfig::default()).expect("load");
        for t in 1..=3 {
            if t == 3 {
                twin.set_switch("bumper_ring", true).unwrap();
            }
            twin.step_1ms(t, &[], &mut out);
        }
        assert_eq!(twin.state_hash(), sim.state_hash());
    }
}
