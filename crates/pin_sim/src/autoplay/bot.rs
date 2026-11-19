//! The seeded heuristic autoplay [`Bot`].
//!
//! The bot plays any table through the public [`Simulation`] API: it reads
//! the published snapshot (balls, flippers), the flipper geometry
//! ([`Simulation::flipper_set`]: pivot, bat direction, relative angle) and
//! gravity ([`Simulation::solver`]), and presses cabinet buttons with
//! [`Simulation::set_switch`] / [`Simulation::release_switch`]. Call
//! [`Bot::tick`] once per tick, before `sim.advance(1)`.
//!
//! # Behaviour
//!
//! * **Start:** presses `start` once after a short PCG-drawn delay, and again
//!   every [`BotConfig::start_retry_ms`] (at most [`BotConfig::max_starts`]
//!   presses) until a ball appears.
//! * **Plunge:** when a ball has rested (speed below
//!   [`BotConfig::rest_speed_m_s`]) away from every flipper for
//!   [`BotConfig::launch_after_ms`], holds `launch` for a PCG-drawn time in
//!   [`BotConfig::launch_hold_ms`]. A table that serves on `launch` has no
//!   ball to rest, so after `start` has been pressed and released the bot
//!   also holds `launch` once the table has been empty for
//!   [`BotConfig::launch_after_ms`] (same hold draw, same
//!   [`BotConfig::launch_interval_ms`] throttle).
//! * **Flip on prediction:** each flipper with a cabinet button is modelled
//!   as an annular sector around its pivot (bat length plus radius plus a
//!   ball radius, from its rest angle across its sweep). A ball on the
//!   flipper's layer whose ballistic path (current position and velocity,
//!   table-plane gravity) enters that sector within
//!   [`BotConfig::lead_ms`] is a *predicted crossing*. The bot then skips
//!   the flip with the skill's miss rate, or plans a press a PCG-drawn
//!   delay in [`BotConfig::flip_delay_ms`] after the prediction (the shot it
//!   tries for: when in its stroke the bat meets the ball) and presses the
//!   button at that plan plus a PCG-drawn reaction error (see
//!   [`ReactionDraw`]; by default uniform in `-jitter..=+jitter`, never
//!   before the prediction), holding it for a PCG-drawn time in
//!   [`BotConfig::flip_hold_ms`].
//! * **Cradle:** a slow ball (below [`BotConfig::cradle_speed_m_s`]) already
//!   in a flipper's sector makes the bot hold that flipper up for a
//!   PCG-drawn time in [`BotConfig::cradle_hold_ms`] (pressed after the
//!   same reaction error around the moment it is seen, without the flip
//!   delay); after the release it
//!   flips the ball once it has rolled out past [`CRADLE_FLIP_REACH`] of the
//!   bat.
//! * **Nudge:** when a ball has rested away from the flippers for
//!   [`NudgeConfig::after_rest_ms`], the bot nudges the cabinet sideways
//!   ([`NudgeConfig::strength`] of full nudge-axis deflection) at most every
//!   [`NudgeConfig::min_interval_ms`] and [`NudgeConfig::max_per_game`]
//!   times, and only while the tilt bob swings less than
//!   [`NudgeConfig::max_bob_fraction`] of its ring gap, so a normal game
//!   cannot tilt. `action` and `tilt` are never pressed.
//!
//! No branch names a table or a behaviour (PLAN.md §2.3 rule 6): everything
//! comes from the geometry and the seeded PCG. The skill levels differ only
//! by [`SkillParams`] (reaction jitter and miss rate); every other knob is
//! in [`BotConfig`] and shared.
//!
//! # Determinism
//!
//! The bot owns one [`Pcg32`] seeded from its seed and draws from it in a
//! fixed order (flippers by snapshot order, then launch, then nudge), so a
//! given (seed, skill, table) gives the same button presses and the same
//! `state_hash` sequence (D9). It never reads the wall clock.

use arrayvec::ArrayVec;
use glam::{Vec2, Vec3};
use pin_core::input::CabinetInput;
use pin_core::snapshot::{MAX_BALLS, MAX_FLIPPERS};
use pin_core::{FlipperId, Pcg32, Rng, SeedableRng};

use super::table::{DEFAULT_BAT_LENGTH_M, TableInfo};
use crate::Simulation;
use crate::replay::{GameOp, GameOpSink};
use crate::tick::NUDGE_DV_PER_TICK;

/// Nominal ball radius for the sweep sector, m (a standard 27 mm ball).
pub const BALL_RADIUS_M: f32 = 0.0135;

/// Bat radius assumed when the table's geometry is unknown, m.
pub const DEFAULT_BAT_RADIUS_M: f32 = 0.011;

/// Sweep assumed when the table's geometry is unknown, rad (50°; the sign
/// follows the side, see [`FlipperModel::infer`]).
pub const DEFAULT_SWEEP_RAD: f32 = 0.872_664_6;

/// After a cradle the bot flips a slow ball on the bat once it is this
/// fraction of the reach out from the pivot.
pub const CRADLE_FLIP_REACH: f32 = 0.55;

/// Prediction time step, s.
const PREDICT_STEP_S: f32 = 0.002;

/// A ball moving up the table faster than this (m/s) was just hit; the bot
/// does not flip at it.
const AWAY_SPEED_M_S: f32 = 0.3;

/// Ticks a skipped (missed) prediction blocks new decisions on that
/// flipper.
const MISS_LOCKOUT_TICKS: u64 = 150;

/// Ticks the bot holds `start`.
const START_HOLD_TICKS: u64 = 100;

/// How skilled the bot plays.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SkillLevel {
    /// 40 ms reaction jitter, 30 % misses.
    Novice,
    /// 15 ms reaction jitter, 10 % misses.
    Standard,
    /// 3 ms reaction jitter, 1 % misses.
    Expert,
}

impl SkillLevel {
    /// Every level, worst first.
    pub const ALL: [SkillLevel; 3] = [SkillLevel::Novice, SkillLevel::Standard, SkillLevel::Expert];

    /// The default parameters of this level.
    #[must_use]
    pub fn params(self) -> SkillParams {
        match self {
            SkillLevel::Novice => SkillParams {
                reaction_jitter_ms: 40,
                miss_rate: 0.30,
            },
            SkillLevel::Standard => SkillParams {
                reaction_jitter_ms: 15,
                miss_rate: 0.10,
            },
            SkillLevel::Expert => SkillParams {
                reaction_jitter_ms: 3,
                miss_rate: 0.01,
            },
        }
    }

    /// Lower-case name (`novice`, `standard`, `expert`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            SkillLevel::Novice => "novice",
            SkillLevel::Standard => "standard",
            SkillLevel::Expert => "expert",
        }
    }

    /// Parses a [`name`](Self::name) (ASCII case-insensitive).
    #[must_use]
    pub fn from_name(s: &str) -> Option<SkillLevel> {
        SkillLevel::ALL
            .into_iter()
            .find(|l| l.name().eq_ignore_ascii_case(s))
    }
}

/// How the reaction error around the planned press time is drawn.
///
/// The planned press time of a flip is a delay drawn from
/// [`BotConfig::flip_delay_ms`] after the prediction; of a cradle, the
/// moment the slow ball is seen. `j` is [`SkillParams::reaction_jitter_ms`].
/// Either way it is one PCG draw per decision.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReactionDraw {
    /// Uniform in `-j..=+j` ms around the planned time, clamped so the press
    /// is never before the decision tick. A steadier player hits nearer the
    /// planned time, both early and late.
    Centred,
    /// Uniform in `0..=j` ms after the planned time (the original 2.9 bot:
    /// a reaction can only be late). Kept so pinned historical soaks can be
    /// replayed.
    Late,
}

/// What distinguishes the skill levels, and nothing else.
///
/// | level | `reaction_jitter_ms` | `miss_rate` |
/// |---|---|---|
/// | Novice | 40 | 0.30 |
/// | Standard | 15 | 0.10 |
/// | Expert | 3 | 0.01 |
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SkillParams {
    /// Reaction error bound, ms (1 ms ticks): the press lands this far at
    /// most from the planned time, as [`BotConfig::reaction`] draws it.
    pub reaction_jitter_ms: u32,
    /// Probability (0..=1) of skipping a predicted flip.
    pub miss_rate: f32,
}

/// Cabinet nudging (see the [module docs](self)).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct NudgeConfig {
    /// Whether the bot nudges at all.
    pub enabled: bool,
    /// Rest time of a ball away from the flippers before a nudge, ms.
    pub after_rest_ms: u32,
    /// Shortest time between two nudges, ms.
    pub min_interval_ms: u32,
    /// Most nudges per game.
    pub max_per_game: u32,
    /// Nudge strength as a fraction of full nudge-axis deflection (the
    /// cabinet velocity change is `strength × NUDGE_DV_PER_TICK`), 0..=1.
    pub strength: f32,
    /// Nudge only while the tilt bob's offset is below this fraction of its
    /// ring gap.
    pub max_bob_fraction: f32,
}

impl Default for NudgeConfig {
    /// Enabled, after 3 s of rest, at most every 2 s and 20 times a game,
    /// half strength, bob below a quarter of its gap.
    fn default() -> Self {
        Self {
            enabled: true,
            after_rest_ms: 3000,
            min_interval_ms: 2000,
            max_per_game: 20,
            strength: 0.5,
            max_bob_fraction: 0.25,
        }
    }
}

/// Bot behaviour shared by every skill level (see the [module docs](self)).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BotConfig {
    /// Prediction horizon: a crossing within this time triggers a flip, ms.
    pub lead_ms: u32,
    /// Range (inclusive) of the planned delay from a predicted crossing to
    /// the flip press, ms, before the reaction error; one PCG draw per flip
    /// (none when the range is a single value). The draw is the shot the
    /// bot tries for: pressing at the prediction (0 ms) brings the bat to
    /// the end of its 20-35 ms stroke (`physics_defaults.toml`) as the ball
    /// arrives, a dead bounce, while later presses meet the ball mid-stroke
    /// and send it elsewhere. Shared by every skill, so the skills still
    /// differ only by reaction error and miss rate: a steady player lands
    /// the planned shot, a shaky one often lands a different one.
    /// Calibrated over the grey box, Apex Circuit and Feature Lab
    /// (`docs/decisions/2026-11-18-fix-2.9-bot-timing.md`).
    pub flip_delay_ms: (u32, u32),
    /// How the skill's reaction error is drawn around the planned press.
    pub reaction: ReactionDraw,
    /// Flip button hold time range (inclusive), ms.
    pub flip_hold_ms: (u32, u32),
    /// Pause after releasing a flipper before the next decision on it, ms.
    pub cooldown_ms: u32,
    /// Whether the bot cradles slow balls.
    pub cradle: bool,
    /// Speed below which a ball on a bat is cradled, m/s.
    pub cradle_speed_m_s: f32,
    /// Cradle hold time range (inclusive), ms.
    pub cradle_hold_ms: (u32, u32),
    /// After a cradle, time during which that flipper flips instead of
    /// cradling again, ms.
    pub post_cradle_ms: u32,
    /// Speed below which a ball counts as resting, m/s.
    pub rest_speed_m_s: f32,
    /// Rest time before the bot plunges, ms.
    pub launch_after_ms: u32,
    /// `launch` hold time range (inclusive), ms.
    pub launch_hold_ms: (u32, u32),
    /// Shortest time between two plunges, ms.
    pub launch_interval_ms: u32,
    /// Delay range of the first `start` press (inclusive), ms.
    pub start_delay_ms: (u32, u32),
    /// Time without a ball after which `start` is pressed again, ms.
    pub start_retry_ms: u32,
    /// Most `start` presses per game.
    pub max_starts: u32,
    /// Nudging.
    pub nudge: NudgeConfig,
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            lead_ms: 30,
            flip_delay_ms: (0, 35),
            reaction: ReactionDraw::Centred,
            flip_hold_ms: (100, 180),
            cooldown_ms: 80,
            cradle: true,
            cradle_speed_m_s: 0.25,
            cradle_hold_ms: (400, 1500),
            post_cradle_ms: 800,
            rest_speed_m_s: 0.05,
            launch_after_ms: 300,
            launch_hold_ms: (100, 800),
            launch_interval_ms: 1500,
            start_delay_ms: (20, 200),
            start_retry_ms: 3000,
            max_starts: 3,
            nudge: NudgeConfig::default(),
        }
    }
}

/// What the bot did (counts since [`Bot::new`]).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BotStats {
    /// Predicted crossings (flip decisions, taken or missed).
    pub predictions: u64,
    /// Flips pressed because of a prediction.
    pub flips_on_prediction: u64,
    /// Predicted crossings skipped by the miss rate.
    pub misses: u64,
    /// Cradles.
    pub cradles: u64,
    /// `launch` presses.
    pub launches: u64,
    /// Nudges.
    pub nudges: u64,
    /// `start` presses.
    pub starts: u64,
}

impl BotStats {
    /// Adds `o` field by field (saturating).
    pub fn add(&mut self, o: &BotStats) {
        self.predictions = self.predictions.saturating_add(o.predictions);
        self.flips_on_prediction = self
            .flips_on_prediction
            .saturating_add(o.flips_on_prediction);
        self.misses = self.misses.saturating_add(o.misses);
        self.cradles = self.cradles.saturating_add(o.cradles);
        self.launches = self.launches.saturating_add(o.launches);
        self.nudges = self.nudges.saturating_add(o.nudges);
        self.starts = self.starts.saturating_add(o.starts);
    }
}

/// A flipper's sweep sector in the table plane.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FlipperModel {
    /// Flipper id.
    pub id: FlipperId,
    /// Collision layer index.
    pub layer: u8,
    /// Unit direction of the bat at rest (table plane).
    pub rest_dir: Vec2,
    /// Signed sweep, rad (+ = counter-clockwise).
    pub sweep_rad: f32,
    /// Bat length, m.
    pub length_m: f32,
    /// Bat radius, m.
    pub radius_m: f32,
    /// The button driving it.
    pub button: Option<CabinetInput>,
}

fn rotate(v: Vec2, a: f32) -> Vec2 {
    let (s, c) = (libm::sinf(a), libm::cosf(a));
    Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y)
}

impl FlipperModel {
    /// The model of flipper `id` from the simulation's flipper state: the
    /// rest direction is the current bat direction turned back by the
    /// relative angle. Length, radius, sweep, layer and button come from
    /// `info` when given; otherwise the defaults ([`DEFAULT_BAT_LENGTH_M`],
    /// [`DEFAULT_BAT_RADIUS_M`], layer 0) and the side: a bat pointing to
    /// `+x` at rest is a left flipper (counter-clockwise sweep, the
    /// `left_flipper` button), else a right one.
    #[must_use]
    pub fn infer(sim: &Simulation, id: FlipperId, info: Option<&TableInfo>) -> Option<Self> {
        let fs = sim.flipper_set();
        let dir = fs.bat_direction(id)?;
        let rel = fs.relative_angle_rad(id);
        let rel = if rel.is_finite() { rel } else { 0.0 };
        let rest = rotate(Vec2::new(dir.x, dir.y), -rel).normalize_or_zero();
        let left = rest.x >= 0.0;
        let known = info.and_then(|t| t.flippers.iter().find(|f| f.id == id));
        Some(match known {
            Some(f) => FlipperModel {
                id,
                layer: f.layer,
                rest_dir: rest,
                sweep_rad: f.sweep_rad,
                length_m: f.length_m,
                radius_m: f.radius_m,
                button: f.button,
            },
            None => FlipperModel {
                id,
                layer: 0,
                rest_dir: rest,
                sweep_rad: if left {
                    DEFAULT_SWEEP_RAD
                } else {
                    -DEFAULT_SWEEP_RAD
                },
                length_m: DEFAULT_BAT_LENGTH_M,
                radius_m: DEFAULT_BAT_RADIUS_M,
                button: Some(if left {
                    CabinetInput::LeftFlipper
                } else {
                    CabinetInput::RightFlipper
                }),
            },
        })
    }

    /// Reach of the sector from the pivot to a touching ball centre, m.
    #[must_use]
    pub fn reach(&self) -> f32 {
        self.length_m + self.radius_m + BALL_RADIUS_M
    }

    /// Whether a ball centred at `q` (table plane) lies in the swept sector
    /// of the bat pivoting at `pivot`.
    #[must_use]
    pub fn in_sector(&self, pivot: Vec2, q: Vec2) -> bool {
        let d = q - pivot;
        let r = d.length();
        if !(r.is_finite() && r <= self.reach()) {
            return false;
        }
        if r < 1.0e-4 {
            return true;
        }
        let ang = libm::atan2f(self.rest_dir.perp_dot(d), self.rest_dir.dot(d));
        let s = if self.sweep_rad >= 0.0 { ang } else { -ang };
        let touch = libm::asinf(((self.radius_m + BALL_RADIUS_M) / r).min(1.0));
        (-0.05..=self.sweep_rad.abs() + touch).contains(&s)
    }

    /// The first time (s, in steps of 2 ms up to `horizon_s`) at which a
    /// ball at `p` with velocity `v` under plane gravity `g` is in the
    /// sector, or `None`.
    #[must_use]
    pub fn predict_entry(
        &self,
        pivot: Vec2,
        p: Vec2,
        v: Vec2,
        g: Vec2,
        horizon_s: f32,
    ) -> Option<f32> {
        let steps = (horizon_s / PREDICT_STEP_S).max(0.0) as u32;
        (0..=steps).find_map(|k| {
            let t = k as f32 * PREDICT_STEP_S;
            let q = p + v * t + g * (0.5 * t * t);
            self.in_sector(pivot, q).then_some(t)
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Action {
    Flip,
    Cradle,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum FlipState {
    Idle,
    Pending { at: u64, action: Action },
    Pressed { release_at: u64, cradle: bool },
    Cooldown { until: u64 },
}

#[derive(Copy, Clone, Debug)]
struct FlipperBrain {
    model: FlipperModel,
    state: FlipState,
    ignore_until: u64,
    post_cradle_until: u64,
}

/// A ball the bot sees this tick (table plane).
#[derive(Copy, Clone, Debug)]
struct SeenBall {
    slot: u8,
    p: Vec2,
    v: Vec2,
    layer: u8,
}

/// The seeded heuristic player (see the [module docs](self)).
#[derive(Clone, Debug)]
pub struct Bot {
    rng: Pcg32,
    seed: u64,
    skill: SkillLevel,
    params: SkillParams,
    cfg: BotConfig,
    table: Option<TableInfo>,
    brains: Option<ArrayVec<FlipperBrain, MAX_FLIPPERS>>,
    /// Buttons as last applied to the simulation (`CabinetInput::ALL`
    /// order).
    applied: [bool; 8],
    seen_ball: bool,
    next_start_at: u64,
    /// Raw draw taken at construction, so a later config can reschedule the
    /// first start without consuming more randomness.
    start_draw: u32,
    start_release_at: u64,
    launch_release_at: u64,
    next_launch_at: u64,
    next_nudge_at: u64,
    /// Rest ticks per ball slot (away from the flippers).
    rest_ticks: [u32; MAX_BALLS],
    /// Consecutive ticks with no ball on the table.
    empty_ticks: u32,
    stats: BotStats,
}

impl Bot {
    /// A bot drawing from a [`Pcg32`] seeded with `seed`, playing at
    /// `skill` with the default [`BotConfig`]. Without
    /// [`with_table`](Self::with_table) it infers the flipper geometry
    /// (see [`FlipperModel::infer`]).
    #[must_use]
    pub fn new(seed: u64, skill: SkillLevel) -> Bot {
        let mut rng = Pcg32::seed_from_u64(seed);
        let cfg = BotConfig::default();
        let start_draw = rng.next_u32();
        let next_start_at = 1 + u64::from(range_from_draw(start_draw, cfg.start_delay_ms));
        Bot {
            rng,
            seed,
            skill,
            params: skill.params(),
            cfg,
            table: None,
            brains: None,
            applied: [false; 8],
            seen_ball: false,
            next_start_at,
            start_draw,
            start_release_at: 0,
            launch_release_at: 0,
            next_launch_at: 0,
            next_nudge_at: 0,
            rest_ticks: [0; MAX_BALLS],
            empty_ticks: 0,
            stats: BotStats::default(),
        }
    }

    /// Uses `table`'s flipper data (length, radius, sweep, layer, button).
    #[must_use]
    pub fn with_table(mut self, table: &TableInfo) -> Bot {
        self.table = Some(table.clone());
        self.brains = None;
        self
    }

    /// Replaces the shared behaviour settings.
    #[must_use]
    pub fn with_config(mut self, cfg: BotConfig) -> Bot {
        self.cfg = cfg;
        if self.stats.starts == 0 {
            // The first start has not been pressed yet: reschedule it from
            // the configured delay using the draw taken in `new`.
            self.next_start_at =
                1 + u64::from(range_from_draw(self.start_draw, self.cfg.start_delay_ms));
        }
        self
    }

    /// Replaces the skill parameters (the level keeps its name).
    #[must_use]
    pub fn with_skill_params(mut self, params: SkillParams) -> Bot {
        self.params = params;
        self
    }

    /// The seed.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The skill level.
    #[must_use]
    pub fn skill(&self) -> SkillLevel {
        self.skill
    }

    /// The skill parameters in use.
    #[must_use]
    pub fn skill_params(&self) -> SkillParams {
        self.params
    }

    /// The behaviour settings in use.
    #[must_use]
    pub fn config(&self) -> &BotConfig {
        &self.cfg
    }

    /// What the bot has done so far.
    #[must_use]
    pub fn stats(&self) -> BotStats {
        self.stats
    }

    /// Whether a ball has been on the table since the bot started.
    #[must_use]
    pub fn seen_ball(&self) -> bool {
        self.seen_ball
    }

    /// The flipper models (built on the first [`tick`](Self::tick)).
    #[must_use]
    pub fn flipper_models(&self) -> Vec<FlipperModel> {
        self.brains
            .as_ref()
            .map(|b| b.iter().map(|x| x.model).collect())
            .unwrap_or_default()
    }

    fn chance(&mut self, p: f32) -> bool {
        let u = (self.rng.next_u32() >> 8) as f32 / (1u32 << 24) as f32;
        u < p
    }

    /// The planned delay of one flip, ms: drawn from
    /// [`BotConfig::flip_delay_ms`] (no draw when the range is one value).
    fn planned_flip_delay(&mut self) -> u32 {
        let (lo, hi) = self.cfg.flip_delay_ms;
        if hi <= lo {
            lo
        } else {
            draw_range(&mut self.rng, (lo, hi))
        }
    }

    /// Ticks from the decision to the press: `planned_ms` plus one reaction
    /// error draw ([`ReactionDraw`]), never negative.
    fn reaction_delay(&mut self, planned_ms: u32) -> u64 {
        let j = self.params.reaction_jitter_ms;
        match self.cfg.reaction {
            ReactionDraw::Late => {
                u64::from(planned_ms) + u64::from(draw_range(&mut self.rng, (0, j)))
            }
            ReactionDraw::Centred => {
                // The span 2*j is computed in u64 so it never saturates. When
                // it fits a u32 the draw is the single 32-bit draw it always
                // was; only a wider span takes a 64-bit draw.
                let span = u64::from(j) * 2;
                let raw = match u32::try_from(span) {
                    Ok(s) => u64::from(draw_range(&mut self.rng, (0, s))),
                    Err(_) => self.rng.next_u64() % (span + 1),
                };
                let err = raw as i64 - i64::from(j);
                u64::try_from(i64::from(planned_ms) + err).unwrap_or(0)
            }
        }
    }

    /// Presses a flipper for `action`: counts it and draws the hold time.
    fn press(&mut self, now: u64, action: Action) -> FlipState {
        let (range, cradle) = match action {
            Action::Flip => {
                self.stats.flips_on_prediction += 1;
                (self.cfg.flip_hold_ms, false)
            }
            Action::Cradle => {
                self.stats.cradles += 1;
                (self.cfg.cradle_hold_ms, true)
            }
        };
        let hold = u64::from(draw_range(&mut self.rng, range)).max(1);
        FlipState::Pressed {
            release_at: now + hold,
            cradle,
        }
    }

    fn build(&mut self, sim: &Simulation) {
        let mut brains = ArrayVec::new();
        for f in sim.snapshot().flippers() {
            if let Some(model) = FlipperModel::infer(sim, f.id, self.table.as_ref()) {
                brains.push(FlipperBrain {
                    model,
                    state: FlipState::Idle,
                    ignore_until: 0,
                    post_cradle_until: 0,
                });
            }
        }
        self.brains = Some(brains);
    }

    /// Decides this tick's inputs and applies them to `sim` (button presses
    /// and releases through the switch API, nudges as a cabinet velocity
    /// change). Call once per tick, before `sim.advance(1)`.
    pub fn tick(&mut self, sim: &mut Simulation) {
        self.tick_recorded(sim, &mut ());
    }

    /// [`tick`](Self::tick), also reporting every operation it applies to
    /// `sim` to `sink`, in application order, with the tick it is applied
    /// before (`sim.tick() + 1`): the nudge first, then the switch changes.
    /// Only successful calls are reported. The decisions do not depend on
    /// `sink`; with `()` this is exactly [`tick`](Self::tick). A
    /// [`GameRecording`](crate::replay::GameRecording) sink makes the game
    /// replayable without the bot ([`crate::replay::replay_game`]).
    pub fn tick_recorded<S: GameOpSink>(&mut self, sim: &mut Simulation, sink: &mut S) {
        if self.brains.is_none() {
            self.build(sim);
        }
        let now = sim.tick() + 1;
        let g3 = sim.solver().config().gravity;
        let g = Vec2::new(g3.x, g3.y);
        let up = (-g).normalize_or_zero();

        let mut balls: ArrayVec<SeenBall, MAX_BALLS> = ArrayVec::new();
        for h in sim.ball_handles() {
            if let Some(b) = sim.ball(h) {
                let p = Vec3::from_array(b.position_m);
                let v = Vec3::from_array(b.velocity_m_s);
                if p.is_finite() && v.is_finite() {
                    balls.push(SeenBall {
                        slot: h.slot(),
                        p: Vec2::new(p.x, p.y),
                        v: Vec2::new(v.x, v.y),
                        layer: b.layer,
                    });
                }
            }
        }
        if balls.is_empty() {
            self.empty_ticks = self.empty_ticks.saturating_add(1);
        } else {
            self.seen_ball = true;
            self.empty_ticks = 0;
        }

        let mut desired = [false; 8];
        self.update_flippers(sim, now, &balls, g, up, &mut desired);
        let resting = self.update_rest(sim, &balls);
        self.update_start(now, &mut desired);
        self.update_launch(now, resting, self.empty_ticks, &mut desired);
        self.update_nudge(sim, now, resting, sink);
        self.apply(sim, now, &desired, sink);
    }

    fn update_flippers(
        &mut self,
        sim: &Simulation,
        now: u64,
        balls: &[SeenBall],
        g: Vec2,
        up: Vec2,
        desired: &mut [bool; 8],
    ) {
        let Some(mut brains) = self.brains.take() else {
            return;
        };
        let horizon = self.cfg.lead_ms as f32 * 1.0e-3;
        for brain in &mut brains {
            let m = brain.model;
            let Some(button) = m.button else {
                continue;
            };
            let Some(pivot3) = sim.flipper_set().pivot_m(m.id) else {
                continue;
            };
            let pivot = Vec2::new(pivot3.x, pivot3.y);
            // State transitions first.
            match brain.state {
                FlipState::Pending { at, action } if now >= at => {
                    brain.state = self.press(now, action);
                }
                FlipState::Pressed { release_at, cradle } if now >= release_at => {
                    if cradle {
                        brain.post_cradle_until = now + u64::from(self.cfg.post_cradle_ms);
                    }
                    brain.state = FlipState::Cooldown {
                        until: now + u64::from(self.cfg.cooldown_ms),
                    };
                }
                FlipState::Cooldown { until } if now >= until => brain.state = FlipState::Idle,
                _ => {}
            }
            if brain.state == FlipState::Idle && now >= brain.ignore_until {
                let mut decision = None;
                for b in balls.iter().filter(|b| b.layer == m.layer) {
                    if b.v.dot(up) > AWAY_SPEED_M_S {
                        continue;
                    }
                    let inside = m.in_sector(pivot, b.p);
                    if inside && b.v.length() < self.cfg.cradle_speed_m_s {
                        if self.cfg.cradle && now >= brain.post_cradle_until {
                            decision = Some(Action::Cradle);
                            break;
                        }
                        if b.p.distance(pivot) >= CRADLE_FLIP_REACH * m.reach() {
                            decision = Some(Action::Flip);
                            break;
                        }
                        continue;
                    }
                    if m.predict_entry(pivot, b.p, b.v, g, horizon).is_some() {
                        decision = Some(Action::Flip);
                        break;
                    }
                }
                match decision {
                    Some(Action::Flip) => {
                        self.stats.predictions += 1;
                        if self.chance(self.params.miss_rate) {
                            self.stats.misses += 1;
                            brain.ignore_until = now + MISS_LOCKOUT_TICKS;
                        } else {
                            let planned = self.planned_flip_delay();
                            let at = now + self.reaction_delay(planned);
                            brain.state = FlipState::Pending {
                                at,
                                action: Action::Flip,
                            };
                        }
                    }
                    Some(Action::Cradle) => {
                        let at = now + self.reaction_delay(0);
                        brain.state = FlipState::Pending {
                            at,
                            action: Action::Cradle,
                        };
                    }
                    None => {}
                }
                // A zero delay presses in this very tick.
                if let FlipState::Pending { at, action } = brain.state
                    && now >= at
                {
                    brain.state = self.press(now, action);
                }
            }
            if matches!(brain.state, FlipState::Pressed { .. }) {
                desired[button.index()] = true;
            }
        }
        self.brains = Some(brains);
    }

    /// Updates the per-slot rest counters; returns whether some ball has
    /// rested away from the flippers for at least `launch_after_ms`, and
    /// the longest rest in ticks.
    fn update_rest(&mut self, sim: &Simulation, balls: &[SeenBall]) -> (bool, u32) {
        let mut present = [false; MAX_BALLS];
        let mut longest = 0u32;
        for b in balls {
            let slot = usize::from(b.slot);
            present[slot] = true;
            let near_flipper = self.brains.as_ref().is_some_and(|brains| {
                brains.iter().any(|x| {
                    x.model.layer == b.layer
                        && sim
                            .flipper_set()
                            .pivot_m(x.model.id)
                            .is_some_and(|p| b.p.distance(Vec2::new(p.x, p.y)) <= x.model.reach())
                })
            });
            if b.v.length() < self.cfg.rest_speed_m_s && !near_flipper {
                self.rest_ticks[slot] = self.rest_ticks[slot].saturating_add(1);
            } else {
                self.rest_ticks[slot] = 0;
            }
            longest = longest.max(self.rest_ticks[slot]);
        }
        for (slot, p) in present.iter().enumerate() {
            if !p {
                self.rest_ticks[slot] = 0;
            }
        }
        (longest >= self.cfg.launch_after_ms, longest)
    }

    fn update_start(&mut self, now: u64, desired: &mut [bool; 8]) {
        if now < self.start_release_at {
            desired[CabinetInput::Start.index()] = true;
            return;
        }
        if !self.seen_ball
            && now >= self.next_start_at
            && self.stats.starts < u64::from(self.cfg.max_starts)
        {
            self.stats.starts += 1;
            self.start_release_at = now + START_HOLD_TICKS;
            self.next_start_at = now + u64::from(self.cfg.start_retry_ms);
            desired[CabinetInput::Start.index()] = true;
        }
    }

    fn update_launch(
        &mut self,
        now: u64,
        (resting, _): (bool, u32),
        empty_ticks: u32,
        desired: &mut [bool; 8],
    ) {
        if now < self.launch_release_at {
            desired[CabinetInput::Launch.index()] = true;
            return;
        }
        let waiting = self.stats.starts > 0
            && now >= self.start_release_at
            && empty_ticks >= self.cfg.launch_after_ms;
        if (resting || waiting) && now >= self.next_launch_at {
            let hold = u64::from(draw_range(&mut self.rng, self.cfg.launch_hold_ms)).max(1);
            self.stats.launches += 1;
            self.launch_release_at = now + hold;
            self.next_launch_at = now + hold + u64::from(self.cfg.launch_interval_ms);
            desired[CabinetInput::Launch.index()] = true;
        }
    }

    fn update_nudge<S: GameOpSink>(
        &mut self,
        sim: &mut Simulation,
        now: u64,
        (_, longest): (bool, u32),
        sink: &mut S,
    ) {
        let n = self.cfg.nudge;
        if !n.enabled
            || longest < n.after_rest_ms
            || now < self.next_nudge_at
            || self.stats.nudges >= u64::from(n.max_per_game)
        {
            return;
        }
        let bob = sim.mech_set().tilt_bob();
        let gap = bob.params().gap;
        if bob.offset().length() >= n.max_bob_fraction * gap {
            return;
        }
        let dir = if self.rng.next_u32() & 1 == 0 {
            1.0
        } else {
            -1.0
        };
        let dv = Vec3::new(
            dir * n.strength.clamp(0.0, 1.0) * NUDGE_DV_PER_TICK,
            0.0,
            0.0,
        );
        // The same cabinet velocity change a `NudgeX` axis event applies in
        // tick step 1; applied between ticks, outside the tick path.
        sim.mechs.nudge(dv);
        sink.op(now, GameOp::Nudge { dv: dv.to_array() });
        self.stats.nudges += 1;
        self.next_nudge_at = now + u64::from(n.min_interval_ms);
    }

    fn apply<S: GameOpSink>(
        &mut self,
        sim: &mut Simulation,
        now: u64,
        desired: &[bool; 8],
        sink: &mut S,
    ) {
        for c in CabinetInput::ALL {
            let i = c.index();
            if desired[i] == self.applied[i] {
                continue;
            }
            // Cabinet switches always exist (`[cabinet_switches]` is
            // mandatory), so these cannot fail on a loaded table.
            let (ok, op) = if desired[i] {
                (
                    sim.set_switch(c.name(), true).is_ok(),
                    GameOp::SetSwitch {
                        input: c,
                        closed: true,
                    },
                )
            } else {
                (
                    sim.release_switch(c.name()).is_ok(),
                    GameOp::ReleaseSwitch { input: c },
                )
            };
            if ok {
                sink.op(now, op);
                self.applied[i] = desired[i];
            }
        }
    }
}

/// A uniform draw from `lo..=hi` (`hi < lo` gives `lo`).
fn draw_range(rng: &mut Pcg32, range: (u32, u32)) -> u32 {
    range_from_draw(rng.next_u32(), range)
}

/// Maps a raw 32-bit draw into the inclusive `(lo, hi)` range.
fn range_from_draw(draw: u32, (lo, hi): (u32, u32)) -> u32 {
    if hi <= lo {
        return lo;
    }
    let span = u64::from(hi - lo) + 1;
    lo + (u64::from(draw) % span) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn left() -> FlipperModel {
        FlipperModel {
            id: FlipperId(0),
            layer: 0,
            rest_dir: rotate(Vec2::X, (-30.0f32).to_radians()),
            sweep_rad: 50.0f32.to_radians(),
            length_m: 0.076,
            radius_m: 0.011,
            button: Some(CabinetInput::LeftFlipper),
        }
    }

    #[test]
    fn skill_names_round_trip() {
        for l in SkillLevel::ALL {
            assert_eq!(SkillLevel::from_name(l.name()), Some(l));
            assert_eq!(SkillLevel::from_name(&l.name().to_uppercase()), Some(l));
        }
        assert_eq!(SkillLevel::from_name("pro"), None);
        let n = SkillLevel::Novice.params();
        let e = SkillLevel::Expert.params();
        assert!(n.reaction_jitter_ms > e.reaction_jitter_ms && n.miss_rate > e.miss_rate);
    }

    #[test]
    fn sector_contains_ball_on_bat_not_below() {
        let m = left();
        let pivot = Vec2::ZERO;
        // On top of the rest bat, half way out.
        let along = m.rest_dir * 0.04;
        let normal = rotate(m.rest_dir, core::f32::consts::FRAC_PI_2);
        assert!(m.in_sector(pivot, along + normal * 0.02));
        // Below the bat.
        assert!(!m.in_sector(pivot, along - normal * 0.03));
        // Too far out.
        assert!(!m.in_sector(pivot, m.rest_dir * 0.2));
        // At the pivot.
        assert!(m.in_sector(pivot, Vec2::ZERO));
        assert!(!m.in_sector(pivot, Vec2::NAN));
    }

    #[test]
    fn right_flipper_sector_is_mirrored() {
        let l = left();
        let r = FlipperModel {
            rest_dir: Vec2::new(-l.rest_dir.x, l.rest_dir.y),
            sweep_rad: -l.sweep_rad,
            ..l
        };
        for q in [
            Vec2::new(0.03, 0.0),
            Vec2::new(0.05, -0.01),
            Vec2::new(0.02, -0.03),
            Vec2::new(0.06, 0.04),
        ] {
            let mirrored = Vec2::new(-q.x, q.y);
            assert_eq!(
                l.in_sector(Vec2::ZERO, q),
                r.in_sector(Vec2::ZERO, mirrored),
                "{q}"
            );
        }
    }

    #[test]
    fn prediction_finds_falling_ball() {
        let m = left();
        let g = Vec2::new(0.0, -1.1);
        // A ball 3 cm above the bat's middle, falling at 1 m/s.
        let start = m.rest_dir * 0.04 + Vec2::new(0.0, 0.06);
        let t = m.predict_entry(Vec2::ZERO, start, Vec2::new(0.0, -1.0), g, 0.06);
        assert!(t.is_some_and(|t| t > 0.0 && t < 0.06), "{t:?}");
        // Moving away: never enters within the horizon.
        assert_eq!(
            m.predict_entry(Vec2::ZERO, start, Vec2::new(0.0, 1.0), g, 0.03),
            None
        );
        // Zero horizon only checks now.
        assert_eq!(m.predict_entry(Vec2::ZERO, start, Vec2::ZERO, g, 0.0), None);
    }

    #[test]
    fn draw_range_bounds() {
        let mut rng = Pcg32::seed_from_u64(7);
        for _ in 0..1000 {
            let v = draw_range(&mut rng, (5, 9));
            assert!((5..=9).contains(&v));
        }
        assert_eq!(draw_range(&mut rng, (4, 4)), 4);
        assert_eq!(draw_range(&mut rng, (9, 3)), 9);
    }

    #[test]
    fn stats_add_saturates() {
        let mut a = BotStats {
            predictions: u64::MAX,
            ..BotStats::default()
        };
        let b = BotStats {
            predictions: 1,
            flips_on_prediction: 2,
            misses: 3,
            cradles: 4,
            launches: 5,
            nudges: 6,
            starts: 7,
        };
        a.add(&b);
        assert_eq!(a.predictions, u64::MAX);
        assert_eq!(a.starts, 7);
        assert_eq!(a.nudges, 6);
    }

    fn timing_bot(delay: (u32, u32), reaction: ReactionDraw, jitter: u32) -> Bot {
        Bot::new(11, SkillLevel::Standard)
            .with_config(BotConfig {
                flip_delay_ms: delay,
                reaction,
                ..BotConfig::default()
            })
            .with_skill_params(SkillParams {
                reaction_jitter_ms: jitter,
                miss_rate: 0.0,
            })
    }

    #[test]
    fn default_flip_timing_is_the_calibrated_window() {
        let c = BotConfig::default();
        assert_eq!(c.flip_delay_ms, (0, 35));
        assert_eq!(c.reaction, ReactionDraw::Centred);
        assert_eq!(c.lead_ms, 30);
    }

    #[test]
    fn single_value_window_takes_no_draw() {
        let mut a = timing_bot((12, 12), ReactionDraw::Centred, 5);
        let mut b = a.clone();
        assert_eq!(a.planned_flip_delay(), 12);
        // The generator did not move: both bots draw the same next value.
        assert_eq!(a.rng.next_u32(), b.rng.next_u32());
        let mut w = timing_bot((0, 35), ReactionDraw::Centred, 5);
        let seen: std::collections::BTreeSet<u32> =
            (0..2000).map(|_| w.planned_flip_delay()).collect();
        assert_eq!(seen.first(), Some(&0));
        assert_eq!(seen.last(), Some(&35));
        assert_eq!(seen.len(), 36, "every value of the window is reachable");
    }

    #[test]
    fn centred_reaction_spreads_both_ways_late_only_after() {
        let mut c = timing_bot((0, 0), ReactionDraw::Centred, 10);
        let d: Vec<u64> = (0..4000).map(|_| c.reaction_delay(20)).collect();
        assert_eq!(d.iter().min(), Some(&10));
        assert_eq!(d.iter().max(), Some(&30));
        let mean = d.iter().sum::<u64>() as f64 / d.len() as f64;
        assert!((mean - 20.0).abs() < 0.5, "centred mean {mean}");
        let mut l = timing_bot((0, 0), ReactionDraw::Late, 10);
        let d: Vec<u64> = (0..4000).map(|_| l.reaction_delay(20)).collect();
        assert_eq!(d.iter().min(), Some(&20));
        assert_eq!(d.iter().max(), Some(&30));
        // Clamped at the decision tick: never negative.
        let mut z = timing_bot((0, 0), ReactionDraw::Centred, 40);
        let d: Vec<u64> = (0..4000).map(|_| z.reaction_delay(0)).collect();
        assert_eq!(d.iter().min(), Some(&0));
        assert_eq!(d.iter().max(), Some(&40));
        assert!(
            d.iter().filter(|&&x| x == 0).count() > 1500,
            "early presses clamp to 0"
        );
    }

    #[test]
    fn centred_reaction_keeps_full_range_for_huge_jitter() {
        for j in [u32::MAX, u32::MAX / 2 + 1] {
            let mut b = timing_bot((0, 0), ReactionDraw::Centred, j);
            let d: Vec<u64> = (0..2000).map(|_| b.reaction_delay(0)).collect();
            assert!(d.iter().any(|&x| x > 0), "positive error reachable, j={j}");
            assert!(d.contains(&0), "negative error clamps, j={j}");
            assert!(d.iter().all(|&x| x <= u64::from(j)), "j={j}");
            let above = d.iter().filter(|&&x| x > 0).count();
            assert!((800..1200).contains(&above), "about half positive: {above}");
        }
    }

    #[test]
    fn centred_reaction_matches_the_old_formula_up_to_half_u32() {
        for j in [0u32, 1, 35, 120, u32::MAX / 2] {
            let mut b = timing_bot((0, 0), ReactionDraw::Centred, j);
            let mut rng = b.rng.clone();
            for _ in 0..1000 {
                let span = j.saturating_mul(2);
                let err = i64::from(draw_range(&mut rng, (0, span))) - i64::from(j);
                let want = u64::try_from(20i64 + err).unwrap_or(0);
                assert_eq!(b.reaction_delay(20), want, "j={j}");
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn reaction_delay_stays_in_bounds(
            seed in proptest::prelude::any::<u64>(),
            planned in 0u32..1000,
            jitter in 0u32..u32::MAX,
            late in proptest::prelude::any::<bool>(),
        ) {
            let reaction = if late { ReactionDraw::Late } else { ReactionDraw::Centred };
            let mut b = Bot::new(seed, SkillLevel::Novice)
                .with_config(BotConfig { reaction, ..BotConfig::default() })
                .with_skill_params(SkillParams { reaction_jitter_ms: jitter, miss_rate: 0.0 });
            let d = b.reaction_delay(planned);
            let (p, j) = (u64::from(planned), u64::from(jitter));
            if late {
                proptest::prop_assert!(d >= p && d <= p + j);
            } else {
                proptest::prop_assert!(d >= p.saturating_sub(j) && d <= p + j);
            }
        }

        #[test]
        fn planned_flip_delay_stays_in_window(
            seed in proptest::prelude::any::<u64>(),
            lo in 0u32..u32::MAX,
            hi in 0u32..u32::MAX,
        ) {
            let mut b = Bot::new(seed, SkillLevel::Expert)
                .with_config(BotConfig { flip_delay_ms: (lo, hi), ..BotConfig::default() });
            let d = b.planned_flip_delay();
            if hi <= lo {
                proptest::prop_assert_eq!(d, lo);
            } else {
                proptest::prop_assert!((lo..=hi).contains(&d));
            }
        }
    }
}
