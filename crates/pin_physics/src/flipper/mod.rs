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
//! * **Rubber.** Restitution and friction of the contact come from the
//!   material-pair table (the bat is [`Material::Rubber`]). `rubber_damping`
//!   is a loss that grows with the bat's own surface speed along the contact
//!   normal, see [`FlipperTuning::rubber_loss_speed_m_s`].
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
use crate::world::layers;
use arrayvec::ArrayVec;
use glam::{Quat, Vec3};
use pin_core::ids::{FlipperId, SwitchId};
use pin_core::snapshot::MAX_FLIPPERS;
use pin_core::{FlipperConfig, FlipperShape, FlipperStagingMode, Material};

/// Why [`FlipperSet::new`] rejected a configuration.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FlipperError {
    /// The bat shape is not available yet (`CurvedBanana` before Subphase
    /// 2.6).
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
            FlipperError::ShapeNotSupported { id } => write!(
                f,
                "flipper {}: this bat shape is not supported yet (CurvedBanana arrives with Subphase 2.6)",
                id.0
            ),
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
    /// pull lumped together). 0 = instant.
    pub coil_rise_s: f32,
    /// Time constant (seconds) with which the coil torque collapses toward
    /// a lower target (release, or power → hold at EOS). 0 = instant.
    pub coil_fall_s: f32,
    /// Bat surface speed along the contact normal (m/s) at which
    /// `rubber_damping = 1` would double the contact's loading. The rubber
    /// loss factor is `g = min(rubber_damping · |v_n| / this, rubber_restitution)`
    /// and the bat reports the inverse mass `(1 + g)·w + g/m_ref` along the
    /// normal while it answers an impulse with its real `w`: for the
    /// reference ball the impulse is `1/(1 + g)` of the lossless one. A bat
    /// that does not move (held on a stop, at rest) has `g = 0`.
    pub rubber_loss_speed_m_s: f32,
    /// Reference ball mass `m_ref` (kg) of the rubber loss.
    pub reference_ball_mass_kg: f32,
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
            reference_ball_mass_kg: 0.080,
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

/// The bat shapes a flipper can carry.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Bat {
    Straight(StraightCapsule),
}

impl Bat {
    fn shape(&self) -> &dyn BatShape {
        match self {
            Bat::Straight(s) => s,
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
    seg_t: f32,
    seg_q: f32,
    /// Kinematic velocity of the segment (limited so the bat reaches a stop
    /// at the end of the tick at the earliest).
    seg_w: f32,
    /// The velocity the bat's momentum corresponds to. Differs from `seg_w`
    /// while a stop is taking up the difference.
    true_w: f32,
    /// The bat reaches (or has just reached) a stop in this tick with the
    /// momentum `true_w`; it leaves it with the stop's restitution.
    arriving: bool,
    slide_v: f32,
    tick_q0: f32,
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
            angle_min: self.rest_angle.min(self.end_angle),
            angle_max: self.rest_angle.max(self.end_angle),
        }
    }

    /// Travel at `time` of the current tick.
    fn q_at(&self, time: f32) -> f32 {
        (self.seg_q + self.seg_w * (time - self.seg_t).max(0.0)).clamp(0.0, self.travel)
    }

    /// Travel velocity at `time`: zero once a stop holds the bat.
    fn w_at(&self, time: f32) -> f32 {
        let q = self.q_at(time);
        let w = self.seg_w;
        if (w > 0.0 && self.travel - q <= STOP_ANGLE_TOLERANCE)
            || (w < 0.0 && q <= STOP_ANGLE_TOLERANCE)
        {
            0.0
        } else {
            w
        }
    }

    /// Starts a segment at `time` with the bat at `q` and momentum velocity
    /// `true_w`: the kinematic velocity is `true_w`, limited so that the bat
    /// reaches a stop at the end of the tick at the earliest (the stop takes
    /// the rest), and zero against a stop it already touches.
    fn start_segment(&mut self, time: f32, q: f32, true_w: f32, dt: f32) {
        let remaining = dt - time;
        let room = if true_w > 0.0 { self.travel - q } else { q };
        let speed = true_w.abs();
        let (limited, arriving) = if speed == 0.0 {
            (0.0, false)
        } else if room <= STOP_ANGLE_TOLERANCE {
            // On the stop: still the arrival if this tick brought the bat
            // here with momentum in the same direction.
            let same = self.arriving && (self.true_w > 0.0) == (true_w > 0.0);
            (0.0, same)
        } else if remaining > 0.0 && speed * remaining >= room {
            (room / remaining, true)
        } else {
            (speed, false)
        };
        self.arriving = arriving;
        self.seg_t = time;
        self.seg_q = q;
        self.seg_w = if true_w > 0.0 { limited } else { -limited };
        self.true_w = true_w;
    }

    /// Whether the stop in travel direction `toward` (±1) is engaged at
    /// `time`: the bat is at it and not moving away.
    fn engaged(&self, time: f32, toward: f32) -> bool {
        let q = self.q_at(time);
        if toward > 0.0 {
            self.travel - q <= STOP_ANGLE_TOLERANCE && self.seg_w >= 0.0
        } else {
            q <= STOP_ANGLE_TOLERANCE && self.seg_w <= 0.0
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
        self.drive = self.coil_torque - self.spring_torque;
        self.tick_q0 = self.q;
        let true_w = self.omega + self.drive / self.inertia * dt;
        self.arriving = false;
        self.start_segment(0.0, self.q, true_w, dt);

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
        let q = self.q_at(dt);
        // A bat that arrives at a stop with momentum leaves it with the
        // stop's restitution; one that is held there has none.
        let at_end = self.travel - q <= STOP_ANGLE_TOLERANCE;
        let at_rest = q <= STOP_ANGLE_TOLERANCE;
        let bounce = if self.arriving {
            -self.stop_restitution * self.true_w
        } else {
            0.0
        };
        if at_end && self.true_w > 0.0 {
            self.q = self.travel;
            self.omega = bounce;
        } else if at_rest && self.true_w < 0.0 {
            self.q = 0.0;
            self.omega = bounce;
        } else {
            self.q = q;
            self.omega = self.w_at(dt);
        }
        self.drive_work += f64::from(self.drive) * f64::from(self.q - self.tick_q0);
        self.slide = (self.slide + self.slide_v * dt).clamp(0.0, self.slide_extent);
        self.slide_v = 0.0;
        // Queries between ticks see the committed state, at rest.
        self.seg_t = 0.0;
        self.seg_q = self.q;
        self.seg_w = 0.0;
        self.true_w = 0.0;
        self.arriving = false;
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
            FlipperShape::CurvedBanana { .. } => {
                return Err(FlipperError::ShapeNotSupported { id });
            }
        };
        let normal = finite3(id, "surface_normal", c.surface_normal)?;
        let axis = normal
            .try_normalize()
            .ok_or(FlipperError::OutOfRange {
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
            seg_t: 0.0,
            seg_q: 0.0,
            seg_w: 0.0,
            true_w: 0.0,
            arriving: false,
            slide_v: 0.0,
            tick_q0: 0.0,
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
    /// [`FlipperError::ShapeNotSupported`] for a `CurvedBanana` bat (until
    /// Subphase 2.6), and the other variants for configurations the solver
    /// cannot run: non-finite numbers, a non-positive inertia, negative
    /// torques, restitutions outside `0..=1`, an EOS fraction outside
    /// `0..=1`, a zero normal, a degenerate bat, a layer above 7, duplicate
    /// ids, more than [`MAX_FLIPPERS`] flippers.
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
        in_range(first, "tuning.coil_rise_s", tuning.coil_rise_s, non_negative)?;
        in_range(first, "tuning.coil_fall_s", tuning.coil_fall_s, non_negative)?;
        in_range(
            first,
            "tuning.rubber_loss_speed_m_s",
            tuning.rubber_loss_speed_m_s,
            positive,
        )?;
        in_range(
            first,
            "tuning.reference_ball_mass_kg",
            tuning.reference_ball_mass_kg,
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

    /// Absolute bat angle about the surface normal, degrees
    /// (`rest_angle_deg` at rest). NaN for an unknown id.
    #[must_use]
    pub fn angle_deg(&self, id: FlipperId) -> f32 {
        self.get(id)
            .map_or(f32::NAN, |f| f.angle_of(f.q).to_degrees())
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

    /// Work done on the bat by coil and spring since construction, joules
    /// (negative when the bat was pushed against them). 0 for an unknown id.
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
        Some(world_closest_point(
            f.bat.shape(),
            &f.motion(),
            time,
            point,
        ))
    }

    fn velocity_at(&self, body: usize, time: f32, point: Vec3) -> Vec3 {
        let f = &self.flippers[body];
        let pivot = f.base + f.slide_dir * (f.slide + f.slide_v * time);
        f.axis.cross(point - pivot) * (f.sign * f.w_at(time)) + f.slide_dir * f.slide_v
    }

    fn inverse_mass_at(&self, body: usize, time: f32, point: Vec3, direction: Vec3) -> f32 {
        let f = &self.flippers[body];
        let arm = f.arm(time, point, direction);
        if arm == 0.0 || arm.is_nan() || f.engaged(time, arm.signum()) {
            return 0.0;
        }
        let w = arm * arm / f.inertia;
        // Rubber loss: extra loading along the surface normal that grows
        // with the bat's own normal surface speed (FlipperTuning).
        let w_bat = f.w_at(time);
        if w_bat == 0.0 || f.rubber_damping == 0.0 {
            return w;
        }
        let motion = f.motion();
        let normal = world_closest_point(f.bat.shape(), &motion, time, point).normal;
        let along = direction.dot(normal);
        let pivot = f.base + f.slide_dir * (f.slide + f.slide_v * time);
        let v_n = (f.axis.cross(point - pivot) * (f.sign * w_bat)).dot(normal);
        let g = (f.rubber_damping * v_n.abs() / self.tuning.rubber_loss_speed_m_s)
            .min(f.rubber_restitution);
        w + g * (w + 1.0 / self.tuning.reference_ball_mass_kg) * along * along
    }

    fn apply_impulse(&mut self, body: usize, time: f32, point: Vec3, impulse: Vec3) {
        let dt = self.dt;
        let f = &mut self.flippers[body];
        let delta = f.arm(time, point, impulse) / f.inertia;
        if delta == 0.0 || !delta.is_finite() {
            return;
        }
        let q = f.q_at(time);
        // A push into a stop the bat is held on is taken by the stop.
        if f.engaged(time, delta.signum()) {
            // Keep the momentum the bat brought to the stop (it decides the
            // bounce); the bat stays where it is.
            let keep = f.true_w;
            f.start_segment(time, q, keep, dt);
            return;
        }
        // The impulse changes the bat's momentum velocity (what it carries,
        // which is more than the limited speed of a segment ending on a
        // stop, and includes this tick's drive for a bat held on a stop).
        let carried = f.true_w;
        f.start_segment(time, q, carried + delta, dt);
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
