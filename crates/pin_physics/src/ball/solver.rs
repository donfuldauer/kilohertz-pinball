//! The ball solver: one [`BallSolver::step`] per 1 ms tick.
//!
//! A tick is a small discrete-event simulation on the time line `[0, dt]`:
//!
//! * every ball has its own number of sub-steps, chosen from its own speed
//!   and radius at the start of the tick, so a ball's trajectory does not
//!   depend on balls it does not touch;
//! * at each sub-step boundary of a ball its contacts are gathered,
//!   penetrations are projected out, forces are integrated (gravity, magnets,
//!   external hooks) and the touching contacts are solved at velocity level;
//! * between boundaries balls move in straight lines up to the earliest
//!   time of impact (static world, guides, captive balls, other balls), found
//!   by swept-sphere casts, where the impact impulse is applied;
//! * every ball has its own clock: its position is only advanced at its own
//!   boundaries and impacts and when another ball comes within contact
//!   range, so a ball that interacts with nothing takes exactly the same
//!   floating-point path whatever the other balls do.
//!
//! Moving bodies ([`MovingBodies`], lent to
//! [`BallSolver::step_with_bodies`]) take part like the static geometry:
//! their casts join the event loop, their contacts the boundary solve (see
//! [`super::body`] for the contract).
//!
//! Nothing here allocates; iteration order is fixed (ascending handles,
//! collider and guide order, body sets in slice order), so a step is
//! deterministic.

use arrayvec::ArrayVec;
use glam::Vec3;
use pin_core::ids::{MagnetId, ProfileId};
use pin_core::{BallProfile, Material};

use super::body::{
    BODY_REST_SPEED, BODY_SKIN, BodyCast, BodyContactState, MAX_BODY_SOLVES_PER_BALL,
    MAX_BODY_SOLVES_PER_TICK, MovingBodies, solve_body_contact,
};
use super::contact::{
    ContactCoefficients, ContactImpulse, SphereBody, apply_ball_impulse, apply_surface_impulse,
    effective_restitution, integrate_orientation, resolve_ball_contact, resolve_surface_contact,
    solve_ball_contact, solve_surface_contact, sphere_toi,
};
use super::newton::NewtonChain;
use super::types::{
    Ball, BallEvent, BallEvents, BallHandle, BallSolverConfig, ExternalForce, ForceContext,
    Geometry, IMPACT_REPORT_SPEED, ImpactTarget, MAX_BALLS, Magnet, MagnetState, Sensor,
    SensorKind, SurfaceVelocity,
};
use crate::world::{
    ColliderHandle, CubicSpline, GuideCast, GuideHit, SphereContact, SphereHit, layers,
};

/// Gap (metres) within which a surface counts as touched: a later impact on
/// a surface the ball already touches in that direction is a continuing
/// contact (no restitution, no impact event).
const TOUCH_TOL: f32 = 2.0e-5;
/// World contacts are solved at a sub-step boundary when they are within
/// the touch skin of `StaticWorld::cast_sphere` (1 µm): the cast stops a ball
/// anywhere inside that skin, and a ball resting there must be held where it
/// is instead of creeping through the skin. Anything further away is left to
/// the casts.
const ACTIVE_EPS: f32 = 1.0e-6;
/// A ball separating from a surface slower than this (m/s) while inside its
/// contact skin is resting on it.
const SEAT_SPEED: f32 = 1.0e-6;
/// Guide walls report cast contacts within this skin (see `GuideCurve`).
const GUIDE_SKIN: f32 = 1.0e-5;
/// Two contact normals closer than this (cosine, about 20°) support the
/// ball in the same direction.
const PERSIST_COS: f32 = 0.94;
/// Friction coefficient when the material pair is not in the table.
const FALLBACK_FRICTION: f32 = 0.2;
/// Impacts a ball may have per sub-step before it waits for its next one.
const MAX_EVENTS_PER_SUBSTEP: u8 = 12;
/// Velocity-solver sweeps over the touching contacts at a sub-step boundary.
const PRESOLVE_PASSES: usize = 64;
/// Pass after which a ball held only by static surfaces, other balls and
/// kinematic bodies (at least one), whose solve has not at least halved its change since pass
/// `STALL_PASSES / 2`, stops iterating: such a pinch (a kicker driving a ball
/// into a wall closer than its diameter) has no solution, and iterating it to
/// the pass limit after every event could stall the tick. The next sub-step
/// boundary projects the penetration out.
const STALL_PASSES: usize = 8;
/// Penetration (m) beyond which a stalled kinematic contact counts as a
/// pinch (five times the body contact skin).
const PINCH_DEPTH: f32 = 5.0e-5;
/// The sweeps stop when one changes no velocity by more than this (m/s).
const PRESOLVE_TOLERANCE: f32 = 1.0e-7;
/// Normals closer than this (cosine) are the same contact from one
/// sub-step to the next (warm start).
const WARM_COS: f32 = 0.99;
/// Closing speed (m/s) below which two touching balls are left to the next
/// sub-step boundary instead of raising an impact (they sink by at most a
/// micrometre per millisecond, which the position correction removes).
const REST_SLOP_SPEED: f32 = 1.0e-3;
/// Travel per sub-step as a fraction of the ball radius.
const TRAVEL_FRACTION: f32 = 0.25;
/// Balls up to this radius (m, the 27 mm standard ball) keep their
/// tick-start sub-step grid; larger ones refine it when a body speeds up
/// (see `BallSolver::refine_substeps`).
const REFINE_MIN_RADIUS: f32 = 0.0135;
/// Same, while the ball is near a wireform (rails have no sphere cast).
const RAIL_TRAVEL_FRACTION: f32 = 1.0 / 16.0;
/// Capacity of `StaticWorld::sphere_contacts` (not re-exported by `world`).
const MAX_SPHERE_CONTACTS: usize = 16;
const MAX_TOUCH: usize = 8;
const MAX_CONTACTS: usize = 24;
type ContactList = ArrayVec<Contact, MAX_CONTACTS>;
/// Guides / wireforms with a per-step bounding box (the rest are always
/// tested).
const MAX_BOUNDS: usize = 64;

const MATERIALS: usize = Material::ALL.len();

type Aabb = (Vec3, Vec3);
/// Directions (unit normals toward the ball) in which a ball is supported.
/// An entry is *established* once it has been seen at two consecutive
/// sub-step boundaries or was hit during the current sub-step: only then is
/// a new impact in that direction a continuing contact.
type TouchSet = ArrayVec<(Vec3, bool), MAX_TOUCH>;

/// Whether the set has any entry along `n`.
fn touches(set: &TouchSet, n: Vec3) -> bool {
    set.iter().any(|(t, _)| t.dot(n) > PERSIST_COS)
}

/// Whether the set has an established entry along `n`.
fn rests_on(set: &TouchSet, n: Vec3) -> bool {
    set.iter().any(|(t, est)| *est && t.dot(n) > PERSIST_COS)
}

fn add_touch(set: &mut TouchSet, n: Vec3, established: bool) {
    if let Some(entry) = set.iter_mut().find(|(t, _)| t.dot(n) > PERSIST_COS) {
        entry.1 |= established;
        return;
    }
    if set.is_full() {
        set.remove(0);
    }
    set.push((n, established));
}

/// [`sphere_toi`] for spheres that may rest on each other: a pair within
/// [`TOUCH_TOL`] that closes in slower than [`REST_SLOP_SPEED`] is not an
/// impact.
fn sphere_toi_resting(p: Vec3, w: Vec3, radius_sum: f32, window: f32) -> Option<f32> {
    let d = p.length();
    if d - radius_sum <= TOUCH_TOL && -p.dot(w) < REST_SLOP_SPEED * d {
        return None;
    }
    sphere_toi(p, w, radius_sum, window)
}

fn spline_aabb(s: &CubicSpline) -> Aabb {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for seg in s.segments() {
        for p in seg.p {
            min = min.min(p);
            max = max.max(p);
        }
    }
    (min, max)
}

/// Whether the segment `a → b`, padded by `pad`, can reach `aabb`.
fn near(aabb: Option<&Aabb>, a: Vec3, b: Vec3, pad: f32) -> bool {
    let Some((min, max)) = aabb else {
        return true;
    };
    let lo = a.min(b) - Vec3::splat(pad);
    let hi = a.max(b) + Vec3::splat(pad);
    lo.cmple(*max).all() && hi.cmpge(*min).all()
}

/// Per-step bounding boxes of the guides and wireforms.
struct Bounds {
    guides: ArrayVec<Aabb, MAX_BOUNDS>,
    wires: ArrayVec<Aabb, MAX_BOUNDS>,
}

impl Bounds {
    fn compute(geo: &Geometry<'_>) -> Self {
        let mut guides = ArrayVec::new();
        for g in geo.guides.iter().take(MAX_BOUNDS) {
            let (min, mut max) = spline_aabb(g.spline());
            max.z += g.height();
            guides.push((min, max));
        }
        let mut wires = ArrayVec::new();
        for w in geo.wireforms.iter().take(MAX_BOUNDS) {
            let (min, max) = spline_aabb(w.centerline());
            let pad = Vec3::splat(0.5 * w.gauge());
            wires.push((min - pad, max + pad));
        }
        Self { guides, wires }
    }
}

/// Precomputed per-profile data.
#[derive(Copy, Clone)]
struct ProfileData {
    body: SphereBody,
    restitution: f32,
    coupling: f32,
    conductive: bool,
    material: Material,
    valid: bool,
}

#[derive(Copy, Clone)]
enum StaticEvent {
    World(SphereHit),
    /// Guide index and its contact; `None` = the cast only advanced.
    Guide(usize, Option<GuideHit>),
    /// Chain index and captive-ball index.
    Chain(usize, usize),
    /// Body set, body, and the contact (point, normal); `None` = the cast
    /// only advanced.
    Body(usize, usize, Option<(Vec3, Vec3)>),
}

/// The moving bodies lent to one step.
struct Bodies<'s, 'b> {
    sets: &'s mut [&'b mut dyn MovingBodies],
    /// A body with a finite mass was pushed since the last check: its
    /// trajectory changed, so every ball's cast is stale.
    moved: bool,
    /// Body-contact solve steps this tick, per ball (see
    /// `MAX_BODY_SOLVES_PER_BALL`) and in all (`MAX_BODY_SOLVES_PER_TICK`).
    solves: [u32; MAX_BALLS],
    total: u32,
}

impl Bodies<'_, '_> {
    /// Whether ball `i` has spent its body-solve budget this tick (or the
    /// global cap is spent).
    fn starved(&self, i: usize) -> bool {
        self.solves[i] >= MAX_BODY_SOLVES_PER_BALL || self.total >= MAX_BODY_SOLVES_PER_TICK
    }
}

#[derive(Copy, Clone)]
enum Cache {
    /// Must be cast again.
    Dirty,
    /// Free until the ball's next sub-step boundary.
    Free,
    /// Next impact, at an absolute time within the tick.
    Event(f32, StaticEvent),
}

/// Per-tick state of one ball slot.
#[derive(Copy, Clone)]
struct Scratch {
    active: bool,
    body: SphereBody,
    pidx: usize,
    n_sub: u32,
    /// Index of the next sub-step boundary (`n_sub` = none left).
    k: u32,
    h: f32,
    /// Start of the sub-step grid: boundary `k` is at `t0 + k·h` (0 unless
    /// the grid was refined mid-tick, see `refine_substeps`).
    t0: f32,
    events: u8,
    frozen: bool,
    cache: Cache,
    /// The ball's own clock: the time within the tick at which its stored
    /// position is valid.
    t: f32,
}

impl Scratch {
    const IDLE: Self = Self {
        active: false,
        body: SphereBody {
            radius: 0.0,
            mass: 1.0,
        },
        pidx: 0,
        n_sub: 0,
        k: 0,
        h: 0.0,
        t0: 0.0,
        events: 0,
        frozen: false,
        cache: Cache::Free,
        t: 0.0,
    };

    /// Time of the next sub-step boundary, or `dt` when none is left.
    fn horizon(&self, dt: f32) -> f32 {
        if self.k < self.n_sub {
            self.boundary_time()
        } else {
            dt
        }
    }

    /// Time of boundary `k` (exactly `k·h` while `t0` is 0).
    fn boundary_time(&self) -> f32 {
        self.t0 + self.k as f32 * self.h
    }

    fn count_event(&mut self) {
        self.events = self.events.saturating_add(1);
        if self.events > MAX_EVENTS_PER_SUBSTEP {
            self.frozen = true;
        }
        self.cache = Cache::Dirty;
    }
}

#[derive(Copy, Clone)]
enum ContactKind {
    Surface(ImpactTarget, Material),
    Ball(usize),
    Chain(usize, usize),
}

#[derive(Copy, Clone)]
struct Contact {
    /// Unit normal toward the ball centre.
    normal: Vec3,
    point: Vec3,
    /// Penetration depth (negative = gap).
    depth: f32,
    /// The contact is solved when `depth >= -active_eps`.
    active_eps: f32,
    kind: ContactKind,
    persistent: bool,
    /// Normal velocity the boundary solve asks for: 0 for a contact that
    /// touches, minus gap over sub-step for one a little apart, so that a
    /// ball stopped inside a contact skin may close the gap but not pass it.
    target: f32,
    /// Impulses applied by the boundary solve.
    acc: ContactImpulse,
    /// What the solve learnt about a moving body (body contacts only).
    body: BodyContactState,
}

impl ContactKind {
    /// Whether two contacts are with the same thing.
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Surface(a, _), Self::Surface(b, _)) => a == b,
            (Self::Ball(a), Self::Ball(b)) => a == b,
            (Self::Chain(a, b), Self::Chain(c, d)) => a == c && b == d,
            _ => false,
        }
    }
}

/// Impulses of a resting contact, kept from one boundary solve to the next:
/// started from them, the solve of a resting cluster converges in a sweep or
/// two instead of dozens.
#[derive(Copy, Clone)]
struct WarmContact {
    kind: ContactKind,
    normal: Vec3,
    acc: ContactImpulse,
    /// Sub-step the impulses were solved for (they scale with it).
    h: f32,
}

type WarmList = ArrayVec<WarmContact, MAX_CONTACTS>;

enum Next {
    End,
    Boundary,
    Static(usize),
    Pair(usize, usize),
}

/// The ball dynamics core. See the [module documentation](crate::ball).
pub struct BallSolver {
    cfg: BallSolverConfig,
    profiles: Vec<BallProfile>,
    pdata: Vec<ProfileData>,
    /// (restitution, friction) per material pair, if the table has it.
    pair_table: [[Option<(f32, f32)>; MATERIALS]; MATERIALS],
    slots: [Option<Ball>; MAX_BALLS],
    /// Directions in which each ball is currently supported.
    touch: [TouchSet; MAX_BALLS],
    /// Contact impulses of each ball's last boundary solve.
    warm: [WarmList; MAX_BALLS],
    /// Compensation term of the position sums (Kahan), so that the many
    /// small advances of a tick do not accumulate rounding.
    carry: [Vec3; MAX_BALLS],
    /// Position at the end of the last step; a ball moved from outside
    /// (teleport) drops its compensation term.
    last_position: [Vec3; MAX_BALLS],
    collider_materials: Vec<Material>,
    guide_materials: Vec<Material>,
    guide_layers: Vec<u8>,
    wireform_materials: Vec<Material>,
    wireform_layers: Vec<u8>,
    magnets: Vec<MagnetState>,
    sensors: Vec<(Sensor, bool)>,
    /// Steps completed, and their total duration (seconds), for
    /// [`ForceContext`].
    steps: u64,
    elapsed: f64,
    /// `dt` of the step in progress.
    tick_dt: f32,
    /// Body-contact solve steps per ball in the last step.
    body_solves: [u32; MAX_BALLS],
    hooks: Vec<Box<dyn ExternalForce>>,
    surface_velocity: Option<Box<dyn SurfaceVelocity>>,
    chains: Vec<NewtonChain>,
}

fn set_at<T: Copy>(v: &mut Vec<T>, index: usize, value: T, default: T) {
    if index >= v.len() {
        v.resize(index + 1, default);
    }
    v[index] = value;
}

impl BallSolver {
    /// Creates a solver. `profiles` is indexed by [`ProfileId`]; a profile
    /// with a non-positive or non-finite mass or diameter cannot be used.
    #[must_use]
    pub fn new(cfg: BallSolverConfig, profiles: &[BallProfile]) -> Self {
        let mut pair_table = [[None; MATERIALS]; MATERIALS];
        // The first entry of a pair wins, as in `pin_core::lookup_pair`.
        for p in cfg.pairs.iter().rev() {
            let (a, b) = (p.materials[0] as usize, p.materials[1] as usize);
            pair_table[a][b] = Some((p.restitution, p.friction));
            pair_table[b][a] = Some((p.restitution, p.friction));
        }
        let pdata = profiles
            .iter()
            .map(|p| {
                let body = SphereBody {
                    radius: p.radius_m(),
                    mass: p.mass_kg,
                };
                ProfileData {
                    body,
                    restitution: p.restitution,
                    coupling: p.magnetic_coupling,
                    conductive: p.conductive,
                    material: p.material,
                    valid: body.radius.is_finite()
                        && body.radius > 0.0
                        && body.mass.is_finite()
                        && body.mass > 0.0,
                }
            })
            .collect();
        Self {
            cfg,
            profiles: profiles.to_vec(),
            pdata,
            pair_table,
            slots: [None; MAX_BALLS],
            touch: std::array::from_fn(|_| ArrayVec::new()),
            warm: std::array::from_fn(|_| ArrayVec::new()),
            carry: [Vec3::ZERO; MAX_BALLS],
            last_position: [Vec3::ZERO; MAX_BALLS],
            collider_materials: Vec::new(),
            guide_materials: Vec::new(),
            guide_layers: Vec::new(),
            wireform_materials: Vec::new(),
            wireform_layers: Vec::new(),
            magnets: Vec::new(),
            sensors: Vec::new(),
            steps: 0,
            elapsed: 0.0,
            tick_dt: 0.0,
            body_solves: [0; MAX_BALLS],
            hooks: Vec::new(),
            surface_velocity: None,
            chains: Vec::new(),
        }
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &BallSolverConfig {
        &self.cfg
    }

    /// Changes gravity (nudge and tilt are applied by the caller this way).
    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.cfg.gravity = gravity;
    }

    /// The profile list.
    #[must_use]
    pub fn profiles(&self) -> &[BallProfile] {
        &self.profiles
    }

    fn profile_data(&self, id: ProfileId) -> Option<&ProfileData> {
        self.pdata.get(id.index()).filter(|p| p.valid)
    }

    /// Adds a ball in the lowest free slot. `None` when all
    /// [`MAX_BALLS`] slots are taken or the ball's profile is unknown.
    /// Every force hook is told ([`ExternalForce::ball_added`]).
    pub fn add_ball(&mut self, ball: Ball) -> Option<BallHandle> {
        self.profile_data(ball.profile)?;
        let i = self.slots.iter().position(Option::is_none)?;
        self.slots[i] = Some(ball);
        self.touch[i].clear();
        self.warm[i].clear();
        self.carry[i] = Vec3::ZERO;
        self.last_position[i] = ball.position;
        let h = BallHandle(i as u8);
        for hook in &mut self.hooks {
            hook.ball_added(h, &ball);
        }
        Some(h)
    }

    /// Removes a ball and returns its last state. Every force hook is told
    /// ([`ExternalForce::ball_removed`]).
    pub fn remove_ball(&mut self, h: BallHandle) -> Option<Ball> {
        let ball = self.slots.get_mut(usize::from(h.0))?.take()?;
        for hook in &mut self.hooks {
            hook.ball_removed(h);
        }
        Some(ball)
    }

    /// The ball in slot `h`.
    #[must_use]
    pub fn ball(&self, h: BallHandle) -> Option<&Ball> {
        self.slots.get(usize::from(h.0))?.as_ref()
    }

    /// Mutable access to the ball in slot `h` (teleports, layer changes).
    pub fn ball_mut(&mut self, h: BallHandle) -> Option<&mut Ball> {
        self.slots.get_mut(usize::from(h.0))?.as_mut()
    }

    /// The balls in ascending handle order.
    pub fn balls(&self) -> impl Iterator<Item = (BallHandle, &Ball)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.as_ref().map(|b| (BallHandle(i as u8), b)))
    }

    /// Number of balls.
    #[must_use]
    pub fn ball_count(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// Applies an impulse (N·s) through the centre of ball `h`: its velocity
    /// changes by `impulse / mass` at once. Unknown handles and non-finite
    /// impulses are ignored.
    pub fn apply_impulse(&mut self, h: BallHandle, impulse: Vec3) {
        if !impulse.is_finite() {
            return;
        }
        let Some(ball) = self.ball(h).copied() else {
            return;
        };
        let Some(mass) = self.profile_data(ball.profile).map(|p| p.body.mass) else {
            return;
        };
        if let Some(b) = self.ball_mut(h) {
            b.velocity += impulse / mass;
        }
    }

    /// Sets the surface material of a static collider (default: wood).
    /// Setup-time only (may allocate).
    pub fn set_collider_material(&mut self, c: ColliderHandle, m: Material) {
        set_at(&mut self.collider_materials, c.index(), m, Material::Wood);
    }

    /// Sets the material of guide `index` (default: steel). Setup-time only.
    pub fn set_guide_material(&mut self, index: usize, m: Material) {
        set_at(&mut self.guide_materials, index, m, Material::Steel);
    }

    /// Sets the material of wireform `index` (default: steel). Setup-time only.
    pub fn set_wireform_material(&mut self, index: usize, m: Material) {
        set_at(&mut self.wireform_materials, index, m, Material::Steel);
    }

    /// Sets the layers on which guide `index` collides (default: all).
    /// Setup-time only.
    pub fn set_guide_layers(&mut self, index: usize, mask: u8) {
        set_at(&mut self.guide_layers, index, mask, layers::ALL);
    }

    /// Sets the layers on which wireform `index` collides (default: all).
    /// Setup-time only.
    pub fn set_wireform_layers(&mut self, index: usize, mask: u8) {
        set_at(&mut self.wireform_layers, index, mask, layers::ALL);
    }

    /// Adds an electromagnet, initially off, with the remanence τ of the
    /// configuration. Ids count up from 0.
    pub fn add_magnet(&mut self, m: Magnet) -> MagnetId {
        self.magnets.push(MagnetState {
            magnet: m,
            power: 0.0,
            remanence_tau_s: self.cfg.magnet_remanence_tau_s,
        });
        MagnetId((self.magnets.len() - 1) as u16)
    }

    /// Sets the commanded power of a magnet, clamped to `[0, 1]` (NaN = 0).
    /// Unknown ids are ignored.
    pub fn set_magnet_power(&mut self, id: MagnetId, p: f32) {
        if let Some(m) = self.magnets.get_mut(id.index()) {
            m.power = if p.is_nan() { 0.0 } else { p.clamp(0.0, 1.0) };
        }
    }

    /// Commanded power of a magnet (0 for an unknown id).
    #[must_use]
    pub fn magnet_power(&self, id: MagnetId) -> f32 {
        self.magnets.get(id.index()).map_or(0.0, |m| m.power)
    }

    /// Sets the remanence time constant τ (seconds) of a magnet; the solver
    /// stores it for the force hooks ([`MagnetState::remanence_tau_s`]).
    /// Unknown ids, NaN and negative values are ignored.
    pub fn set_magnet_remanence_tau(&mut self, id: MagnetId, tau_s: f32) {
        if let Some(m) = self.magnets.get_mut(id.index())
            && tau_s >= 0.0
        {
            m.remanence_tau_s = tau_s;
        }
    }

    /// Every magnet, indexed by [`MagnetId`].
    #[must_use]
    pub fn magnets(&self) -> &[MagnetState] {
        &self.magnets
    }

    /// Number of completed steps (the [`ForceContext::step_index`] of the
    /// next one).
    #[must_use]
    pub fn step_count(&self) -> u64 {
        self.steps
    }

    /// Body-contact solve steps each ball used in the last step (diagnostic;
    /// a ball at [`MAX_BODY_SOLVES_PER_BALL`] was starved).
    #[doc(hidden)]
    #[must_use]
    pub fn body_solves_last_step(&self) -> [u32; MAX_BALLS] {
        self.body_solves
    }

    /// Simulated time of all completed steps, seconds.
    #[must_use]
    pub fn sim_time(&self) -> f64 {
        self.elapsed
    }

    /// Adds a sensor, initially open. Setup-time only.
    pub fn add_sensor(&mut self, s: Sensor) {
        self.sensors.push((s, false));
    }

    /// Registers an external force (setup-time only). With no force
    /// registered the solver takes exactly the same floating-point path as
    /// if the hook list did not exist. The hook is told about every ball
    /// already present ([`ExternalForce::ball_added`], ascending handles).
    pub fn add_external_force(&mut self, mut f: Box<dyn ExternalForce>) {
        for (i, slot) in self.slots.iter().enumerate() {
            if let Some(ball) = slot {
                f.ball_added(BallHandle(i as u8), ball);
            }
        }
        self.hooks.push(f);
    }

    /// Installs (or removes) the source of surface velocities for moving
    /// colliders (static colliders, guides, wireforms; moving bodies give
    /// their own [`MovingBodies::velocity_at`]). Setup-time only.
    pub fn set_surface_velocity(&mut self, s: Option<Box<dyn SurfaceVelocity>>) {
        self.surface_velocity = s;
    }

    /// Adds a chain of captive balls and returns its index. The ball size,
    /// mass and ball–ball restitution come from the chain's profile; a chain
    /// with an unknown profile never moves or collides.
    pub fn add_newton_chain(&mut self, mut chain: NewtonChain) -> usize {
        if let Some(p) = self.profile_data(chain.profile()).copied() {
            let e = self.pair_table[p.material as usize][p.material as usize]
                .map_or(p.restitution, |(e, _)| e);
            chain.bind(p.body.radius, p.body.mass, e);
        }
        self.chains.push(chain);
        self.chains.len() - 1
    }

    /// Chain `i` (panics if out of range).
    #[must_use]
    pub fn newton_chain(&self, i: usize) -> &NewtonChain {
        &self.chains[i]
    }

    /// Mutable access to chain `i`, e.g. to [`NewtonChain::strike`] a ball
    /// (panics if out of range).
    pub fn newton_chain_mut(&mut self, i: usize) -> &mut NewtonChain {
        &mut self.chains[i]
    }

    /// Number of Newton chains.
    #[must_use]
    pub fn newton_chain_count(&self) -> usize {
        self.chains.len()
    }

    /// Sets bit `switch` of `out` for every closed sensor. Only the bits of
    /// this solver's own sensors are cleared first; switch numbers above 255
    /// are ignored.
    pub fn sensor_switches(&self, out: &mut [u64; 4]) {
        for (s, _) in &self.sensors {
            let n = s.switch.index();
            if n < 256 {
                out[n / 64] &= !(1u64 << (n % 64));
            }
        }
        for (s, closed) in &self.sensors {
            let n = s.switch.index();
            if *closed && n < 256 {
                out[n / 64] |= 1u64 << (n % 64);
            }
        }
    }

    fn surface_coefficients(&self, pidx: usize, material: Material) -> ContactCoefficients {
        let p = &self.pdata[pidx];
        let (restitution, friction) = self.pair_table[p.material as usize][material as usize]
            .unwrap_or((p.restitution, FALLBACK_FRICTION));
        ContactCoefficients {
            restitution,
            friction,
            rolling_resistance: self.cfg.rolling_resistance,
        }
    }

    fn ball_coefficients(&self, pa: usize, pb: usize) -> ContactCoefficients {
        let (a, b) = (&self.pdata[pa], &self.pdata[pb]);
        let (restitution, friction) = self.pair_table[a.material as usize][b.material as usize]
            .unwrap_or((a.restitution.min(b.restitution), FALLBACK_FRICTION));
        ContactCoefficients {
            restitution,
            friction,
            rolling_resistance: 0.0,
        }
    }

    fn collider_material(&self, c: ColliderHandle) -> Material {
        self.collider_materials
            .get(c.index())
            .copied()
            .unwrap_or(Material::Wood)
    }

    fn guide_material(&self, i: usize) -> Material {
        self.guide_materials
            .get(i)
            .copied()
            .unwrap_or(Material::Steel)
    }

    fn wireform_material(&self, i: usize) -> Material {
        self.wireform_materials
            .get(i)
            .copied()
            .unwrap_or(Material::Steel)
    }

    fn guide_mask(&self, i: usize) -> u8 {
        self.guide_layers.get(i).copied().unwrap_or(layers::ALL)
    }

    fn wireform_mask(&self, i: usize) -> u8 {
        self.wireform_layers.get(i).copied().unwrap_or(layers::ALL)
    }

    fn surface_velocity_at(&self, target: ImpactTarget, point: Vec3) -> Vec3 {
        self.surface_velocity
            .as_ref()
            .map_or(Vec3::ZERO, |s| s.velocity_at(target, point))
    }

    /// Velocity of the surface of `target` at `point` at `time` of the tick.
    fn target_velocity(
        &self,
        bodies: &Bodies<'_, '_>,
        target: ImpactTarget,
        time: f32,
        point: Vec3,
    ) -> Vec3 {
        match target {
            ImpactTarget::Body { set, body } => bodies
                .sets
                .get(set)
                .map_or(Vec3::ZERO, |s| s.velocity_at(body, time, point)),
            _ => self.surface_velocity_at(target, point),
        }
    }

    /// Whether ball `i` is in a real pinch, the case [`STALL_PASSES`] gives
    /// up on: its touching contacts are static surfaces, other free balls and
    /// kinematic bodies (zero inverse mass both ways along the normal), at
    /// least one body, and a kinematic body contact is opposed (normals more
    /// than 120° apart) by another contact while the body closes in on the
    /// ball, or the ball runs into an opposing surface, faster than
    /// [`BODY_REST_SPEED`], or the ball is already pushed more than
    /// [`PINCH_DEPTH`] into a contact. A stack resting on a body
    /// (or riding a lift) has opposed normals but nothing closing in, so it
    /// keeps iterating. Lists without a body never qualify, so the solver
    /// without bodies is unchanged.
    fn kinematic_pinch(
        &self,
        i: usize,
        list: &ContactList,
        time: f32,
        bodies: &Bodies<'_, '_>,
    ) -> bool {
        let Some(ball) = self.slots[i] else {
            return false;
        };
        let active = || list.iter().filter(|c| c.depth >= -c.active_eps);
        let mut deep = false;
        for c in active() {
            deep |= c.depth > PINCH_DEPTH;
            match c.kind {
                ContactKind::Surface(ImpactTarget::Body { set, body }, _) => {
                    let Some(s) = bodies.sets.get(set) else {
                        return false;
                    };
                    let free = |d: Vec3| {
                        let w = s.inverse_mass_at(body, time, c.point, d);
                        w.is_finite() && w > 0.0
                    };
                    if free(c.normal) || free(-c.normal) {
                        return false;
                    }
                }
                ContactKind::Surface(..) | ContactKind::Ball(_) => {}
                ContactKind::Chain(..) => return false,
            }
        }
        for c in active() {
            let ContactKind::Surface(ImpactTarget::Body { set, body }, _) = c.kind else {
                continue;
            };
            let Some(s) = bodies.sets.get(set) else {
                continue;
            };
            let closing = (s.velocity_at(body, time, c.point) - ball.velocity).dot(c.normal);
            for o in active().filter(|o| o.normal.dot(c.normal) < -0.5) {
                // How fast the ball runs into an opposing surface (a ball
                // above it in a settling stack does not count).
                let approach = match o.kind {
                    ContactKind::Surface(t, _) => -(ball.velocity
                        - self.target_velocity(bodies, t, time, o.point))
                    .dot(o.normal),
                    _ => 0.0,
                };
                if deep || closing > BODY_REST_SPEED || approach > BODY_REST_SPEED {
                    return true;
                }
            }
        }
        false
    }

    /// One solver step (or, with `acc` fresh, a one-shot resolution) of the
    /// contact of ball `i` with a moving body; see [`solve_body_contact`].
    #[allow(clippy::too_many_arguments)]
    fn body_solve(
        &mut self,
        i: usize,
        sc: &Scratch,
        bodies: &mut Bodies<'_, '_>,
        (set, body): (usize, usize),
        material: Material,
        normal: Vec3,
        point: Vec3,
        target_normal_speed: f32,
        acc: &mut ContactImpulse,
        st: &mut BodyContactState,
    ) -> f32 {
        if bodies.starved(i) {
            return 0.0;
        }
        bodies.solves[i] += 1;
        bodies.total += 1;
        let k = self.surface_coefficients(sc.pidx, material);
        let (Some(ball), Some(s)) = (self.slots[i].as_mut(), bodies.sets.get_mut(set)) else {
            return 0.0;
        };
        let (change, moved) = solve_body_contact(
            ball,
            sc.body,
            &mut **s,
            body,
            sc.t,
            point,
            normal,
            k,
            target_normal_speed,
            acc,
            st,
        );
        bodies.moved |= moved;
        change
    }

    /// Impact of ball `i` on a surface (one shot, with restitution).
    /// Returns whether an impulse was applied.
    #[allow(clippy::too_many_arguments)]
    fn surface_hit(
        &mut self,
        i: usize,
        sc: &Scratch,
        normal: Vec3,
        point: Vec3,
        target: ImpactTarget,
        material: Material,
        persistent: bool,
        min_approach: f32,
        bodies: &mut Bodies<'_, '_>,
        events: &mut BallEvents,
    ) -> bool {
        let Some(mut ball) = self.slots[i] else {
            return false;
        };
        let k = self.surface_coefficients(sc.pidx, material);
        if let ImpactTarget::Body { set, body } = target {
            // Two-body impact: restitution on the relative normal velocity.
            let vs = self.target_velocity(bodies, target, sc.t, point);
            let approach = -(ball.velocity - vs).dot(normal);
            if approach.is_nan() || approach <= min_approach {
                return false;
            }
            // The owner may give its own restitution (None: pair table).
            let own = bodies
                .sets
                .get(set)
                .and_then(|s| s.restitution(body, sc.t, point, approach))
                .filter(|e| !e.is_nan());
            let restitution = own.map_or(k.restitution, |e| e.clamp(0.0, 1.0));
            let e = effective_restitution(restitution, approach, persistent);
            let mut acc = ContactImpulse::default();
            let mut st = BodyContactState::NEW;
            self.body_solve(
                i,
                sc,
                bodies,
                (set, body),
                material,
                normal,
                point,
                e * approach,
                &mut acc,
                &mut st,
            );
            if acc.normal <= 0.0 {
                return false;
            }
            if !persistent && approach >= IMPACT_REPORT_SPEED {
                if let Some(s) = bodies.sets.get_mut(set) {
                    let impulse = -(normal * acc.normal + acc.tangent);
                    s.impact(body, sc.t, point, impulse, approach);
                }
                events.push(BallEvent::Impact {
                    ball: BallHandle(i as u8),
                    other: target,
                    normal_speed: approach,
                });
            }
            return true;
        }
        let vs = self.surface_velocity_at(target, point);
        let out =
            resolve_surface_contact(&mut ball, sc.body, normal, vs, k, persistent, min_approach);
        if out.normal_impulse <= 0.0 {
            return false;
        }
        self.slots[i] = Some(ball);
        if !persistent && out.approach_speed >= IMPACT_REPORT_SPEED {
            events.push(BallEvent::Impact {
                ball: BallHandle(i as u8),
                other: target,
                normal_speed: out.approach_speed,
            });
        }
        true
    }

    /// Balls `i` and `j`, along their current line of centres.
    fn pair_hit(
        &mut self,
        i: usize,
        j: usize,
        sc: &[Scratch; MAX_BALLS],
        persistent: bool,
        min_approach: f32,
        events: &mut BallEvents,
    ) -> bool {
        let (Some(mut a), Some(mut b)) = (self.slots[i], self.slots[j]) else {
            return false;
        };
        let Some(normal) = (a.position - b.position).try_normalize() else {
            return false;
        };
        let k = self.ball_coefficients(sc[i].pidx, sc[j].pidx);
        let out = resolve_ball_contact(
            &mut a,
            sc[i].body,
            &mut b,
            sc[j].body,
            normal,
            k,
            persistent,
            min_approach,
        );
        if out.normal_impulse <= 0.0 {
            return false;
        }
        self.slots[i] = Some(a);
        self.slots[j] = Some(b);
        if !persistent && out.approach_speed >= IMPACT_REPORT_SPEED {
            let (lo, hi) = (i.min(j), i.max(j));
            events.push(BallEvent::Impact {
                ball: BallHandle(lo as u8),
                other: ImpactTarget::Ball(BallHandle(hi as u8)),
                normal_speed: out.approach_speed,
            });
        }
        true
    }

    /// One solver step for ball `i` against captive ball `bi` of chain `ci`;
    /// `normal` points from the captive ball toward the free ball. The
    /// captive ball can only move along its path, so the impulse acts on it
    /// through the path tangent; the contact is frictionless. `acc` is the
    /// normal impulse applied so far. Returns the size of the change (m/s).
    #[allow(clippy::too_many_arguments)]
    fn chain_solve(
        &mut self,
        i: usize,
        sc: &Scratch,
        ci: usize,
        bi: usize,
        normal: Vec3,
        target_normal_speed: f32,
        acc: &mut f32,
    ) -> f32 {
        let Some(ball) = self.slots[i].as_mut() else {
            return 0.0;
        };
        let chain = &mut self.chains[ci];
        let tangent = chain.tangent(bi);
        let un = (ball.velocity - tangent * chain.ball_speed(bi)).dot(normal);
        if un.is_nan() {
            return 0.0;
        }
        let nt = normal.dot(tangent);
        let (im, imc) = (1.0 / sc.body.mass, 1.0 / chain.mass());
        let total = (*acc + (target_normal_speed - un) / (im + nt * nt * imc)).max(0.0);
        let dj = total - *acc;
        *acc = total;
        if dj == 0.0 {
            return 0.0;
        }
        ball.velocity += normal * (dj * im);
        chain.add_speed(bi, -dj * nt * imc);
        chain.resolve_touching();
        (dj * im).abs()
    }

    /// Impact of ball `i` on captive ball `bi` of chain `ci` (one shot, with
    /// restitution); `normal` points from the captive ball toward the free
    /// ball.
    #[allow(clippy::too_many_arguments)]
    fn chain_hit(
        &mut self,
        i: usize,
        sc: &Scratch,
        ci: usize,
        bi: usize,
        normal: Vec3,
        persistent: bool,
        min_approach: f32,
        events: &mut BallEvents,
    ) -> bool {
        let Some(ball) = self.slots[i] else {
            return false;
        };
        let chain = &self.chains[ci];
        let approach = -(ball.velocity - chain.tangent(bi) * chain.ball_speed(bi)).dot(normal);
        if approach.is_nan() || approach <= min_approach {
            return false;
        }
        let k = self.ball_coefficients(sc.pidx, chain.profile().index());
        let e = effective_restitution(k.restitution, approach, persistent);
        let mut acc = 0.0;
        self.chain_solve(i, sc, ci, bi, normal, e * approach, &mut acc);
        if !persistent && approach >= IMPACT_REPORT_SPEED {
            events.push(BallEvent::Impact {
                ball: BallHandle(i as u8),
                other: ImpactTarget::Newton(ci),
                normal_speed: approach,
            });
        }
        true
    }

    /// Moves ball `i` along its velocity up to the time `to` of the tick.
    fn advance_ball(&mut self, i: usize, to: f32, sc: &mut [Scratch; MAX_BALLS]) {
        let s = &mut sc[i];
        let tau = to - s.t;
        if tau.is_nan() || tau <= 0.0 {
            return;
        }
        s.t = to;
        if let (Some(ball), false) = (self.slots[i].as_mut(), s.frozen) {
            // Compensated sum: the rounding error of this addition is
            // carried into the next one.
            let step = ball.velocity * tau - self.carry[i];
            let moved = ball.position + step;
            self.carry[i] = (moved - ball.position) - step;
            ball.position = moved;
            ball.orientation = integrate_orientation(ball.orientation, ball.angular_velocity, tau);
        }
    }

    /// Where ball `j` is at `time` (its stored position is at its own clock).
    fn position_at(&self, ball: &Ball, s: &Scratch, time: f32) -> Vec3 {
        if s.frozen || time <= s.t {
            ball.position
        } else {
            ball.position + ball.velocity * (time - s.t)
        }
    }

    /// Collects everything ball `i` touches (within [`TOUCH_TOL`]).
    #[allow(clippy::needless_range_loop)] // the index addresses several per-ball arrays
    fn gather(
        &self,
        geo: &Geometry<'_>,
        bounds: &Bounds,
        bodies: &Bodies<'_, '_>,
        i: usize,
        sc: &[Scratch; MAX_BALLS],
    ) -> ContactList {
        let mut out = ContactList::new();
        let Some(ball) = self.slots[i] else {
            return out;
        };
        let r = sc[i].body.radius;
        let pos = ball.position;
        let mask = layers::layer_bit(ball.layer);
        let own = &self.touch[i];
        let mut add = |c: Contact| {
            let _ = out.try_push(c);
        };

        let mut buf: ArrayVec<SphereContact, MAX_SPHERE_CONTACTS> = ArrayVec::new();
        geo.world
            .sphere_contacts(pos, r + TOUCH_TOL, mask, &mut buf);
        for c in &buf {
            add(Contact {
                normal: c.normal,
                point: c.point,
                depth: c.depth - TOUCH_TOL,
                active_eps: ACTIVE_EPS,
                kind: ContactKind::Surface(
                    ImpactTarget::Collider(c.collider),
                    self.collider_material(c.collider),
                ),
                persistent: touches(own, c.normal),
                target: 0.0,
                acc: ContactImpulse::default(),
                body: BodyContactState::NEW,
            });
        }

        let pad = r + TOUCH_TOL;
        for (gi, guide) in geo.guides.iter().enumerate() {
            if self.guide_mask(gi) & mask == 0 || !near(bounds.guides.get(gi), pos, pos, pad) {
                continue;
            }
            let cp = guide.closest_point(pos, None);
            let gap = cp.distance - r;
            if gap <= TOUCH_TOL && cp.distance > 1.0e-9 {
                let normal = (pos - cp.point) / cp.distance;
                add(Contact {
                    normal,
                    point: cp.point,
                    depth: -gap,
                    active_eps: GUIDE_SKIN,
                    kind: ContactKind::Surface(ImpactTarget::Guide(gi), self.guide_material(gi)),
                    persistent: touches(own, normal),
                    target: 0.0,
                    acc: ContactImpulse::default(),
                    body: BodyContactState::NEW,
                });
            }
        }

        for (wi, wire) in geo.wireforms.iter().enumerate() {
            if self.wireform_mask(wi) & mask == 0 || !near(bounds.wires.get(wi), pos, pos, pad) {
                continue;
            }
            for cp in wire.closest_on_rails(pos, None) {
                let gap = cp.distance - r;
                if gap <= TOUCH_TOL && cp.distance > 1.0e-9 {
                    let normal = (pos - cp.point) / cp.distance;
                    add(Contact {
                        normal,
                        point: cp.point,
                        depth: -gap,
                        active_eps: ACTIVE_EPS,
                        kind: ContactKind::Surface(
                            ImpactTarget::Wireform(wi),
                            self.wireform_material(wi),
                        ),
                        persistent: touches(own, normal),
                        target: 0.0,
                        acc: ContactImpulse::default(),
                        body: BodyContactState::NEW,
                    });
                }
            }
        }

        let time = sc[i].t;
        for (si, set) in bodies.sets.iter().enumerate() {
            for b in 0..set.body_count() {
                if set.layer_mask(b) & mask == 0 || !near(Some(&set.swept_bounds(b)), pos, pos, pad)
                {
                    continue;
                }
                let Some(cp) = set.closest_point(b, time, pos) else {
                    continue;
                };
                let gap = cp.distance - r;
                if gap <= TOUCH_TOL && cp.normal.is_finite() {
                    add(Contact {
                        normal: cp.normal,
                        point: cp.point,
                        depth: -gap,
                        active_eps: BODY_SKIN,
                        kind: ContactKind::Surface(
                            ImpactTarget::Body { set: si, body: b },
                            set.material(b),
                        ),
                        persistent: touches(own, cp.normal),
                        target: 0.0,
                        acc: ContactImpulse::default(),
                        body: BodyContactState::NEW,
                    });
                }
            }
        }

        for j in 0..MAX_BALLS {
            let Some(other) = self.slots[j] else {
                continue;
            };
            if j == i || !sc[j].active || other.layer != ball.layer {
                continue;
            }
            let other_pos = self.position_at(&other, &sc[j], sc[i].t);
            let d = pos.distance(other_pos);
            let gap = d - r - sc[j].body.radius;
            if gap <= TOUCH_TOL && d > 1.0e-9 {
                let normal = (pos - other_pos) / d;
                add(Contact {
                    normal,
                    point: other_pos + normal * sc[j].body.radius,
                    depth: -gap,
                    active_eps: TOUCH_TOL,
                    kind: ContactKind::Ball(j),
                    persistent: touches(own, normal) || touches(&self.touch[j], -normal),
                    target: 0.0,
                    acc: ContactImpulse::default(),
                    body: BodyContactState::NEW,
                });
            }
        }

        for (ci, chain) in self.chains.iter().enumerate() {
            let rc = chain.ball_radius();
            if chain.layer_mask() & mask == 0 || rc.is_nan() || rc <= 0.0 {
                continue;
            }
            for bi in 0..chain.len() {
                let center = chain.ball_position(bi);
                let d = pos.distance(center);
                let gap = d - r - rc;
                if gap <= TOUCH_TOL && d > 1.0e-9 {
                    let normal = (pos - center) / d;
                    add(Contact {
                        normal,
                        point: center + normal * rc,
                        depth: -gap,
                        active_eps: TOUCH_TOL,
                        kind: ContactKind::Chain(ci, bi),
                        persistent: touches(own, normal),
                        target: 0.0,
                        acc: ContactImpulse::default(),
                        body: BodyContactState::NEW,
                    });
                }
            }
        }
        out
    }

    /// First half of a sub-step boundary of ball `i`: gather its contacts,
    /// project it out of penetration and integrate the forces.
    fn boundary_prepare(
        &mut self,
        geo: &Geometry<'_>,
        bounds: &Bounds,
        bodies: &Bodies<'_, '_>,
        i: usize,
        sc: &mut [Scratch; MAX_BALLS],
    ) -> ContactList {
        let mut contacts = self.gather(geo, bounds, bodies, i, sc);
        for c in &mut contacts {
            // Balls may close the gap between them during the sub-step.
            // Surfaces may not: their casts stop a ball anywhere inside the
            // contact skin, so an approach velocity could never be used.
            if !matches!(c.kind, ContactKind::Surface(..)) {
                c.target = c.depth.min(0.0) / sc[i].h;
            }
        }
        // Balls within contact range interact: bring them to this instant.
        for c in &contacts {
            if let ContactKind::Ball(j) = c.kind {
                self.advance_ball(j, sc[i].t, sc);
            }
        }
        let Some(mut ball) = self.slots[i] else {
            return contacts;
        };
        let me = sc[i];

        // The directions that support the ball from now on.
        self.touch[i].clear();
        for c in &contacts {
            add_touch(&mut self.touch[i], c.normal, c.persistent);
        }

        // Position correction. `delta` is the displacement so far, so that
        // two contacts with the same normal are not corrected twice. A ball
        // that rests on a surface a little inside its contact skin (where a
        // cast stopped it) and is not leaving it is seated on the surface:
        // otherwise the rounding residue of its velocity would slowly float
        // it through the skin, to drop back a micrometre every few seconds.
        let mut delta = Vec3::ZERO;
        for c in &contacts {
            let rem = c.depth - delta.dot(c.normal);
            if rem.is_nan() {
                continue;
            }
            match c.kind {
                ContactKind::Ball(j) if rem > 0.0 => {
                    let share = sc[j].body.mass / (me.body.mass + sc[j].body.mass);
                    delta += c.normal * (rem * share);
                    if let Some(other) = self.slots[j].as_mut() {
                        other.position -= c.normal * (rem * (1.0 - share));
                    }
                    sc[j].cache = Cache::Dirty;
                }
                ContactKind::Chain(..) if rem > 0.0 => delta += c.normal * rem,
                ContactKind::Surface(target, _) => {
                    let seat = rem < 0.0
                        && rem >= -c.active_eps
                        && c.persistent
                        && (ball.velocity - self.target_velocity(bodies, target, me.t, c.point))
                            .dot(c.normal)
                            <= SEAT_SPEED;
                    if rem > 0.0 || seat {
                        delta += c.normal * rem;
                    }
                }
                _ => {}
            }
        }
        ball.position += delta;

        // Forces: gravity, then magnets and external hooks. Terms that do
        // not act on this ball are skipped entirely, so they cannot perturb
        // it even by rounding.
        let h = me.h;
        ball.velocity += self.cfg.gravity * h;
        let pd = self.pdata[me.pidx];
        let mask = layers::layer_bit(ball.layer);
        let mut force = Vec3::ZERO;
        let mut any = false;
        if pd.coupling != 0.0 {
            for MagnetState {
                magnet: m, power, ..
            } in &self.magnets
            {
                if *power <= 0.0 || m.layer_mask & mask == 0 {
                    continue;
                }
                let to_magnet = m.position - ball.position;
                let d = to_magnet.length();
                let denom = (d * d * d).max(m.d_min * m.d_min * m.d_min);
                if d <= m.range && d > 1.0e-9 && denom > 0.0 {
                    force += to_magnet * (pd.coupling * power * m.k_coil / (denom * d));
                    any = true;
                }
            }
        }
        let ctx = ForceContext {
            magnets: &self.magnets,
            step_index: self.steps,
            tick_dt: self.tick_dt,
            tick_time: me.t,
            sim_time: self.elapsed + f64::from(me.t),
            substep: me.k,
            substeps: me.n_sub,
        };
        for hook in &mut self.hooks {
            force += hook.force_in(&ctx, BallHandle(i as u8), &ball, &self.profiles[me.pidx], h);
            any = true;
        }
        // An exactly-zero total (e.g. hooks with zero coefficients) is not
        // added, so it cannot even flip the sign of a zero component.
        if any && force.is_finite() && force != Vec3::ZERO {
            ball.velocity += force * (h / me.body.mass);
        }
        self.slots[i] = Some(ball);
        contacts
    }

    /// Impacts among the touching contacts of ball `i`: a contact the ball
    /// was not already resting on, approached at impact speed, bounces with
    /// restitution (one shot) and is reported, exactly as if a cast had
    /// found it.
    fn boundary_impacts(
        &mut self,
        i: usize,
        contacts: &ContactList,
        members: &[bool; MAX_BALLS],
        sc: &mut [Scratch; MAX_BALLS],
        bodies: &mut Bodies<'_, '_>,
        events: &mut BallEvents,
    ) {
        let me = sc[i];
        let min = IMPACT_REPORT_SPEED;
        for c in contacts {
            if c.depth < -c.active_eps || c.persistent {
                continue;
            }
            match c.kind {
                ContactKind::Surface(target, material) => {
                    self.surface_hit(
                        i, &me, c.normal, c.point, target, material, false, min, bodies, events,
                    );
                }
                ContactKind::Ball(j) => {
                    // A pair of two members is handled once, by the lower.
                    if !(members[j] && j < i) && self.pair_hit(i, j, sc, false, min, events) {
                        sc[j].cache = Cache::Dirty;
                    }
                }
                ContactKind::Chain(ci, bi) => {
                    self.chain_hit(i, &me, ci, bi, c.normal, false, min, events);
                }
            }
        }
    }

    /// Starts the resting contacts of ball `i` from the impulses they ended
    /// the previous boundary solve with (scaled to this sub-step). The
    /// impulses stay under the solver's clamps, so one that is no longer
    /// needed is taken back in the first sweep.
    fn warm_start(
        &mut self,
        i: usize,
        contacts: &mut ContactList,
        members: &[bool; MAX_BALLS],
        sc: &mut [Scratch; MAX_BALLS],
    ) {
        let me = sc[i];
        for c in contacts.iter_mut() {
            if c.depth < -c.active_eps || !c.persistent {
                continue;
            }
            let Some(w) = self.warm[i]
                .iter()
                .find(|w| w.kind.same(&c.kind) && w.normal.dot(c.normal) > WARM_COS)
            else {
                continue;
            };
            let scale = me.h / w.h;
            if !scale.is_finite() {
                continue;
            }
            let acc = ContactImpulse {
                normal: w.acc.normal * scale,
                // Friction acts in the tangent plane of the current normal.
                tangent: (w.acc.tangent - c.normal * w.acc.tangent.dot(c.normal)) * scale,
                rolling: w.acc.rolling * scale,
            };
            match c.kind {
                // A body's reaction goes through its own solve (no warm
                // start: the impulses would have to be replayed on it).
                ContactKind::Surface(ImpactTarget::Body { .. }, _) => {}
                ContactKind::Surface(..) => {
                    if let Some(ball) = self.slots[i].as_mut() {
                        apply_surface_impulse(ball, me.body, c.normal, &acc);
                        c.acc = acc;
                    }
                }
                ContactKind::Ball(j) => {
                    if members[j] && j < i {
                        continue;
                    }
                    let (Some(mut a), Some(mut b)) = (self.slots[i], self.slots[j]) else {
                        continue;
                    };
                    let Some(normal) = (a.position - b.position).try_normalize() else {
                        continue;
                    };
                    let acc = ContactImpulse {
                        tangent: acc.tangent - normal * acc.tangent.dot(normal),
                        rolling: Vec3::ZERO,
                        ..acc
                    };
                    apply_ball_impulse(&mut a, me.body, &mut b, sc[j].body, normal, &acc);
                    self.slots[i] = Some(a);
                    self.slots[j] = Some(b);
                    sc[j].cache = Cache::Dirty;
                    c.acc = acc;
                }
                // A captive ball's reaction depends on the whole chain.
                ContactKind::Chain(..) => {}
            }
        }
    }

    /// One velocity-solver sweep over the touching contacts of ball `i`
    /// (resting contacts: no restitution, accumulated impulses): first the
    /// contacts with balls (`surfaces == false`), then, in a second call,
    /// those with surfaces, so that a sweep ends with the surfaces exactly
    /// non-penetrating. Returns the largest velocity change (m/s).
    #[allow(clippy::too_many_arguments)]
    fn boundary_sweep(
        &mut self,
        i: usize,
        contacts: &mut ContactList,
        surfaces: bool,
        members: &[bool; MAX_BALLS],
        settled: &mut [bool; MAX_BALLS],
        sc: &mut [Scratch; MAX_BALLS],
        bodies: &mut Bodies<'_, '_>,
    ) -> f32 {
        let me = sc[i];
        let mut change = 0.0f32;
        for c in contacts.iter_mut() {
            if c.depth < -c.active_eps || matches!(c.kind, ContactKind::Surface(..)) != surfaces {
                continue;
            }
            let moved = match c.kind {
                ContactKind::Surface(ImpactTarget::Body { set, body }, material) => self
                    .body_solve(
                        i,
                        &me,
                        bodies,
                        (set, body),
                        material,
                        c.normal,
                        c.point,
                        c.target,
                        &mut c.acc,
                        &mut c.body,
                    ),
                ContactKind::Surface(target, material) => {
                    let k = self.surface_coefficients(me.pidx, material);
                    let vs = self.surface_velocity_at(target, c.point);
                    match self.slots[i].as_mut() {
                        Some(ball) => solve_surface_contact(
                            ball, me.body, c.normal, vs, k, c.target, &mut c.acc,
                        ),
                        None => 0.0,
                    }
                }
                ContactKind::Ball(j) => {
                    if members[j] && j < i {
                        continue;
                    }
                    let k = self.ball_coefficients(me.pidx, sc[j].pidx);
                    let (Some(mut a), Some(mut b)) = (self.slots[i], self.slots[j]) else {
                        continue;
                    };
                    let Some(normal) = (a.position - b.position).try_normalize() else {
                        continue;
                    };
                    let moved = solve_ball_contact(
                        &mut a, me.body, &mut b, sc[j].body, normal, k, c.target, &mut c.acc,
                    );
                    if moved > 0.0 {
                        self.slots[i] = Some(a);
                        self.slots[j] = Some(b);
                        sc[j].cache = Cache::Dirty;
                    }
                    if moved > PRESOLVE_TOLERANCE {
                        // The partner has to be swept again.
                        settled[j] = false;
                    }
                    moved
                }
                ContactKind::Chain(ci, bi) => {
                    self.chain_solve(i, &me, ci, bi, c.normal, c.target, &mut c.acc.normal)
                }
            };
            change = change.max(moved);
        }
        change
    }

    /// The sub-step boundary at time `time` of every ball that has one
    /// then: contacts are gathered and forces integrated ball by ball, then
    /// the touching contacts of all these balls are solved together
    /// (projected Gauss–Seidel with accumulated impulses), so that balls in
    /// grooves, corners, rows and stacks come to rest.
    #[allow(clippy::needless_range_loop)] // the index addresses several per-ball arrays
    #[allow(clippy::too_many_arguments)]
    fn boundaries(
        &mut self,
        geo: &Geometry<'_>,
        bounds: &Bounds,
        time: f32,
        sc: &mut [Scratch; MAX_BALLS],
        lists: &mut [ContactList; MAX_BALLS],
        bodies: &mut Bodies<'_, '_>,
        events: &mut BallEvents,
    ) {
        let mut members = [false; MAX_BALLS];
        for (member, s) in members.iter_mut().zip(sc.iter()) {
            *member = s.active && s.k < s.n_sub && s.boundary_time() == time;
        }
        for i in 0..MAX_BALLS {
            if members[i] {
                self.advance_ball(i, time, sc);
            }
        }
        for i in 0..MAX_BALLS {
            if members[i] {
                lists[i] = self.boundary_prepare(geo, bounds, bodies, i, sc);
            }
        }
        for i in 0..MAX_BALLS {
            if members[i] {
                self.boundary_impacts(i, &lists[i], &members, sc, bodies, events);
            }
        }
        for i in 0..MAX_BALLS {
            if members[i] {
                self.warm_start(i, &mut lists[i], &members, sc);
            }
        }
        // Each ball is swept until its own contacts have converged: how
        // long one ball iterates must not depend on balls it does not touch.
        let mut settled = [false; MAX_BALLS];
        let mut mid = [0.0f32; MAX_BALLS];
        for pass in 0..PRESOLVE_PASSES {
            let mut change = [0.0f32; MAX_BALLS];
            let sweeping = settled;
            for surfaces in [false, true] {
                for i in 0..MAX_BALLS {
                    if members[i] && !sweeping[i] {
                        let moved = self.boundary_sweep(
                            i,
                            &mut lists[i],
                            surfaces,
                            &members,
                            &mut settled,
                            sc,
                            bodies,
                        );
                        change[i] = change[i].max(moved);
                    }
                }
            }
            let mut all = true;
            for i in 0..MAX_BALLS {
                if members[i] && !sweeping[i] {
                    // A partner's sweep may have reopened this ball.
                    settled[i] = change[i] <= PRESOLVE_TOLERANCE;
                    if pass == STALL_PASSES / 2 {
                        mid[i] = change[i];
                    }
                    if pass == STALL_PASSES
                        && !settled[i]
                        && change[i] > 0.5 * mid[i]
                        && self.kinematic_pinch(i, &lists[i], sc[i].t, bodies)
                    {
                        settled[i] = true;
                    }
                }
                all &= !members[i] || settled[i];
            }
            if all {
                break;
            }
        }
        for i in 0..MAX_BALLS {
            if members[i] {
                let h = sc[i].h;
                self.warm[i].clear();
                for c in lists[i].iter().filter(|c| c.acc.normal > 0.0) {
                    self.warm[i].push(WarmContact {
                        kind: c.kind,
                        normal: c.normal,
                        acc: c.acc,
                        h,
                    });
                }
            }
        }
        for (member, s) in members.iter().zip(sc.iter_mut()) {
            if *member {
                s.events = 0;
                s.frozen = false;
                s.cache = Cache::Dirty;
                s.k += 1;
            }
        }
    }

    /// The next impact of ball `i` with anything but another free ball,
    /// within `window` seconds from `now`.
    #[allow(clippy::too_many_arguments)]
    fn cast_static(
        &self,
        geo: &Geometry<'_>,
        bounds: &Bounds,
        bodies: &Bodies<'_, '_>,
        i: usize,
        sc: &Scratch,
        now: f32,
        window: f32,
    ) -> Cache {
        let Some(ball) = self.slots[i] else {
            return Cache::Free;
        };
        let v = ball.velocity;
        // A ball at rest can still be hit by a moving body.
        let moving = v != Vec3::ZERO;
        if sc.frozen || window.is_nan() || window <= 0.0 || (!moving && bodies.sets.is_empty()) {
            return Cache::Free;
        }
        let r = sc.body.radius;
        let pos = ball.position;
        let mask = layers::layer_bit(ball.layer);
        let mut limit = window;
        let mut best: Option<StaticEvent> = None;

        if moving && let Some(hit) = geo.world.cast_sphere(pos, r, v, limit, mask) {
            limit = hit.time;
            best = Some(StaticEvent::World(hit));
        }
        let end = pos + v * window;
        for (gi, guide) in geo.guides.iter().enumerate() {
            if !moving {
                break;
            }
            if self.guide_mask(gi) & mask == 0
                || !near(bounds.guides.get(gi), pos, end, r + GUIDE_SKIN)
            {
                continue;
            }
            match guide.cast_sphere(pos, r, v, limit, None) {
                Some(GuideCast::Contact(hit)) if hit.time < limit || best.is_none() => {
                    limit = hit.time;
                    best = Some(StaticEvent::Guide(gi, Some(hit)));
                }
                Some(GuideCast::Advance { time }) if time < limit => {
                    limit = time;
                    best = Some(StaticEvent::Guide(gi, None));
                }
                _ => {}
            }
        }
        for (ci, chain) in self.chains.iter().enumerate() {
            let rc = chain.ball_radius();
            if !moving || chain.layer_mask() & mask == 0 || rc.is_nan() || rc <= 0.0 {
                continue;
            }
            for bi in 0..chain.len() {
                let p = pos - chain.ball_position(bi);
                if let Some(t) = sphere_toi_resting(p, v, r + rc, limit)
                    && (t < limit || best.is_none())
                {
                    limit = t;
                    best = Some(StaticEvent::Chain(ci, bi));
                }
            }
        }
        // Once this ball's body-solve budget is spent, it is not cast
        // against bodies either (nothing more would be solved this tick).
        let body_sets = if bodies.starved(i) {
            &[][..]
        } else {
            &bodies.sets[..]
        };
        for (si, set) in body_sets.iter().enumerate() {
            for b in 0..set.body_count() {
                if set.layer_mask(b) & mask == 0
                    || !near(Some(&set.swept_bounds(b)), pos, end, r + BODY_SKIN)
                {
                    continue;
                }
                match set.cast_sphere(b, now, pos, r, v, limit) {
                    Some(BodyCast::Hit {
                        time,
                        point,
                        normal,
                    }) if time >= 0.0 && (time < limit || best.is_none()) => {
                        limit = time;
                        best = Some(StaticEvent::Body(si, b, Some((point, normal))));
                    }
                    Some(BodyCast::Advance { time }) if time > 0.0 && time < limit => {
                        limit = time;
                        best = Some(StaticEvent::Body(si, b, None));
                    }
                    _ => {}
                }
            }
        }
        match best {
            Some(ev) => Cache::Event(now + limit, ev),
            None => Cache::Free,
        }
    }

    /// Ball `i` reaches a surface during a sub-step.
    ///
    /// If the ball does not rest in that direction yet, this is an impact:
    /// one shot, with restitution, reported. Otherwise it is a continuing
    /// contact (a ball sliding along a curved wall, settling into a groove):
    /// the contact joins the ball's contact list of the current sub-step and
    /// the list is solved again with its accumulated impulses, so the other
    /// contacts can give way. Returns whether any velocity changed.
    #[allow(clippy::too_many_arguments)]
    fn surface_event(
        &mut self,
        i: usize,
        normal: Vec3,
        point: Vec3,
        target: ImpactTarget,
        material: Material,
        sc: &mut [Scratch; MAX_BALLS],
        lists: &mut [ContactList; MAX_BALLS],
        bodies: &mut Bodies<'_, '_>,
        events: &mut BallEvents,
    ) -> bool {
        let me = sc[i];
        let resting = rests_on(&self.touch[i], normal);
        add_touch(&mut self.touch[i], normal, true);
        let list = &mut lists[i];
        let known = list.iter().position(|c| {
            matches!(c.kind, ContactKind::Surface(t, _) if t == target)
                && c.normal.dot(normal) > 0.999
        });
        if !resting || (known.is_none() && list.is_full()) {
            return self.surface_hit(
                i, &me, normal, point, target, material, resting, 0.0, bodies, events,
            );
        }
        match known {
            // The boundary solve's contact takes part from now on, with the
            // geometry of this hit.
            Some(k) => {
                let c = &mut list[k];
                c.normal = normal;
                c.point = point;
                c.depth = c.depth.max(0.0);
            }
            None => list.push(Contact {
                normal,
                point,
                depth: 0.0,
                active_eps: 0.0,
                kind: ContactKind::Surface(target, material),
                persistent: true,
                target: 0.0,
                acc: ContactImpulse::default(),
                body: BodyContactState::NEW,
            }),
        }
        // Balls in the list take part: bring them to this instant.
        for k in 0..list.len() {
            if let ContactKind::Ball(j) = list[k].kind
                && list[k].depth >= -list[k].active_eps
            {
                self.advance_ball(j, me.t, sc);
            }
        }
        // Other balls touching the same moving body are solved together
        // with this one (a ball resting on a bat that another ball knocks):
        // alternating single-ball events would converge slowly through the
        // body and run into the per-sub-step event limit.
        let mut partners = [false; MAX_BALLS];
        if let ImpactTarget::Body { set, body } = target {
            for j in 0..MAX_BALLS {
                partners[j] = j != i
                    && sc[j].active
                    && lists[j].iter().any(|c| {
                        c.depth >= -c.active_eps
                            && matches!(c.kind, ContactKind::Surface(ImpactTarget::Body { set: s, body: b }, _) if s == set && b == body)
                    });
                if partners[j] {
                    self.advance_ball(j, me.t, sc);
                }
            }
        }
        // A new solve: what the earlier solves learnt about the bodies
        // (observed responses, impulse caps) may be stale now, e.g. after
        // another ball knocked a body into this one.
        for (j, list) in lists.iter_mut().enumerate() {
            if j == i || partners[j] {
                for c in list.iter_mut() {
                    c.body = BodyContactState::NEW;
                }
            }
        }
        let alone = [false; MAX_BALLS];
        let mut unused = [false; MAX_BALLS];
        let mut total = 0.0f32;
        let mut mid = 0.0f32;
        for pass in 0..PRESOLVE_PASSES {
            let mut change = 0.0f32;
            for j in 0..MAX_BALLS {
                if j == i || partners[j] {
                    let list = &mut lists[j];
                    change = change
                        .max(self.boundary_sweep(j, list, false, &alone, &mut unused, sc, bodies))
                        .max(self.boundary_sweep(j, list, true, &alone, &mut unused, sc, bodies));
                }
            }
            total = total.max(change);
            if change <= PRESOLVE_TOLERANCE {
                break;
            }
            if pass == STALL_PASSES / 2 {
                mid = change;
            }
            if pass == STALL_PASSES
                && change > 0.5 * mid
                && self.kinematic_pinch(i, &lists[i], me.t, bodies)
            {
                break;
            }
        }
        for j in 0..MAX_BALLS {
            if partners[j] {
                sc[j].cache = Cache::Dirty;
            }
        }
        total > 0.0
    }

    /// Applies the impact stored in the cache of ball `i`.
    fn static_event(
        &mut self,
        i: usize,
        ev: StaticEvent,
        sc: &mut [Scratch; MAX_BALLS],
        lists: &mut [ContactList; MAX_BALLS],
        bodies: &mut Bodies<'_, '_>,
        events: &mut BallEvents,
    ) {
        let me = sc[i];
        // An impact that changes nothing (a surface receding faster than the
        // ball follows) would repeat forever: the ball waits instead.
        let mut applied = true;
        match ev {
            StaticEvent::World(hit) => {
                let target = ImpactTarget::Collider(hit.collider);
                let material = self.collider_material(hit.collider);
                applied = self.surface_event(
                    i, hit.normal, hit.point, target, material, sc, lists, bodies, events,
                );
            }
            StaticEvent::Guide(gi, Some(hit)) => {
                let material = self.guide_material(gi);
                let gap = self.slots[i].map_or(0.0, |b| {
                    (b.position - hit.point).dot(hit.normal) - me.body.radius
                });
                applied = gap < 0.0;
                applied |= self.surface_event(
                    i,
                    hit.normal,
                    hit.point,
                    ImpactTarget::Guide(gi),
                    material,
                    sc,
                    lists,
                    bodies,
                    events,
                );
                // A guide cast lets a sliding ball sink into its skin; put
                // the ball back on the wall so that the next cast makes
                // progress.
                if gap < 0.0
                    && let Some(ball) = self.slots[i].as_mut()
                {
                    ball.position -= hit.normal * gap;
                }
            }
            StaticEvent::Guide(_, None) | StaticEvent::Body(_, _, None) => {}
            StaticEvent::Body(si, b, Some((point, normal))) => {
                let material = bodies
                    .sets
                    .get(si)
                    .map_or(Material::Steel, |s| s.material(b));
                let target = ImpactTarget::Body { set: si, body: b };
                applied = self.surface_event(
                    i, normal, point, target, material, sc, lists, bodies, events,
                );
            }
            StaticEvent::Chain(ci, bi) => {
                if let Some(ball) = self.slots[i] {
                    let offset = ball.position - self.chains[ci].ball_position(bi);
                    let d = offset.length();
                    if d > 1.0e-9 {
                        let normal = offset / d;
                        let persistent = rests_on(&self.touch[i], normal);
                        applied = self.chain_hit(i, &me, ci, bi, normal, persistent, 0.0, events);
                        add_touch(&mut self.touch[i], normal, true);
                    }
                }
            }
        }
        sc[i].count_event();
        if !applied {
            sc[i].frozen = true;
        }
    }

    fn pair_event(
        &mut self,
        i: usize,
        j: usize,
        sc: &mut [Scratch; MAX_BALLS],
        events: &mut BallEvents,
    ) {
        let mut applied = false;
        if let (Some(a), Some(b)) = (self.slots[i], self.slots[j]) {
            let offset = a.position - b.position;
            let d = offset.length();
            if d > 1.0e-9 {
                let normal = offset / d;
                let persistent =
                    rests_on(&self.touch[i], normal) || rests_on(&self.touch[j], -normal);
                applied = self.pair_hit(i, j, sc, persistent, 0.0, events);
                add_touch(&mut self.touch[i], normal, true);
                add_touch(&mut self.touch[j], -normal, true);
            }
        }
        sc[i].count_event();
        sc[j].count_event();
        if !applied {
            // One of them is waiting (frozen) and really moving away, or
            // rounding: neither can make progress against the other now.
            sc[i].frozen = true;
            sc[j].frozen = true;
        }
    }

    /// Advances all balls and Newton chains by `dt` seconds (the engine
    /// passes 0.001). Impacts and sensor edges are appended to `events`
    /// (which is not cleared). Never allocates.
    ///
    /// Layer transitions are not handled here: the caller runs
    /// [`crate::world::TransitionSet::update`] and writes
    /// [`Ball::layer`] through [`ball_mut`](Self::ball_mut).
    ///
    /// Same as [`step_with_bodies`](Self::step_with_bodies) with no bodies.
    pub fn step(&mut self, geo: &Geometry<'_>, dt: f32, events: &mut BallEvents) {
        self.step_with_bodies(geo, &mut [], dt, events);
    }

    /// [`step`](Self::step) with moving bodies taking part: `bodies` are
    /// the body sets, in a fixed order (a set's index in this slice is the
    /// `set` of [`ImpactTarget::Body`]). The call order is
    /// [`MovingBodies::begin_tick`] for each set, Newton chains, the balls'
    /// event loop (queries and impulses on the bodies),
    /// [`MovingBodies::end_tick`] for each set, then the sensors; see
    /// [`crate::ball::body`] for the full contract. A non-finite or
    /// non-positive `dt` does nothing (no body is called).
    #[allow(clippy::needless_range_loop)] // the index addresses several per-ball arrays
    pub fn step_with_bodies(
        &mut self,
        geo: &Geometry<'_>,
        bodies: &mut [&mut dyn MovingBodies],
        dt: f32,
        events: &mut BallEvents,
    ) {
        if dt.is_nan() || dt <= 0.0 || !dt.is_finite() {
            return;
        }
        self.tick_dt = dt;
        for set in bodies.iter_mut() {
            set.begin_tick(dt);
        }
        let gravity = self.cfg.gravity;
        for chain in &mut self.chains {
            if chain.ball_radius() > 0.0 {
                chain.advance(dt, gravity, self.cfg.rolling_resistance);
            }
        }

        let mut sc = [Scratch::IDLE; MAX_BALLS];
        let mut any_ball = false;
        for (i, (slot, s)) in self.slots.iter().zip(sc.iter_mut()).enumerate() {
            let Some(ball) = slot else {
                continue;
            };
            if ball.position != self.last_position[i] {
                self.carry[i] = Vec3::ZERO;
            }
            let Some(pd) = self.pdata.get(ball.profile.index()).filter(|p| p.valid) else {
                continue;
            };
            if !(ball.position.is_finite()
                && ball.velocity.is_finite()
                && ball.angular_velocity.is_finite())
            {
                continue;
            }
            s.active = true;
            s.body = pd.body;
            s.pidx = ball.profile.index();
            any_ball = true;
        }

        if any_ball {
            let bounds = Bounds::compute(geo);
            let max_substeps = self.cfg.max_substeps.max(1);
            for i in 0..MAX_BALLS {
                let (Some(ball), true) = (self.slots[i], sc[i].active) else {
                    continue;
                };
                // Sub-steps from the ball's own speed and size only.
                let r = sc[i].body.radius;
                let mut travel = (ball.velocity.length() + gravity.length() * dt) * dt;
                let mask = layers::layer_bit(ball.layer);
                let end = ball.position + ball.velocity * dt;
                // A body that can reach the ball's path this tick adds its
                // surface speed: contacts with it are re-solved as often.
                let mut body_speed = 0.0f32;
                for set in bodies.iter() {
                    for b in 0..set.body_count() {
                        if set.layer_mask(b) & mask != 0
                            && near(Some(&set.swept_bounds(b)), ball.position, end, 2.0 * r)
                        {
                            let u = set.max_surface_speed(b);
                            if u.is_finite() {
                                body_speed = body_speed.max(u);
                            }
                        }
                    }
                }
                if body_speed > 0.0 {
                    travel += body_speed * dt;
                }
                let on_rails = geo.wireforms.iter().enumerate().any(|(wi, _)| {
                    self.wireform_mask(wi) & mask != 0
                        && near(bounds.wires.get(wi), ball.position, end, 2.0 * r)
                });
                let fraction = if on_rails {
                    RAIL_TRAVEL_FRACTION
                } else {
                    TRAVEL_FRACTION
                };
                let wanted = libm::ceilf(travel / (fraction * r));
                let n = if wanted.is_finite() && wanted >= 1.0 {
                    wanted.min(max_substeps as f32) as u32
                } else {
                    1
                };
                sc[i].n_sub = n;
                sc[i].h = dt / n as f32;
                sc[i].t0 = 0.0;
                sc[i].cache = Cache::Dirty;
            }
            let mut ctx = Bodies {
                sets: &mut *bodies,
                moved: false,
                solves: [0; MAX_BALLS],
                total: 0,
            };
            self.run_timeline(geo, &bounds, dt, &mut sc, &mut ctx, events);
            self.body_solves = ctx.solves;
        } else {
            self.body_solves = [0; MAX_BALLS];
        }
        for set in bodies.iter_mut() {
            set.end_tick(dt);
        }
        for (slot, last) in self.slots.iter().zip(self.last_position.iter_mut()) {
            if let Some(ball) = slot {
                *last = ball.position;
            }
        }
        self.steps += 1;
        self.elapsed += f64::from(dt);
        self.update_sensors(events);
    }

    /// The event loop of one tick.
    #[allow(clippy::needless_range_loop)] // the index addresses several per-ball arrays
    fn run_timeline(
        &mut self,
        geo: &Geometry<'_>,
        bounds: &Bounds,
        dt: f32,
        sc: &mut [Scratch; MAX_BALLS],
        bodies: &mut Bodies<'_, '_>,
        events: &mut BallEvents,
    ) {
        let mut now = 0.0f32;
        let mut complete = false;
        let mut lists: [ContactList; MAX_BALLS] = std::array::from_fn(|_| ContactList::new());
        let lists = &mut lists;
        // Every loop pass consumes a boundary or an impact; impacts are
        // limited per ball and sub-step, so this bound is never reached. It
        // only protects the tick's time budget against a logic error.
        let total_substeps: u32 = sc.iter().map(|s| s.n_sub).sum();
        let mut budget = (total_substeps + 1) * (2 * u32::from(MAX_EVENTS_PER_SUBSTEP) + 4);
        loop {
            let mut best_time = dt;
            let mut best = Next::End;
            // Sub-step boundaries first: at equal times they precede impacts.
            for s in sc.iter() {
                if s.active && s.k < s.n_sub {
                    let tb = s.boundary_time();
                    if tb < best_time {
                        best_time = tb;
                        best = Next::Boundary;
                    }
                }
            }
            for i in 0..MAX_BALLS {
                if !sc[i].active {
                    continue;
                }
                if matches!(sc[i].cache, Cache::Dirty) {
                    // A ball is only marked dirty at its own clock.
                    let from = sc[i].t;
                    let window = sc[i].horizon(dt) - from;
                    sc[i].cache = self.cast_static(geo, bounds, bodies, i, &sc[i], from, window);
                }
                if let Cache::Event(t, _) = sc[i].cache
                    && t < best_time
                {
                    best_time = t;
                    best = Next::Static(i);
                }
            }
            for i in 0..MAX_BALLS {
                let (Some(a), true) = (self.slots[i], sc[i].active) else {
                    continue;
                };
                let va = if sc[i].frozen { Vec3::ZERO } else { a.velocity };
                let pa = self.position_at(&a, &sc[i], now);
                for j in i + 1..MAX_BALLS {
                    let (Some(b), true) = (self.slots[j], sc[j].active) else {
                        continue;
                    };
                    if a.layer != b.layer {
                        continue;
                    }
                    let vb = if sc[j].frozen { Vec3::ZERO } else { b.velocity };
                    let pb = self.position_at(&b, &sc[j], now);
                    let rsum = sc[i].body.radius + sc[j].body.radius;
                    if let Some(t) = sphere_toi_resting(pa - pb, va - vb, rsum, best_time - now)
                        && now + t < best_time
                    {
                        best_time = now + t;
                        best = Next::Pair(i, j);
                    }
                }
            }

            // Only the balls that take part in the event move to it.
            now = now.max(best_time);
            match best {
                Next::End => {
                    complete = true;
                    break;
                }
                Next::Boundary => self.boundaries(geo, bounds, now, sc, lists, bodies, events),
                Next::Static(i) => {
                    self.advance_ball(i, now, sc);
                    if let Cache::Event(_, ev) = sc[i].cache {
                        self.static_event(i, ev, sc, lists, bodies, events);
                    }
                }
                Next::Pair(i, j) => {
                    self.advance_ball(i, now, sc);
                    self.advance_ball(j, now, sc);
                    self.pair_event(i, j, sc, events);
                }
            }
            if bodies.moved {
                // A body changed its trajectory at `now`: bring every ball
                // to this instant and cast it again (no body query is ever
                // made for a time before its last impulse).
                bodies.moved = false;
                for j in 0..MAX_BALLS {
                    if sc[j].active {
                        self.advance_ball(j, now, sc);
                        sc[j].cache = Cache::Dirty;
                        let added = self.refine_substeps(j, now, dt, sc, bodies);
                        budget = budget
                            .saturating_add(added * (2 * u32::from(MAX_EVENTS_PER_SUBSTEP) + 4));
                    }
                }
            }
            budget = budget.saturating_sub(1);
            if budget == 0 {
                break;
            }
        }
        // Nothing lies between a ball's clock and the end of the tick (unless
        // the safety budget ran out: then the remaining time is dropped).
        for i in 0..MAX_BALLS {
            if complete && sc[i].active {
                self.advance_ball(i, dt, sc);
            }
        }
    }

    /// After a body changed its trajectory at `now` (Subphase 2.8): a ball
    /// larger than a standard ball gets a finer sub-step grid for the rest
    /// of the tick when a body near it now moves faster than its grid was
    /// planned for (at the start of the tick a resting bat has speed 0). A
    /// moving body then sweeps at most `TRAVEL_FRACTION · r` between the
    /// ball's boundaries, as the ball itself does: the boundaries are where
    /// a touching, non-closing body contact is solved (the casts leave it),
    /// so with the tick-start grid a large ball could end a tick millimetres
    /// inside a bat that an impact had just set spinning. Balls of radius
    /// up to [`REFINE_MIN_RADIUS`] keep their grid (bit-identical to before).
    /// Returns the number of boundaries added.
    fn refine_substeps(
        &self,
        i: usize,
        now: f32,
        dt: f32,
        sc: &mut [Scratch; MAX_BALLS],
        bodies: &Bodies<'_, '_>,
    ) -> u32 {
        let s = sc[i];
        let r = s.body.radius;
        let remaining = dt - now;
        if r.is_nan()
            || r <= REFINE_MIN_RADIUS
            || s.frozen
            || remaining.is_nan()
            || remaining <= 0.0
        {
            return 0;
        }
        let Some(ball) = self.slots[i] else {
            return 0;
        };
        let mask = layers::layer_bit(ball.layer);
        let end = ball.position + ball.velocity * remaining;
        let mut body_speed = 0.0f32;
        for set in bodies.sets.iter() {
            for b in 0..set.body_count() {
                if set.layer_mask(b) & mask != 0
                    && near(Some(&set.swept_bounds(b)), ball.position, end, 2.0 * r)
                {
                    let u = set.max_surface_speed(b);
                    if u.is_finite() {
                        body_speed = body_speed.max(u);
                    }
                }
            }
        }
        let travel = (ball.velocity.length() + body_speed) * remaining;
        let wanted = libm::ceilf(travel / (TRAVEL_FRACTION * r));
        let left = s.n_sub.saturating_sub(s.k) + 1;
        let cap = self.cfg.max_substeps.max(1);
        if !wanted.is_finite() || wanted <= left as f32 || s.n_sub >= cap {
            return 0;
        }
        let n = (wanted as u32)
            .min(cap.saturating_sub(s.n_sub) + left)
            .max(1);
        if n <= left {
            return 0;
        }
        let h = remaining / n as f32;
        if h.is_nan() || h <= 0.0 || h >= s.h {
            return 0;
        }
        // Boundaries at now + h, now + 2h, ..., the last interval ends at dt.
        sc[i].t0 = now;
        sc[i].h = h;
        sc[i].k = 1;
        sc[i].n_sub = n;
        n - left
    }

    fn update_sensors(&mut self, events: &mut BallEvents) {
        let slots = &self.slots;
        let pdata = &self.pdata;
        for (s, closed) in &mut self.sensors {
            let now = slots.iter().flatten().any(|ball| {
                let Some(pd) = pdata.get(ball.profile.index()) else {
                    return false;
                };
                s.layer_mask & layers::layer_bit(ball.layer) != 0
                    && (s.kind == SensorKind::Opto || pd.conductive)
                    && ball.position.distance(s.center) <= s.radius
            });
            if now != *closed {
                *closed = now;
                events.push(BallEvent::SensorEdge {
                    switch: s.switch,
                    closed: now,
                });
            }
        }
    }
}
