//! Magnetic remanence (PLAN.md §3.2): per-ball magnetisation `M` and the
//! residual pull `F_res = c·ρ·M·K_coil·d̂ / max(d³, d_min³)`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use glam::Vec3;
use pin_core::BallProfile;

use super::magnus::NO_FORCE;
use crate::ball::{Ball, BallHandle, ExternalForce, ForceContext, MAX_BALLS, MagnetState};

/// No magnet has magnetised the ball yet.
const NO_SOURCE: u32 = u32::MAX;

/// Remanence τ used before any magnet has touched the ball (only matters
/// for a value of `M` that cannot arise: `M` starts at 0). Equal to
/// `PhysicsDefaults::magnet_remanence_tau_ms / 1000`.
const FALLBACK_TAU_S: f32 = 0.060;

/// `e^(−h/τ)`: the first-order factor of one sub-step. `τ = 0` gives 0
/// (instant), `τ = ∞` gives 1; a NaN τ (not producible through the solver)
/// is treated as instant.
#[must_use]
pub fn remanence_decay(h: f32, tau_s: f32) -> f32 {
    let k = libm::expf(-h / tau_s);
    if k.is_nan() { 0.0 } else { k.clamp(0.0, 1.0) }
}

/// One sub-step of the magnetisation law: `M ← P + (M − P)·e^(−h/τ)`.
/// In a magnet's range `P` is that magnet's power (the ball is
/// magnetised toward it); out of range `P = 0` (pure decay
/// `M ← M·e^(−h/τ)`). The result is clamped to `[0, 1]` (NaN → 0).
#[must_use]
pub fn magnetisation_step(m: f32, target: f32, h: f32, tau_s: f32) -> f32 {
    let target = if target.is_nan() {
        0.0
    } else {
        target.clamp(0.0, 1.0)
    };
    let next = target + (m - target) * remanence_decay(h, tau_s);
    if next.is_nan() {
        0.0
    } else {
        next.clamp(0.0, 1.0)
    }
}

/// The residual force (N) of one magnet on a ball centred at `position` on
/// `layer` with magnetisation `m`: `c·ρ·M · K_coil·d̂ / max(d³, d_min³)`
/// toward the pole when the ball is in the magnet's range (layer and
/// distance, the solver's own test), regardless of the coil's power;
/// otherwise zero.
#[must_use]
pub fn residual_force(
    magnet: &MagnetState,
    position: Vec3,
    layer: u8,
    coupling: f32,
    remanence: f32,
    m: f32,
) -> Vec3 {
    let k = coupling * remanence * m;
    if k == 0.0 || !magnet.in_range(position, layer) {
        return NO_FORCE;
    }
    magnet.pull(position) * k
}

/// The magnetisation cells shared by a [`Remanence`] hook and its clones:
/// `f32` bits in relaxed atomics, so the registered hook writes and any
/// clone reads without a lock or an allocation.
#[derive(Debug)]
struct Cells {
    m: [AtomicU32; MAX_BALLS],
    /// Set by the first instance that receives a solver call; every other
    /// instance sharing these cells is inert.
    claimed: AtomicBool,
}

/// Whether this instance writes the shared cells.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Role {
    /// Has not received a solver call yet.
    Unclaimed,
    /// The registered instance: advances `M` and returns the residual pull.
    Writer,
    /// Another instance already writes these cells: does nothing.
    Inert,
}

/// Magnetic remanence hook (PLAN.md §3.2).
///
/// Each ball slot carries a magnetisation `M ∈ [0, 1]`. Every sub-step of
/// `h` seconds:
///
/// 1. **Force** (from `M` at the start of the sub-step): every magnet whose
///    range holds the ball (layer and distance, [`MagnetState::in_range`])
///    pulls with `c·ρ·M·K_coil·d̂ / max(d³, d_min³)`, whatever its power
///    (`c = magnetic_coupling`, `ρ = remanence` of the profile). The
///    solver adds the coil's own `c·P·…` separately, so with the coil on the
///    ball feels `c·(P + ρM)·…`, with it off `c·ρM·…`.
/// 2. **Magnetisation**: if the ball is in range of at least one magnet,
///    `M` relaxes toward that magnet's power `P` with its τ,
///    `M ← P + (M − P)·e^(−h/τ)` (the strongest such magnet; on a power tie
///    the magnet that last held the ball if it is among them, else the
///    lowest index); otherwise it decays, `M ← M·e^(−h/τ)`, with τ of the magnet
///    that last held it (read from the context each sub-step, so a changed τ
///    applies at once). A ball with `c = 0` is not magnetisable: its `M`
///    stays 0. `M` is clamped to `[0, 1]`.
///
/// With `ρ = 0` (`low_rem_steel`) or `c = 0` the force is exactly zero and
/// the ball is bit-identical to a solver without the hook.
///
/// **Reading `M`.** A registered hook belongs to the solver, so `M` lives in
/// state shared by every clone of a `Remanence`: keep a clone (or the value
/// [`super::register_all`] returns) and read [`super::magnetisation`] /
/// [`Remanence::magnetisation`] between steps. Only one instance can
/// drive the shared state: the first one to receive a solver call
/// (`ball_added` or `force_in`) claims it (one atomic compare-exchange,
/// then a plain field check), and any other clone that is also registered
/// — with the same or another solver — is inert: it returns no force and
/// never changes `M`. Use a fresh [`Remanence::new`] per solver.
///
/// `M` is reset to 0 when a ball is added to a slot (handles are reused)
/// and when it is removed.
#[derive(Debug)]
pub struct Remanence {
    cells: Arc<Cells>,
    role: Role,
    /// Index of the magnet that last held each ball ([`NO_SOURCE`]: none).
    /// Only the registered instance uses it.
    source: [u32; MAX_BALLS],
}

impl Default for Remanence {
    fn default() -> Self {
        Self::new()
    }
}

impl Remanence {
    /// A remanence hook with every `M = 0` (allocates its shared cells
    /// once, here).
    #[must_use]
    pub fn new() -> Self {
        Self {
            cells: Arc::new(Cells {
                m: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
                claimed: AtomicBool::new(false),
            }),
            role: Role::Unclaimed,
            source: [NO_SOURCE; MAX_BALLS],
        }
    }

    /// Claims the shared cells on the first solver call; `true` if this
    /// instance is the writer.
    fn writer(&mut self) -> bool {
        if self.role == Role::Unclaimed {
            let won = self
                .cells
                .claimed
                .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok();
            self.role = if won { Role::Writer } else { Role::Inert };
        }
        self.role == Role::Writer
    }

    /// Magnetisation `M ∈ [0, 1]` of the ball in slot `handle`, as of the
    /// end of the last sub-step the registered hook ran (0 for an empty or
    /// out-of-range slot).
    #[must_use]
    pub fn magnetisation(&self, handle: BallHandle) -> f32 {
        self.cells
            .m
            .get(usize::from(handle.0))
            .map_or(0.0, |c| f32::from_bits(c.load(Ordering::Relaxed)))
    }

    fn set(&self, i: usize, m: f32) {
        self.cells.m[i].store(m.to_bits(), Ordering::Relaxed);
    }

    fn reset(&mut self, handle: BallHandle) {
        let i = usize::from(handle.0);
        if i < MAX_BALLS {
            self.set(i, 0.0);
            self.source[i] = NO_SOURCE;
        }
    }
}

/// A clone shares the magnetisation cells (for reading) but is never the
/// writer unless it is the first instance to receive a solver call.
impl Clone for Remanence {
    fn clone(&self) -> Self {
        Self {
            cells: Arc::clone(&self.cells),
            role: Role::Unclaimed,
            source: [NO_SOURCE; MAX_BALLS],
        }
    }
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
        let i = usize::from(handle.0);
        if i >= MAX_BALLS || !self.writer() {
            return NO_FORCE;
        }
        let c = profile.magnetic_coupling;
        let m = self.magnetisation(handle);

        // 1. Residual force from M at the start of the sub-step.
        let mut force = NO_FORCE;
        if c * profile.remanence * m != 0.0 {
            for ms in ctx.magnets {
                force += residual_force(ms, ball.position, ball.layer, c, profile.remanence, m);
            }
        }

        // 2. Advance M. The magnet that last held the ball keeps it on a
        // power tie (so overlapping idle magnets do not take over its τ);
        // otherwise the strongest in range, lowest index on a tie.
        let mut best: Option<(usize, f32)> = None;
        if c != 0.0 {
            let src = self.source[i] as usize;
            if let Some(ms) = ctx.magnets.get(src)
                && ms.in_range(ball.position, ball.layer)
            {
                best = Some((src, ms.power));
            }
            for (k, ms) in ctx.magnets.iter().enumerate() {
                if ms.in_range(ball.position, ball.layer) && best.is_none_or(|(_, p)| ms.power > p)
                {
                    best = Some((k, ms.power));
                }
            }
        }
        let next = match best {
            Some((k, p)) => {
                self.source[i] = k as u32;
                magnetisation_step(m, p, dt, ctx.magnets[k].remanence_tau_s)
            }
            // Not magnetisable (a magnetised ball whose profile was swapped
            // for one with `c = 0`): `M` is 0, not a decaying remainder.
            None if c == 0.0 => {
                self.source[i] = NO_SOURCE;
                0.0
            }
            None if m != 0.0 => {
                let tau = ctx
                    .magnets
                    .get(self.source[i] as usize)
                    .map_or(FALLBACK_TAU_S, |ms| ms.remanence_tau_s);
                magnetisation_step(m, 0.0, dt, tau)
            }
            None => m,
        };
        if next.to_bits() != m.to_bits() {
            self.set(i, next);
        }
        if force.is_finite() { force } else { NO_FORCE }
    }

    fn ball_added(&mut self, handle: BallHandle, _ball: &Ball) {
        if self.writer() {
            self.reset(handle);
        }
    }

    fn ball_removed(&mut self, handle: BallHandle) {
        if self.writer() {
            self.reset(handle);
        }
    }
}
