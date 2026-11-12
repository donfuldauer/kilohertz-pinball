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
//!   by swept-sphere casts, where the impact impulse is applied.
//!
//! Nothing here allocates; iteration order is fixed (ascending handles,
//! collider and guide order), so a step is deterministic.

use arrayvec::ArrayVec;
use glam::Vec3;
use pin_core::ids::{MagnetId, ProfileId};
use pin_core::{BallProfile, Material};

use super::contact::{
    ContactCoefficients, ContactImpulse, SphereBody, effective_restitution, integrate_orientation,
    resolve_ball_contact, resolve_surface_contact, solve_ball_contact, solve_surface_contact,
    sphere_toi,
};
use super::newton::NewtonChain;
use super::types::{
    Ball, BallEvent, BallEvents, BallHandle, BallSolverConfig, ExternalForce, Geometry,
    IMPACT_REPORT_SPEED, ImpactTarget, MAX_BALLS, Magnet, Sensor, SensorKind, SurfaceVelocity,
};
use crate::world::{
    ColliderHandle, CubicSpline, GuideCast, GuideHit, SphereContact, SphereHit, layers,
};

/// Gap (metres) within which a surface counts as touched: a later impact on
/// a surface the ball already touches in that direction is a continuing
/// contact (no restitution, no impact event).
const TOUCH_TOL: f32 = 2.0e-5;
/// Contacts are solved at a sub-step boundary only when they overlap (to
/// within position rounding); anything further away is left to the casts.
const ACTIVE_EPS: f32 = 2.0e-7;
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
const PRESOLVE_PASSES: usize = 10;
/// The sweeps stop when one changes no velocity by more than this (m/s).
const PRESOLVE_TOLERANCE: f32 = 1.0e-7;
/// Closing speed (m/s) below which two touching balls are left to the next
/// sub-step boundary instead of raising an impact (they sink by at most a
/// micrometre per millisecond, which the position correction removes).
const REST_SLOP_SPEED: f32 = 1.0e-3;
/// Travel per sub-step as a fraction of the ball radius.
const TRAVEL_FRACTION: f32 = 0.25;
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
    events: u8,
    frozen: bool,
    cache: Cache,
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
        events: 0,
        frozen: false,
        cache: Cache::Free,
    };

    /// Time of the next sub-step boundary, or `dt` when none is left.
    fn horizon(&self, dt: f32) -> f32 {
        if self.k < self.n_sub {
            self.k as f32 * self.h
        } else {
            dt
        }
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
    /// Impulses applied by the boundary solve.
    acc: ContactImpulse,
}

enum Next {
    End,
    Boundary,
    Static(usize),
    Pair(usize, usize),
}

/// The ball dynamics core. See the [module documentation](self).
pub struct BallSolver {
    cfg: BallSolverConfig,
    profiles: Vec<BallProfile>,
    pdata: Vec<ProfileData>,
    /// (restitution, friction) per material pair, if the table has it.
    pair_table: [[Option<(f32, f32)>; MATERIALS]; MATERIALS],
    slots: [Option<Ball>; MAX_BALLS],
    /// Directions in which each ball is currently supported.
    touch: [TouchSet; MAX_BALLS],
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
    magnets: Vec<(Magnet, f32)>,
    sensors: Vec<(Sensor, bool)>,
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
            carry: [Vec3::ZERO; MAX_BALLS],
            last_position: [Vec3::ZERO; MAX_BALLS],
            collider_materials: Vec::new(),
            guide_materials: Vec::new(),
            guide_layers: Vec::new(),
            wireform_materials: Vec::new(),
            wireform_layers: Vec::new(),
            magnets: Vec::new(),
            sensors: Vec::new(),
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
    pub fn add_ball(&mut self, ball: Ball) -> Option<BallHandle> {
        self.profile_data(ball.profile)?;
        let i = self.slots.iter().position(Option::is_none)?;
        self.slots[i] = Some(ball);
        self.touch[i].clear();
        self.carry[i] = Vec3::ZERO;
        self.last_position[i] = ball.position;
        Some(BallHandle(i as u8))
    }

    /// Removes a ball and returns its last state.
    pub fn remove_ball(&mut self, h: BallHandle) -> Option<Ball> {
        self.slots.get_mut(usize::from(h.0))?.take()
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

    /// Adds an electromagnet, initially off. Ids count up from 0.
    pub fn add_magnet(&mut self, m: Magnet) -> MagnetId {
        self.magnets.push((m, 0.0));
        MagnetId((self.magnets.len() - 1) as u16)
    }

    /// Sets the commanded power of a magnet, clamped to `[0, 1]` (NaN = 0).
    /// Unknown ids are ignored.
    pub fn set_magnet_power(&mut self, id: MagnetId, p: f32) {
        if let Some(m) = self.magnets.get_mut(id.index()) {
            m.1 = if p.is_nan() { 0.0 } else { p.clamp(0.0, 1.0) };
        }
    }

    /// Commanded power of a magnet (0 for an unknown id).
    #[must_use]
    pub fn magnet_power(&self, id: MagnetId) -> f32 {
        self.magnets.get(id.index()).map_or(0.0, |m| m.1)
    }

    /// Adds a sensor, initially open. Setup-time only.
    pub fn add_sensor(&mut self, s: Sensor) {
        self.sensors.push((s, false));
    }

    /// Registers an external force (setup-time only). With no force
    /// registered the solver takes exactly the same floating-point path as
    /// if the hook list did not exist.
    pub fn add_external_force(&mut self, f: Box<dyn ExternalForce>) {
        self.hooks.push(f);
    }

    /// Installs (or removes) the source of surface velocities for moving
    /// colliders. Setup-time only.
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
        events: &mut BallEvents,
    ) -> bool {
        let Some(mut ball) = self.slots[i] else {
            return false;
        };
        let k = self.surface_coefficients(sc.pidx, material);
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

    /// Collects everything ball `i` touches (within [`TOUCH_TOL`]).
    #[allow(clippy::needless_range_loop)] // the index addresses several per-ball arrays
    fn gather(
        &self,
        geo: &Geometry<'_>,
        bounds: &Bounds,
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
                acc: ContactImpulse::default(),
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
                    acc: ContactImpulse::default(),
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
                        acc: ContactImpulse::default(),
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
            let d = pos.distance(other.position);
            let gap = d - r - sc[j].body.radius;
            if gap <= TOUCH_TOL && d > 1.0e-9 {
                let normal = (pos - other.position) / d;
                add(Contact {
                    normal,
                    point: other.position + normal * sc[j].body.radius,
                    depth: -gap,
                    active_eps: TOUCH_TOL,
                    kind: ContactKind::Ball(j),
                    persistent: touches(own, normal) || touches(&self.touch[j], -normal),
                    acc: ContactImpulse::default(),
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
                        acc: ContactImpulse::default(),
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
        i: usize,
        sc: &mut [Scratch; MAX_BALLS],
    ) -> ContactList {
        let contacts = self.gather(geo, bounds, i, sc);
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
        // two contacts with the same normal are not corrected twice.
        let mut delta = Vec3::ZERO;
        for c in &contacts {
            let rem = c.depth - delta.dot(c.normal);
            if rem.is_nan() || rem <= 0.0 {
                continue;
            }
            match c.kind {
                ContactKind::Ball(j) => {
                    let share = sc[j].body.mass / (me.body.mass + sc[j].body.mass);
                    delta += c.normal * (rem * share);
                    if let Some(other) = self.slots[j].as_mut() {
                        other.position -= c.normal * (rem * (1.0 - share));
                    }
                    sc[j].cache = Cache::Dirty;
                }
                _ => delta += c.normal * rem,
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
            for (m, power) in &self.magnets {
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
        for hook in &mut self.hooks {
            force += hook.force(BallHandle(i as u8), &ball, &self.profiles[me.pidx], h);
            any = true;
        }
        if any && force.is_finite() {
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
                        i, &me, c.normal, c.point, target, material, false, min, events,
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

    /// One velocity-solver sweep over the touching contacts of ball `i`
    /// (resting contacts: no restitution, accumulated impulses): first the
    /// contacts with balls (`surfaces == false`), then, in a second call,
    /// those with surfaces, so that a sweep ends with the surfaces exactly
    /// non-penetrating. Returns the largest velocity change (m/s).
    fn boundary_sweep(
        &mut self,
        i: usize,
        contacts: &mut ContactList,
        surfaces: bool,
        members: &[bool; MAX_BALLS],
        sc: &mut [Scratch; MAX_BALLS],
    ) -> f32 {
        let me = sc[i];
        let mut change = 0.0f32;
        for c in contacts.iter_mut() {
            if c.depth < -c.active_eps || matches!(c.kind, ContactKind::Surface(..)) != surfaces {
                continue;
            }
            let moved = match c.kind {
                ContactKind::Surface(target, material) => {
                    let k = self.surface_coefficients(me.pidx, material);
                    let vs = self.surface_velocity_at(target, c.point);
                    match self.slots[i].as_mut() {
                        Some(ball) => {
                            solve_surface_contact(ball, me.body, c.normal, vs, k, 0.0, &mut c.acc)
                        }
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
                        &mut a, me.body, &mut b, sc[j].body, normal, k, 0.0, &mut c.acc,
                    );
                    if moved > 0.0 {
                        self.slots[i] = Some(a);
                        self.slots[j] = Some(b);
                        sc[j].cache = Cache::Dirty;
                    }
                    moved
                }
                ContactKind::Chain(ci, bi) => {
                    self.chain_solve(i, &me, ci, bi, c.normal, 0.0, &mut c.acc.normal)
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
    fn boundaries(
        &mut self,
        geo: &Geometry<'_>,
        bounds: &Bounds,
        time: f32,
        sc: &mut [Scratch; MAX_BALLS],
        lists: &mut [ContactList; MAX_BALLS],
        events: &mut BallEvents,
    ) {
        let mut members = [false; MAX_BALLS];
        for (member, s) in members.iter_mut().zip(sc.iter()) {
            *member = s.active && s.k < s.n_sub && s.k as f32 * s.h == time;
        }
        for i in 0..MAX_BALLS {
            if members[i] {
                lists[i] = self.boundary_prepare(geo, bounds, i, sc);
            }
        }
        for i in 0..MAX_BALLS {
            if members[i] {
                self.boundary_impacts(i, &lists[i], &members, sc, events);
            }
        }
        for _ in 0..PRESOLVE_PASSES {
            let mut change = 0.0f32;
            for surfaces in [false, true] {
                for i in 0..MAX_BALLS {
                    if members[i] {
                        change = change.max(self.boundary_sweep(
                            i,
                            &mut lists[i],
                            surfaces,
                            &members,
                            sc,
                        ));
                    }
                }
            }
            if change <= PRESOLVE_TOLERANCE {
                break;
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
    fn cast_static(
        &self,
        geo: &Geometry<'_>,
        bounds: &Bounds,
        i: usize,
        sc: &Scratch,
        now: f32,
        window: f32,
    ) -> Cache {
        let Some(ball) = self.slots[i] else {
            return Cache::Free;
        };
        let v = ball.velocity;
        if sc.frozen || window.is_nan() || window <= 0.0 || v == Vec3::ZERO {
            return Cache::Free;
        }
        let r = sc.body.radius;
        let pos = ball.position;
        let mask = layers::layer_bit(ball.layer);
        let mut limit = window;
        let mut best: Option<StaticEvent> = None;

        if let Some(hit) = geo.world.cast_sphere(pos, r, v, limit, mask) {
            limit = hit.time;
            best = Some(StaticEvent::World(hit));
        }
        let end = pos + v * window;
        for (gi, guide) in geo.guides.iter().enumerate() {
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
            if chain.layer_mask() & mask == 0 || rc.is_nan() || rc <= 0.0 {
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
                i, &me, normal, point, target, material, resting, 0.0, events,
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
                acc: ContactImpulse::default(),
            }),
        }
        let alone = [false; MAX_BALLS];
        let mut total = 0.0f32;
        for _ in 0..PRESOLVE_PASSES {
            let change = self
                .boundary_sweep(i, list, false, &alone, sc)
                .max(self.boundary_sweep(i, list, true, &alone, sc));
            total = total.max(change);
            if change <= PRESOLVE_TOLERANCE {
                break;
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
                    i, hit.normal, hit.point, target, material, sc, lists, events,
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
            StaticEvent::Guide(_, None) => {}
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
    #[allow(clippy::needless_range_loop)] // the index addresses several per-ball arrays
    pub fn step(&mut self, geo: &Geometry<'_>, dt: f32, events: &mut BallEvents) {
        if dt.is_nan() || dt <= 0.0 || !dt.is_finite() {
            return;
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
                let travel = (ball.velocity.length() + gravity.length() * dt) * dt;
                let mask = layers::layer_bit(ball.layer);
                let end = ball.position + ball.velocity * dt;
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
                sc[i].cache = Cache::Dirty;
            }
            self.run_timeline(geo, &bounds, dt, &mut sc, events);
        }
        for (slot, last) in self.slots.iter().zip(self.last_position.iter_mut()) {
            if let Some(ball) = slot {
                *last = ball.position;
            }
        }
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
        events: &mut BallEvents,
    ) {
        let mut now = 0.0f32;
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
                    let tb = s.k as f32 * s.h;
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
                    let window = sc[i].horizon(dt) - now;
                    sc[i].cache = self.cast_static(geo, bounds, i, &sc[i], now, window);
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
                for j in i + 1..MAX_BALLS {
                    let (Some(b), true) = (self.slots[j], sc[j].active) else {
                        continue;
                    };
                    if a.layer != b.layer {
                        continue;
                    }
                    let vb = if sc[j].frozen { Vec3::ZERO } else { b.velocity };
                    let rsum = sc[i].body.radius + sc[j].body.radius;
                    if let Some(t) =
                        sphere_toi_resting(a.position - b.position, va - vb, rsum, best_time - now)
                        && now + t < best_time
                    {
                        best_time = now + t;
                        best = Next::Pair(i, j);
                    }
                }
            }

            // Move everything to the event.
            let tau = best_time - now;
            if tau > 0.0 {
                for (i, (slot, s)) in self.slots.iter_mut().zip(sc.iter()).enumerate() {
                    if let (Some(ball), true, false) = (slot.as_mut(), s.active, s.frozen) {
                        // Compensated sum: the rounding error of this
                        // addition is carried into the next one.
                        let step = ball.velocity * tau - self.carry[i];
                        let moved = ball.position + step;
                        self.carry[i] = (moved - ball.position) - step;
                        ball.position = moved;
                        ball.orientation =
                            integrate_orientation(ball.orientation, ball.angular_velocity, tau);
                    }
                }
                now = best_time;
            }

            match best {
                Next::End => break,
                Next::Boundary => self.boundaries(geo, bounds, now, sc, lists, events),
                Next::Static(i) => {
                    if let Cache::Event(_, ev) = sc[i].cache {
                        self.static_event(i, ev, sc, lists, events);
                    }
                }
                Next::Pair(i, j) => self.pair_event(i, j, sc, events),
            }
            budget = budget.saturating_sub(1);
            if budget == 0 {
                break;
            }
        }
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
