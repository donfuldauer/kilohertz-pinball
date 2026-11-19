//! Feature Lab rules (Subphase 6.4) ported one-to-one from `../rules.lua`
//! to Rust on `pin_guest_sdk` (Subphase 6.3), built to `../rules.wasm` by
//! `rebuild.sh`.
//!
//! Keep the two in lockstep: every host call, its order and its arguments
//! match the Lua rules (including the single `pinball.random` draw in
//! `on_init`), so a recorded replay produces the same per-tick command queue
//! under either backend (`crates/pin_app/tests/wasm_ports_*`). The game
//! rules are documented in `../rules.lua` and `../README.md`.
//!
//! Lua idioms and their Rust form:
//! * an id looked up by name is an `Option<u16>` (`nil` is `None`); a
//!   comparison `id == sw.x` with a missing name is false, and passing a
//!   missing id to a `pinball` function faults the rules, as in Lua;
//! * `bind_ids` runs lazily at the top of every callback until the `start`
//!   switch is bound, exactly as the Lua guard `sw.start ~= nil`;
//! * the Lua per-mode arrays (index 1..5) are Rust arrays (index 0..4);
//!   functions that take a mode keep the Lua 1-based `i`;
//! * `%` is Lua's floor modulo (`rem_euclid`).

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
const MODES: i64 = 5;
const MODE_START_POINTS: i64 = 2000;
const VUK_HOLD: i64 = 400;
const GAME_OVER_TICKS: i64 = 4000;
const XL_MODE_TICKS: i64 = 20000;
const GLO_MODE_TICKS: i64 = 30000;
const GLO_LAUNCH_DELAY: i64 = 1500;
const FLIP_SHOT_TICKS: i64 = 1000;
const FLIP_SHOT_POINTS: i64 = 5000;
const ORBIT_TICKS: i64 = 3000;
const ORBIT_POINTS: i64 = 5000;
const CMP_IDLE: i64 = 0;
const CMP_WAIT: i64 = 1;
const CMP_HOLD: i64 = 2;
const CMP_RELEASE: i64 = 3;
const CMP_GAP: i64 = 4;
const CMP_HOLD_TICKS: i64 = 1000;
const CMP_TIMEOUT: i64 = 3000;
const CMP_GAP_TICKS: i64 = 1500;
/// "No such event yet": far enough in the past for every window.
const NEVER: i64 = -1000000;
/// Number of modes as an array length.
const N: usize = MODES as usize;

/// Faults the rules (a trap in the guest), as a Lua error does.
#[cold]
fn fault() -> ! {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::unreachable()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        panic!("feature_lab rules: a nil id was passed to pinball")
    }
}

/// The id a `pinball` function receives: a missing name (`nil`) faults.
fn id(x: Option<u16>) -> u16 {
    match x {
        Some(v) => v,
        None => fault(),
    }
}

/// Array index of the Lua 1-based mode index `i`.
fn ix(i: i64) -> usize {
    (i - 1) as usize
}

/// The id tables of `bind_ids`.
#[derive(Clone, Copy)]
struct Ids {
    start: Option<u16>,
    launch: Option<u16>,
    action: Option<u16>,
    trough_1: Option<u16>,
    trough_2: Option<u16>,
    xl_trough: Option<u16>,
    b1: Option<u16>,
    b2: Option<u16>,
    b3: Option<u16>,
    sl: Option<u16>,
    orbit_entry: Option<u16>,
    orbit_top: Option<u16>,
    vuk_lane: Option<u16>,
    bb_vuk: Option<u16>,
    magnet: Option<u16>,
    xl_lane: Option<u16>,
    bb_eos: Option<u16>,
    bb_drain: Option<u16>,
    plunger_tip: Option<u16>,
    ll_eos: Option<u16>,
    lr_eos: Option<u16>,
    shot_a: [Option<u16>; N],
    shot_b: [Option<u16>; N],
    shot_win: [i64; N],
    coil_b1: Option<u16>,
    coil_b2: Option<u16>,
    coil_b3: Option<u16>,
    coil_sl: Option<u16>,
    coil_vuk: Option<u16>,
    coil_plunger: Option<u16>,
    coil_gate: Option<u16>,
    scene_attract: Option<u16>,
    scene_score: Option<u16>,
    scene_over: Option<u16>,
    mode_scene: [Option<u16>; N],
    snd_bumper: Option<u16>,
    snd_sling: Option<u16>,
    disp_feed: Option<u16>,
    disp_magnet: Option<u16>,
    disp_xl: Option<u16>,
    disp_glo: Option<u16>,
    prof_steel: Option<u16>,
    prof_low_rem: Option<u16>,
    prof_xl: Option<u16>,
    prof_glo: Option<u16>,
    mag: Option<u16>,
    lamp_mode: Option<u16>,
    k_games: Option<u16>,
    k_high: Option<u16>,
    k_ball: Option<u16>,
    k_state: Option<u16>,
    k_sel: Option<u16>,
    k_cmp_std: Option<u16>,
    k_cmp_low: Option<u16>,
    mode_key: [Option<u16>; N],
}

impl Ids {
    const NONE: Ids = Ids {
        start: None,
        launch: None,
        action: None,
        trough_1: None,
        trough_2: None,
        xl_trough: None,
        b1: None,
        b2: None,
        b3: None,
        sl: None,
        orbit_entry: None,
        orbit_top: None,
        vuk_lane: None,
        bb_vuk: None,
        magnet: None,
        xl_lane: None,
        bb_eos: None,
        bb_drain: None,
        plunger_tip: None,
        ll_eos: None,
        lr_eos: None,
        shot_a: [None; N],
        shot_b: [None; N],
        shot_win: [0; N],
        coil_b1: None,
        coil_b2: None,
        coil_b3: None,
        coil_sl: None,
        coil_vuk: None,
        coil_plunger: None,
        coil_gate: None,
        scene_attract: None,
        scene_score: None,
        scene_over: None,
        mode_scene: [None; N],
        snd_bumper: None,
        snd_sling: None,
        disp_feed: None,
        disp_magnet: None,
        disp_xl: None,
        disp_glo: None,
        prof_steel: None,
        prof_low_rem: None,
        prof_xl: None,
        prof_glo: None,
        mag: None,
        lamp_mode: None,
        k_games: None,
        k_high: None,
        k_ball: None,
        k_state: None,
        k_sel: None,
        k_cmp_std: None,
        k_cmp_low: None,
        mode_key: [None; N],
    };
}

/// Every rule-state local of `rules.lua`, plus the id tables.
struct FeatureLab {
    state: i64,
    score: i64,
    ball: i64,
    games: i64,
    high: i64,
    ticks: i64,
    dirty: bool,
    mode_sel: i64,
    last_spawn: i64,
    vuk_at: i64,
    attract_at: i64,
    last_flip: i64,
    last_entry: i64,
    started: [bool; N],
    starts: [i64; N],
    shot_next: [i64; N],
    shot_start: [i64; N],
    banana_on: bool,
    backbox_on: bool,
    xl_on: bool,
    glo_on: bool,
    xl_end: i64,
    glo_end: i64,
    glo_launch: i64,
    cmp_phase: i64,
    cmp_which: i64,
    cmp_deadline: i64,
    cmp_hold_until: i64,
    cmp_off: i64,
    ids: Ids,
    /// `save_state` output buffer.
    blob: [u8; 512],
}

saved_fields!(FeatureLab;
    state, score, ball, games, high, ticks, mode_sel, last_spawn, vuk_at,
    attract_at, last_flip, last_entry, started, starts, shot_next,
    shot_start, banana_on, backbox_on, xl_on, glo_on, xl_end, glo_end,
    glo_launch, cmp_phase, cmp_which, cmp_deadline, cmp_hold_until, cmp_off,
);

impl FeatureLab {
    /// The Lua locals' initial values.
    const INITIAL: FeatureLab = FeatureLab {
        state: ATTRACT,
        score: 0,
        ball: 0,
        games: 0,
        high: 0,
        ticks: 0,
        dirty: true,
        mode_sel: 0,
        last_spawn: NEVER,
        vuk_at: -1,
        attract_at: -1,
        last_flip: NEVER,
        last_entry: NEVER,
        started: [false; N],
        starts: [0; N],
        shot_next: [0; N],
        shot_start: [0; N],
        banana_on: false,
        backbox_on: false,
        xl_on: false,
        glo_on: false,
        xl_end: -1,
        glo_end: -1,
        glo_launch: -1,
        cmp_phase: CMP_IDLE,
        cmp_which: 0,
        cmp_deadline: -1,
        cmp_hold_until: -1,
        cmp_off: -1,
        ids: Ids::NONE,
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
        i.xl_trough = s("xl_trough");
        i.b1 = s("bumper_1_ring");
        i.b2 = s("bumper_2_ring");
        i.b3 = s("bumper_3_ring");
        i.sl = s("sling_left_sw");
        i.orbit_entry = s("orbit_entry");
        i.orbit_top = s("orbit_top");
        i.vuk_lane = s("vuk_lane");
        i.bb_vuk = s("bb_vuk_sw");
        i.magnet = s("magnet_opto");
        i.xl_lane = s("xl_lane");
        i.bb_eos = s("bb_flipper_eos");
        i.bb_drain = s("bb_drain");
        i.plunger_tip = s("plunger_tip");
        i.ll_eos = s("lower_left_eos");
        i.lr_eos = s("lower_right_eos");
        // The qualifying shots, in selector order (table.toml [[shots]]).
        i.shot_a = [
            s("vuk_lane"),
            s("bb_vuk_sw"),
            s("bumper_3_ring"),
            s("magnet_opto"),
            s("bumper_2_ring"),
        ];
        i.shot_b = [
            s("bb_vuk_sw"),
            s("bb_drain"),
            s("bumper_2_ring"),
            s("vuk_lane"),
            s("bumper_3_ring"),
        ];
        i.shot_win = [3000, 10000, 5000, 2000, 5000];
        let c = pin::coil;
        i.coil_b1 = c("bumper_1_coil");
        i.coil_b2 = c("bumper_2_coil");
        i.coil_b3 = c("bumper_3_coil");
        i.coil_sl = c("sling_left_coil");
        i.coil_vuk = c("bb_vuk_coil");
        i.coil_plunger = c("plunger_coil");
        i.coil_gate = c("lane_gate_coil");
        let sc = |n| pin::id_of(Kind::Scene, n);
        i.scene_attract = sc("scene_attract");
        i.scene_score = sc("scene_score");
        i.scene_over = sc("scene_game_over");
        i.mode_scene = [
            sc("scene_banana_shots"),
            sc("scene_backbox_play"),
            sc("scene_xl_ball"),
            sc("scene_magnet_compare"),
            sc("scene_glo_ball"),
        ];
        i.snd_bumper = pin::sound("sfx_bumper");
        i.snd_sling = pin::sound("sfx_sling");
        let d = |n| pin::id_of(Kind::Dispenser, n);
        i.disp_feed = d("feed");
        i.disp_magnet = d("magnet_feed");
        i.disp_xl = d("xl_feed");
        i.disp_glo = d("glo_feed");
        let pr = |n| pin::id_of(Kind::Profile, n);
        i.prof_steel = pr("standard_steel");
        i.prof_low_rem = pr("low_rem_steel");
        i.prof_xl = pr("jumbo_xl_resin");
        i.prof_glo = pr("uv_globall");
        i.mag = pin::id_of(Kind::Magnet, "hold_magnet");
        i.lamp_mode = pin::lamp("mode_lamp");
        let p = pin::persist_key;
        i.k_games = p("games_played");
        i.k_high = p("high_score");
        i.k_ball = p("ball");
        i.k_state = p("game_state");
        i.k_sel = p("mode_sel");
        i.k_cmp_std = p("cmp_release_ms_standard");
        i.k_cmp_low = p("cmp_release_ms_low_rem");
        i.mode_key = [
            p("mode:banana_shots"),
            p("mode:backbox_play"),
            p("mode:xl_ball"),
            p("mode:magnet_compare"),
            p("mode:glo_ball"),
        ];
    }

    /// Lua `add`.
    fn add(&mut self, points: i64) {
        self.score += points;
        self.dirty = true;
    }

    /// Lua `spawn`.
    fn spawn(&mut self, dispenser: Option<u16>, profile: Option<u16>) {
        pin::spawn_ball(id(dispenser), id(profile));
        self.last_spawn = self.ticks;
    }

    /// Lua `set_sel`.
    fn set_sel(&mut self, v: i64) {
        self.mode_sel = v.rem_euclid(MODES);
        pin::persist_set(id(self.ids.k_sel), self.mode_sel);
    }

    /// Lua `end_modes`.
    fn end_modes(&mut self) {
        self.banana_on = false;
        self.backbox_on = false;
        self.xl_on = false;
        self.glo_on = false;
        self.last_flip = NEVER;
        self.last_entry = NEVER;
        self.xl_end = -1;
        self.glo_end = -1;
        self.glo_launch = -1;
        if self.cmp_phase != CMP_IDLE {
            self.cmp_phase = CMP_IDLE;
            pin::set_magnet(id(self.ids.mag), 0.0);
        }
        for i in 0..N {
            self.started[i] = false;
            self.shot_next[i] = 0;
        }
        pin::set_lamp(id(self.ids.lamp_mode), 0.0);
    }

    /// Lua `serve_ball`.
    fn serve_ball(&mut self) {
        self.set_sel(self.mode_sel + 1);
        pin::persist_set(id(self.ids.k_ball), self.ball);
        self.spawn(self.ids.disp_feed, self.ids.prof_steel);
    }

    /// Lua `start_game`.
    fn start_game(&mut self) {
        self.state = GAME;
        self.score = 0;
        self.ball = 1;
        self.attract_at = -1;
        self.dirty = true;
        self.end_modes();
        pin::persist_set(id(self.ids.k_state), GAME);
        pin::show_scene(id(self.ids.scene_score));
        self.serve_ball();
    }

    /// Lua `end_ball`.
    fn end_ball(&mut self) {
        self.end_modes();
        if self.ball < BALLS_PER_GAME {
            self.ball += 1;
            self.serve_ball();
            return;
        }
        self.state = OVER;
        self.ball = 0;
        self.games += 1;
        if self.score > self.high {
            self.high = self.score;
        }
        let i = self.ids;
        pin::persist_set(id(i.k_games), self.games);
        pin::persist_set(id(i.k_high), self.high);
        pin::persist_set(id(i.k_ball), 0);
        pin::persist_set(id(i.k_state), OVER);
        pin::show_scene(id(i.scene_over));
        self.attract_at = self.ticks + GAME_OVER_TICKS;
    }

    /// Lua `cmp_serve`.
    fn cmp_serve(&mut self) {
        pin::set_magnet(id(self.ids.mag), 1.0);
        if self.cmp_which == 0 {
            self.spawn(self.ids.disp_magnet, self.ids.prof_steel);
        } else {
            self.spawn(self.ids.disp_magnet, self.ids.prof_low_rem);
        }
        self.cmp_phase = CMP_WAIT;
        self.cmp_deadline = self.ticks + CMP_TIMEOUT;
    }

    /// Lua `cmp_next`.
    fn cmp_next(&mut self) {
        pin::set_magnet(id(self.ids.mag), 0.0);
        if self.cmp_which == 0 {
            self.cmp_which = 1;
            self.cmp_phase = CMP_GAP;
            self.cmp_deadline = self.ticks + CMP_GAP_TICKS;
        } else {
            self.cmp_phase = CMP_IDLE;
        }
    }

    /// Lua `start_mode(i)` (1-based `i`).
    fn start_mode(&mut self, i: i64) {
        let k = ix(i);
        self.started[k] = true;
        self.starts[k] += 1;
        pin::persist_set(id(self.ids.mode_key[k]), self.starts[k]);
        pin::show_scene(id(self.ids.mode_scene[k]));
        pin::set_lamp(id(self.ids.lamp_mode), 1.0);
        self.add(MODE_START_POINTS);
        if i == 1 {
            self.banana_on = true;
        } else if i == 2 {
            self.backbox_on = true;
        } else if i == 3 {
            self.xl_on = true;
            self.xl_end = self.ticks + XL_MODE_TICKS;
            self.spawn(self.ids.disp_xl, self.ids.prof_xl);
        } else if i == 4 {
            if self.cmp_phase == CMP_IDLE {
                self.cmp_which = 0;
                self.cmp_serve();
            }
        } else {
            self.glo_on = true;
            self.glo_end = self.ticks + GLO_MODE_TICKS;
            self.glo_launch = self.ticks + GLO_LAUNCH_DELAY;
            self.spawn(self.ids.disp_glo, self.ids.prof_glo);
        }
    }

    /// Lua `shot_made(i)` (1-based `i`).
    fn shot_made(&mut self, i: i64) {
        if i == 1 && self.banana_on {
            self.add(25000);
        }
        if self.state == GAME && self.mode_sel == i - 1 && !self.started[ix(i)] {
            self.start_mode(i);
        }
    }

    /// Lua `shot_edge(id)`.
    fn shot_edge(&mut self, sw: u16) {
        for i in 1..=MODES {
            let k = ix(i);
            if self.shot_next[k] > 0 && self.ticks - self.shot_start[k] > self.ids.shot_win[k] {
                self.shot_next[k] = 0;
            }
            if self.shot_next[k] == 1 && self.ids.shot_b[k] == Some(sw) {
                self.shot_next[k] = 0;
                self.shot_made(i);
            } else if self.ids.shot_a[k] == Some(sw) {
                self.shot_next[k] = 1;
                self.shot_start[k] = self.ticks;
            }
        }
    }
}

impl Rules for FeatureLab {
    fn on_init(&mut self) {
        self.bind_ids();
        let i = self.ids;
        self.ticks = pin::tick();
        self.games = pin::persist_get(id(i.k_games));
        self.high = pin::persist_get(id(i.k_high));
        self.mode_sel = pin::random(0, MODES - 1);
        for k in 0..N {
            self.starts[k] = pin::persist_get(id(i.mode_key[k]));
        }
        pin::persist_set(id(i.k_ball), 0);
        pin::persist_set(id(i.k_state), ATTRACT);
        pin::persist_set(id(i.k_sel), self.mode_sel);
        pin::show_scene(id(i.scene_attract));
    }

    fn on_switch_edge(&mut self, sw: u16, closed: bool) {
        self.bind_ids();
        let s = self.ids;
        let is = |x: Option<u16>| x == Some(sw);
        self.ticks = pin::tick();
        if !closed {
            if is(s.ll_eos) || is(s.lr_eos) {
                self.last_flip = self.ticks;
            }
            if is(s.magnet) && self.cmp_phase == CMP_RELEASE {
                let ms = self.ticks - self.cmp_off;
                if self.cmp_which == 0 {
                    pin::persist_set(id(s.k_cmp_std), ms);
                } else {
                    pin::persist_set(id(s.k_cmp_low), ms);
                }
                if self.state == GAME {
                    self.add(10000);
                }
                self.cmp_next();
            }
            return;
        }
        if is(s.start) {
            if self.state != GAME {
                self.start_game();
            }
            return;
        }
        // The backbox VUK ejects whatever it holds, in every state.
        if is(s.bb_vuk) {
            self.vuk_at = self.ticks + VUK_HOLD;
        }
        if is(s.magnet) && self.cmp_phase == CMP_WAIT {
            self.cmp_phase = CMP_HOLD;
            self.cmp_hold_until = self.ticks + CMP_HOLD_TICKS;
        }
        if is(s.xl_trough) {
            self.xl_on = false;
            self.xl_end = -1;
        }
        if self.state != GAME {
            return;
        }
        if is(s.action) {
            self.set_sel(self.mode_sel + 1);
            return;
        }
        self.shot_edge(sw);
        if (is(s.orbit_entry) || is(s.vuk_lane) || is(s.magnet) || is(s.b1) || is(s.b2) || is(s.b3))
            && self.ticks - self.last_flip <= FLIP_SHOT_TICKS
        {
            // One bonus per flip.
            self.last_flip = NEVER;
            self.add(FLIP_SHOT_POINTS);
        }
        if is(s.launch) {
            pin::pulse_coil(id(s.coil_gate), 1500);
            pin::pulse_coil(id(s.coil_plunger), 300);
        } else if is(s.b1) || is(s.b2) || is(s.b3) {
            if is(s.b1) {
                pin::pulse_coil(id(s.coil_b1), 20);
            } else if is(s.b2) {
                pin::pulse_coil(id(s.coil_b2), 20);
            } else {
                pin::pulse_coil(id(s.coil_b3), 20);
            }
            pin::play_sound(id(s.snd_bumper), 1.0);
            if self.glo_on {
                self.add(20);
            } else {
                self.add(10);
            }
        } else if is(s.sl) {
            pin::pulse_coil(id(s.coil_sl), 25);
            pin::play_sound(id(s.snd_sling), 1.0);
            self.add(10);
        } else if is(s.orbit_entry) {
            self.last_entry = self.ticks;
            self.add(100);
        } else if is(s.orbit_top) {
            if self.ticks - self.last_entry <= ORBIT_TICKS {
                self.add(ORBIT_POINTS);
            } else {
                self.add(500);
            }
        } else if is(s.vuk_lane) {
            self.add(100);
        } else if is(s.bb_vuk) {
            self.add(500);
        } else if is(s.bb_eos) {
            if self.backbox_on {
                self.add(1000);
            }
        } else if is(s.bb_drain) {
            if self.backbox_on {
                self.add(5000);
            } else {
                self.add(500);
            }
        } else if is(s.magnet) {
            self.add(100);
        } else if is(s.xl_lane) {
            self.add(500);
        }
    }

    fn on_tick(&mut self) {
        self.bind_ids();
        let s = self.ids;
        self.ticks = pin::tick();
        if self.vuk_at >= 0 && self.ticks >= self.vuk_at {
            self.vuk_at = -1;
            pin::pulse_coil(id(s.coil_vuk), 20);
        }
        if self.cmp_phase == CMP_WAIT && self.ticks > self.cmp_deadline {
            self.cmp_next();
        } else if self.cmp_phase == CMP_HOLD && self.ticks >= self.cmp_hold_until {
            pin::set_magnet(id(s.mag), 0.0);
            self.cmp_off = self.ticks;
            self.cmp_phase = CMP_RELEASE;
            self.cmp_deadline = self.ticks + CMP_TIMEOUT;
        } else if self.cmp_phase == CMP_RELEASE && self.ticks > self.cmp_deadline {
            self.cmp_next();
        } else if self.cmp_phase == CMP_GAP && self.ticks >= self.cmp_deadline {
            self.cmp_serve();
        }
        if self.xl_end >= 0 && self.ticks >= self.xl_end {
            self.xl_on = false;
            self.xl_end = -1;
        }
        if self.glo_launch >= 0 && self.ticks >= self.glo_launch {
            self.glo_launch = -1;
            pin::pulse_coil(id(s.coil_gate), 1500);
            pin::pulse_coil(id(s.coil_plunger), 300);
        }
        if self.glo_end >= 0 && self.ticks >= self.glo_end {
            self.glo_on = false;
            self.glo_end = -1;
        }
        // A ball ends when no ball is left in play; a ball served this or
        // the last tick is not counted yet.
        if self.state == GAME && pin::balls_in_play() == 0 && self.ticks - self.last_spawn > 1 {
            self.end_ball();
        }
        if self.attract_at >= 0 && self.ticks >= self.attract_at {
            self.attract_at = -1;
            if self.state == OVER {
                self.state = ATTRACT;
                pin::persist_set(id(s.k_state), ATTRACT);
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

pin_guest_sdk::export_rules!(FeatureLab = FeatureLab::INITIAL);

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
        let mut a = FeatureLab::INITIAL;
        a.score = 42_000;
        a.mode_sel = 3;
        a.started = [false, true, false, false, true];
        a.starts = [1, 2, 3, 4, 5];
        a.shot_start = [-1, -2, -3, -4, -5];
        a.glo_on = true;
        a.cmp_phase = CMP_GAP;
        let mut buf = [0u8; 512];
        let n = a.encode_state(&mut buf);
        assert_eq!(n, FeatureLab::SAVED_BYTES);
        // 24 scalars plus four arrays of five (the Lua s1..s5, n1..n5,
        // q1..q5, t1..t5).
        assert_eq!(n, (24 + 4 * 5) * 8);
        let mut b = FeatureLab::INITIAL;
        assert!(b.decode_state(&buf[..n]));
        assert_eq!(b.started, a.started);
        assert_eq!(b.starts, a.starts);
        assert_eq!(b.shot_start, a.shot_start);
        assert_eq!((b.score, b.mode_sel, b.cmp_phase), (42_000, 3, CMP_GAP));
        assert!(b.glo_on && !b.xl_on);
    }

    #[test]
    fn a_blob_of_the_wrong_length_changes_nothing() {
        let mut b = FeatureLab::INITIAL;
        assert!(!b.decode_state(&[9u8; 24]));
        assert_eq!(b.last_spawn, NEVER);
    }

    #[test]
    fn mode_indices_are_lua_one_based() {
        assert_eq!(ix(1), 0);
        assert_eq!(ix(5), 4);
        assert_eq!((-1i64).rem_euclid(MODES), 4, "Lua floor modulo");
        assert!(std::panic::catch_unwind(|| id(None)).is_err());
    }
}
