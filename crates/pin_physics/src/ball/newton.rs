//! Captive "Newton" balls: balls constrained to a 1D spline that pass an
//! impact along the chain.

use arrayvec::ArrayVec;
use glam::Vec3;
use pin_core::ids::ProfileId;

use crate::world::{CubicSpline, layers};

/// Maximum number of captive balls in one chain.
pub const MAX_CHAIN_BALLS: usize = 8;

/// Gap (metres) up to which two captive balls count as touching.
pub(crate) const CHAIN_TOUCH: f32 = 2.0e-5;

/// Errors from [`NewtonChain::new`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NewtonError {
    /// No ball parameters were given.
    Empty,
    /// More than [`MAX_CHAIN_BALLS`] balls.
    TooManyBalls,
    /// A parameter is NaN or infinite.
    NonFinite,
    /// A parameter lies outside `[0, path.max_t()]`.
    OutOfRange,
    /// The parameters are not strictly ascending.
    NotAscending,
}

impl core::fmt::Display for NewtonError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            NewtonError::Empty => "a Newton chain needs at least one ball",
            NewtonError::TooManyBalls => "too many balls in a Newton chain",
            NewtonError::NonFinite => "non-finite ball parameter",
            NewtonError::OutOfRange => "ball parameter outside the path",
            NewtonError::NotAscending => "ball parameters must be strictly ascending",
        };
        f.write_str(s)
    }
}

impl std::error::Error for NewtonError {}

/// Captive balls on a spline path. Each ball has one degree of freedom (its
/// curve parameter); touching neighbours exchange impulses along the path.
#[derive(Clone, Debug)]
pub struct NewtonChain {
    path: CubicSpline,
    profile: ProfileId,
    t: ArrayVec<f32, MAX_CHAIN_BALLS>,
    speed: ArrayVec<f32, MAX_CHAIN_BALLS>,
    /// Ball radius and mass; set when the chain is added to a solver.
    radius: f32,
    mass: f32,
    /// Restitution between two captive balls, and of the path's end stops.
    restitution: f32,
    layer_mask: u8,
}

impl NewtonChain {
    /// Places balls of `profile` on `path` at the curve parameters
    /// `params_t` (strictly ascending, inside the path). Neighbours whose
    /// surfaces touch transmit impulses through the chain.
    ///
    /// The ball size is not known here: it is taken from the profile list
    /// when the chain is added to a [`super::BallSolver`], which also pushes
    /// overlapping neighbours apart during its first steps.
    ///
    /// # Errors
    /// See [`NewtonError`].
    pub fn new(
        path: CubicSpline,
        profile: ProfileId,
        params_t: &[f32],
    ) -> Result<Self, NewtonError> {
        if params_t.is_empty() {
            return Err(NewtonError::Empty);
        }
        if params_t.len() > MAX_CHAIN_BALLS {
            return Err(NewtonError::TooManyBalls);
        }
        if params_t.iter().any(|t| !t.is_finite()) {
            return Err(NewtonError::NonFinite);
        }
        if params_t.iter().any(|&t| t < 0.0 || t > path.max_t()) {
            return Err(NewtonError::OutOfRange);
        }
        if params_t.windows(2).any(|w| w[1] <= w[0]) {
            return Err(NewtonError::NotAscending);
        }
        let mut t = ArrayVec::new();
        let mut speed = ArrayVec::new();
        for &p in params_t {
            t.push(p);
            speed.push(0.0);
        }
        Ok(Self {
            path,
            profile,
            t,
            speed,
            radius: 0.0,
            mass: 1.0,
            restitution: 0.0,
            layer_mask: layers::ALL,
        })
    }

    /// Number of captive balls.
    #[must_use]
    pub fn len(&self) -> usize {
        self.t.len()
    }

    /// Always false: a chain has at least one ball.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.t.is_empty()
    }

    /// The path.
    #[must_use]
    pub fn path(&self) -> &CubicSpline {
        &self.path
    }

    /// Profile of the captive balls.
    #[must_use]
    pub fn profile(&self) -> ProfileId {
        self.profile
    }

    /// Curve parameter of ball `i` (panics if out of range).
    #[must_use]
    pub fn ball_t(&self, i: usize) -> f32 {
        self.t[i]
    }

    /// Speed of ball `i` along the path, m/s, positive toward increasing
    /// parameter (panics if out of range).
    #[must_use]
    pub fn ball_speed(&self, i: usize) -> f32 {
        self.speed[i]
    }

    /// World position of ball `i` (panics if out of range).
    #[must_use]
    pub fn ball_position(&self, i: usize) -> Vec3 {
        self.path.point(self.t[i])
    }

    /// Sets the path speed of ball `i` (a coil or script strike). Unknown
    /// indices and non-finite speeds are ignored. The impulse travels
    /// through touching neighbours during the next solver step.
    pub fn strike(&mut self, i: usize, speed_along_path: f32) {
        if let Some(s) = self.speed.get_mut(i)
            && speed_along_path.is_finite()
        {
            *s = speed_along_path;
        }
    }

    /// Layers on which free balls collide with the chain (default: all).
    #[must_use]
    pub fn layer_mask(&self) -> u8 {
        self.layer_mask
    }

    /// Sets the layers on which free balls collide with the chain.
    pub fn set_layer_mask(&mut self, mask: u8) {
        self.layer_mask = mask;
    }

    /// Radius of the captive balls (0 until the chain is added to a solver).
    #[must_use]
    pub fn ball_radius(&self) -> f32 {
        self.radius
    }

    pub(crate) fn bind(&mut self, radius: f32, mass: f32, restitution: f32) {
        self.radius = radius;
        self.mass = mass;
        self.restitution = restitution.clamp(0.0, 1.0);
    }

    pub(crate) fn mass(&self) -> f32 {
        self.mass
    }

    /// Unit tangent of the path at ball `i`.
    pub(crate) fn tangent(&self, i: usize) -> Vec3 {
        self.path.tangent(self.t[i]).normalize_or_zero()
    }

    /// Changes the path speed of ball `i` by `dv` (m/s).
    pub(crate) fn add_speed(&mut self, i: usize, dv: f32) {
        self.speed[i] += dv;
    }

    /// Surface gap between balls `k` and `k + 1`.
    fn gap(&self, k: usize) -> f32 {
        self.ball_position(k).distance(self.ball_position(k + 1)) - 2.0 * self.radius
    }

    /// Surface gap between balls `k` and `k + 1` and the cosines `(a_k,
    /// a_{k+1})` between each ball's path tangent and the unit centre line
    /// `d` from ball `k` to ball `k + 1`, so that a ball's world velocity
    /// along `d` is `a_i * speed[i]`. `None` when the centres coincide or
    /// are not finite (no centre line). A ball whose tangent vanishes gets
    /// cosine 0: it cannot move along the centre line.
    ///
    /// The cosines are formed as `T·D / (|T| |D|)` from the raw vectors, so
    /// they are exactly ±1 whenever the tangent and the centre line are
    /// parallel coordinate-wise (a straight chain).
    fn centre_line(&self, k: usize) -> Option<(f32, f32, f32)> {
        let delta = self.ball_position(k + 1) - self.ball_position(k);
        let dist = delta.length();
        if !(dist > 0.0 && dist.is_finite()) {
            return None;
        }
        let cosine = |t: f32| {
            let tan = self.path.tangent(t);
            let len = tan.length();
            if len > 0.0 && len.is_finite() {
                tan.dot(delta) / (len * dist)
            } else {
                0.0
            }
        };
        Some((
            dist - 2.0 * self.radius,
            cosine(self.t[k]),
            cosine(self.t[k + 1]),
        ))
    }

    /// Exchanges impulses between touching neighbours that approach each
    /// other along their centre line, sweeping until the chain is quiet
    /// (equal masses).
    ///
    /// Pair `(k, k + 1)` collides when its gap is at most [`CHAIN_TOUCH`]
    /// and its closing speed along the centre line `d`,
    /// `c = a_k u_k - a_{k+1} u_{k+1}` (see [`Self::centre_line`]), is
    /// positive; on a curved or hairpin path this differs from comparing the
    /// path speeds. The equal and opposite normal impulse `±J d` acts on
    /// balls that can only move along their path, so each path speed changes
    /// by the impulse projected onto its tangent: `Δu_k = -a_k J / m`,
    /// `Δu_{k+1} = a_{k+1} J / m`. `J / m = (1 + e) c / (a_k² + a_{k+1}²)`
    /// (the pair's effective mass along `d`) turns `c` into exactly `-e c`,
    /// and the kinetic energy changes by `-(1 - e) J c / 2 <= 0`. The update
    /// below is that formula rearranged so that with `a_k = a_{k+1} = ±1`
    /// (a straight chain) it is bit for bit the 1D exchange
    /// `u_k' = ((1 - e) u_k + (1 + e) u_{k+1}) / 2` (docs/DECISIONS.md,
    /// "issue-8 — Newton-chain contact on curved paths").
    pub(crate) fn resolve_touching(&mut self) {
        let n = self.len();
        if n < 2 {
            return;
        }
        let e = self.restitution;
        for _ in 0..2 * n {
            let mut any = false;
            for k in 0..n - 1 {
                let Some((gap, ca, cb)) = self.centre_line(k) else {
                    continue;
                };
                let (a, b) = (self.speed[k], self.speed[k + 1]);
                let closing = ca * a - cb * b;
                let sum = ca * ca + cb * cb;
                if closing > 0.0 && gap <= CHAIN_TOUCH && sum > 0.0 {
                    let qa = 2.0 * ca * ca / sum;
                    let qb = 2.0 * cb * cb / sum;
                    let r = 2.0 * ca * cb / sum;
                    self.speed[k] =
                        0.5 * (((1.0 - e) + (1.0 + e) * (1.0 - qa)) * a + (1.0 + e) * r * b);
                    self.speed[k + 1] =
                        0.5 * ((1.0 + e) * r * a + ((1.0 - e) + (1.0 + e) * (1.0 - qb)) * b);
                    any = true;
                }
            }
            if !any {
                break;
            }
        }
    }

    /// Moves every ball for `tau` seconds along the path (midpoint rule in
    /// the curve parameter); balls reaching an end of the path bounce off it.
    fn drift(&mut self, tau: f32) {
        let max_t = self.path.max_t();
        for i in 0..self.len() {
            let u = self.speed[i];
            if u == 0.0 {
                continue;
            }
            let t0 = self.t[i];
            let len0 = self.path.tangent(t0).length();
            if len0.is_nan() || len0 <= 0.0 {
                continue;
            }
            let mid = (t0 + 0.5 * u * tau / len0).clamp(0.0, max_t);
            let len_mid = self.path.tangent(mid).length();
            let len = if len_mid > 0.0 { len_mid } else { len0 };
            let t1 = t0 + u * tau / len;
            // Within the touch distance of the stop it is reached (the event
            // search stopped the step there, up to rounding).
            let touch = CHAIN_TOUCH / len;
            if t1 <= touch && u < 0.0 || t1 >= max_t - touch && u > 0.0 {
                self.speed[i] = -self.restitution * u;
            }
            self.t[i] = t1.clamp(0.0, max_t);
        }
    }

    /// Pushes overlapping neighbours apart along the path, keeping order.
    fn separate(&mut self) {
        let n = self.len();
        let max_t = self.path.max_t();
        for _ in 0..2 {
            for k in 0..n.saturating_sub(1) {
                let gap = self.gap(k);
                if gap >= 0.0 {
                    continue;
                }
                let la = self.path.tangent(self.t[k]).length();
                let lb = self.path.tangent(self.t[k + 1]).length();
                if la.is_nan() || lb.is_nan() || la <= 0.0 || lb <= 0.0 {
                    continue;
                }
                let half = -0.5 * gap;
                let ta = self.t[k] - half / la;
                let tb = self.t[k + 1] + half / lb;
                // What one ball cannot move at an end stop, the other does.
                let spill_a = (0.0 - ta).max(0.0) * la;
                let spill_b = (tb - max_t).max(0.0) * lb;
                self.t[k] = (ta - spill_b / la).clamp(0.0, max_t);
                self.t[k + 1] = (tb + spill_a / lb).clamp(0.0, max_t);
            }
        }
    }

    /// Advances the chain by `dt`: gravity along the path (rolling, hence
    /// the factor 5/7), rolling resistance with static hold, impulse
    /// transmission between touching balls, motion with 1D continuous
    /// collision detection, and overlap correction.
    pub(crate) fn advance(&mut self, dt: f32, gravity: Vec3, rolling_resistance: f32) {
        let n = self.len();
        for i in 0..n {
            let tan = self.tangent(i);
            let along = gravity.dot(tan);
            let across = (gravity - tan * along).length();
            let mut u = self.speed[i] + (5.0 / 7.0) * along * dt;
            let hold = (5.0 / 7.0) * rolling_resistance.max(0.0) * across * dt;
            u = if u.abs() <= hold {
                0.0
            } else {
                u - hold.copysign(u)
            };
            self.speed[i] = u;
        }
        self.resolve_touching();
        let mut remaining = dt;
        let max_t = self.path.max_t();
        // Each end-stop bounce is one more event: two per ball at most.
        for _ in 0..6 * n {
            // Earliest time at which two neighbours come into contact, or a
            // ball reaches an end stop (it bounces there and the rest of the
            // step is integrated from the stop).
            let mut tau = remaining;
            // The closing speed is the one `resolve_touching` tests.
            for k in 0..n.saturating_sub(1) {
                if let Some((gap, ca, cb)) = self.centre_line(k) {
                    let closing = ca * self.speed[k] - cb * self.speed[k + 1];
                    if closing > 0.0 && gap > CHAIN_TOUCH {
                        tau = tau.min(gap / closing);
                    }
                }
            }
            for i in 0..n {
                let (t, u) = (self.t[i], self.speed[i]);
                let room = if u > 0.0 {
                    max_t - t
                } else if u < 0.0 {
                    t
                } else {
                    continue;
                };
                let len = self.path.tangent(t).length();
                if room > 0.0 && len > 0.0 {
                    let hit = room * len / u.abs();
                    if hit.is_finite() && hit < tau {
                        tau = hit;
                    }
                }
            }
            self.drift(tau);
            self.resolve_touching();
            remaining -= tau;
            if remaining.is_nan() || remaining <= 0.0 {
                break;
            }
        }
        self.separate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: f32 = 0.0135;

    fn straight(n: usize) -> CubicSpline {
        let pts: Vec<Vec3> = (0..n)
            .map(|i| Vec3::new(2.0 * R * i as f32, 0.0, R))
            .collect();
        CubicSpline::interpolate(&pts).unwrap()
    }

    fn chain(params: &[f32], e: f32) -> NewtonChain {
        let mut c = NewtonChain::new(straight(12), ProfileId(0), params).unwrap();
        c.bind(R, 0.08, e);
        c
    }

    #[test]
    fn constructor_errors() {
        let p = straight(4);
        let mk = |t: &[f32]| NewtonChain::new(p.clone(), ProfileId(0), t).map(|_| ());
        assert_eq!(mk(&[]), Err(NewtonError::Empty));
        assert_eq!(mk(&[0.1; 9]), Err(NewtonError::TooManyBalls));
        assert_eq!(mk(&[f32::NAN]), Err(NewtonError::NonFinite));
        assert_eq!(mk(&[-0.1]), Err(NewtonError::OutOfRange));
        assert_eq!(mk(&[3.5]), Err(NewtonError::OutOfRange));
        assert_eq!(mk(&[1.0, 1.0]), Err(NewtonError::NotAscending));
        assert_eq!(mk(&[1.0, 2.0]), Ok(()));
        assert!(!NewtonError::Empty.to_string().is_empty());
    }

    #[test]
    fn accessors() {
        let mut c = chain(&[3.0, 4.0], 1.0);
        assert_eq!(c.len(), 2);
        assert!(!c.is_empty());
        assert_eq!(c.profile(), ProfileId(0));
        assert_eq!(c.ball_t(1), 4.0);
        assert!((c.ball_position(1) - Vec3::new(8.0 * R, 0.0, R)).length() < 1e-6);
        assert_eq!(c.ball_radius(), R);
        assert_eq!(c.path().segment_count(), 11);
        c.set_layer_mask(2);
        assert_eq!(c.layer_mask(), 2);
        c.strike(7, 1.0);
        c.strike(0, f32::NAN);
        assert_eq!(c.ball_speed(0), 0.0);
    }

    #[test]
    fn elastic_strike_passes_through_the_chain() {
        let mut c = chain(&[3.0, 4.0, 5.0, 6.0], 1.0);
        c.strike(0, 1.0);
        c.advance(1.0e-3, Vec3::ZERO, 0.0);
        assert_eq!(c.ball_speed(0), 0.0);
        assert_eq!(c.ball_speed(1), 0.0);
        assert_eq!(c.ball_speed(2), 0.0);
        assert_eq!(c.ball_speed(3), 1.0);
        assert!(c.ball_t(3) > 6.0);
    }

    #[test]
    fn inelastic_chain_conserves_momentum_and_loses_energy() {
        let mut c = chain(&[3.0, 4.0, 5.0], 0.6);
        c.strike(0, 2.0);
        for _ in 0..50 {
            c.advance(1.0e-3, Vec3::ZERO, 0.0);
            let p: f32 = (0..3).map(|i| c.ball_speed(i)).sum();
            let e: f32 = (0..3).map(|i| c.ball_speed(i).powi(2)).sum();
            assert!((p - 2.0).abs() < 1e-5);
            assert!(e <= 4.0 + 1e-5);
            assert!(c.ball_speed(0) <= c.ball_speed(1) + 1e-6);
            assert!(c.ball_speed(1) <= c.ball_speed(2) + 1e-6);
        }
        assert!(c.ball_speed(2) > 1.0);
    }

    #[test]
    fn separated_balls_collide_later_without_passing() {
        let mut c = chain(&[2.0, 4.0], 1.0);
        c.strike(0, 1.0);
        // The gap is one diameter (27 mm): contact after 27 ms.
        for tick in 0..40 {
            c.advance(1.0e-3, Vec3::ZERO, 0.0);
            assert!(c.ball_t(0) < c.ball_t(1));
            assert!(c.gap(0) > -1e-5, "tick {tick}");
            if tick < 25 {
                assert_eq!(c.ball_speed(1), 0.0);
            }
        }
        assert!((c.ball_speed(1) - 1.0).abs() < 1e-5);
        assert!(c.ball_speed(0).abs() < 1e-5);
    }

    #[test]
    fn end_stops_reflect() {
        let mut c = chain(&[10.9], 0.5);
        c.strike(0, 1.0);
        for _ in 0..20 {
            c.advance(1.0e-3, Vec3::ZERO, 0.0);
        }
        assert!(c.ball_t(0) <= 11.0);
        assert!((c.ball_speed(0) + 0.5).abs() < 1e-6);
    }

    #[test]
    fn end_stop_bounce_keeps_the_rest_of_the_step() {
        // 0.1 mm before the end at 1 m/s, elastic, 1 ms: it reaches the stop
        // after 0.1 ms and travels 0.9 mm back (issue #7, PR #4 r4573613066).
        let max_t = 11.0;
        let mut c = chain(&[max_t - 1.0e-4 / (2.0 * R)], 1.0);
        assert_eq!(c.path().max_t(), max_t);
        c.strike(0, 1.0);
        c.advance(1.0e-3, Vec3::ZERO, 0.0);
        assert!((c.ball_speed(0) + 1.0).abs() < 1e-6, "{}", c.ball_speed(0));
        let back = c.ball_position(0).distance(c.path().point(max_t));
        assert!((back - 0.9e-3).abs() < 1e-6, "{back} m from the end");
    }

    #[test]
    fn gravity_and_rolling_resistance() {
        // Level path: gravity across the path, resistance stops the ball.
        let mut c = chain(&[5.0], 1.0);
        c.strike(0, 0.01);
        let g = Vec3::new(0.0, 0.0, -9.81);
        for _ in 0..2000 {
            c.advance(1.0e-3, g, 0.002);
        }
        assert_eq!(c.ball_speed(0), 0.0);
        // Gravity along the path accelerates at 5/7 g.
        let mut c = chain(&[5.0], 1.0);
        c.advance(1.0e-3, Vec3::new(9.81, 0.0, 0.0), 0.0);
        assert!((c.ball_speed(0) - 9.81e-3 * 5.0 / 7.0).abs() < 1e-7);
    }

    #[test]
    fn overlapping_balls_are_pushed_apart() {
        let mut c = chain(&[3.0, 3.9, 4.8], 1.0);
        for _ in 0..20 {
            c.advance(1.0e-3, Vec3::ZERO, 0.0);
        }
        assert!(c.gap(0) > -1e-5 && c.gap(1) > -1e-5);
        // At an end stop the inner ball takes the whole correction.
        let mut c = chain(&[0.0, 0.5], 1.0);
        for _ in 0..20 {
            c.advance(1.0e-3, Vec3::ZERO, 0.0);
        }
        assert_eq!(c.ball_t(0), 0.0);
        assert!(c.gap(0) > -1e-5);
    }

    /// Hairpin path: arm A along +x at y = 0 (t = 0..20, 10 mm per knot), a
    /// half turn of diameter `w` (t = 20..23), arm B back along -x at y = `w`
    /// (t = 23..43). With `w < 2R`, balls on opposite arms touch.
    fn hairpin(w: f32) -> CubicSpline {
        let mut pts: Vec<Vec3> = (0..=20)
            .map(|i| Vec3::new(0.01 * i as f32, 0.0, R))
            .collect();
        let rho = 0.5 * w;
        for j in 1..3 {
            let a = -core::f32::consts::FRAC_PI_2 + core::f32::consts::PI * j as f32 / 3.0;
            pts.push(Vec3::new(0.2 + rho * a.cos(), rho + rho * a.sin(), R));
        }
        pts.extend((0..=20).map(|i| Vec3::new(0.2 - 0.01 * i as f32, w, R)));
        CubicSpline::interpolate(&pts).unwrap()
    }

    /// Curve parameter of the point at `x` on arm A or arm B.
    fn arm_a(x: f32) -> f32 {
        x / 0.01
    }
    fn arm_b(x: f32) -> f32 {
        23.0 + (0.2 - x) / 0.01
    }

    fn hairpin_chain(w: f32, x0: f32, x1: f32, e: f32) -> NewtonChain {
        let mut c = NewtonChain::new(hairpin(w), ProfileId(0), &[arm_a(x0), arm_b(x1)]).unwrap();
        c.bind(R, 0.08, e);
        c
    }

    fn centre_distance(c: &NewtonChain) -> f32 {
        c.ball_position(0).distance(c.ball_position(1))
    }

    /// Issue #8: ball 0 runs back along arm A (path speed < 0, away from
    /// ball 1 in path terms) toward ball 1 resting on arm B, less than a
    /// diameter away across the hairpin. It must hit ball 1 rather than pass
    /// through it (the old path-speed test applied no impulse and let the
    /// centres come to 22.8 mm, well inside 2R = 27 mm).
    #[test]
    fn hairpin_neighbours_collide_across_the_bend() {
        for &w in &[1.2 * R, 1.5 * R, 1.8 * R] {
            for &v in &[0.3f32, 1.0, 2.5] {
                for &e in &[1.0f32, 0.6, 0.0] {
                    let mut c = hairpin_chain(w, 0.15, 0.08, e);
                    c.strike(0, -v);
                    let energy0 = v * v;
                    let mut min_d = f32::INFINITY;
                    let mut ticks = 0;
                    // Until ball 0 has reached ball 1 and ball 1 has moved
                    // away (well before ball 1 reaches its end stop).
                    while ticks < 2000 && c.ball_position(1).x > 0.04 {
                        c.advance(1.0e-3, Vec3::ZERO, 0.0);
                        min_d = min_d.min(centre_distance(&c));
                        let energy = c.ball_speed(0).powi(2) + c.ball_speed(1).powi(2);
                        assert!(energy <= energy0 * (1.0 + 1e-5), "w {w} v {v} e {e}");
                        ticks += 1;
                    }
                    assert!(
                        min_d >= 2.0 * R - CHAIN_TOUCH,
                        "w {w} v {v} e {e}: centres {min_d} m apart"
                    );
                    // Momentum went across the centre line: ball 1 runs
                    // along arm B (-x, path speed > 0), and the pair is apart
                    // (or, perfectly inelastic, rides on touching).
                    assert!(c.ball_position(1).x <= 0.04, "w {w} v {v} e {e}");
                    assert!(c.ball_speed(1) > 0.0, "w {w} v {v} e {e}");
                    if e > 0.0 {
                        assert!(centre_distance(&c) > 2.0 * R, "w {w} v {v} e {e}");
                    }
                    if e == 1.0 {
                        // Symmetric geometry: an elastic hit hands over the
                        // whole speed, as on a straight chain.
                        assert!((c.ball_speed(1) - v).abs() < 1e-3 * v, "w {w} v {v}");
                    }
                }
            }
        }
    }

    /// PR #4 review r4573613069: on a three-point hairpin, two balls with
    /// equal path speeds approach each other in world space (zero path
    /// closing speed). They must collide instead of overlapping.
    #[test]
    fn equal_path_speeds_still_collide_on_a_hairpin() {
        let path = CubicSpline::interpolate(&[
            Vec3::new(-0.1, 0.0, 0.0),
            Vec3::new(0.0, 0.01, 0.0),
            Vec3::new(-0.1, 0.02, 0.0),
        ])
        .unwrap();
        for &e in &[1.0f32, 0.5] {
            for &u in &[0.5f32, 3.0] {
                let mut c = NewtonChain::new(path.clone(), ProfileId(0), &[0.2, 1.6]).unwrap();
                c.bind(R, 0.08, e);
                c.strike(0, u);
                c.strike(1, u);
                let mut min_d = centre_distance(&c);
                let mut hit = false;
                for _ in 0..40 {
                    c.advance(1.0e-3, Vec3::ZERO, 0.0);
                    min_d = min_d.min(centre_distance(&c));
                    hit |= c.ball_speed(0) != u || c.ball_speed(1) != u;
                    let energy = c.ball_speed(0).powi(2) + c.ball_speed(1).powi(2);
                    assert!(energy <= 2.0 * u * u * (1.0 + 1e-5), "e {e} u {u}");
                }
                assert!(hit, "e {e} u {u}: no impulse");
                assert!(
                    min_d >= 2.0 * R - CHAIN_TOUCH,
                    "e {e} u {u}: centres {min_d} m apart"
                );
            }
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        /// One contact on the hairpin: the closing speed along the centre
        /// line becomes `-e` times itself, kinetic energy does not increase,
        /// and a separating pair is left alone.
        #[test]
        fn hairpin_contact_reflects_the_closing_speed(
            w in 1.0f32 * R..1.95 * R,
            x1 in 0.04f32..0.16,
            overlap in 0.0f32..1.0,
            u0 in -3.0f32..3.0,
            u1 in -3.0f32..3.0,
            e in 0.0f32..=1.0,
        ) {
            let reach = (4.0 * R * R - w * w).sqrt();
            let x0 = x1 + reach * (1.0 - 0.5 * overlap);
            let mut c = hairpin_chain(w, x0, x1, e);
            c.strike(0, u0);
            c.strike(1, u1);
            let (gap, ca, cb) = c.centre_line(0).unwrap();
            proptest::prop_assert!(gap <= CHAIN_TOUCH);
            let before = ca * u0 - cb * u1;
            c.resolve_touching();
            let (a, b) = (c.ball_speed(0), c.ball_speed(1));
            let after = ca * a - cb * b;
            if before > 0.0 {
                proptest::prop_assert!((after + e * before).abs() <= 1e-4 * (1.0 + before));
            } else {
                proptest::prop_assert_eq!((a, b), (u0, u1));
            }
            proptest::prop_assert!(a * a + b * b <= (u0 * u0 + u1 * u1) * (1.0 + 1e-5) + 1e-9);
        }
    }
}
