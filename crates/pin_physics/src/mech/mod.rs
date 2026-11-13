//! Bumpers, slingshots, targets, plunger and tilt bob.
//!
//! Owner: Subphase 2.4. Stub created by Subphase 1.1.
//!
//! A [`MechSet`] holds every `[[mechs]]` entry of a table (built from
//! [`MechSpec`]s, one per entry, in manifest order: the *mech index* used by
//! every accessor is the index in that slice) plus the cabinet [`TiltBob`].
//! Coils drive it ([`fire_coil`](MechSet::fire_coil),
//! [`hold_coil`](MechSet::hold_coil)), the plunger input and nudges come in
//! between ticks, and it reports switch states and edges after each tick.
//! Its moving parts are a [`MovingBodies`] set lent to
//! [`BallSolver::step_with_bodies`]; every mech is body `index` of the set
//! (kinds without a collider report no surface).
//!
//! # Tick order
//!
//! ```ignore
//! mechs.pre_step(&mut solver, dt);                 // nudge, bumper kicks, VUK eject/hold, tilt bob
//! let first = events.as_slice().len();
//! solver.step_with_bodies(&geo, &mut [&mut flippers, &mut mechs], dt, &mut events);
//! mechs.post_step(&mut solver, &events.as_slice()[first..], dt); // drops, captures, switches, edges
//! ```
//!
//! or [`MechSet::step`], which does exactly that. The set index of the
//! mechs in the body slice is [`DEFAULT_BODY_SET`] (1: flippers are set 0,
//! Orchestrator decision); change it with [`MechSet::set_body_set`] when the
//! slice differs ([`MechSet::step`] sets it itself).
//!
//! # Frame and positions
//!
//! The mech frame is the solver's world frame with `+z` the playfield normal
//! and the playfield at the `z` of the mech's `position`. `angle_deg` (default
//! 90°, i.e. `+y`, up the table) gives a direction `u` in the playfield plane.
//!
//! | kind | `position` | collider | kick model |
//! |---|---|---|---|
//! | bumper | ring axis on the playfield | upright steel cylinder, kinematic, static | impulse in `pre_step` while the coil is on |
//! | sling, kickback | centre of the kicker footprint at rest; `u` = kick direction (face normal) | rubber box `thickness × length × height`, 1-DOF finite-mass slider | coil force on the slider (moving body via the seam) |
//! | drop target | centre of the footprint | plastic box, kinematic | dropped by an impact event ≥ threshold; reset rises it kinematically |
//! | spinner | playfield point under the pivot; `u` = direction balls pass | steel flap, 1-DOF rotor (finite inertia) about a horizontal axis | the ball spins it through the seam |
//! | VUK | scoop centre (floor of the scoop) | none | capture by position; eject by teleport + velocity |
//! | plunger | centre of the tip footprint at rest; `u` = launch direction | rubber box, 1-DOF finite-mass slider, spring `−k·x` | spring and hand; auto-plunger coil = pull to `auto_pull` and release |
//! | diverter, gate, newton ball | — | none (not 2.4) | — |
//!
//! * **Bumper.** The ring switch is closed on every tick in which a ball
//!   touches the ring (an impulse on the cylinder, or a gap ≤ 0.1 mm after
//!   the step). While the coil is on, every ball on the bumper's layer whose
//!   gap to the ring is at most `kick_reach` gets the radial velocity of a
//!   collision with a ring moving outward at `V = kick_speed`:
//!   `v_r' = V + e·(V − u)` (`e`: ball–steel restitution from the pair
//!   table), where `u = v_r` for a ball leaving the ring and `u = −e·v_r`
//!   for a ball still closing in (it rebounds off the ring first; the skirt
//!   switch can close a tick before the ball touches). A ball with
//!   `u ≥ V` only rebounds. The radial exit speed is therefore
//!   `max(u, (1 + e)·V − e·u)`: between `V` and `(1 + e)·V` (1.7 and 2.72
//!   m/s with the defaults and a steel ball), or the plain rebound `u` when
//!   that is faster. It depends only on the radial
//!   speed, so every approach angle behaves alike.
//! * **Sling, kickback.** A slider of mass `m` on its rest stop (`x = 0`),
//!   end stop at `stroke`, return spring `k`. While the coil is on the
//!   force `F` drives it; the ball is pushed by the two-body contact of the
//!   seam, so the exit speed emerges from `F`, `m`, `stroke` and the
//!   restitution. At rest the kicker is locked (static geometry). The
//!   switch closes on contact. Latency matters: with the defaults (60 N,
//!   60 g, 12 mm) and the coil fired on the tick of the switch edge or the
//!   next one (the engine's script latency), approaches of 0.1–3 m/s exit
//!   at 1.80–3.35 m/s; a coil fired several ticks later can miss a fast
//!   rebound (the ball has left the stroke) and the exit is the passive
//!   rebound (e.g. 1.2–1.5 m/s at 4 ticks). On a flat table the kick of a
//!   resting ball is the same in every orientation (bit-identical);
//!   on a pitched table it varies by a few per cent with orientation
//!   because gravity acts along (or against) the kick during the contact
//!   and the measurement window.
//! * **Drop target.** Up: a static box. An [`BallEvent::Impact`] on it with
//!   `normal_speed ≥ threshold_speed` drops it (no surface, switch closed),
//!   if the target is fully up and the push was on a face (within ~45° of
//!   the face normal: a ball landing on its top does not drop it).
//!   While its coil (the bank's reset coil: every target sharing it) is on,
//!   a dropped target rises from below the playfield at `kick_speed` as a
//!   kinematic box, so a ball over it is lifted out, never trapped inside;
//!   the switch opens when it is fully up.
//! * **Spinner.** See [`spinner`]: exponential spin-down
//!   (`dω/dt = −d·ω`, total rotation `ω₀/d`), one pulse per 180°
//!   ([`MechSet::spinner_pulses`]); the switch is closed on a pulse tick.
//! * **VUK.** A ball on the capture `layer` whose centre is within `radius`
//!   (horizontally) of the scoop, not below its floor and at most 3 mm above
//!   the rest height (`position.z + r`) is captured when no other ball
//!   overlaps that hold point: held at rest there, switch closed.
//!   While the coil is on, the held ball is moved to `position +
//!   exit_offset` with velocity `eject_speed·z + kick_speed·u`, on
//!   `exit_layer` (the move enters the scoop's [`LayerTransition`], listed in
//!   [`MechSet::transitions`]).
//! * **Plunger.** `x ≤ 0` is the forward displacement from rest; the rod
//!   travels `−stroke ≤ x ≤ 0`, the spring force is `−k·x`, the rest stop
//!   (`x = 0`) is inelastic. [`MechSet::set_plunger`] (0 rest .. 1 full
//!   pull) is the hand: a one-sided constraint `x ≤ −depth·stroke` (a hand
//!   pulls, never pushes); the hand pulls back at most at [`PULL_SPEED`]
//!   (a step in the input is followed at that speed, so the tip's reported
//!   velocity is its real motion and never exceeds 1.5 m/s while pulled). Pulling or a release slower than the spring is
//!   kinematic; a fast release lets the spring drive the finite-mass rod,
//!   which launches the ball through the seam. The rod's velocity is
//!   constant within a tick (semi-implicit Euler: at full pull the spring
//!   changes it by `k·x/m·dt` ≈ 1 m/s per tick, `ω·dt` = 0.14), so when the
//!   ball is not resting on the tip at release the launch depends on the
//!   tick phase of the contact by a few per cent (near full pull on a 6.5°
//!   lane: 4.24 m/s at pull 0.99, 4.16 at 0.995, 4.19 at 1.0); with the ball
//!   resting on the tip the launch is monotone in pull depth. At rest the rod is locked, so
//!   a resting ball is never pushed. While the plunger's coil is on, the rod
//!   is held at `auto_pull` (auto-plunger); releasing the coil fires it.
//! * **Nudge.** [`MechSet::nudge`]`(Δv)` is a cabinet velocity change; at
//!   the next `pre_step` every ball's velocity changes by `−Δv` (the table
//!   frame accelerates under it) and the tilt bob is nudged by `Δv`.
//!
//! Nothing called per tick allocates, locks or does I/O; iteration order is
//! fixed (mechs by index, balls by handle, switches by number).

mod geom;
mod slider;
mod spec;
pub mod spinner;
pub mod tilt;

use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::materials::{Material, MaterialPair, lookup_pair};
use pin_core::schema::table::MechKind;
use pin_core::{PhysicsDefaults, SwitchEdge};

use crate::ball::{
    BallEvent, BallEvents, BallHandle, BallSolver, BodyPoint, Geometry, ImpactTarget, MAX_BALLS,
    MovingBodies,
};
use crate::world::layers::layer_bit;
use crate::world::{LayerTransition, TransitionSet};
use geom::{Cylinder, OBox};
use slider::Slider;
use spec::{KindP, material, resolve};
use spinner::Spinner;

pub use spec::{MechError, MechSpec};
pub use tilt::{TiltBob, TiltBobParams};

/// Most mechs in one [`MechSet`].
pub const MAX_MECHS: usize = 256;

/// Index of the mech set in the engine's body slice
/// (`[&mut flippers, &mut mechs]`).
pub const DEFAULT_BODY_SET: usize = 1;

/// Gap (m) at or below which a ball counts as touching a mech after a step.
pub const CONTACT_GAP: f32 = 1.0e-4;

/// Fastest pull (m/s) of the plunger rod by the hand or the auto-plunger
/// coil (a step in [`MechSet::set_plunger`] is followed at this speed).
pub const PULL_SPEED: f32 = 1.5;

/// A drop-target impulse counts as a face hit when it is within ~45° of the
/// face normal.
const FACE_HIT_COS: f32 = 0.7;

/// How far (m) below the playfield a dropped target starts its rise.
const DROP_HIDE: f32 = 0.002;

/// Height (m) above the scoop floor within which a VUK captures a ball.
const VUK_CAPTURE_HEIGHT: f32 = 0.003;

/// Half size (m) of a VUK's exit transition zone.
const VUK_ZONE: f32 = 0.02;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum DropState {
    Up,
    Down,
    Rising,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct Drop {
    up: OBox,
    threshold: f32,
    rise_speed: f32,
    state: DropState,
    /// Vertical offset (≤ 0) at the start of the tick and the segment speed.
    z: f32,
    vz: f32,
    /// A ball pushed on a face (not the top or an edge) this step.
    face_hit: bool,
}

impl Drop {
    fn body(&self, t: f32) -> OBox {
        let mut b = self.up;
        b.center.z += (self.z + self.vz * t).min(0.0);
        b
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct Vuk {
    position: Vec3,
    dir: Vec3,
    radius: f32,
    eject_speed: f32,
    kick_speed: f32,
    exit_layer: u8,
    exit_offset: Vec3,
    zone: LayerTransition,
    held: Option<BallHandle>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Part {
    None,
    Bumper {
        ring: Cylinder,
        kick_speed: f32,
        kick_reach: f32,
    },
    Slider {
        s: Slider,
        plunger: bool,
        stroke: f32,
        auto_pull: f32,
        /// Current hand (or auto-plunger) pull depth, m.
        pulled: f32,
    },
    Drop(Drop),
    Spinner(Spinner),
    Vuk(Vuk),
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct Mech {
    kind: MechKind,
    coil: Option<CoilId>,
    switch: Option<SwitchId>,
    layer: u8,
    mask: u8,
    material: Material,
    part: Part,
    /// Remaining coil pulse, ns.
    coil_ns: u64,
    coil_held: bool,
    /// Received an impulse during the current step.
    touched: bool,
    /// Switch state after the last step.
    closed: bool,
}

impl Mech {
    fn coil_on(&self) -> bool {
        self.coil_held || self.coil_ns > 0
    }
}

/// The playfield mechs and the cabinet tilt bob (see the [module
/// documentation](self)).
pub struct MechSet {
    mechs: Vec<Mech>,
    pairs: Vec<MaterialPair>,
    tilt: TiltBob,
    tilt_switch: Option<SwitchId>,
    body_set: usize,
    pending_nudge: Vec3,
    /// Plunger hand input, 0..1.
    plunger: f32,
    edges: Vec<(SwitchId, SwitchEdge)>,
    states: [u64; 4],
    own: [u64; 4],
    transitions: TransitionSet,
    /// Events of the current step were lost (full `BallEvents`).
    events_lost: bool,
}

fn set_bit(bits: &mut [u64; 4], s: SwitchId) {
    let n = s.index();
    if n < 256 {
        bits[n / 64] |= 1u64 << (n % 64);
    }
}

impl MechSet {
    /// Builds the set from `specs` (one per `[[mechs]]` entry, in manifest
    /// order; see [`MechSpec`] for the input type and units) with the
    /// `[mechs.*]` defaults, material pairs and tilt-bob parameters of
    /// `defaults`. Setup-time only (allocates).
    ///
    /// # Errors
    /// [`MechError`] for a non-finite position, an out-of-range parameter, or
    /// more than [`MAX_MECHS`] specs.
    pub fn new(specs: &[MechSpec], defaults: &PhysicsDefaults) -> Result<MechSet, MechError> {
        if specs.len() > MAX_MECHS {
            return Err(MechError::TooMany);
        }
        let mut mechs = Vec::with_capacity(specs.len());
        let mut zones = Vec::new();
        let mut own = [0u64; 4];
        for (i, spec) in specs.iter().enumerate() {
            let (c, kp) = resolve(i, spec, defaults)?;
            let pos = spec.position;
            let u = c.dir;
            let side = Vec3::Z.cross(u);
            let part = match kp {
                KindP::Bumper(b) => Part::Bumper {
                    ring: Cylinder {
                        base: pos,
                        radius: b.radius,
                        height: b.height,
                    },
                    kick_speed: b.kick_speed,
                    kick_reach: b.kick_reach,
                },
                KindP::Sling(p) | KindP::Kickback(p) | KindP::Plunger(p) => {
                    let plunger = matches!(kp, KindP::Plunger(_));
                    let rest_box = OBox {
                        center: pos + Vec3::Z * (0.5 * p.height),
                        axes: [u, side, Vec3::Z],
                        half: Vec3::new(0.5 * p.thickness, 0.5 * p.length, 0.5 * p.height),
                    };
                    let range = if plunger {
                        (-p.stroke, 0.0)
                    } else {
                        (0.0, p.stroke)
                    };
                    let mut s = Slider::new(rest_box, u, range, 0.0);
                    s.mass = p.mass;
                    s.spring_k = p.spring_k;
                    s.force = p.force;
                    Part::Slider {
                        s,
                        plunger,
                        stroke: p.stroke,
                        auto_pull: p.auto_pull,
                        pulled: 0.0,
                    }
                }
                KindP::Drop(d) => Part::Drop(Drop {
                    up: OBox {
                        center: pos + Vec3::Z * (0.5 * d.height),
                        axes: [u, side, Vec3::Z],
                        half: Vec3::new(0.5 * d.thickness, 0.5 * d.length, 0.5 * d.height),
                    },
                    threshold: d.threshold,
                    rise_speed: d.rise_speed,
                    state: DropState::Up,
                    z: 0.0,
                    vz: 0.0,
                    face_hit: false,
                }),
                KindP::Spinner(p) => {
                    let mut s = Spinner::new(pos + Vec3::Z * p.pivot_height, u);
                    s.width = p.width;
                    s.flap = p.flap;
                    s.thickness = p.thickness;
                    s.inertia = p.inertia;
                    s.damping = p.damping;
                    s.friction = p.friction;
                    Part::Spinner(s)
                }
                KindP::Vuk(v) => {
                    let exit = pos + v.exit_offset;
                    let zone = LayerTransition {
                        from_layer: c.layer,
                        to_layer: v.exit_layer,
                        min: exit - Vec3::splat(VUK_ZONE),
                        max: exit + Vec3::splat(VUK_ZONE),
                    };
                    zones.push(zone);
                    Part::Vuk(Vuk {
                        position: pos,
                        dir: u,
                        radius: v.radius,
                        eject_speed: v.eject_speed,
                        kick_speed: v.kick_speed,
                        exit_layer: v.exit_layer,
                        exit_offset: v.exit_offset,
                        zone,
                        held: None,
                    })
                }
                KindP::Inert => Part::None,
            };
            if let Some(s) = spec.switch {
                set_bit(&mut own, s);
            }
            mechs.push(Mech {
                kind: spec.kind,
                coil: spec.coil,
                switch: spec.switch,
                layer: c.layer,
                mask: layer_bit(c.layer),
                material: material(spec.kind),
                part,
                coil_ns: 0,
                coil_held: false,
                touched: false,
                closed: false,
            });
        }
        let edges = Vec::with_capacity(2 * (mechs.len() + 1));
        Ok(MechSet {
            mechs,
            pairs: defaults.material_pairs.clone(),
            tilt: TiltBob::new(TiltBobParams::from_defaults(defaults)),
            tilt_switch: None,
            body_set: DEFAULT_BODY_SET,
            pending_nudge: Vec3::ZERO,
            plunger: 0.0,
            edges,
            states: [0; 4],
            own,
            transitions: TransitionSet::new(zones),
            events_lost: false,
        })
    }

    /// Number of mechs (the body count).
    #[must_use]
    pub fn len(&self) -> usize {
        self.mechs.len()
    }

    /// Whether the set has no mechs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mechs.is_empty()
    }

    /// Kind of mech `index`.
    #[must_use]
    pub fn kind(&self, index: usize) -> Option<MechKind> {
        self.mechs.get(index).map(|m| m.kind)
    }

    /// The tilt switch (the `tilt` cabinet switch); `None` (default) keeps the
    /// bob out of [`switch_states`](Self::switch_states) and the edges.
    pub fn set_tilt_switch(&mut self, switch: Option<SwitchId>) {
        if let Some(old) = self.tilt_switch {
            let n = old.index();
            if n < 256 && !self.mechs.iter().any(|m| m.switch == Some(old)) {
                self.own[n / 64] &= !(1u64 << (n % 64));
                self.states[n / 64] &= !(1u64 << (n % 64));
            }
        }
        self.tilt_switch = switch;
        if let Some(s) = switch {
            set_bit(&mut self.own, s);
        }
    }

    /// Replaces the tilt bob (e.g. other parameters or a fresh one).
    pub fn set_tilt_bob(&mut self, bob: TiltBob) {
        self.tilt = bob;
    }

    /// Index of this set in the body slice of
    /// [`BallSolver::step_with_bodies`] (default [`DEFAULT_BODY_SET`]); used
    /// to recognise impacts on the drop targets.
    pub fn set_body_set(&mut self, set: usize) {
        self.body_set = set;
    }

    /// See [`set_body_set`](Self::set_body_set).
    #[must_use]
    pub fn body_set(&self) -> usize {
        self.body_set
    }

    /// Pulses `coil` for `duration_ms` (every mech it drives). A pulse that
    /// is already longer is kept; 0 ms does nothing; unknown coils are
    /// ignored.
    pub fn fire_coil(&mut self, coil: CoilId, duration_ms: u16) {
        let ns = u64::from(duration_ms) * 1_000_000;
        for m in &mut self.mechs {
            if m.coil == Some(coil) {
                m.coil_ns = m.coil_ns.max(ns);
            }
        }
    }

    /// Holds `coil` on (`true`) until released (`false`, which also ends a
    /// running pulse).
    pub fn hold_coil(&mut self, coil: CoilId, on: bool) {
        for m in &mut self.mechs {
            if m.coil == Some(coil) {
                m.coil_held = on;
                if !on {
                    m.coil_ns = 0;
                }
            }
        }
    }

    /// Whether `coil` is on (pulsed or held) for the next tick.
    #[must_use]
    pub fn coil_on(&self, coil: CoilId) -> bool {
        self.mechs
            .iter()
            .any(|m| m.coil == Some(coil) && m.coil_on())
    }

    /// Analog plunger input: 0 = rest .. 1 = full pull (clamped; NaN = 0).
    /// Applies to every plunger.
    pub fn set_plunger(&mut self, position: f32) {
        self.plunger = if position.is_nan() {
            0.0
        } else {
            position.clamp(0.0, 1.0)
        };
    }

    /// Cabinet velocity change (m/s, solver frame), applied at the next
    /// [`pre_step`](Self::pre_step). Several nudges between ticks add.
    /// Non-finite input is ignored.
    pub fn nudge(&mut self, impulse_m_s: Vec3) {
        if impulse_m_s.is_finite() {
            self.pending_nudge += impulse_m_s;
        }
    }

    /// The tilt bob.
    #[must_use]
    pub fn tilt_bob(&self) -> &TiltBob {
        &self.tilt
    }

    /// Offset of the tilt bob from rest (perpendicular to gravity), m.
    #[must_use]
    pub fn tilt_bob_offset(&self) -> Vec3 {
        self.tilt.offset()
    }

    /// Whether the bob touched its ring during the last tick.
    #[must_use]
    pub fn tilt_closed(&self) -> bool {
        self.tilt.closed()
    }

    /// Spinner pulses (one per 180°) of mech `index` so far; 0 for other
    /// kinds.
    #[must_use]
    pub fn spinner_pulses(&self, index: usize) -> u64 {
        match self.mechs.get(index).map(|m| &m.part) {
            Some(Part::Spinner(s)) => s.pulses,
            _ => 0,
        }
    }

    /// Spinner angle (rad, unwrapped, 0 = hanging) and angular speed
    /// (rad/s) of mech `index`.
    #[must_use]
    pub fn spinner_state(&self, index: usize) -> Option<(f32, f32)> {
        match self.mechs.get(index).map(|m| &m.part) {
            Some(Part::Spinner(s)) => Some((s.angle, s.omega)),
            _ => None,
        }
    }

    /// Sets the spinner's angular speed (rad/s), e.g. for tests.
    pub fn set_spinner_speed(&mut self, index: usize, omega: f32) {
        if let Some(Part::Spinner(s)) = self.mechs.get_mut(index).map(|m| &mut m.part)
            && omega.is_finite()
        {
            s.omega = omega;
        }
    }

    /// Slider displacement (m) and velocity (m/s) of a sling, kickback
    /// (`0..stroke`, toward the kick) or plunger (`−stroke..0`, forward).
    #[must_use]
    pub fn slider_state(&self, index: usize) -> Option<(f32, f32)> {
        match self.mechs.get(index).map(|m| &m.part) {
            Some(Part::Slider { s, .. }) => Some((s.x, s.v)),
            _ => None,
        }
    }

    /// Whether drop target `index` is down (or still rising).
    #[must_use]
    pub fn drop_target_down(&self, index: usize) -> Option<bool> {
        match self.mechs.get(index).map(|m| &m.part) {
            Some(Part::Drop(d)) => Some(d.state != DropState::Up),
            _ => None,
        }
    }

    /// The ball VUK `index` holds.
    #[must_use]
    pub fn vuk_ball(&self, index: usize) -> Option<BallHandle> {
        match self.mechs.get(index).map(|m| &m.part) {
            Some(Part::Vuk(v)) => v.held,
            _ => None,
        }
    }

    /// The VUK exit zones (one per VUK, in mech order), for the table's
    /// transition handling.
    #[must_use]
    pub fn transitions(&self) -> &TransitionSet {
        &self.transitions
    }

    /// Whether mech `index`'s switch was closed after the last tick.
    #[must_use]
    pub fn mech_switch_closed(&self, index: usize) -> bool {
        self.mechs.get(index).is_some_and(|m| m.closed)
    }

    /// Sets bit `switch` of `out` for every closed mech switch and the tilt
    /// switch. Only this set's own switch bits are cleared first.
    pub fn switch_states(&self, out: &mut [u64; 4]) {
        for (o, (own, st)) in out.iter_mut().zip(self.own.iter().zip(self.states.iter())) {
            *o = (*o & !own) | (st & own);
        }
    }

    /// Switch edges of the last tick, ascending switch number.
    #[must_use]
    pub fn edges(&self) -> &[(SwitchId, SwitchEdge)] {
        &self.edges
    }

    /// Restitution of a ball of `ball` material on a mech of `mech`
    /// material (pair table; the ball profile's own value without a pair).
    fn restitution(&self, ball: Material, mech: Material, fallback: f32) -> f32 {
        lookup_pair(&self.pairs, ball, mech).map_or(fallback, |p| p.restitution)
    }

    /// Before the ball step: nudge, tilt bob, bumper kicks, VUK eject/hold.
    pub fn pre_step(&mut self, solver: &mut BallSolver, dt: f32) {
        self.edges.clear();
        if dt.is_nan() || dt <= 0.0 || !dt.is_finite() {
            return;
        }
        let nudge = self.pending_nudge;
        self.pending_nudge = Vec3::ZERO;
        let gravity = solver.config().gravity;
        self.tilt.set_gravity(gravity);
        if gravity.is_finite() {
            for m in &mut self.mechs {
                if let Part::Spinner(s) = &mut m.part {
                    s.gravity = gravity;
                }
            }
        }
        self.tilt.nudge(nudge);
        self.tilt.step(dt);
        if nudge != Vec3::ZERO {
            for i in 0..MAX_BALLS {
                if let Some(b) = solver.ball_mut(BallHandle(i as u8)) {
                    b.velocity -= nudge;
                }
            }
        }
        for mi in 0..self.mechs.len() {
            let m = self.mechs[mi];
            self.mechs[mi].touched = false;
            if let Part::Drop(d) = &mut self.mechs[mi].part {
                d.face_hit = false;
            }
            match m.part {
                Part::Bumper {
                    ring,
                    kick_speed,
                    kick_reach,
                } if m.coil_on() => {
                    self.kick_bumper(solver, &m, ring, kick_speed, kick_reach);
                }
                Part::Vuk(mut v) => {
                    if let Some(h) = v.held {
                        if solver.ball(h).is_none() {
                            v.held = None;
                        } else if m.coil_on() {
                            eject(solver, &v, h);
                            v.held = None;
                        } else {
                            hold(solver, &v, h);
                        }
                    }
                    self.mechs[mi].part = Part::Vuk(v);
                }
                _ => {}
            }
        }
    }

    fn kick_bumper(
        &self,
        solver: &mut BallSolver,
        m: &Mech,
        ring: Cylinder,
        speed: f32,
        reach: f32,
    ) {
        for i in 0..MAX_BALLS {
            let h = BallHandle(i as u8);
            let Some(b) = solver.ball(h).copied() else {
                continue;
            };
            if layer_bit(b.layer) & m.mask == 0 {
                continue;
            }
            let Some(prof) = solver.profiles().get(b.profile.index()) else {
                continue;
            };
            let r = prof.radius_m();
            let z = b.position.z - ring.base.z;
            if z < -r || z > ring.height + r {
                continue;
            }
            let (n, rho) = ring.radial(b.position);
            if rho > ring.radius + r + reach {
                continue;
            }
            let vr = b.velocity.dot(n);
            let e = self.restitution(prof.material, m.material, prof.restitution);
            // A ball still closing in on the ring rebounds first (the skirt
            // is part of the ring's contact zone): u = −e·v_r.
            let u = if vr < 0.0 { -e * vr } else { vr };
            if u >= speed {
                if vr < 0.0
                    && let Some(bm) = solver.ball_mut(h)
                {
                    bm.velocity += n * (u - vr);
                }
                continue;
            }
            let out = speed + e * (speed - u);
            if let Some(bm) = solver.ball_mut(h) {
                bm.velocity += n * (out - vr);
            }
        }
    }

    /// Tells the next [`post_step`](Self::post_step) that some of this
    /// step's events were lost (`BallEvents::dropped` grew): every standing
    /// drop target that a ball touched this step then drops, whatever the
    /// speed (the impact speed is unknown). [`step`](Self::step) does this
    /// itself.
    pub fn note_events_dropped(&mut self) {
        self.events_lost = true;
    }

    /// After the ball step: drop-target hits from `events` (this tick's
    /// events only: clear the `BallEvents` every tick, or pass the slice
    /// appended by this step), VUK captures, switch states and edges, coil
    /// timers.
    pub fn post_step(&mut self, solver: &mut BallSolver, events: &[BallEvent], dt: f32) {
        if core::mem::take(&mut self.events_lost) {
            for m in &mut self.mechs {
                if let Part::Drop(d) = &mut m.part
                    && d.face_hit
                    && d.state == DropState::Up
                {
                    d.state = DropState::Down;
                    d.z = -(d.up.half.z * 2.0 + DROP_HIDE);
                    d.vz = 0.0;
                }
            }
        }
        for e in events {
            if let BallEvent::Impact {
                other: ImpactTarget::Body { set, body },
                normal_speed,
                ..
            } = *e
                && set == self.body_set
                && let Some(Mech {
                    part: Part::Drop(d),
                    ..
                }) = self.mechs.get_mut(body)
                && d.state == DropState::Up
                && d.face_hit
                && normal_speed >= d.threshold
            {
                d.state = DropState::Down;
                d.z = -(d.up.half.z * 2.0 + DROP_HIDE);
                d.vz = 0.0;
            }
        }
        for mi in 0..self.mechs.len() {
            let closed = match self.mechs[mi].part {
                Part::None => false,
                Part::Bumper { .. } | Part::Slider { .. } => {
                    self.mechs[mi].touched || self.touching(solver, mi)
                }
                Part::Drop(d) => d.state != DropState::Up,
                Part::Spinner(s) => s.pulsed,
                Part::Vuk(mut v) => {
                    if v.held.is_some_and(|h| solver.ball(h).is_none()) {
                        v.held = None;
                    }
                    if v.held.is_none() {
                        v.held = self.capture(solver, &v, self.mechs[mi].layer);
                    }
                    if let Some(h) = v.held {
                        hold(solver, &v, h);
                    }
                    self.mechs[mi].part = Part::Vuk(v);
                    v.held.is_some()
                }
            };
            self.mechs[mi].closed = closed;
        }
        // Switch states and edges.
        let mut states = [0u64; 4];
        for m in &self.mechs {
            if m.closed
                && let Some(s) = m.switch
            {
                set_bit(&mut states, s);
            }
        }
        if self.tilt.closed()
            && let Some(s) = self.tilt_switch
        {
            set_bit(&mut states, s);
        }
        self.edges.clear();
        for (w, (&now, (&was, &own))) in states
            .iter()
            .zip(self.states.iter().zip(self.own.iter()))
            .enumerate()
        {
            let mut changed = (now ^ was) & own;
            while changed != 0 {
                let b = changed.trailing_zeros();
                changed &= changed - 1;
                let id = SwitchId((w * 64) as u16 + b as u16);
                let edge = if now & (1u64 << b) != 0 {
                    SwitchEdge::Closed
                } else {
                    SwitchEdge::Opened
                };
                if self.edges.len() < self.edges.capacity() {
                    self.edges.push((id, edge));
                }
            }
        }
        self.states = states;
        // Coil pulses.
        if dt.is_finite() && dt > 0.0 {
            let ns = (f64::from(dt) * 1.0e9).round() as u64;
            for m in &mut self.mechs {
                m.coil_ns = m.coil_ns.saturating_sub(ns);
            }
        }
    }

    /// Whether a ball touches mech `mi` (gap ≤ [`CONTACT_GAP`]) now.
    fn touching(&self, solver: &BallSolver, mi: usize) -> bool {
        let mask = self.mechs[mi].mask;
        solver.balls().any(|(_, b)| {
            layer_bit(b.layer) & mask != 0
                && solver.profiles().get(b.profile.index()).is_some_and(|p| {
                    self.closest_point(mi, 0.0, b.position)
                        .is_some_and(|c| c.distance - p.radius_m() <= CONTACT_GAP)
                })
        })
    }

    /// The first ball (handle order) in VUK `v`'s capture zone.
    fn capture(&self, solver: &BallSolver, v: &Vuk, layer: u8) -> Option<BallHandle> {
        let taken = |h: BallHandle| {
            self.mechs
                .iter()
                .any(|m| matches!(m.part, Part::Vuk(o) if o.held == Some(h)))
        };
        solver.balls().find_map(|(h, b)| {
            let r = solver.profiles().get(b.profile.index())?.radius_m();
            let d = b.position - v.position;
            let horizontal = libm::sqrtf(d.x * d.x + d.y * d.y);
            let hold = v.position + Vec3::Z * r;
            // The hold point must be free: no other ball overlaps it (the
            // capture moves the ball there).
            let free = solver.balls().all(|(o, ob)| {
                o == h
                    || solver
                        .profiles()
                        .get(ob.profile.index())
                        .is_none_or(|p| (ob.position - hold).length() >= r + p.radius_m())
            });
            (b.layer == layer
                && horizontal <= v.radius
                && d.z <= r + VUK_CAPTURE_HEIGHT
                && d.z >= 0.0
                && !taken(h)
                && free)
                .then_some(h)
        })
    }

    /// One tick: [`pre_step`](Self::pre_step), the ball step (into
    /// `events`: clear it before each tick; a full list loses impacts, see
    /// [`note_events_dropped`](Self::note_events_dropped)) with
    /// `flippers` (set 0, the mechs set 1) or with the mechs alone (set 0),
    /// and [`post_step`](Self::post_step) on this step's events.
    pub fn step(
        &mut self,
        solver: &mut BallSolver,
        geo: &Geometry<'_>,
        flippers: Option<&mut dyn MovingBodies>,
        dt: f32,
        events: &mut BallEvents,
    ) {
        self.pre_step(solver, dt);
        let first = events.as_slice().len();
        let lost = events.dropped();
        match flippers {
            Some(f) => {
                self.body_set = 1;
                solver.step_with_bodies(geo, &mut [f, self], dt, events);
            }
            None => {
                self.body_set = 0;
                solver.step_with_bodies(geo, &mut [self], dt, events);
            }
        }
        if events.dropped() > lost {
            self.events_lost = true;
        }
        let n = events.as_slice().len();
        let slice = &events.as_slice()[first.min(n)..];
        self.post_step(solver, slice, dt);
    }
}

/// Pins the held ball on the scoop floor.
fn hold(solver: &mut BallSolver, v: &Vuk, h: BallHandle) {
    let r = solver
        .ball(h)
        .and_then(|b| solver.profiles().get(b.profile.index()))
        .map_or(0.0, pin_core::BallProfile::radius_m);
    if let Some(b) = solver.ball_mut(h) {
        b.position = v.position + Vec3::Z * r;
        b.velocity = Vec3::ZERO;
        b.angular_velocity = Vec3::ZERO;
    }
}

/// Ejects the held ball to the exit point and layer.
fn eject(solver: &mut BallSolver, v: &Vuk, h: BallHandle) {
    if let Some(b) = solver.ball_mut(h) {
        let from = b.position;
        let to = v.position + v.exit_offset;
        b.position = to;
        b.velocity = Vec3::Z * v.eject_speed + v.dir * v.kick_speed;
        b.angular_velocity = Vec3::ZERO;
        // Through the scoop's zone; a zone that cannot fire (exit point
        // inside the hold zone) still moves the ball to its exit layer.
        let via_zone = zone_update(&v.zone, b.layer, from, to);
        b.layer = if via_zone == b.layer {
            v.exit_layer
        } else {
            via_zone
        };
    }
}

/// [`TransitionSet::update`] for one zone.
fn zone_update(zone: &LayerTransition, layer: u8, from: Vec3, to: Vec3) -> u8 {
    if zone.from_layer == layer && zone.contains(to) && !zone.contains(from) {
        zone.to_layer
    } else {
        layer
    }
}

impl MovingBodies for MechSet {
    fn body_count(&self) -> usize {
        self.mechs.len()
    }

    fn begin_tick(&mut self, dt: f32) {
        let pull = self.plunger;
        for m in &mut self.mechs {
            let on = m.coil_on();
            match &mut m.part {
                Part::Slider {
                    s,
                    plunger,
                    stroke,
                    auto_pull,
                    pulled,
                } => {
                    if *plunger {
                        // The hand (or the auto-plunger coil) pulls the rod
                        // back at most at PULL_SPEED; letting go is instant.
                        let depth = if on { pull.max(*auto_pull) } else { pull };
                        let target = depth * *stroke;
                        *pulled = if target > *pulled {
                            target.min(*pulled + PULL_SPEED * dt)
                        } else {
                            target
                        };
                        s.hand = if *pulled > 0.0 { -*pulled } else { s.hi };
                    } else {
                        s.coil_on = on;
                    }
                    s.begin_tick(dt);
                }
                Part::Spinner(s) => s.begin_tick(dt),
                Part::Drop(d) => {
                    if on && d.state == DropState::Down {
                        d.state = DropState::Rising;
                    }
                    d.vz = if d.state == DropState::Rising {
                        d.rise_speed.min(-d.z / dt).max(0.0)
                    } else {
                        0.0
                    };
                }
                _ => {}
            }
        }
    }

    fn end_tick(&mut self, dt: f32) {
        for m in &mut self.mechs {
            match &mut m.part {
                Part::Slider { s, .. } => s.end_tick(dt),
                Part::Spinner(s) => s.end_tick(dt),
                Part::Drop(d) => {
                    if d.state == DropState::Rising {
                        d.z = (d.z + d.vz * dt).min(0.0);
                        if d.z >= 0.0 {
                            d.z = 0.0;
                            d.state = DropState::Up;
                        }
                    }
                    d.vz = 0.0;
                }
                _ => {}
            }
        }
    }

    fn layer_mask(&self, body: usize) -> u8 {
        self.mechs[body].mask
    }

    fn material(&self, body: usize) -> Material {
        self.mechs[body].material
    }

    fn swept_bounds(&self, body: usize) -> (Vec3, Vec3) {
        match &self.mechs[body].part {
            Part::Bumper { ring, .. } => ring.aabb(),
            Part::Slider { s, .. } => s.swept_bounds(),
            Part::Drop(d) => {
                let (lo, hi) = d.up.aabb();
                (lo - Vec3::Z * (2.0 * d.up.half.z + DROP_HIDE), hi)
            }
            Part::Spinner(s) => s.swept_bounds(),
            Part::None | Part::Vuk(_) => (Vec3::ZERO, Vec3::ZERO),
        }
    }

    fn max_surface_speed(&self, body: usize) -> f32 {
        match &self.mechs[body].part {
            Part::Slider { s, .. } => s.max_surface_speed(),
            Part::Spinner(s) => s.max_surface_speed(),
            Part::Drop(d) => d.vz,
            _ => 0.0,
        }
    }

    fn closest_point(&self, body: usize, time: f32, point: Vec3) -> Option<BodyPoint> {
        match &self.mechs[body].part {
            Part::Bumper { ring, .. } => Some(ring.closest(point)),
            Part::Slider { s, .. } => Some(s.closest(time, point)),
            Part::Drop(d) => match d.state {
                DropState::Down => None,
                _ => Some(d.body(time).closest(point)),
            },
            Part::Spinner(s) => Some(s.closest(time, point)),
            Part::None | Part::Vuk(_) => None,
        }
    }

    fn velocity_at(&self, body: usize, time: f32, point: Vec3) -> Vec3 {
        match &self.mechs[body].part {
            Part::Slider { s, .. } => s.velocity(time),
            Part::Spinner(s) => s.velocity(point),
            Part::Drop(d) if d.z + d.vz * time < 0.0 => Vec3::Z * d.vz,
            _ => Vec3::ZERO,
        }
    }

    fn inverse_mass_at(&self, body: usize, time: f32, point: Vec3, direction: Vec3) -> f32 {
        match &self.mechs[body].part {
            Part::Slider { s, .. } => s.inverse_mass(time, direction),
            Part::Spinner(s) => s.inverse_mass(point, direction),
            _ => 0.0,
        }
    }

    fn apply_impulse(&mut self, body: usize, time: f32, point: Vec3, impulse: Vec3) {
        let m = &mut self.mechs[body];
        m.touched = true;
        match &mut m.part {
            Part::Slider { s, .. } => s.apply_impulse(time, impulse),
            Part::Spinner(s) => s.apply_impulse(time, point, impulse),
            Part::Drop(d) => {
                // Only a push on a face drops a target (not a ball landing
                // on its top or lifted by it while it rises).
                let along = impulse.dot(d.up.axes[0]).abs();
                if along >= FACE_HIT_COS * impulse.length() {
                    d.face_hit = true;
                }
            }
            _ => {}
        }
    }
}
