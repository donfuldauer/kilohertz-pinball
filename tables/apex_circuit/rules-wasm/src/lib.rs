//! Apex Circuit rules (Subphase 6.1, variant A) ported one-to-one from
//! `../rules.lua` to Rust on `pin_guest_sdk` (Subphase 6.3), built to
//! `../rules.wasm` by `rebuild.sh`.
//!
//! Keep the two in lockstep: every host call, its order and its arguments
//! match the Lua rules, so a recorded replay produces the same per-tick
//! command queue under either backend (`crates/pin_app/tests/wasm_ports_*`).
//! The game rules themselves are documented in `../rules.lua` and
//! `../README.md`; the comments below only point at the Lua functions.
//!
//! Lua idioms and their Rust form:
//! * an id looked up by name is an `Option<u16>` (`nil` is `None`); a
//!   comparison `id == sw.x` with a missing name is false, and passing a
//!   missing id to a `pinball` function faults the rules, as in Lua;
//! * `bind_ids` runs lazily at the top of every callback until the `start`
//!   switch is bound, exactly as the Lua guard `sw.start ~= nil`;
//! * Lua integers are `i64`; `%` and `//` are floor operations
//!   (`rem_euclid` / `div_euclid`).

// no_std in the guest; the host build (unit tests, clippy) links std for
// its panic handler.
#![cfg_attr(target_arch = "wasm32", no_std)]

mod saved;

use pin_guest_sdk::{self as pin, Kind, Rules};
use saved::saved_fields;

const ATTRACT: i64 = 0;
const GAME: i64 = 1;
const OVER: i64 = 2;
const BALLS_PER_GAME: i64 = 3;
const POWERBALL_BALL: i64 = 2;
const DETECT_WINDOW: i64 = 30;
const MAGNET_PULSE: i64 = 40;
const SKILL_WINDOW: i64 = 6000;
const SKILL_VALUE: i64 = 10000;
const LANE_VALUE: i64 = 1000;
const COMBO_WINDOW: i64 = 4000;
const COMBO_DECAY: i64 = 12000;
const COMBO_MAX: i64 = 5;
const MULT_CAP: i64 = 10;
const OD_RAMPS: i64 = 3;
const OD_STEPS: i64 = 5;
const INLANE_WINDOW: i64 = 3000;
const ZIPPER_TICKS: i64 = 5000;
const JACKPOT_START: i64 = 50000;
const JACKPOT_STEP: i64 = 1000;
const JACKPOT_STEP_TICKS: i64 = 100;
const JACKPOT_FLOOR: i64 = 5000;
const BONUS_UNIT: i64 = 1000;
const BONUS_TICKS: i64 = 2000;
const HIGH_SCORE_TICKS: i64 = 3000;
const GAME_OVER_TICKS: i64 = 4000;
const SCOOP_HOLD: i64 = 1000;
const DROP_RESET_DELAY: i64 = 1000;
const NEVER: i64 = -1000000;

// Points per switch (multiplied).
const P_BUMPER: i64 = 100;
const P_SLING: i64 = 10;
const P_SPINNER: i64 = 50;
const P_ORBIT_TOP: i64 = 2000;
const P_RO_ENTRY: i64 = 1000;
const P_MAGNET: i64 = 2500;
const P_RAMP_ENTRY: i64 = 500;
const P_RAMP_MADE: i64 = 10000;
const P_DROP: i64 = 750;
const P_BANK: i64 = 10000;
const P_LOCK_LANE: i64 = 100;
const P_SCOOP: i64 = 5000;
const P_INLANE: i64 = 100;

/// Persist keys hold at most this many slots (`pin_core::script::MAX_PERSIST`).
const PERSIST_SLOTS: usize = 64;

/// Faults the rules (a trap in the guest), as a Lua error does.
#[cold]
fn fault() -> ! {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::unreachable()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        panic!("apex_circuit rules: a nil id was passed to pinball")
    }
}

/// The id a `pinball` function receives: a missing name (`nil`) faults.
fn id(x: Option<u16>) -> u16 {
    match x {
        Some(v) => v,
        None => fault(),
    }
}

/// The Lua `written` cache slot of persist key `k` (keys are `< 64`; a
/// larger one faults like the `persist_set` it would reach).
fn slot(k: u16) -> usize {
    let k = usize::from(k);
    if k >= PERSIST_SLOTS {
        fault();
    }
    k
}

/// The id tables of `bind_ids` (`sw`, `coil`, `scene`, `snd`, `lamp`, `key`,
/// `lane_sw`, `lane_lamp` and the single ids).
#[derive(Clone, Copy)]
struct Ids {
    start: Option<u16>,
    launch: Option<u16>,
    action: Option<u16>,
    trough_1: Option<u16>,
    trough_2: Option<u16>,
    b1: Option<u16>,
    b2: Option<u16>,
    b3: Option<u16>,
    sl: Option<u16>,
    sr: Option<u16>,
    spinner: Option<u16>,
    orbit_top: Option<u16>,
    ramp_entry: Option<u16>,
    ramp_made: Option<u16>,
    scoop: Option<u16>,
    lock_lane: Option<u16>,
    eddy: Option<u16>,
    opto: Option<u16>,
    d1: Option<u16>,
    d2: Option<u16>,
    d3: Option<u16>,
    ro_entry: Option<u16>,
    shooter_lane: Option<u16>,
    left_inlane: Option<u16>,
    right_inlane: Option<u16>,
    lane_sw: [Option<u16>; 3],
    coil_b1: Option<u16>,
    coil_b2: Option<u16>,
    coil_b3: Option<u16>,
    coil_sl: Option<u16>,
    coil_sr: Option<u16>,
    coil_scoop: Option<u16>,
    coil_plunger: Option<u16>,
    coil_drop: Option<u16>,
    scene_attract: Option<u16>,
    scene_score: Option<u16>,
    scene_over: Option<u16>,
    scene_pb: Option<u16>,
    scene_chain: Option<u16>,
    scene_od: Option<u16>,
    scene_bonus: Option<u16>,
    scene_high: Option<u16>,
    scene_skill: Option<u16>,
    scene_fw: Option<u16>,
    scene_jp: Option<u16>,
    snd_bumper: Option<u16>,
    snd_sling: Option<u16>,
    lamp_pb: Option<u16>,
    lamp_od: Option<u16>,
    lamp_combo: Option<u16>,
    lane_lamp: [Option<u16>; 3],
    disp_feed: Option<u16>,
    prof_steel: Option<u16>,
    prof_power: Option<u16>,
    mag: Option<u16>,
    fl_left: Option<u16>,
    fl_right: Option<u16>,
    key_ball: Option<u16>,
    key_state: Option<u16>,
    key_high: Option<u16>,
    key_games: Option<u16>,
    key_fw: Option<u16>,
    key_jp: Option<u16>,
    key_skill: Option<u16>,
    key_pb: Option<u16>,
    key_chain: Option<u16>,
    key_combo: Option<u16>,
    key_od: Option<u16>,
    key_bonus: Option<u16>,
}

impl Ids {
    const NONE: Ids = Ids {
        start: None,
        launch: None,
        action: None,
        trough_1: None,
        trough_2: None,
        b1: None,
        b2: None,
        b3: None,
        sl: None,
        sr: None,
        spinner: None,
        orbit_top: None,
        ramp_entry: None,
        ramp_made: None,
        scoop: None,
        lock_lane: None,
        eddy: None,
        opto: None,
        d1: None,
        d2: None,
        d3: None,
        ro_entry: None,
        shooter_lane: None,
        left_inlane: None,
        right_inlane: None,
        lane_sw: [None; 3],
        coil_b1: None,
        coil_b2: None,
        coil_b3: None,
        coil_sl: None,
        coil_sr: None,
        coil_scoop: None,
        coil_plunger: None,
        coil_drop: None,
        scene_attract: None,
        scene_score: None,
        scene_over: None,
        scene_pb: None,
        scene_chain: None,
        scene_od: None,
        scene_bonus: None,
        scene_high: None,
        scene_skill: None,
        scene_fw: None,
        scene_jp: None,
        snd_bumper: None,
        snd_sling: None,
        lamp_pb: None,
        lamp_od: None,
        lamp_combo: None,
        lane_lamp: [None; 3],
        disp_feed: None,
        prof_steel: None,
        prof_power: None,
        mag: None,
        fl_left: None,
        fl_right: None,
        key_ball: None,
        key_state: None,
        key_high: None,
        key_games: None,
        key_fw: None,
        key_jp: None,
        key_skill: None,
        key_pb: None,
        key_chain: None,
        key_combo: None,
        key_od: None,
        key_bonus: None,
    };

    /// Every persist key of the Lua `key` table (its `pairs` iteration only
    /// reads or fills cache slots, so the order has no observable effect).
    fn keys(&self) -> [Option<u16>; 12] {
        [
            self.key_ball,
            self.key_state,
            self.key_high,
            self.key_games,
            self.key_fw,
            self.key_jp,
            self.key_skill,
            self.key_pb,
            self.key_chain,
            self.key_combo,
            self.key_od,
            self.key_bonus,
        ]
    }
}

/// Every rule-state local of `rules.lua`, plus the id tables and the
/// `written` persist cache.
struct Apex {
    state: i64,
    score: i64,
    ball: i64,
    games: i64,
    high: i64,
    ticks: i64,
    dirty: bool,
    ball_live: bool,
    pb_mode: bool,
    combo: i64,
    combo_steps: i64,
    chain_started: bool,
    feed_tick: i64,
    combo_tick: i64,
    decay_tick: i64,
    ramps_ball: i64,
    od_ramps: i64,
    od_steps: i64,
    od_ready: bool,
    od_armed_tick: i64,
    counters: i64,
    skill_open: bool,
    launch_tick: i64,
    lit_lane: i64,
    inlane_tick: i64,
    d1: bool,
    d2: bool,
    d3: bool,
    opto_tick: i64,
    eddy_tick: i64,
    opto_pending: bool,
    magnet_off_at: i64,
    scoop_at: i64,
    drop_at: i64,
    zipper_off_at: i64,
    end_at: i64,
    over_at: i64,
    attract_at: i64,
    pb_starts: i64,
    chain_starts: i64,
    fw_count: i64,
    jp_count: i64,
    skill_count: i64,
    ids: Ids,
    /// Lua `written`: the last value written per persist key.
    written: [i64; PERSIST_SLOTS],
    /// `save_state` output buffer.
    blob: [u8; 512],
}

saved_fields!(Apex;
    state, score, ball, games, high, ticks, ball_live, pb_mode, combo,
    combo_steps, chain_started, feed_tick, combo_tick, decay_tick,
    ramps_ball, od_ramps, od_steps, od_ready, od_armed_tick, counters,
    skill_open, launch_tick, lit_lane, inlane_tick, d1, d2, d3,
    opto_tick, eddy_tick, opto_pending, magnet_off_at, scoop_at, drop_at,
    zipper_off_at, end_at, over_at, attract_at, pb_starts, chain_starts,
    fw_count, jp_count, skill_count,
);

impl Apex {
    /// The Lua locals' initial values.
    const INITIAL: Apex = Apex {
        state: ATTRACT,
        score: 0,
        ball: 0,
        games: 0,
        high: 0,
        ticks: 0,
        dirty: true,
        ball_live: false,
        pb_mode: false,
        combo: 1,
        combo_steps: 0,
        chain_started: false,
        feed_tick: NEVER,
        combo_tick: NEVER,
        decay_tick: NEVER,
        ramps_ball: 0,
        od_ramps: 0,
        od_steps: 0,
        od_ready: false,
        od_armed_tick: NEVER,
        counters: 0,
        skill_open: false,
        launch_tick: NEVER,
        lit_lane: 1,
        inlane_tick: NEVER,
        d1: false,
        d2: false,
        d3: false,
        opto_tick: NEVER,
        eddy_tick: NEVER,
        opto_pending: false,
        magnet_off_at: -1,
        scoop_at: -1,
        drop_at: -1,
        zipper_off_at: -1,
        end_at: -1,
        over_at: -1,
        attract_at: -1,
        pb_starts: 0,
        chain_starts: 0,
        fw_count: 0,
        jp_count: 0,
        skill_count: 0,
        ids: Ids::NONE,
        written: [0; PERSIST_SLOTS],
        blob: [0; 512],
    };

    /// Lua `bind_ids`.
    fn bind_ids(&mut self) {
        if self.ids.start.is_some() {
            return;
        }
        let s = pin::sw;
        let i = &mut self.ids;
        i.start = s("start");
        i.launch = s("launch");
        i.action = s("action");
        i.trough_1 = s("trough_1");
        i.trough_2 = s("trough_2");
        i.b1 = s("bumper_1_ring");
        i.b2 = s("bumper_2_ring");
        i.b3 = s("bumper_3_ring");
        i.sl = s("sling_left_sw");
        i.sr = s("sling_right_sw");
        i.spinner = s("left_spinner");
        i.orbit_top = s("orbit_top");
        i.ramp_entry = s("ramp_entry");
        i.ramp_made = s("ramp_made");
        i.scoop = s("lock_scoop_sw");
        i.lock_lane = s("lock_lane");
        i.eddy = s("magnet_eddy");
        i.opto = s("magnet_opto");
        i.d1 = s("drop_1_sw");
        i.d2 = s("drop_2_sw");
        i.d3 = s("drop_3_sw");
        i.ro_entry = s("right_orbit_entry");
        i.shooter_lane = s("shooter_lane");
        i.left_inlane = s("left_inlane");
        i.right_inlane = s("right_inlane");
        i.lane_sw = [s("lane_1"), s("lane_2"), s("lane_3")];
        let c = pin::coil;
        i.coil_b1 = c("bumper_1_coil");
        i.coil_b2 = c("bumper_2_coil");
        i.coil_b3 = c("bumper_3_coil");
        i.coil_sl = c("sling_left_coil");
        i.coil_sr = c("sling_right_coil");
        i.coil_scoop = c("lock_scoop_coil");
        i.coil_plunger = c("plunger_coil");
        i.coil_drop = c("drop_reset_coil");
        let sc = |n| pin::id_of(Kind::Scene, n);
        i.scene_attract = sc("scene_attract");
        i.scene_score = sc("scene_score");
        i.scene_over = sc("scene_game_over");
        i.scene_pb = sc("scene_powerball_mania");
        i.scene_chain = sc("scene_combo_chain");
        i.scene_od = sc("scene_overdrive");
        i.scene_bonus = sc("scene_bonus");
        i.scene_high = sc("scene_high_score");
        i.scene_skill = sc("scene_skill_shot");
        i.scene_fw = sc("scene_firewall_save");
        i.scene_jp = sc("scene_apex_jackpot");
        i.snd_bumper = pin::sound("sfx_bumper");
        i.snd_sling = pin::sound("sfx_sling");
        let l = pin::lamp;
        i.lamp_pb = l("powerball_lamp");
        i.lamp_od = l("overdrive_lamp");
        i.lamp_combo = l("combo_lamp");
        i.lane_lamp = [l("lane_1_lamp"), l("lane_2_lamp"), l("lane_3_lamp")];
        i.disp_feed = pin::id_of(Kind::Dispenser, "feed");
        i.prof_steel = pin::id_of(Kind::Profile, "standard_steel");
        i.prof_power = pin::id_of(Kind::Profile, "ceramic_powerball");
        i.mag = pin::id_of(Kind::Magnet, "apex_magnet");
        i.fl_left = pin::id_of(Kind::Flipper, "left_flipper");
        i.fl_right = pin::id_of(Kind::Flipper, "right_flipper");
        let p = pin::persist_key;
        i.key_ball = p("ball");
        i.key_state = p("game_state");
        i.key_high = p("high_score");
        i.key_games = p("games_played");
        i.key_fw = p("firewall_save_count");
        i.key_jp = p("apex_jackpot_count");
        i.key_skill = p("skill_shot_count");
        i.key_pb = p("mode:powerball_mania");
        i.key_chain = p("mode:combo_chain");
        i.key_combo = p("combo_level");
        i.key_od = p("overdrive_ready");
        i.key_bonus = p("last_bonus");
        // One cache slot per key (filled here, so later writes add no entry).
        for k in self.ids.keys().into_iter().flatten() {
            self.written[slot(k)] = NEVER;
        }
    }

    /// Lua `pset`: writes a persist key only when its value changes.
    fn pset(&mut self, k: Option<u16>, v: i64) {
        let k = id(k);
        let s = slot(k);
        if self.written[s] != v {
            self.written[s] = v;
            pin::persist_set(k, v);
        }
    }

    /// Lua `multiplier`.
    fn multiplier(&self) -> i64 {
        let mut m = self.combo;
        if self.pb_mode {
            m *= 2;
        }
        if m > MULT_CAP {
            m = MULT_CAP;
        }
        m
    }

    /// Lua `add`: playfield scoring (multiplied), only while a ball is in play.
    fn add(&mut self, points: i64) {
        if self.state != GAME || !self.ball_live {
            return;
        }
        self.score += points * self.multiplier();
        self.dirty = true;
    }

    /// Lua `award`: fixed awards, not multiplied.
    fn award(&mut self, points: i64) {
        self.score += points;
        self.dirty = true;
    }

    /// Lua `set_lane_lamps`.
    fn set_lane_lamps(&self) {
        for i in 1..=3i64 {
            let lamp = id(self.ids.lane_lamp[(i - 1) as usize]);
            if self.skill_open && i == self.lit_lane {
                pin::set_lamp(lamp, 1.0);
            } else {
                pin::set_lamp(lamp, 0.0);
            }
        }
    }

    /// Lua `arm_overdrive`.
    fn arm_overdrive(&mut self) {
        if self.od_ready {
            return;
        }
        self.od_ready = true;
        self.od_armed_tick = self.ticks;
        self.od_ramps = 0;
        self.od_steps = 0;
        pin::set_lamp(id(self.ids.lamp_od), 1.0);
        self.pset(self.ids.key_od, 1);
        pin::show_scene(id(self.ids.scene_od));
    }

    /// Lua `clear_overdrive`.
    fn clear_overdrive(&mut self) {
        self.od_ready = false;
        pin::set_lamp(id(self.ids.lamp_od), 0.0);
        self.pset(self.ids.key_od, 0);
    }

    /// Lua `start_chain`.
    fn start_chain(&mut self) {
        self.chain_started = true;
        self.chain_starts += 1;
        self.pset(self.ids.key_chain, self.chain_starts);
        pin::show_scene(id(self.ids.scene_chain));
    }

    /// Lua `combo_step`.
    fn combo_step(&mut self) {
        self.combo_steps += 1;
        self.combo_tick = self.ticks;
        self.decay_tick = self.ticks;
        if self.combo < COMBO_MAX {
            self.combo += 1;
            self.pset(self.ids.key_combo, self.combo);
            pin::set_lamp(id(self.ids.lamp_combo), 1.0);
        }
        if self.combo >= 2 && !self.chain_started {
            self.start_chain();
        }
        self.od_steps += 1;
        if self.od_steps >= OD_STEPS {
            self.arm_overdrive();
        }
    }

    /// Lua `shot_hit`.
    fn shot_hit(&mut self) {
        self.counters += 1;
        let mut since = self.feed_tick;
        if self.combo_tick > since {
            since = self.combo_tick;
        }
        if since > NEVER && self.ticks - since <= COMBO_WINDOW {
            self.combo_step();
        }
    }

    /// Lua `start_powerball`.
    fn start_powerball(&mut self) {
        self.pb_mode = true;
        self.pb_starts += 1;
        self.pset(self.ids.key_pb, self.pb_starts);
        pin::set_lamp(id(self.ids.lamp_pb), 1.0);
        pin::show_scene(id(self.ids.scene_pb));
    }

    /// Lua `reset_ball_state`.
    fn reset_ball_state(&mut self) {
        self.pb_mode = false;
        self.combo = 1;
        self.combo_steps = 0;
        self.chain_started = false;
        self.feed_tick = NEVER;
        self.combo_tick = NEVER;
        self.decay_tick = NEVER;
        self.ramps_ball = 0;
        self.od_ramps = 0;
        self.od_steps = 0;
        self.counters = 0;
        self.skill_open = false;
        self.launch_tick = NEVER;
        self.inlane_tick = NEVER;
        self.opto_pending = false;
        self.pset(self.ids.key_combo, 1);
        pin::set_lamp(id(self.ids.lamp_pb), 0.0);
        pin::set_lamp(id(self.ids.lamp_combo), 0.0);
        if self.od_ready {
            self.clear_overdrive();
        }
    }

    /// Lua `serve_ball`.
    fn serve_ball(&mut self) {
        self.reset_ball_state();
        self.lit_lane = (self.ball - 1).rem_euclid(3) + 1;
        self.ball_live = true;
        if self.d1 || self.d2 || self.d3 {
            self.drop_at = self.ticks;
        }
        if self.ball == POWERBALL_BALL {
            pin::spawn_ball(id(self.ids.disp_feed), id(self.ids.prof_power));
        } else {
            pin::spawn_ball(id(self.ids.disp_feed), id(self.ids.prof_steel));
        }
        self.pset(self.ids.key_ball, self.ball);
        self.set_lane_lamps();
    }

    /// Lua `start_game`.
    fn start_game(&mut self) {
        self.state = GAME;
        self.score = 0;
        self.ball = 1;
        self.end_at = -1;
        self.over_at = -1;
        self.attract_at = -1;
        self.dirty = true;
        self.pset(self.ids.key_state, GAME);
        pin::show_scene(id(self.ids.scene_score));
        self.serve_ball();
    }

    /// Lua `drain`.
    fn drain(&mut self) {
        self.ball_live = false;
        self.opto_pending = false;
        let bonus = self.counters * BONUS_UNIT * self.combo;
        self.award(bonus);
        self.pset(self.ids.key_bonus, bonus);
        pin::show_scene(id(self.ids.scene_bonus));
        // Mania and the combo end with the ball.
        self.pb_mode = false;
        self.combo = 1;
        self.pset(self.ids.key_combo, 1);
        pin::set_lamp(id(self.ids.lamp_pb), 0.0);
        pin::set_lamp(id(self.ids.lamp_combo), 0.0);
        if self.od_ready {
            self.clear_overdrive();
        }
        self.skill_open = false;
        self.set_lane_lamps();
        self.end_at = self.ticks + BONUS_TICKS;
    }

    /// Lua `game_over`.
    fn game_over(&mut self) {
        self.state = OVER;
        self.ball = 0;
        self.games += 1;
        self.pset(self.ids.key_games, self.games);
        self.pset(self.ids.key_ball, 0);
        self.pset(self.ids.key_state, OVER);
        if self.score > self.high {
            self.high = self.score;
            self.pset(self.ids.key_high, self.high);
            pin::show_scene(id(self.ids.scene_high));
            self.over_at = self.ticks + HIGH_SCORE_TICKS;
        } else {
            pin::show_scene(id(self.ids.scene_over));
            self.attract_at = self.ticks + GAME_OVER_TICKS;
        }
    }

    /// Lua `next_ball`.
    fn next_ball(&mut self) {
        if self.ball < BALLS_PER_GAME {
            self.ball += 1;
            pin::show_scene(id(self.ids.scene_score));
            self.serve_ball();
        } else {
            self.game_over();
        }
    }

    /// Lua `lane(i)`.
    fn lane(&mut self, i: i64) {
        self.counters += 1;
        if self.skill_open && self.ticks - self.launch_tick <= SKILL_WINDOW {
            self.skill_open = false;
            if i == self.lit_lane {
                self.award(SKILL_VALUE * self.ball);
                self.skill_count += 1;
                self.pset(self.ids.key_skill, self.skill_count);
                pin::show_scene(id(self.ids.scene_skill));
            } else {
                self.award(LANE_VALUE);
            }
            self.set_lane_lamps();
        } else {
            self.add(LANE_VALUE);
        }
    }

    /// Lua `action`.
    fn action(&mut self) {
        if self.state != GAME || !self.ball_live || !self.od_ready {
            return;
        }
        self.clear_overdrive();
        if pin::balls_in_play() == 1 && self.ticks - self.inlane_tick <= INLANE_WINDOW {
            pin::set_zipper(id(self.ids.fl_left), true);
            pin::set_zipper(id(self.ids.fl_right), true);
            self.zipper_off_at = self.ticks + ZIPPER_TICKS;
            self.fw_count += 1;
            self.pset(self.ids.key_fw, self.fw_count);
            pin::show_scene(id(self.ids.scene_fw));
        } else {
            let mut v = JACKPOT_START
                - JACKPOT_STEP * (self.ticks - self.od_armed_tick).div_euclid(JACKPOT_STEP_TICKS);
            if v < JACKPOT_FLOOR {
                v = JACKPOT_FLOOR;
            }
            self.award(v);
            self.jp_count += 1;
            self.pset(self.ids.key_jp, self.jp_count);
            pin::show_scene(id(self.ids.scene_jp));
        }
    }

    /// Lua `bank_check`.
    fn bank_check(&mut self) {
        if self.d1 && self.d2 && self.d3 {
            self.add(P_BANK);
            self.drop_at = self.ticks + DROP_RESET_DELAY;
        }
    }
}

impl Rules for Apex {
    fn on_init(&mut self) {
        self.bind_ids();
        let ids = self.ids;
        self.games = pin::persist_get(id(ids.key_games));
        self.high = pin::persist_get(id(ids.key_high));
        self.fw_count = pin::persist_get(id(ids.key_fw));
        self.jp_count = pin::persist_get(id(ids.key_jp));
        self.skill_count = pin::persist_get(id(ids.key_skill));
        self.pb_starts = pin::persist_get(id(ids.key_pb));
        self.chain_starts = pin::persist_get(id(ids.key_chain));
        for k in ids.keys().into_iter().flatten() {
            self.written[slot(k)] = pin::persist_get(k);
        }
        self.pset(ids.key_ball, 0);
        self.pset(ids.key_state, ATTRACT);
        self.pset(ids.key_combo, 1);
        self.pset(ids.key_od, 0);
        pin::show_scene(id(ids.scene_attract));
    }

    fn on_switch_edge(&mut self, sw: u16, closed: bool) {
        self.bind_ids();
        let s = self.ids;
        let is = |x: Option<u16>| x == Some(sw);
        // Drop-target state follows both edges (a target is down while closed).
        if is(s.d1) {
            self.d1 = closed;
        } else if is(s.d2) {
            self.d2 = closed;
        } else if is(s.d3) {
            self.d3 = closed;
        }
        if !closed {
            return;
        }
        // Mechanisms work in every state (an injected ball is never left held).
        if is(s.b1) {
            pin::pulse_coil(id(s.coil_b1), 20);
            pin::play_sound(id(s.snd_bumper), 1.0);
            self.add(P_BUMPER);
        } else if is(s.b2) {
            pin::pulse_coil(id(s.coil_b2), 20);
            pin::play_sound(id(s.snd_bumper), 1.0);
            self.add(P_BUMPER);
        } else if is(s.b3) {
            pin::pulse_coil(id(s.coil_b3), 20);
            pin::play_sound(id(s.snd_bumper), 1.0);
            self.add(P_BUMPER);
        } else if is(s.sl) {
            pin::pulse_coil(id(s.coil_sl), 25);
            pin::play_sound(id(s.snd_sling), 1.0);
            self.add(P_SLING);
        } else if is(s.sr) {
            pin::pulse_coil(id(s.coil_sr), 25);
            pin::play_sound(id(s.snd_sling), 1.0);
            self.add(P_SLING);
        } else if is(s.scoop) {
            self.scoop_at = self.ticks + SCOOP_HOLD;
            self.add(P_SCOOP);
            if self.state == GAME && self.ball_live {
                self.shot_hit();
            }
        } else if is(s.launch) {
            pin::pulse_coil(id(s.coil_plunger), 300);
        } else if is(s.start) {
            if self.state != GAME {
                self.start_game();
            }
        } else if is(s.eddy) {
            self.eddy_tick = self.ticks;
            pin::set_magnet(id(s.mag), 1.0);
            self.magnet_off_at = self.ticks + MAGNET_PULSE;
        } else if is(s.opto) {
            self.opto_tick = self.ticks;
            self.opto_pending = true;
            self.add(P_MAGNET);
        }
        if self.state != GAME || !self.ball_live {
            return;
        }
        if is(s.trough_1) || is(s.trough_2) {
            self.drain();
        } else if is(s.action) {
            self.action();
        } else if is(s.spinner) {
            self.feed_tick = self.ticks;
            self.add(P_SPINNER);
        } else if is(s.orbit_top) {
            self.add(P_ORBIT_TOP);
            self.shot_hit();
        } else if is(s.ro_entry) {
            self.add(P_RO_ENTRY);
            self.shot_hit();
        } else if is(s.ramp_entry) {
            self.add(P_RAMP_ENTRY);
        } else if is(s.ramp_made) {
            self.add(P_RAMP_MADE);
            self.ramps_ball += 1;
            self.od_ramps += 1;
            self.shot_hit();
            if self.od_ramps >= OD_RAMPS {
                self.arm_overdrive();
            }
        } else if is(s.d1) || is(s.d2) || is(s.d3) {
            self.add(P_DROP);
            self.shot_hit();
            self.bank_check();
        } else if is(s.lock_lane) {
            self.add(P_LOCK_LANE);
        } else if is(s.lane_sw[0]) {
            self.lane(1);
        } else if is(s.lane_sw[1]) {
            self.lane(2);
        } else if is(s.lane_sw[2]) {
            self.lane(3);
        } else if is(s.shooter_lane) {
            // A plunge: the ball runs up the shooter lane. Opens the skill shot.
            self.skill_open = true;
            self.launch_tick = self.ticks;
            self.set_lane_lamps();
        } else if is(s.left_inlane) || is(s.right_inlane) {
            self.inlane_tick = self.ticks;
            self.add(P_INLANE);
        }
    }

    fn on_tick(&mut self) {
        self.bind_ids();
        let s = self.ids;
        self.ticks += 1;
        if self.opto_pending && self.ticks - self.opto_tick > DETECT_WINDOW {
            self.opto_pending = false;
            // No eddy edge around the opto edge: a non-conductive ball.
            if self.state == GAME
                && self.ball_live
                && !self.pb_mode
                && self.eddy_tick < self.opto_tick - DETECT_WINDOW
            {
                self.start_powerball();
            }
        }
        if self.magnet_off_at >= 0 && self.ticks >= self.magnet_off_at {
            self.magnet_off_at = -1;
            pin::set_magnet(id(s.mag), 0.0);
        }
        if self.scoop_at >= 0 && self.ticks >= self.scoop_at {
            self.scoop_at = -1;
            pin::pulse_coil(id(s.coil_scoop), 20);
        }
        if self.drop_at >= 0 && self.ticks >= self.drop_at {
            self.drop_at = -1;
            pin::pulse_coil(id(s.coil_drop), 150);
        }
        if self.zipper_off_at >= 0 && self.ticks >= self.zipper_off_at {
            self.zipper_off_at = -1;
            pin::set_zipper(id(s.fl_left), false);
            pin::set_zipper(id(s.fl_right), false);
        }
        if self.combo > 1 && self.ticks - self.decay_tick >= COMBO_DECAY {
            self.combo -= 1;
            self.decay_tick = self.ticks;
            self.pset(s.key_combo, self.combo);
            if self.combo == 1 {
                pin::set_lamp(id(s.lamp_combo), 0.0);
            }
        }
        if self.skill_open && self.ticks - self.launch_tick > SKILL_WINDOW {
            self.skill_open = false;
            self.set_lane_lamps();
        }
        if self.end_at >= 0 && self.ticks >= self.end_at {
            self.end_at = -1;
            if self.state == GAME {
                self.next_ball();
            }
        }
        if self.over_at >= 0 && self.ticks >= self.over_at {
            self.over_at = -1;
            if self.state == OVER {
                pin::show_scene(id(s.scene_over));
                self.attract_at = self.ticks + GAME_OVER_TICKS;
            }
        }
        if self.attract_at >= 0 && self.ticks >= self.attract_at {
            self.attract_at = -1;
            if self.state == OVER {
                self.state = ATTRACT;
                self.pset(s.key_state, ATTRACT);
                pin::show_scene(id(s.scene_attract));
            }
        }
        if self.dirty {
            pin::set_score(1, self.score);
            self.dirty = false;
        }
    }

    /// Lua `on_save_state`: every rule-state local.
    fn save_state(&mut self) -> &[u8] {
        let mut buf = [0u8; 512];
        let n = self.encode_state(&mut buf);
        self.blob = buf;
        &self.blob[..n]
    }

    /// Lua `on_restore_state`: binds the ids, takes every local from the
    /// blob (a blob of another length is ignored) and redraws the score.
    fn restore_state(&mut self, blob: &[u8]) {
        self.bind_ids();
        if !self.decode_state(blob) {
            return;
        }
        self.dirty = true;
    }
}

pin_guest_sdk::export_rules!(Apex = Apex::INITIAL);

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_blob_round_trips_every_saved_local() {
        let mut a = Apex::INITIAL;
        a.score = 123_456;
        a.ball = 2;
        a.pb_mode = true;
        a.combo = 4;
        a.d3 = true;
        a.skill_count = -7;
        a.opto_pending = true;
        let mut buf = [0u8; 512];
        let n = a.encode_state(&mut buf);
        assert_eq!(n, Apex::SAVED_BYTES);
        assert_eq!(n, 42 * 8);
        let mut b = Apex::INITIAL;
        assert!(b.decode_state(&buf[..n]));
        let mut again = [0u8; 512];
        assert_eq!(b.encode_state(&mut again), n);
        assert_eq!(buf[..n], again[..n]);
        assert_eq!(
            (b.score, b.ball, b.combo, b.skill_count),
            (123_456, 2, 4, -7)
        );
        assert!(b.pb_mode && b.d3 && b.opto_pending && !b.d1);
    }

    #[test]
    fn a_blob_of_the_wrong_length_changes_nothing() {
        let mut b = Apex::INITIAL;
        assert!(!b.decode_state(&[1u8; 8]));
        assert!(!b.decode_state(&[]));
        assert_eq!(b.score, 0);
        assert_eq!(b.combo, 1);
    }

    #[test]
    fn multiplier_doubles_in_mania_and_caps_at_ten() {
        let mut a = Apex::INITIAL;
        a.combo = 3;
        assert_eq!(a.multiplier(), 3);
        a.pb_mode = true;
        assert_eq!(a.multiplier(), 6);
        a.combo = 5;
        assert_eq!(a.multiplier(), MULT_CAP);
    }

    #[test]
    fn missing_ids_fault() {
        assert!(std::panic::catch_unwind(|| id(None)).is_err());
        assert_eq!(id(Some(7)), 7);
        assert!(std::panic::catch_unwind(|| slot(64)).is_err());
        assert_eq!(slot(63), 63);
    }
}
