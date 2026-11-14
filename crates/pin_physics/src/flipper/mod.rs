//! Torque-driven flipper solver (standard, upper, zipper, Thing Flip).
//!
//! Owner: Subphase 2.3. Stub created by Subphase 1.1.
//!
//! A flipper is a one-degree-of-freedom rigid body (PLAN.md §3.3): a bat
//! turning about `surface_normal` through its pivot, driven by a dual-wound
//! coil against a return spring between two end stops. [`FlipperSet`] holds
//! every flipper of a table and is a [`MovingBodies`] set: the ball solver
//! finds the ball–bat impacts by continuous collision against the moving
//! bat and exchanges a two-body impulse with it, so the ball pushes the bat
//! back as well as the reverse. Nothing here knows about playing techniques;
//! what a flip does to a ball follows from the bat's inertia, the coil
//! torque, the stops and the contact.
//!
//! # Per-tick sequence
//!
//! Once per 1 kHz tick, on the sim thread (nothing here allocates):
//!
//! 1. Inputs, any order: [`FlipperSet::set_energised`] (staging router or a
//!    script coil hold), [`FlipperSet::set_zipper`] (host command queue),
//!    [`FlipperSet::on_switch_edge`] for the switch edges of the previous
//!    tick (Thing Flip triggers).
//! 2. [`FlipperSet::step_tick(tick)`](FlipperSet::step_tick): runs the Thing
//!    Flip schedules for tick `tick`.
//! 3. `BallSolver::step_with_bodies(&geo, &mut [&mut flippers, &mut mechs], dt, &mut events)`
//!    (flippers are set 0). The solver calls [`MovingBodies::begin_tick`]
//!    (coil, spring and stops predict the bat over the tick), queries and
//!    pushes the bats while it moves the balls, then calls
//!    [`MovingBodies::end_tick`] (pose, velocity and EOS switch commit).
//!    Without a ball solver, [`FlipperSet::advance`] does the same two calls.
//! 4. Outputs: [`FlipperSet::eos_edges`] (EOS switch edges of this tick, to
//!    be delivered as switch edges), [`FlipperSet::angle_deg`],
//!    [`FlipperSet::base_offset_mm`] and friends for the frame snapshot.
//!
//! # Model
//!
//! * **Kinematics.** The pivot is `base_position_mm` (plus the zipper
//!   slide); the bat turns about the unit `surface_normal`. Angle 0 points
//!   along the image of world +X under the shortest rotation that takes
//!   world +Z to the normal ([`plane_axes`]), positive angles turn
//!   counter-clockwise seen from the tip of the normal. A flipper on a tilted
//!   plane is therefore the flat flipper rotated by that tilt.
//! * **Coil.** Energised, the coil's target torque is `power_torque_nm`
//!   while the EOS switch is closed and `hold_torque_nm` once it has opened
//!   (at `eos_angle_fraction` of the sweep); released, zero. The torque
//!   follows its target with a first-order lag ([`FlipperTuning`]: slow
//!   build-up, fast collapse). The return spring always pushes toward rest.
//! * **Stops.** Rest and end angle are rigid one-sided stops. A bat that
//!   reaches a stop leaves it with `stop_restitution` times its arrival
//!   speed; pushed against a stop it is immovable in that direction.
//! * **Rubber.** An impact approached at relative normal speed `u` has the
//!   restitution `e(u) = rubber_restitution / (1 + rubber_damping · u / u₀)`
//!   with `u₀` = [`FlipperTuning::rubber_loss_speed_m_s`] (the seam's
//!   `MovingBodies::restitution` hook). Friction comes from the material-pair
//!   table (the bat is [`Material::Rubber`]). The bat reports its true
//!   one-sided inverse mass.
//! * **Zipper.** The pivot slides along `zipper_slide_axis_mm` (direction
//!   and extent) at `zipper_slide_speed_mm_s`; the slide is motor-driven
//!   (the ball cannot push it back).
//! * **Thing Flip.** A closing edge of the trigger switch at tick `T`
//!   energises the flipper for the ticks `T + delay` up to, not including,
//!   `T + delay + hold` (both rounded to whole ticks).

mod shape;

pub use shape::{
    BatMotion, BatPoint, BatShape, STOP_ANGLE_TOLERANCE, StraightCapsule, world_closest_point,
};

use crate::ball::{BodyCast, BodyPoint, MovingBodies};
use crate::flipper_banana::{BananaError, CurvedBanana};
use crate::world::layers;
use arrayvec::ArrayVec;
use glam::{Quat, Vec3};
use pin_core::ids::{FlipperId, SwitchId};
use pin_core::snapshot::MAX_FLIPPERS;
use pin_core::{FlipperConfig, FlipperShape, FlipperStagingMode, Material};

/// Why [`FlipperSet::new`] rejected a configuration.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FlipperError {
    /// The bat shape is not available. No longer returned: every
    /// `FlipperShape` is supported since Subphase 2.6 added `CurvedBanana`
    /// (kept so existing matches still compile).
    ShapeNotSupported {
        /// The flipper.
        id: FlipperId,
    },
    /// More than [`MAX_FLIPPERS`] flippers.
    TooManyFlippers {
        /// How many were given.
        count: usize,
    },
    /// Two flippers share an id.
    DuplicateId {
        /// The id.
        id: FlipperId,
    },
    /// A field is NaN or infinite.
    NonFinite {
        /// The flipper.
        id: FlipperId,
        /// The field's name in `FlipperConfig`.
        field: &'static str,
    },
    /// A field is outside its valid range (e.g. zero inertia, a negative
    /// torque, a restitution above 1, a zero normal, a layer above 7).
    OutOfRange {
        /// The flipper.
        id: FlipperId,
        /// The field's name in `FlipperConfig`.
        field: &'static str,
    },
}

impl core::fmt::Display for FlipperError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            FlipperError::ShapeNotSupported { id } => {
                write!(f, "flipper {}: this bat shape is not supported", id.0)
            }
            FlipperError::TooManyFlippers { count } => {
                write!(f, "{count} flippers, at most {MAX_FLIPPERS} are supported")
            }
            FlipperError::DuplicateId { id } => write!(f, "flipper id {} is used twice", id.0),
            FlipperError::NonFinite { id, field } => {
                write!(f, "flipper {}: `{field}` is not a finite number", id.0)
            }
            FlipperError::OutOfRange { id, field } => {
                write!(f, "flipper {}: `{field}` is out of range", id.0)
            }
        }
    }
}

impl std::error::Error for FlipperError {}

/// Model constants that are not part of `FlipperConfig` (the same for every
/// flipper of a set).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FlipperTuning {
    /// Time constant (seconds) with which the coil torque builds up toward a
    /// higher target (coil inductance and the plunger's stroke-dependent
    /// pull lumped together): `T·(1 − e^(−t/τ))`. 0 = instant.
    pub coil_rise_s: f32,
    /// Time constant (seconds) with which the coil torque collapses toward
    /// a lower target (release, or power → hold at EOS). 0 = instant.
    pub coil_fall_s: f32,
    /// Approach speed (m/s) at which `rubber_damping = 1` halves the
    /// rubber's restitution: an impact approached at `u` (relative normal
    /// speed) has `e(u) = rubber_restitution / (1 + rubber_damping · u / this)`.
    pub rubber_loss_speed_m_s: f32,
    /// Height (metres) of the bat above its mounting plane.
    pub bat_height_m: f32,
    /// Sim ticks per millisecond for the Thing Flip schedule (1 at 1 kHz).
    pub ticks_per_ms: f32,
}

impl Default for FlipperTuning {
    fn default() -> Self {
        Self {
            coil_rise_s: 0.016,
            coil_fall_s: 0.002,
            rubber_loss_speed_m_s: 4.0,
            bat_height_m: 0.030,
            ticks_per_ms: 1.0,
        }
    }
}

/// An EOS switch edge that happened during the last tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EosEdge {
    /// The flipper.
    pub flipper: FlipperId,
    /// Its EOS switch (`FlipperConfig::eos_switch`).
    pub switch: SwitchId,
    /// New state: closed below the EOS angle, open at and beyond it.
    pub closed: bool,
}

/// The in-plane axes of a mounting plane with unit normal `normal`:
/// `(zero, quarter)`, the directions of bat angle 0° and +90°.
///
/// They are world +X and +Y carried along by the shortest rotation that
/// takes world +Z to `normal`; for the one normal without a shortest
/// rotation, −Z, the half turn about world +X is used (`(+X, −Y)`).
#[must_use]
pub fn plane_axes(normal: Vec3) -> (Vec3, Vec3) {
    if normal.z < -0.999_999 {
        return (Vec3::X, Vec3::NEG_Y);
    }
    let q = Quat::from_rotation_arc(Vec3::Z, normal);
    let zero = q * Vec3::X;
    // Re-orthogonalise against the normal so the frame is exact.
    let zero = (zero - normal * zero.dot(normal)).normalize();
    (zero, normal.cross(zero))
}

/// Coil torque difference (N·m) below which the lag has reached its target.
const COIL_TORQUE_EPSILON: f32 = 1.0e-6;

/// The bat shapes a flipper can carry.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Bat {
    Straight(StraightCapsule),
    Banana(CurvedBanana),
}

impl Bat {
    fn shape(&self) -> &dyn BatShape {
        match self {
            Bat::Straight(s) => s,
            Bat::Banana(b) => b,
        }
    }
}

/// Thing Flip schedule of one flipper.
#[derive(Copy, Clone, Debug, PartialEq)]
struct AutoFlip {
    trigger: SwitchId,
    delay_ticks: u64,
    hold_ticks: u64,
    /// `(first energised tick, first released tick)` of the running
    /// schedule.
    pending: Option<(u64, u64)>,
    on: bool,
}

/// One flipper: configuration in SI units and state.
///
/// The travel coordinate `q` runs from 0 (rest stop) to `travel` (end stop)
/// whatever the sign of the sweep; `sign` converts to angles about the
/// normal.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Flipper {
    id: FlipperId,
    eos_switch: SwitchId,
    layer_mask: u8,
    bat: Bat,
    base: Vec3,
    axis: Vec3,
    zero: Vec3,
    quarter: Vec3,
    /// `rest_angle_deg` and `sweep_angle_deg` as configured, so the angle
    /// reported at the stops is exactly the configured one.
    rest_angle_deg: f32,
    sweep_deg: f32,
    rest_angle: f32,
    end_angle: f32,
    sign: f32,
    travel: f32,
    eos_travel: f32,
    inertia: f32,
    power_torque: f32,
    hold_torque: f32,
    spring_torque: f32,
    stop_restitution: f32,
    rubber_restitution: f32,
    rubber_damping: f32,
    slide_dir: Vec3,
    slide_extent: f32,
    slide_speed: f32,
    auto: Option<AutoFlip>,

    // Inputs.
    energised: bool,
    zipper_closed: bool,

    // Committed state (start of tick).
    q: f32,
    omega: f32,
    coil_torque: f32,
    slide: f32,
    eos_closed: bool,
    drive_work: f64,

    // Prediction of the current tick.
    /// Net drive torque of this tick (coil − spring), in travel direction.
    drive: f32,
    /// Its angular acceleration, `drive / inertia`.
    accel: f32,
    /// Start of the current segment: time, travel, and the constant
    /// velocity the bat's surface moves with from there on.
    seg_t: f32,
    seg_q: f32,
    seg_w: f32,
    phase: Phase,
    slide_v: f32,
}

/// What the bat does in the current segment.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Phase {
    /// Moves at constant velocity, clear of the stops until the end of the
    /// tick.
    Free,
    /// Reaches a stop within this tick. The surface moves at the constant
    /// speed that gets it there at the end of the tick (never predicting
    /// more travel than the stop allows), or is already pressed on it;
    /// `speed` is the velocity its momentum corresponds to, which decides
    /// the bounce.
    Arriving { speed: f32 },
    /// Sits on a stop without momentum.
    Held,
}

impl Flipper {
    fn energised(&self) -> bool {
        self.energised || self.auto.is_some_and(|a| a.on)
    }

    fn angle_of(&self, q: f32) -> f32 {
        self.rest_angle + self.sign * q
    }

    /// The bat's motion from the last segment start on.
    fn motion(&self) -> BatMotion {
        BatMotion {
            start: self.seg_t,
            pivot: self.base + self.slide_dir * (self.slide + self.slide_v * self.seg_t),
            pivot_velocity: self.slide_dir * self.slide_v,
            pivot_stop_time: f32::INFINITY,
            axis: self.axis,
            zero: self.zero,
            quarter: self.quarter,
            angle: self.angle_of(self.seg_q),
            angular_velocity: self.sign * self.seg_w,
            angular_acceleration: 0.0,
            angle_min: self.rest_angle.min(self.end_angle),
            angle_max: self.rest_angle.max(self.end_angle),
        }
    }

    /// Travel at `time` of the current tick.
    fn q_at(&self, time: f32) -> f32 {
        (self.seg_q + self.seg_w * (time - self.seg_t).max(0.0)).clamp(0.0, self.travel)
    }

    /// Velocity of the bat's surface (travel direction) at `time`: zero once
    /// a stop holds the bat.
    fn w_at(&self, time: f32) -> f32 {
        let w = self.seg_w;
        let q = self.q_at(time);
        if (w > 0.0 && self.travel - q <= STOP_ANGLE_TOLERANCE)
            || (w < 0.0 && q <= STOP_ANGLE_TOLERANCE)
        {
            0.0
        } else {
            w
        }
    }

    /// The velocity the bat's momentum corresponds to: more than the
    /// limited speed of its surface while a stop is taking up the rest.
    fn momentum_w(&self) -> f32 {
        match self.phase {
            Phase::Free => self.seg_w,
            Phase::Arriving { speed } => speed,
            Phase::Held => 0.0,
        }
    }

    /// Starts a segment at `time` with the bat at `q` and momentum velocity
    /// `w` (this tick's drive included).
    ///
    /// * On a stop and pushing into it: [`Phase::Arriving`] with the surface
    ///   at rest ([`Phase::Held`] without momentum).
    /// * If `w` would carry it to a stop before the end of the tick:
    ///   [`Phase::Arriving`], the surface moving at the constant speed that
    ///   reaches the stop exactly at the end of the tick.
    /// * Otherwise [`Phase::Free`].
    fn start_segment(&mut self, time: f32, q: f32, w: f32, dt: f32) {
        let remaining = dt - time;
        let room = if w > 0.0 { self.travel - q } else { q };
        let speed = w.abs();
        self.seg_t = time;
        self.seg_q = q;
        let (limited, phase) = if speed == 0.0 {
            let on_stop = self.travel - q <= STOP_ANGLE_TOLERANCE || q <= STOP_ANGLE_TOLERANCE;
            (0.0, if on_stop { Phase::Held } else { Phase::Free })
        } else if room <= STOP_ANGLE_TOLERANCE {
            (0.0, Phase::Arriving { speed: w })
        } else if remaining > 0.0 && speed * remaining >= room {
            (room / remaining, Phase::Arriving { speed: w })
        } else {
            (speed, Phase::Free)
        };
        self.seg_w = limited.copysign(w);
        self.phase = phase;
    }

    /// Whether the stop in travel direction `toward` (±1) takes a push in
    /// that direction: the bat is held on it, or is on its way into it
    /// within this tick (the stop already takes up part of its momentum, so
    /// it takes any more as well).
    fn engaged(&self, toward: f32) -> bool {
        match self.phase {
            Phase::Free => false,
            Phase::Arriving { speed } => (speed > 0.0) == (toward > 0.0),
            Phase::Held => {
                if toward > 0.0 {
                    self.travel - self.seg_q <= STOP_ANGLE_TOLERANCE
                } else {
                    self.seg_q <= STOP_ANGLE_TOLERANCE
                }
            }
        }
    }

    fn begin(&mut self, dt: f32, tuning: &FlipperTuning) {
        let target = if !self.energised() {
            0.0
        } else if self.eos_closed {
            self.power_torque
        } else {
            self.hold_torque
        };
        // The coil torque follows its target with a first-order lag.
        let tau = if target > self.coil_torque {
            tuning.coil_rise_s
        } else {
            tuning.coil_fall_s
        };
        let blend = if tau > 0.0 {
            1.0 - libm::expf(-dt / tau)
        } else {
            1.0
        };
        self.coil_torque += (target - self.coil_torque) * blend;
        if (target - self.coil_torque).abs() < COIL_TORQUE_EPSILON {
            self.coil_torque = target;
        }
        self.drive = self.coil_torque - self.spring_torque;
        self.accel = self.drive / self.inertia;
        // The drive acts as one angular impulse at the start of the tick
        // (semi-implicit Euler), so that a ball resting on the bat feels the
        // whole tick's push in one contact solve. Its work is the kinetic
        // energy that impulse adds.
        let kicked = self.omega + self.accel * dt;
        self.start_segment(0.0, self.q, kicked, dt);
        // A bat pressed on its stop takes the impulse without moving: the
        // stop does no work and neither does the drive.
        if self.seg_w != 0.0 || self.omega != 0.0 {
            self.drive_work += 0.5
                * f64::from(self.inertia)
                * (f64::from(kicked) * f64::from(kicked)
                    - f64::from(self.omega) * f64::from(self.omega));
        }

        let goal = if self.zipper_closed {
            self.slide_extent
        } else {
            0.0
        };
        let step = self.slide_speed * dt;
        let move_by = (goal - self.slide).clamp(-step, step);
        self.slide_v = move_by / dt;
    }

    fn end(&mut self, dt: f32) {
        match self.phase {
            Phase::Free => {
                self.q = self.q_at(dt);
                self.omega = self.w_at(dt);
            }
            Phase::Arriving { speed } => {
                // The bat leaves the stop with the stop's restitution,
                // unless the drive presses it back on within a tick (a bat
                // held on its stop arrives with the tick's drive only).
                self.q = if speed > 0.0 { self.travel } else { 0.0 };
                let bounce = -self.stop_restitution * speed;
                self.omega = if bounce.abs() > self.accel.abs() * dt {
                    bounce
                } else {
                    0.0
                };
            }
            Phase::Held => {
                self.q = if self.travel - self.seg_q <= STOP_ANGLE_TOLERANCE {
                    self.travel
                } else {
                    0.0
                };
                self.omega = 0.0;
            }
        }
        self.slide = (self.slide + self.slide_v * dt).clamp(0.0, self.slide_extent);
        self.slide_v = 0.0;
        // Queries between ticks see the committed state, at rest.
        self.accel = 0.0;
        self.seg_t = 0.0;
        self.seg_q = self.q;
        self.seg_w = 0.0;
        self.phase = Phase::Held;
    }

    /// Torque arm, in travel direction, of a unit impulse along `direction`
    /// at `point`.
    fn arm(&self, time: f32, point: Vec3, direction: Vec3) -> f32 {
        let pivot = self.base + self.slide_dir * (self.slide + self.slide_v * time);
        self.sign * (point - pivot).cross(direction).dot(self.axis)
    }
}

/// Every flipper of a table: the state, the drive and the
/// [`MovingBodies`] surface the ball solver collides with (see the
/// [module documentation](self) for the per-tick sequence).
///
/// Body index `i` of the set is the `i`-th configuration given to
/// [`FlipperSet::new`].
#[derive(Clone, Debug)]
pub struct FlipperSet {
    flippers: ArrayVec<Flipper, MAX_FLIPPERS>,
    tuning: FlipperTuning,
    edges: ArrayVec<EosEdge, MAX_FLIPPERS>,
    dt: f32,
}

fn finite(id: FlipperId, field: &'static str, value: f32) -> Result<f32, FlipperError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(FlipperError::NonFinite { id, field })
    }
}

fn in_range(
    id: FlipperId,
    field: &'static str,
    value: f32,
    ok: impl Fn(f32) -> bool,
) -> Result<f32, FlipperError> {
    let value = finite(id, field, value)?;
    if ok(value) {
        Ok(value)
    } else {
        Err(FlipperError::OutOfRange { id, field })
    }
}

fn finite3(id: FlipperId, field: &'static str, v: [f32; 3]) -> Result<Vec3, FlipperError> {
    let v = Vec3::from_array(v);
    if v.is_finite() {
        Ok(v)
    } else {
        Err(FlipperError::NonFinite { id, field })
    }
}

/// Milliseconds → whole ticks (nearest; negative and NaN count as 0).
fn ticks(ms: f32, ticks_per_ms: f32) -> u64 {
    let t = libm::roundf(ms * ticks_per_ms);
    if t > 0.0 { t as u64 } else { 0 }
}

impl Flipper {
    fn from_config(c: &FlipperConfig, tuning: &FlipperTuning) -> Result<Self, FlipperError> {
        let id = c.id;
        let bat = match c.shape {
            FlipperShape::StraightCapsule {
                length_mm,
                base_radius_mm,
                tip_radius_mm,
            } => {
                let length = finite(id, "shape.length_mm", length_mm)? * 1.0e-3;
                let base = finite(id, "shape.base_radius_mm", base_radius_mm)? * 1.0e-3;
                let tip = finite(id, "shape.tip_radius_mm", tip_radius_mm)? * 1.0e-3;
                let capsule = StraightCapsule::new(length, base, tip, 0.0, tuning.bat_height_m)
                    .ok_or(FlipperError::OutOfRange { id, field: "shape" })?;
                Bat::Straight(capsule)
            }
            FlipperShape::CurvedBanana { .. } => Bat::Banana(
                CurvedBanana::from_config(c, tuning.bat_height_m).map_err(|e| match e {
                    BananaError::NonFinite { field } => FlipperError::NonFinite { id, field },
                    BananaError::OutOfRange { field } => FlipperError::OutOfRange { id, field },
                    BananaError::NotBanana => FlipperError::ShapeNotSupported { id },
                })?,
            ),
        };
        let normal = finite3(id, "surface_normal", c.surface_normal)?;
        let axis = normal.try_normalize().ok_or(FlipperError::OutOfRange {
            id,
            field: "surface_normal",
        })?;
        let (zero, quarter) = plane_axes(axis);
        let base = finite3(id, "base_position_mm", c.base_position_mm)? * 1.0e-3;
        let rest = finite(id, "rest_angle_deg", c.rest_angle_deg)?.to_radians();
        let sweep = in_range(id, "sweep_angle_deg", c.sweep_angle_deg, |s| {
            s.abs() <= 360.0
        })?
        .to_radians();
        let fraction = in_range(id, "eos_angle_fraction", c.eos_angle_fraction, |f| {
            (0.0..=1.0).contains(&f)
        })?;
        let non_negative = |v: f32| v >= 0.0;
        let unit = |v: f32| (0.0..=1.0).contains(&v);
        let inertia = in_range(id, "inertia_kg_m2", c.inertia_kg_m2, |v| v > 0.0)?;
        let power_torque = in_range(id, "power_torque_nm", c.power_torque_nm, non_negative)?;
        let hold_torque = in_range(id, "hold_torque_nm", c.hold_torque_nm, non_negative)?;
        let spring_torque = in_range(
            id,
            "return_spring_torque_nm",
            c.return_spring_torque_nm,
            non_negative,
        )?;
        let stop_restitution = in_range(id, "stop_restitution", c.stop_restitution, unit)?;
        let rubber_restitution = in_range(id, "rubber_restitution", c.rubber_restitution, unit)?;
        let rubber_damping = in_range(id, "rubber_damping", c.rubber_damping, non_negative)?;
        if c.collision_layer > 7 {
            return Err(FlipperError::OutOfRange {
                id,
                field: "collision_layer",
            });
        }
        let (slide_dir, slide_extent, slide_speed) = match c.zipper_slide_axis_mm {
            Some(axis_mm) => {
                let v = finite3(id, "zipper_slide_axis_mm", axis_mm)? * 1.0e-3;
                let speed = in_range(
                    id,
                    "zipper_slide_speed_mm_s",
                    c.zipper_slide_speed_mm_s,
                    non_negative,
                )? * 1.0e-3;
                match v.try_normalize() {
                    Some(dir) => (dir, v.length(), speed),
                    None => (Vec3::ZERO, 0.0, 0.0),
                }
            }
            None => (Vec3::ZERO, 0.0, 0.0),
        };
        let auto = match c.staging {
            FlipperStagingMode::AutoSensorThingFlip {
                trigger_switch,
                delay_ms,
                hold_duration_ms,
            } => {
                let delay = in_range(id, "staging.delay_ms", delay_ms, non_negative)?;
                let hold = in_range(
                    id,
                    "staging.hold_duration_ms",
                    hold_duration_ms,
                    non_negative,
                )?;
                Some(AutoFlip {
                    trigger: trigger_switch,
                    delay_ticks: ticks(delay, tuning.ticks_per_ms),
                    hold_ticks: ticks(hold, tuning.ticks_per_ms),
                    pending: None,
                    on: false,
                })
            }
            _ => None,
        };
        let travel = sweep.abs();
        let sign = if sweep < 0.0 { -1.0 } else { 1.0 };
        let eos_travel = travel * fraction;
        Ok(Flipper {
            id,
            eos_switch: c.eos_switch,
            layer_mask: layers::layer_bit(c.collision_layer),
            bat,
            base,
            axis,
            zero,
            quarter,
            rest_angle_deg: c.rest_angle_deg,
            sweep_deg: c.sweep_angle_deg,
            rest_angle: rest,
            end_angle: rest + sign * travel,
            sign,
            travel,
            eos_travel,
            inertia,
            power_torque,
            hold_torque,
            spring_torque,
            stop_restitution,
            rubber_restitution,
            rubber_damping,
            slide_dir,
            slide_extent,
            slide_speed,
            auto,
            energised: false,
            zipper_closed: false,
            q: 0.0,
            omega: 0.0,
            coil_torque: 0.0,
            slide: 0.0,
            eos_closed: eos_is_closed(0.0, eos_travel),
            drive_work: 0.0,
            drive: 0.0,
            accel: 0.0,
            seg_t: 0.0,
            seg_q: 0.0,
            seg_w: 0.0,
            phase: Phase::Held,
            slide_v: 0.0,
        })
    }
}

/// The EOS switch is closed below the EOS travel and open from it on.
fn eos_is_closed(q: f32, eos_travel: f32) -> bool {
    q < eos_travel - STOP_ANGLE_TOLERANCE
}

impl FlipperSet {
    /// Builds the flippers of a table with the default [`FlipperTuning`].
    ///
    /// # Errors
    /// Configurations the solver cannot run: non-finite numbers, a
    /// non-positive inertia, negative torques, restitutions outside
    /// `0..=1`, an EOS fraction outside `0..=1`, a zero normal, a degenerate
    /// bat (for a `CurvedBanana` see
    /// [`CurvedBanana::from_shape`](crate::flipper_banana::CurvedBanana::from_shape)),
    /// a layer above 7, duplicate ids, more than [`MAX_FLIPPERS`] flippers.
    pub fn new(configs: &[FlipperConfig]) -> Result<FlipperSet, FlipperError> {
        Self::with_tuning(configs, FlipperTuning::default())
    }

    /// As [`FlipperSet::new`] with explicit model constants.
    ///
    /// # Errors
    /// As [`FlipperSet::new`]; a non-finite or negative tuning value is
    /// reported as [`FlipperError::OutOfRange`] with a `tuning.*` field name
    /// on the first flipper (or `FlipperId(0)` when there is none).
    pub fn with_tuning(
        configs: &[FlipperConfig],
        tuning: FlipperTuning,
    ) -> Result<FlipperSet, FlipperError> {
        if configs.len() > MAX_FLIPPERS {
            return Err(FlipperError::TooManyFlippers {
                count: configs.len(),
            });
        }
        let first = configs.first().map_or(FlipperId(0), |c| c.id);
        let positive = |v: f32| v > 0.0;
        let non_negative = |v: f32| v >= 0.0;
        in_range(
            first,
            "tuning.coil_rise_s",
            tuning.coil_rise_s,
            non_negative,
        )?;
        in_range(
            first,
            "tuning.coil_fall_s",
            tuning.coil_fall_s,
            non_negative,
        )?;
        in_range(
            first,
            "tuning.rubber_loss_speed_m_s",
            tuning.rubber_loss_speed_m_s,
            positive,
        )?;
        in_range(first, "tuning.bat_height_m", tuning.bat_height_m, positive)?;
        in_range(first, "tuning.ticks_per_ms", tuning.ticks_per_ms, positive)?;
        let mut flippers = ArrayVec::new();
        for (i, c) in configs.iter().enumerate() {
            if configs[..i].iter().any(|other| other.id == c.id) {
                return Err(FlipperError::DuplicateId { id: c.id });
            }
            flippers.push(Flipper::from_config(c, &tuning)?);
        }
        Ok(FlipperSet {
            flippers,
            tuning,
            edges: ArrayVec::new(),
            dt: 0.0,
        })
    }

    /// The model constants in use.
    #[must_use]
    pub fn tuning(&self) -> &FlipperTuning {
        &self.tuning
    }

    /// Number of flippers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.flippers.len()
    }

    /// True without flippers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.flippers.is_empty()
    }

    /// Body index (within this [`MovingBodies`] set) of flipper `id`.
    #[must_use]
    pub fn body_index(&self, id: FlipperId) -> Option<usize> {
        self.flippers.iter().position(|f| f.id == id)
    }

    /// Id of body `body`.
    #[must_use]
    pub fn id_of(&self, body: usize) -> Option<FlipperId> {
        self.flippers.get(body).map(|f| f.id)
    }

    fn get(&self, id: FlipperId) -> Option<&Flipper> {
        self.flippers.iter().find(|f| f.id == id)
    }

    fn get_mut(&mut self, id: FlipperId) -> Option<&mut Flipper> {
        self.flippers.iter_mut().find(|f| f.id == id)
    }

    /// Coil command for flipper `id` (staging router output or a script
    /// coil hold); takes effect from the next tick's prediction. Unknown ids
    /// are ignored.
    pub fn set_energised(&mut self, id: FlipperId, on: bool) {
        if let Some(f) = self.get_mut(id) {
            f.energised = on;
        }
    }

    /// Zipper command for flipper `id`: `closed` slides the base to the end
    /// of `zipper_slide_axis_mm`, open slides it home. Ignored for unknown
    /// ids; without a slide axis nothing moves.
    pub fn set_zipper(&mut self, id: FlipperId, closed: bool) {
        if let Some(f) = self.get_mut(id) {
            f.zipper_closed = closed;
        }
    }

    /// A switch changed at tick `tick`. A closing edge of a Thing Flip
    /// flipper's trigger switch schedules its flip: energised for the ticks
    /// `tick + delay` up to, not including, `tick + delay + hold`. Edges
    /// that arrive while a schedule is waiting or running are ignored.
    pub fn on_switch_edge(&mut self, switch: SwitchId, closed: bool, tick: u64) {
        if !closed {
            return;
        }
        for f in &mut self.flippers {
            if let Some(auto) = &mut f.auto
                && auto.trigger == switch
                && auto.pending.is_none()
            {
                let start = tick.saturating_add(auto.delay_ticks);
                auto.pending = Some((start, start.saturating_add(auto.hold_ticks)));
            }
        }
    }

    /// Runs the Thing Flip schedules for tick `tick`. Call once per tick
    /// after the inputs and before the ball solver's step (see the
    /// [module documentation](self)); `tick` is the number of the tick about
    /// to be simulated and must not decrease.
    pub fn step_tick(&mut self, tick: u64) {
        for f in &mut self.flippers {
            if let Some(auto) = &mut f.auto
                && let Some((start, stop)) = auto.pending
            {
                auto.on = tick >= start && tick < stop;
                if tick >= stop {
                    auto.pending = None;
                }
            }
        }
    }

    /// One tick of the flippers alone: [`MovingBodies::begin_tick`] then
    /// [`MovingBodies::end_tick`], for runs without a ball solver.
    pub fn advance(&mut self, dt: f32) {
        self.begin_tick(dt);
        self.end_tick(dt);
    }

    /// EOS switch edges of the last tick (cleared when the next begins).
    #[must_use]
    pub fn eos_edges(&self) -> &[EosEdge] {
        &self.edges
    }

    /// Absolute bat angle about the surface normal, degrees: exactly
    /// `rest_angle_deg` at rest and `rest_angle_deg + sweep_angle_deg` on
    /// the end stop. NaN for an unknown id.
    #[must_use]
    pub fn angle_deg(&self, id: FlipperId) -> f32 {
        self.get(id).map_or(f32::NAN, |f| {
            if f.q >= f.travel {
                f.rest_angle_deg + f.sweep_deg
            } else {
                f.rest_angle_deg + f.sign * f.q.to_degrees()
            }
        })
    }

    /// Bat angle relative to rest, radians, signed like the sweep
    /// (`FlipperState::angle_rad`). NaN for an unknown id.
    #[must_use]
    pub fn relative_angle_rad(&self, id: FlipperId) -> f32 {
        self.get(id).map_or(f32::NAN, |f| f.sign * f.q)
    }

    /// Angular velocity about the surface normal, rad/s, with the sign
    /// convention of `sweep_angle_deg` (+ = counter-clockwise). NaN for an
    /// unknown id.
    #[must_use]
    pub fn angular_velocity(&self, id: FlipperId) -> f32 {
        self.get(id).map_or(f32::NAN, |f| f.sign * f.omega)
    }

    /// State of the EOS switch: closed from rest up to
    /// `eos_angle_fraction` of the sweep, open beyond. False for an unknown
    /// id.
    #[must_use]
    pub fn eos_closed(&self, id: FlipperId) -> bool {
        self.get(id).is_some_and(|f| f.eos_closed)
    }

    /// Zipper slide of the base along its axis, mm (0 = home). NaN for an
    /// unknown id.
    #[must_use]
    pub fn base_offset_mm(&self, id: FlipperId) -> f32 {
        self.get(id).map_or(f32::NAN, |f| f.slide * 1.0e3)
    }

    /// Whether the coil of flipper `id` is energised (command or Thing
    /// Flip).
    #[must_use]
    pub fn is_energised(&self, id: FlipperId) -> bool {
        self.get(id).is_some_and(Flipper::energised)
    }

    /// Coil torque currently applied, N·m (0 for an unknown id).
    #[must_use]
    pub fn coil_torque_nm(&self, id: FlipperId) -> f32 {
        self.get(id).map_or(0.0, |f| f.coil_torque)
    }

    /// Work coil and spring have done on the bat since construction, joules.
    /// The drive acts as one angular impulse `τ·dt` per tick; its work is
    /// the kinetic energy that impulse adds, `τ·dt·(ω_before + ω_after)/2`
    /// (negative while the drive brakes the bat; at most `τ·Δθ` in a tick
    /// without ball contact). Nothing is counted while the drive only
    /// presses the bat on a stop, including the tick in which a ball knocks
    /// a held bat off it (that tick's hold impulse is left out, so the sum
    /// is an under-count and the bound below is the stricter for it). The
    /// kinetic energy of the bat and the balls
    /// it touched never exceeds what they started with plus this. 0 for an
    /// unknown id.
    #[must_use]
    pub fn drive_work_j(&self, id: FlipperId) -> f64 {
        self.get(id).map_or(0.0, |f| f.drive_work)
    }

    /// Kinetic energy of bat `id`, joules (0 for an unknown id).
    #[must_use]
    pub fn kinetic_energy_j(&self, id: FlipperId) -> f64 {
        self.get(id).map_or(0.0, |f| {
            0.5 * f64::from(f.inertia) * f64::from(f.omega) * f64::from(f.omega)
        })
    }

    /// Pivot position (base plus zipper slide), world space, metres.
    #[must_use]
    pub fn pivot_m(&self, id: FlipperId) -> Option<Vec3> {
        self.get(id).map(|f| f.base + f.slide_dir * f.slide)
    }

    /// Unit vector from the pivot along the bat, world space.
    #[must_use]
    pub fn bat_direction(&self, id: FlipperId) -> Option<Vec3> {
        self.get(id).map(|f| {
            let a = f.angle_of(f.q);
            f.zero * libm::cosf(a) + f.quarter * libm::sinf(a)
        })
    }
}

impl MovingBodies for FlipperSet {
    fn body_count(&self) -> usize {
        self.flippers.len()
    }

    fn begin_tick(&mut self, dt: f32) {
        if !(dt.is_finite() && dt > 0.0) {
            return;
        }
        self.dt = dt;
        self.edges.clear();
        let tuning = self.tuning;
        for f in &mut self.flippers {
            f.begin(dt, &tuning);
        }
    }

    fn end_tick(&mut self, dt: f32) {
        if !(dt.is_finite() && dt > 0.0) {
            return;
        }
        for f in &mut self.flippers {
            f.end(dt);
            let closed = eos_is_closed(f.q, f.eos_travel);
            if closed != f.eos_closed {
                f.eos_closed = closed;
                self.edges.push(EosEdge {
                    flipper: f.id,
                    switch: f.eos_switch,
                    closed,
                });
            }
        }
    }

    fn layer_mask(&self, body: usize) -> u8 {
        self.flippers[body].layer_mask
    }

    fn material(&self, _body: usize) -> Material {
        Material::Rubber
    }

    fn swept_bounds(&self, body: usize) -> (Vec3, Vec3) {
        let f = &self.flippers[body];
        let shape = f.bat.shape();
        let (low, high) = shape.height_range();
        let reach = shape.reach();
        let r = libm::sqrtf(reach * reach + low.abs().max(high.abs()).powi(2));
        let a = f.base + f.slide_dir * f.slide;
        let b = f.base + f.slide_dir * (f.slide + f.slide_v * self.dt);
        (a.min(b) - Vec3::splat(r), a.max(b) + Vec3::splat(r))
    }

    fn max_surface_speed(&self, body: usize) -> f32 {
        let f = &self.flippers[body];
        f.seg_w.abs() * f.bat.shape().reach() + f.slide_v.abs()
    }

    fn closest_point(&self, body: usize, time: f32, point: Vec3) -> Option<BodyPoint> {
        let f = &self.flippers[body];
        Some(world_closest_point(f.bat.shape(), &f.motion(), time, point))
    }

    fn velocity_at(&self, body: usize, time: f32, point: Vec3) -> Vec3 {
        let f = &self.flippers[body];
        let pivot = f.base + f.slide_dir * (f.slide + f.slide_v * time);
        f.axis.cross(point - pivot) * (f.sign * f.w_at(time)) + f.slide_dir * f.slide_v
    }

    fn inverse_mass_at(&self, body: usize, time: f32, point: Vec3, direction: Vec3) -> f32 {
        let f = &self.flippers[body];
        let arm = f.arm(time, point, direction);
        if arm == 0.0 || arm.is_nan() || f.engaged(arm.signum()) {
            return 0.0;
        }
        arm * arm / f.inertia
    }

    /// The rubber's restitution: `rubber_restitution`, reduced with the
    /// approach speed by `rubber_damping`
    /// ([`FlipperTuning::rubber_loss_speed_m_s`]). Replaces the pair table's
    /// value for ball–bat impacts; friction still comes from the pair table.
    fn restitution(
        &self,
        body: usize,
        _time: f32,
        _point: Vec3,
        approach_speed: f32,
    ) -> Option<f32> {
        let f = &self.flippers[body];
        let u = if approach_speed > 0.0 {
            approach_speed
        } else {
            0.0
        };
        Some(
            f.rubber_restitution / (1.0 + f.rubber_damping * u / self.tuning.rubber_loss_speed_m_s),
        )
    }

    fn apply_impulse(&mut self, body: usize, time: f32, point: Vec3, impulse: Vec3) {
        let dt = self.dt;
        let f = &mut self.flippers[body];
        let delta = f.arm(time, point, impulse) / f.inertia;
        if delta == 0.0 || !delta.is_finite() {
            return;
        }
        // A push into a stop that holds the bat, or that the bat is running
        // into, is taken by the stop: the bat keeps its motion and the
        // momentum that decides its bounce.
        if f.engaged(delta.signum()) {
            return;
        }
        // The impulse changes the bat's momentum; the rest of the tick is
        // predicted again from here, under the same drive.
        let q = f.q_at(time);
        let w = f.momentum_w();
        f.start_segment(time, q, w + delta, dt);
    }

    fn cast_sphere(
        &self,
        body: usize,
        time: f32,
        center: Vec3,
        radius: f32,
        velocity: Vec3,
        window: f32,
    ) -> Option<BodyCast> {
        let f = &self.flippers[body];
        f.bat
            .shape()
            .cast_sphere(&f.motion(), time, center, radius, velocity, window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::CabinetInput;
    use pin_core::ids::CoilId;

    const DT: f32 = 1.0e-3;
    const ID: FlipperId = FlipperId(3);

    fn config() -> FlipperConfig {
        FlipperConfig {
            id: ID,
            coil: CoilId(1),
            eos_switch: SwitchId(40),
            collision_layer: 0,
            base_position_mm: [100.0, 50.0, 0.0],
            surface_normal: [0.0, 0.0, 1.0],
            rest_angle_deg: -30.0,
            sweep_angle_deg: 50.0,
            eos_angle_fraction: 0.92,
            shape: FlipperShape::StraightCapsule {
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
            staging: FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
            zipper_slide_axis_mm: None,
            zipper_slide_speed_mm_s: 0.0,
        }
    }

    fn set() -> FlipperSet {
        FlipperSet::new(&[config()]).unwrap()
    }

    fn rejected(edit: impl Fn(&mut FlipperConfig)) -> FlipperError {
        let mut c = config();
        edit(&mut c);
        FlipperSet::new(&[c]).unwrap_err()
    }

    #[test]
    fn banana_is_built_and_its_bad_fields_are_named() {
        let banana = |arc_radius_mm: f32, tube_radius_mm: f32| {
            let mut c = config();
            c.shape = FlipperShape::CurvedBanana {
                length_mm: 80.0,
                arc_radius_mm,
                arc_sweep_deg: 30.0,
                tube_radius_mm,
            };
            FlipperSet::new(&[c])
        };
        let s = banana(100.0, 8.0).unwrap();
        // Reach = chord + tube radius, as for a straight bat of that length.
        assert!((s.max_surface_speed(0) - 0.0).abs() < 1e-9);
        let (lo, hi) = s.swept_bounds(0);
        assert!(((hi - lo).x - 2.0 * libm::sqrtf(0.088 * 0.088 + 0.03 * 0.03)).abs() < 1e-6);
        assert_eq!(
            banana(30.0, 8.0).unwrap_err(),
            FlipperError::OutOfRange {
                id: ID,
                field: "shape.arc_radius_mm"
            }
        );
        assert_eq!(
            banana(100.0, f32::NAN).unwrap_err(),
            FlipperError::NonFinite {
                id: ID,
                field: "shape.tube_radius_mm"
            }
        );
        assert!(
            !FlipperError::ShapeNotSupported { id: ID }
                .to_string()
                .is_empty()
        );
    }

    #[test]
    fn bad_configurations_are_rejected_with_the_field() {
        let field = |e: FlipperError| match e {
            FlipperError::NonFinite { id, field } | FlipperError::OutOfRange { id, field } => {
                assert_eq!(id, ID);
                field
            }
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(field(rejected(|c| c.inertia_kg_m2 = 0.0)), "inertia_kg_m2");
        assert_eq!(field(rejected(|c| c.inertia_kg_m2 = -1.0)), "inertia_kg_m2");
        assert_eq!(
            rejected(|c| c.inertia_kg_m2 = f32::NAN),
            FlipperError::NonFinite {
                id: ID,
                field: "inertia_kg_m2"
            }
        );
        assert_eq!(
            field(rejected(|c| c.power_torque_nm = -0.1)),
            "power_torque_nm"
        );
        assert_eq!(
            field(rejected(|c| c.hold_torque_nm = f32::INFINITY)),
            "hold_torque_nm"
        );
        assert_eq!(
            field(rejected(|c| c.return_spring_torque_nm = -1.0)),
            "return_spring_torque_nm"
        );
        assert_eq!(
            field(rejected(|c| c.stop_restitution = 1.5)),
            "stop_restitution"
        );
        assert_eq!(
            field(rejected(|c| c.rubber_restitution = -0.1)),
            "rubber_restitution"
        );
        assert_eq!(
            field(rejected(|c| c.rubber_damping = -0.1)),
            "rubber_damping"
        );
        assert_eq!(
            field(rejected(|c| c.eos_angle_fraction = 1.2)),
            "eos_angle_fraction"
        );
        assert_eq!(
            field(rejected(|c| c.rest_angle_deg = f32::NAN)),
            "rest_angle_deg"
        );
        assert_eq!(
            field(rejected(|c| c.sweep_angle_deg = 400.0)),
            "sweep_angle_deg"
        );
        assert_eq!(
            field(rejected(|c| c.surface_normal = [0.0; 3])),
            "surface_normal"
        );
        assert_eq!(
            field(rejected(|c| c.surface_normal = [0.0, f32::NAN, 1.0])),
            "surface_normal"
        );
        assert_eq!(
            field(rejected(|c| c.base_position_mm = [f32::INFINITY, 0.0, 0.0])),
            "base_position_mm"
        );
        assert_eq!(
            field(rejected(|c| c.collision_layer = 8)),
            "collision_layer"
        );
        let shape = |length_mm, base_radius_mm, tip_radius_mm| {
            move |c: &mut FlipperConfig| {
                c.shape = FlipperShape::StraightCapsule {
                    length_mm,
                    base_radius_mm,
                    tip_radius_mm,
                }
            }
        };
        assert_eq!(field(rejected(shape(0.0, 11.0, 6.5))), "shape");
        assert_eq!(field(rejected(shape(76.0, -1.0, 6.5))), "shape");
        assert_eq!(
            field(rejected(shape(f32::NAN, 11.0, 6.5))),
            "shape.length_mm"
        );
        assert_eq!(
            field(rejected(shape(76.0, f32::NAN, 6.5))),
            "shape.base_radius_mm"
        );
        assert_eq!(
            field(rejected(shape(76.0, 11.0, f32::INFINITY))),
            "shape.tip_radius_mm"
        );
        assert_eq!(
            field(rejected(|c| {
                c.zipper_slide_axis_mm = Some([f32::NAN, 0.0, 0.0]);
            })),
            "zipper_slide_axis_mm"
        );
        assert_eq!(
            field(rejected(|c| {
                c.zipper_slide_axis_mm = Some([12.0, 0.0, 0.0]);
                c.zipper_slide_speed_mm_s = -5.0;
            })),
            "zipper_slide_speed_mm_s"
        );
        let thing = |delay_ms, hold_duration_ms| {
            move |c: &mut FlipperConfig| {
                c.staging = FlipperStagingMode::AutoSensorThingFlip {
                    trigger_switch: SwitchId(1),
                    delay_ms,
                    hold_duration_ms,
                }
            }
        };
        assert_eq!(field(rejected(thing(-1.0, 10.0))), "staging.delay_ms");
        assert_eq!(
            field(rejected(thing(1.0, f32::NAN))),
            "staging.hold_duration_ms"
        );
        for e in [
            FlipperError::NonFinite { id: ID, field: "x" },
            FlipperError::OutOfRange { id: ID, field: "x" },
            FlipperError::DuplicateId { id: ID },
            FlipperError::TooManyFlippers { count: 17 },
        ] {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn set_level_errors_and_tuning() {
        assert_eq!(
            FlipperSet::new(&[config(), config()]).unwrap_err(),
            FlipperError::DuplicateId { id: ID }
        );
        let many: Vec<FlipperConfig> = (0..17)
            .map(|i| FlipperConfig {
                id: FlipperId(i),
                ..config()
            })
            .collect();
        assert_eq!(
            FlipperSet::new(&many).unwrap_err(),
            FlipperError::TooManyFlippers { count: 17 }
        );
        assert_eq!(FlipperSet::new(&many[..16]).unwrap().len(), 16);
        let empty = FlipperSet::new(&[]).unwrap();
        assert!(empty.is_empty() && empty.body_count() == 0);
        let bad = |edit: fn(&mut FlipperTuning)| {
            let mut t = FlipperTuning::default();
            edit(&mut t);
            FlipperSet::with_tuning(&[config()], t).unwrap_err()
        };
        assert!(matches!(
            bad(|t| t.coil_rise_s = -1.0),
            FlipperError::OutOfRange {
                field: "tuning.coil_rise_s",
                ..
            }
        ));
        assert!(matches!(
            bad(|t| t.coil_fall_s = f32::NAN),
            FlipperError::NonFinite { .. }
        ));
        assert!(matches!(
            bad(|t| t.rubber_loss_speed_m_s = 0.0),
            FlipperError::OutOfRange { .. }
        ));
        assert!(matches!(
            bad(|t| t.bat_height_m = 0.0),
            FlipperError::OutOfRange { .. }
        ));
        assert!(matches!(
            bad(|t| t.ticks_per_ms = 0.0),
            FlipperError::OutOfRange { .. }
        ));
        assert!(matches!(
            FlipperSet::with_tuning(
                &[],
                FlipperTuning {
                    bat_height_m: -1.0,
                    ..FlipperTuning::default()
                }
            ),
            Err(FlipperError::OutOfRange {
                id: FlipperId(0),
                ..
            })
        ));
        // Instant coil: the first tick already has the full power torque.
        let mut s = FlipperSet::with_tuning(
            &[config()],
            FlipperTuning {
                coil_rise_s: 0.0,
                coil_fall_s: 0.0,
                ..FlipperTuning::default()
            },
        )
        .unwrap();
        assert_eq!(s.tuning().coil_rise_s, 0.0);
        s.set_energised(ID, true);
        s.advance(DT);
        assert!((s.coil_torque_nm(ID) - 1.2).abs() < 1e-6);
        assert!((s.angular_velocity(ID) - 11.7).abs() < 1e-3);
        s.set_energised(ID, false);
        s.advance(DT);
        assert_eq!(s.coil_torque_nm(ID), 0.0);
    }

    #[test]
    fn unknown_ids_are_harmless() {
        let mut s = set();
        let nobody = FlipperId(99);
        s.set_energised(nobody, true);
        s.set_zipper(nobody, true);
        s.advance(DT);
        assert!(s.angle_deg(nobody).is_nan());
        assert!(s.angular_velocity(nobody).is_nan());
        assert!(s.base_offset_mm(nobody).is_nan());
        assert!(s.relative_angle_rad(nobody).is_nan());
        assert!(!s.eos_closed(nobody) && !s.is_energised(nobody));
        assert_eq!(s.coil_torque_nm(nobody), 0.0);
        assert_eq!(s.drive_work_j(nobody), 0.0);
        assert_eq!(s.kinetic_energy_j(nobody), 0.0);
        assert!(s.pivot_m(nobody).is_none() && s.bat_direction(nobody).is_none());
        assert_eq!(s.body_index(nobody), None);
        assert_eq!(s.body_index(ID), Some(0));
        assert_eq!(s.id_of(0), Some(ID));
        assert_eq!(s.id_of(1), None);
        assert_eq!(s.angle_deg(ID), -30.0);
        assert_eq!(s.material(0), Material::Rubber);
        assert_eq!(s.layer_mask(0), 1);
    }

    #[test]
    fn bad_time_steps_change_nothing() {
        let mut s = set();
        s.set_energised(ID, true);
        for dt in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            s.advance(dt);
        }
        assert_eq!(s.angle_deg(ID), -30.0);
        assert_eq!(s.coil_torque_nm(ID), 0.0);
    }

    /// World point on top of the bat (the side the end stop is on) at
    /// `along` metres from the pivot, at tick time `time`.
    fn on_top(s: &FlipperSet, time: f32, along: f32) -> (Vec3, Vec3) {
        let f = &s.flippers[0];
        let m = f.motion();
        let probe = m.to_world(time, Vec3::new(along, 0.05, 0.01));
        let cp = s.closest_point(0, time, probe).unwrap();
        (cp.point, cp.normal)
    }

    #[test]
    fn stops_are_one_sided_and_report_their_real_velocity() {
        let mut s = set();
        // At rest the spring presses the bat on the rest stop.
        s.begin_tick(DT);
        let (p, n) = on_top(&s, 0.0, 0.04);
        assert_eq!(s.velocity_at(0, 0.0, p), Vec3::ZERO);
        assert_eq!(s.max_surface_speed(0), 0.0);
        // A push from above goes into the rest stop: immovable. A push from
        // below lifts the bat: the free value r²/I.
        assert_eq!(s.inverse_mass_at(0, 0.0, p, -n), 0.0);
        let w = s.inverse_mass_at(0, 0.0, p, n);
        let arm = (p - Vec3::new(0.1, 0.05, 0.0)).cross(n).z;
        assert!((w - arm * arm / 1.0e-4).abs() < 1e-3 * w, "{w}");
        assert_eq!(s.inverse_mass_at(0, 0.0, p, Vec3::Z), 0.0);
        // An impulse into the stop moves nothing, also at that very instant.
        s.apply_impulse(0, 0.3e-3, p, -n * 0.5);
        assert_eq!(s.velocity_at(0, 0.3e-3, p), Vec3::ZERO);
        assert_eq!(s.max_surface_speed(0), 0.0);
        // An impulse away from the stop does: Δω = r·J/I.
        s.apply_impulse(0, 0.5e-3, p, n * 0.01);
        let v = s.velocity_at(0, 0.5e-3, p);
        let expected = arm * 0.01 / 1.0e-4 - 0.03 / 1.0e-4 * DT;
        assert!((v.dot(n) - expected * arm).abs() < 1e-3, "{v}");
        // Now it is moving away from the stop: free both ways.
        assert!(s.inverse_mass_at(0, 0.6e-3, p, -n) > 0.0);
        // Taking the impulse back at the same instant puts it back on the
        // stop, at rest.
        s.apply_impulse(0, 0.5e-3, p, -n * 0.01);
        assert_eq!(s.velocity_at(0, 0.5e-3, p), Vec3::ZERO);
        // Null and non-finite impulses are ignored.
        s.apply_impulse(0, 0.6e-3, p, Vec3::ZERO);
        s.apply_impulse(0, 0.6e-3, p, Vec3::splat(f32::NAN));
        s.end_tick(DT);
        assert_eq!(s.angle_deg(ID), -30.0);
        assert_eq!(s.angular_velocity(ID), 0.0);
    }

    #[test]
    fn the_bat_never_predicts_travel_beyond_a_stop() {
        let mut s = set();
        s.set_energised(ID, true);
        let mut arrived = false;
        for _ in 0..40 {
            s.begin_tick(DT);
            let f = &s.flippers[0];
            // The prediction never carries the bat past the stop.
            assert!(f.q_at(DT) <= f.travel);
            assert!(f.seg_q + f.seg_w * DT <= f.travel + 1e-6);
            if let Phase::Arriving { speed } = f.phase {
                arrived = true;
                assert!(f.seg_w < speed, "the stop takes up part of the motion");
                assert_eq!(f.momentum_w(), speed);
                // On its way into the stop the bat takes no push toward it,
                // and gives way to one away from it.
                let (p, n) = on_top(&s, 0.5e-3, 0.04);
                assert_eq!(s.inverse_mass_at(0, 0.5e-3, p, n), 0.0);
                assert!(s.inverse_mass_at(0, 0.5e-3, p, -n) > 0.0);
                let before = speed;
                s.apply_impulse(0, 0.5e-3, p, n * 0.01);
                assert_eq!(s.flippers[0].phase, Phase::Arriving { speed });
                // It reaches the stop exactly at the end of the tick.
                assert!((s.flippers[0].q_at(DT) - s.flippers[0].travel).abs() < 1e-5);
                s.end_tick(DT);
                // And bounces off with the stop's restitution.
                assert!((s.angular_velocity(ID) + 0.15 * before).abs() < 1e-3);
                assert_eq!(s.angle_deg(ID), 20.0);
                break;
            }
            s.end_tick(DT);
        }
        assert!(arrived);
        // Held on the end stop: at rest, immovable from below, free from
        // above (the ball pushes it off the stop against the hold torque).
        for _ in 0..100 {
            s.advance(DT);
        }
        s.begin_tick(DT);
        let (p, n) = on_top(&s, 0.0, 0.04);
        assert_eq!(s.velocity_at(0, 0.0, p), Vec3::ZERO);
        assert_eq!(s.inverse_mass_at(0, 0.0, p, n), 0.0);
        assert!(s.inverse_mass_at(0, 0.0, p, -n) > 0.0);
        s.end_tick(DT);
        assert_eq!(s.angular_velocity(ID), 0.0);
    }

    #[test]
    fn rubber_restitution_falls_with_the_approach_speed() {
        let s = set();
        let e = |u: f32| s.restitution(0, 0.0, Vec3::ZERO, u).unwrap();
        assert_eq!(e(0.0), 0.65);
        assert_eq!(e(-1.0), 0.65);
        assert!((e(4.0) - 0.65 / 1.35).abs() < 1e-6);
        assert!(e(1.0) > e(2.0) && e(2.0) > e(8.0) && e(8.0) > 0.0);
        let mut c = config();
        c.rubber_damping = 0.0;
        c.rubber_restitution = 0.3;
        let s = FlipperSet::new(&[c]).unwrap();
        assert_eq!(s.restitution(0, 0.0, Vec3::ZERO, 9.0), Some(0.3));
    }

    #[test]
    fn a_moving_bat_reports_its_true_inverse_mass() {
        let mut s = set();
        s.set_energised(ID, true);
        for _ in 0..12 {
            s.advance(DT);
        }
        s.begin_tick(DT);
        let (p, n) = on_top(&s, 0.0, 0.04);
        let arm = (p - Vec3::new(0.1, 0.05, 0.0)).cross(n).z;
        let free = arm * arm / 1.0e-4;
        assert!(s.velocity_at(0, 0.0, p).dot(n) > 0.5, "the bat is moving");
        for d in [n, -n] {
            let w = s.inverse_mass_at(0, 0.0, p, d);
            assert!((w - free).abs() < 1e-4 * free, "{w} vs {free}");
        }
        let before = s.velocity_at(0, 0.0, p).dot(n);
        s.apply_impulse(0, 0.0, p, -n * 0.001);
        let after = s.velocity_at(0, 0.0, p).dot(n);
        assert!(((before - after) - free * 0.001).abs() < 1e-4);
    }

    #[test]
    fn zipper_slides_at_its_speed_and_carries_the_surface() {
        let mut c = config();
        c.zipper_slide_axis_mm = Some([0.0, 12.0, 0.0]);
        c.zipper_slide_speed_mm_s = 100.0;
        let mut s = FlipperSet::new(&[c]).unwrap();
        s.set_zipper(ID, true);
        s.begin_tick(DT);
        let (p, n) = on_top(&s, 0.0, 0.04);
        assert!((s.velocity_at(0, 0.0, p) - Vec3::new(0.0, 0.1, 0.0)).length() < 1e-6);
        assert!((s.max_surface_speed(0) - 0.1).abs() < 1e-6);
        // The slide is motor-driven: with the bat on its rest stop a push
        // from above meets no give at all.
        assert_eq!(s.inverse_mass_at(0, 0.0, p, -n), 0.0);
        let (lo, hi) = s.swept_bounds(0);
        assert!(lo.y < 0.05 - 0.08 && hi.y > 0.05 + 0.08 + 0.0001);
        assert!(lo.x < 0.1 - 0.08 && hi.x > 0.1 + 0.08 && hi.z >= 0.03);
        s.end_tick(DT);
        assert!((s.base_offset_mm(ID) - 0.1).abs() < 1e-5);
        for _ in 0..200 {
            s.advance(DT);
        }
        assert!((s.base_offset_mm(ID) - 12.0).abs() < 1e-4);
        assert!((s.pivot_m(ID).unwrap() - Vec3::new(0.1, 0.062, 0.0)).length() < 1e-6);
        s.set_zipper(ID, false);
        for _ in 0..60 {
            s.advance(DT);
        }
        assert!((s.base_offset_mm(ID) - 6.0).abs() < 1e-3);
        for _ in 0..100 {
            s.advance(DT);
        }
        assert_eq!(s.base_offset_mm(ID), 0.0);
        // A zero-length axis or no axis: nothing slides.
        let mut c = config();
        c.zipper_slide_axis_mm = Some([0.0; 3]);
        c.zipper_slide_speed_mm_s = 100.0;
        let mut s = FlipperSet::new(&[c]).unwrap();
        s.set_zipper(ID, true);
        s.advance(DT);
        assert_eq!(s.base_offset_mm(ID), 0.0);
    }

    #[test]
    fn thing_flip_schedule_rounds_to_ticks() {
        assert_eq!(ticks(12.4, 1.0), 12);
        assert_eq!(ticks(12.5, 1.0), 13);
        assert_eq!(ticks(-3.0, 1.0), 0);
        assert_eq!(ticks(f32::NAN, 1.0), 0);
        assert_eq!(ticks(2.0, 2.0), 4);
        let mut c = config();
        c.staging = FlipperStagingMode::AutoSensorThingFlip {
            trigger_switch: SwitchId(5),
            delay_ms: 3.0,
            hold_duration_ms: 0.0,
        };
        // A zero hold never energises.
        let mut s = FlipperSet::new(&[c]).unwrap();
        s.on_switch_edge(SwitchId(5), true, 10);
        for t in 10..20 {
            s.step_tick(t);
            assert!(!s.is_energised(ID));
        }
        // The manual command and the schedule add up.
        s.set_energised(ID, true);
        assert!(s.is_energised(ID));
    }

    #[test]
    fn mirrored_and_tilted_flippers_report_consistent_angles() {
        let mut c = config();
        c.rest_angle_deg = 210.0;
        c.sweep_angle_deg = -50.0;
        c.surface_normal = [0.0, 0.6, 0.8];
        let mut s = FlipperSet::new(&[c]).unwrap();
        s.set_energised(ID, true);
        let mut fastest = 0.0f32;
        for _ in 0..100 {
            s.advance(DT);
            fastest = fastest.min(s.angular_velocity(ID));
        }
        assert_eq!(s.angle_deg(ID), 160.0);
        assert!(
            fastest < -50.0,
            "a negative sweep turns clockwise: {fastest}"
        );
        assert!((s.relative_angle_rad(ID) + 50.0f32.to_radians()).abs() < 1e-6);
        // The bat lies in the mounting plane.
        let dir = s.bat_direction(ID).unwrap();
        assert!(dir.dot(Vec3::new(0.0, 0.6, 0.8)).abs() < 1e-6);
        assert!(s.drive_work_j(ID) > 0.0);
    }

    #[test]
    fn cast_through_the_set_finds_the_moving_bat() {
        let mut s = set();
        s.set_energised(ID, true);
        for _ in 0..10 {
            s.advance(DT);
        }
        s.begin_tick(DT);
        let (p, n) = on_top(&s, 0.0, 0.04);
        // A sphere at rest just above the rising bat is hit within the tick.
        let center = p + n * (0.0135 + 2.0e-4);
        let hit = s.cast_sphere(0, 0.0, center, 0.0135, Vec3::ZERO, DT);
        assert!(
            matches!(hit, Some(BodyCast::Hit { time, .. }) if time > 0.0 && time < DT),
            "{hit:?}"
        );
        // One far away is not.
        let far = p + n * 0.2;
        assert_eq!(s.cast_sphere(0, 0.0, far, 0.0135, Vec3::ZERO, DT), None);
    }
}
