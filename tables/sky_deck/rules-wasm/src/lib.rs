//! Sky Deck rules (Subphase 6.2, variant B) ported one-to-one from
//! `../rules.lua` to Rust on `pin_guest_sdk` (Subphase 6.3), built to
//! `../rules.wasm` by `rebuild.sh`.
//!
//! Keep the two in lockstep: every host call, its order and its arguments
//! match the Lua rules, so a recorded replay produces the same per-tick
//! command queue under either backend (`crates/pin_app/tests/wasm_ports_*`).
//! The game rules are documented in `../rules.lua` and `../README.md`.
//!
//! Lua idioms and their Rust form:
//! * an id looked up by name is an `Option<u16>` (`nil` is `None`); a
//!   comparison `id == sw.x` with a missing name is false, and passing a
//!   missing id to a `pinball` function faults the rules, as in Lua;
//! * `bind_ids` runs lazily at the top of every callback until the `start`
//!   switch is bound, exactly as the Lua guard `sw.start ~= nil`;
//! * the UV tint components are the Lua (f64) literals converted to `f32`
//!   as the host converts a Lua number (`n as f32`).

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
const VUK_HOLD: i64 = 4000;
const VUK_QUICK: i64 = 500;
const DROP_RESET_DELAY: i64 = 1500;
const GAME_OVER_TICKS: i64 = 4000;
const BRIDGE_TICKS: i64 = 8000;
const GLO_DELAY: i64 = 300;
const RAMP_DEBOUNCE: i64 = 1500;
const LOOP_WINDOW: i64 = 3000;
const CHECK_EVERY: i64 = 256;
// UV GI tint (r, g, b, intensity): Lua number literals, converted like the
// host converts them.
const UV_R: f32 = 0.35_f64 as f32;
const UV_G: f32 = 0.15_f64 as f32;
const UV_B: f32 = 1.0_f64 as f32;
const UV_I: f32 = 0.8_f64 as f32;
/// The "long ago" value of the shot windows.
const LONG_AGO: i64 = -1000000;

/// Faults the rules (a trap in the guest), as a Lua error does.
#[cold]
fn fault() -> ! {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::unreachable()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        panic!("sky_deck rules: a nil id was passed to pinball")
    }
}

/// The id a `pinball` function receives: a missing name (`nil`) faults.
fn id(x: Option<u16>) -> u16 {
    match x {
        Some(v) => v,
        None => fault(),
    }
}

/// The id tables of `bind_ids`.
#[derive(Clone, Copy)]
struct Ids {
    start: Option<u16>,
    launch: Option<u16>,
    tilt: Option<u16>,
    action: Option<u16>,
    trough_1: Option<u16>,
    trough_2: Option<u16>,
    sl: Option<u16>,
    sr: Option<u16>,
    loop_entry: Option<u16>,
    loop_top: Option<u16>,
    ramp_entry: Option<u16>,
    ramp_made: Option<u16>,
    vuk: Option<u16>,
    deck: Option<u16>,
    newton: Option<u16>,
    d1: Option<u16>,
    d2: Option<u16>,
    d3: Option<u16>,
    coil_sl: Option<u16>,
    coil_sr: Option<u16>,
    coil_vuk: Option<u16>,
    coil_plunger: Option<u16>,
    coil_drop: Option<u16>,
    coil_bridge: Option<u16>,
    scene_attract: Option<u16>,
    scene_score: Option<u16>,
    scene_over: Option<u16>,
    scene_od: Option<u16>,
    snd_bumper: Option<u16>,
    snd_sling: Option<u16>,
    disp_feed: Option<u16>,
    disp_glo: Option<u16>,
    prof_steel: Option<u16>,
    prof_glo: Option<u16>,
    diverter: Option<u16>,
    lamp_od: Option<u16>,
    lamp_bridge: Option<u16>,
    k_games: Option<u16>,
    k_high: Option<u16>,
    k_ball: Option<u16>,
    k_state: Option<u16>,
    k_mode: Option<u16>,
    k_bridge: Option<u16>,
    k_ready: Option<u16>,
}

impl Ids {
    const NONE: Ids = Ids {
        start: None,
        launch: None,
        tilt: None,
        action: None,
        trough_1: None,
        trough_2: None,
        sl: None,
        sr: None,
        loop_entry: None,
        loop_top: None,
        ramp_entry: None,
        ramp_made: None,
        vuk: None,
        deck: None,
        newton: None,
        d1: None,
        d2: None,
        d3: None,
        coil_sl: None,
        coil_sr: None,
        coil_vuk: None,
        coil_plunger: None,
        coil_drop: None,
        coil_bridge: None,
        scene_attract: None,
        scene_score: None,
        scene_over: None,
        scene_od: None,
        snd_bumper: None,
        snd_sling: None,
        disp_feed: None,
        disp_glo: None,
        prof_steel: None,
        prof_glo: None,
        diverter: None,
        lamp_od: None,
        lamp_bridge: None,
        k_games: None,
        k_high: None,
        k_ball: None,
        k_state: None,
        k_mode: None,
        k_bridge: None,
        k_ready: None,
    };
}

/// Every rule-state local of `rules.lua`, plus the id tables.
struct SkyDeck {
    state: i64,
    score: i64,
    ball: i64,
    games: i64,
    high: i64,
    ticks: i64,
    dirty: bool,
    live: i64,
    last_drain: i64,
    tilted: bool,
    vuk_at: i64,
    drop_at: i64,
    attract_at: i64,
    bridge_until: i64,
    last_ramp_made: i64,
    last_loop_entry: i64,
    newton_hit: bool,
    /// Bit 1, 2, 4: drop_1..3 down.
    drops: i64,
    drops_done: bool,
    od_ready: bool,
    od_active: bool,
    od_used: bool,
    od_starts: i64,
    glo_at: i64,
    glo_after_eject: bool,
    glo_rising: bool,
    glo_out: bool,
    empty_checks: i64,
    ids: Ids,
    /// `save_state` output buffer.
    blob: [u8; BLOB_LEN],
}

saved_fields!(SkyDeck;
    state, score, ball, games, high, ticks, live, last_drain, tilted,
    vuk_at, drop_at, attract_at, bridge_until, last_ramp_made,
    last_loop_entry, newton_hit, drops, drops_done, od_ready, od_active,
    od_used, od_starts, glo_at, glo_after_eject, glo_rising, glo_out,
    empty_checks,
);

/// Length of the `save_state` output buffer, bytes.
const BLOB_LEN: usize = 512;

// The state blob (fingerprint slot included) must fit the buffer.
const _: () = assert!(SkyDeck::SAVED_BYTES <= BLOB_LEN);

impl SkyDeck {
    /// The Lua locals' initial values.
    const INITIAL: SkyDeck = SkyDeck {
        state: ATTRACT,
        score: 0,
        ball: 0,
        games: 0,
        high: 0,
        ticks: 0,
        dirty: true,
        live: 0,
        last_drain: -1,
        tilted: false,
        vuk_at: -1,
        drop_at: -1,
        attract_at: -1,
        bridge_until: -1,
        last_ramp_made: LONG_AGO,
        last_loop_entry: LONG_AGO,
        newton_hit: false,
        drops: 0,
        drops_done: false,
        od_ready: false,
        od_active: false,
        od_used: false,
        od_starts: 0,
        glo_at: -1,
        glo_after_eject: false,
        glo_rising: false,
        glo_out: false,
        empty_checks: 0,
        ids: Ids::NONE,
        blob: [0; BLOB_LEN],
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
        i.tilt = s("tilt");
        i.action = s("action");
        i.trough_1 = s("trough_1");
        i.trough_2 = s("trough_2");
        i.sl = s("sling_left_sw");
        i.sr = s("sling_right_sw");
        i.loop_entry = s("loop_entry");
        i.loop_top = s("loop_top");
        i.ramp_entry = s("ramp_entry");
        i.ramp_made = s("ramp_made");
        i.vuk = s("vuk_sw");
        i.deck = s("deck_opto");
        i.newton = s("newton_sw");
        i.d1 = s("drop_1_sw");
        i.d2 = s("drop_2_sw");
        i.d3 = s("drop_3_sw");
        let c = pin::coil;
        i.coil_sl = c("sling_left_coil");
        i.coil_sr = c("sling_right_coil");
        i.coil_vuk = c("vuk_coil");
        i.coil_plunger = c("plunger_coil");
        i.coil_drop = c("drop_reset_coil");
        i.coil_bridge = c("bridge_coil");
        let sc = |n| pin::id_of(Kind::Scene, n);
        i.scene_attract = sc("scene_attract");
        i.scene_score = sc("scene_score");
        i.scene_over = sc("scene_game_over");
        i.scene_od = sc("scene_night_overdrive");
        i.snd_bumper = pin::sound("sfx_bumper");
        i.snd_sling = pin::sound("sfx_sling");
        i.disp_feed = pin::id_of(Kind::Dispenser, "feed");
        i.disp_glo = pin::id_of(Kind::Dispenser, "glo_feed");
        i.prof_steel = pin::id_of(Kind::Profile, "standard_steel");
        i.prof_glo = pin::id_of(Kind::Profile, "uv_globall");
        i.diverter = pin::id_of(Kind::Diverter, "bridge_diverter");
        i.lamp_od = pin::lamp("overdrive_lamp");
        i.lamp_bridge = pin::lamp("bridge_lamp");
        let p = pin::persist_key;
        i.k_games = p("games_played");
        i.k_high = p("high_score");
        i.k_ball = p("ball");
        i.k_state = p("game_state");
        i.k_mode = p("mode:night_overdrive");
        i.k_bridge = p("bridge_open");
        i.k_ready = p("overdrive_ready");
    }

    /// Lua `add` (voided while tilted).
    fn add(&mut self, points: i64) {
        if self.tilted {
            return;
        }
        self.score += points;
        self.dirty = true;
    }

    /// Lua `add_deck`: upper-deck points, doubled while Night Overdrive runs.
    fn add_deck(&mut self, mut points: i64) {
        if self.od_active {
            points *= 2;
        }
        self.add(points);
    }

    /// Lua `serve_ball`.
    fn serve_ball(&mut self) {
        pin::spawn_ball(id(self.ids.disp_feed), id(self.ids.prof_steel));
        self.live = 1;
        self.empty_checks = 0;
        pin::persist_set(id(self.ids.k_ball), self.ball);
    }

    /// Lua `close_bridge`.
    fn close_bridge(&mut self) {
        if self.bridge_until < 0 {
            return;
        }
        self.bridge_until = -1;
        pin::set_diverter(id(self.ids.diverter), false);
        pin::hold_coil(id(self.ids.coil_bridge), false);
        pin::set_lamp(id(self.ids.lamp_bridge), 0.0);
        pin::persist_set(id(self.ids.k_bridge), 0);
    }

    /// Lua `open_bridge`.
    fn open_bridge(&mut self) {
        self.bridge_until = self.ticks + BRIDGE_TICKS;
        pin::set_diverter(id(self.ids.diverter), true);
        pin::hold_coil(id(self.ids.coil_bridge), true);
        pin::set_lamp(id(self.ids.lamp_bridge), 1.0);
        pin::persist_set(id(self.ids.k_bridge), 1);
    }

    /// Lua `set_ready(on)`.
    fn set_ready(&mut self, on: bool) {
        if self.od_ready == on {
            return;
        }
        self.od_ready = on;
        if on {
            pin::persist_set(id(self.ids.k_ready), 1);
            pin::set_lamp(id(self.ids.lamp_od), 1.0);
        } else {
            pin::persist_set(id(self.ids.k_ready), 0);
            if !self.od_active {
                pin::set_lamp(id(self.ids.lamp_od), 0.0);
            }
        }
    }

    /// Lua `end_overdrive`.
    fn end_overdrive(&mut self) {
        if !self.od_active {
            return;
        }
        self.od_active = false;
        pin::set_gi_tint(1.0, 1.0, 1.0, 1.0);
        pin::set_lamp(id(self.ids.lamp_od), 0.0);
        pin::show_scene(id(self.ids.scene_score));
    }

    /// Lua `start_overdrive`.
    fn start_overdrive(&mut self) {
        self.set_ready(false);
        self.od_active = true;
        self.od_used = true;
        self.od_starts += 1;
        pin::persist_set(id(self.ids.k_mode), self.od_starts);
        pin::set_gi_tint(UV_R, UV_G, UV_B, UV_I);
        pin::set_lamp(id(self.ids.lamp_od), 1.0);
        pin::show_scene(id(self.ids.scene_od));
        // The Glo-Ball counts as on the table from now on.
        self.live += 1;
        self.glo_rising = true;
        self.glo_out = false;
        if self.vuk_at >= 0 {
            // A ball is in the scoop: lift it now, then dispense the Glo-Ball.
            self.glo_after_eject = true;
            if self.vuk_at > self.ticks + VUK_QUICK {
                self.vuk_at = self.ticks + VUK_QUICK;
            }
        } else {
            self.glo_at = self.ticks;
        }
    }

    /// Lua `vuk_hold`: ticks the VUK holds a ball captured now.
    fn vuk_hold(&self) -> i64 {
        if self.glo_rising {
            return VUK_QUICK;
        }
        VUK_HOLD
    }

    /// Lua `check_armed`.
    fn check_armed(&mut self) {
        if self.newton_hit && self.drops_done && !self.od_used && !self.od_active {
            self.set_ready(true);
        }
    }

    /// Lua `reset_bank`.
    fn reset_bank(&mut self) {
        self.drops = 0;
        self.drop_at = -1;
        pin::pulse_coil(id(self.ids.coil_drop), 150);
    }

    /// Lua `start_game`.
    fn start_game(&mut self) {
        self.state = GAME;
        self.score = 0;
        self.ball = 1;
        self.tilted = false;
        self.attract_at = -1;
        self.dirty = true;
        pin::persist_set(id(self.ids.k_state), GAME);
        pin::show_scene(id(self.ids.scene_score));
        self.serve_ball();
    }

    /// Lua `end_ball`.
    fn end_ball(&mut self) {
        self.end_overdrive();
        self.set_ready(false);
        self.glo_at = -1;
        self.glo_after_eject = false;
        self.glo_rising = false;
        self.glo_out = false;
        self.newton_hit = false;
        self.drops_done = false;
        self.od_used = false;
        self.tilted = false;
        // Shot windows and the ramp debounce do not carry across balls.
        self.last_loop_entry = LONG_AGO;
        self.last_ramp_made = LONG_AGO;
        if self.drops != 0 || self.drop_at >= 0 {
            self.reset_bank();
        }
        self.live = 0;
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
        self.close_bridge();
        pin::set_gi_tint(1.0, 1.0, 1.0, 1.0);
        pin::persist_set(id(self.ids.k_games), self.games);
        pin::persist_set(id(self.ids.k_high), self.high);
        pin::persist_set(id(self.ids.k_ball), 0);
        pin::persist_set(id(self.ids.k_state), OVER);
        pin::show_scene(id(self.ids.scene_over));
        self.attract_at = self.ticks + GAME_OVER_TICKS;
    }

    /// Lua `drained`.
    fn drained(&mut self) {
        // One drain can close both trough switches in the same tick.
        if self.last_drain == self.ticks {
            return;
        }
        self.last_drain = self.ticks;
        self.live -= 1;
        // The first drain of the Night Overdrive multiball ends the mode.
        self.end_overdrive();
        if self.live > 0 && self.glo_at < 0 && !self.glo_after_eject && pin::balls_in_play() == 0 {
            // Two balls drained in one tick: nothing is left on the table.
            self.live = 0;
        }
        if self.live <= 0 {
            self.end_ball();
        }
    }

    /// Lua `drop_down(bit)`.
    fn drop_down(&mut self, bit: i64) {
        if self.drops & bit != 0 {
            return;
        }
        self.drops |= bit;
        self.add_deck(100);
        if self.drops == 7 {
            self.add_deck(1000);
            self.drops_done = true;
            self.drop_at = self.ticks + DROP_RESET_DELAY;
            self.check_armed();
        }
    }
}

impl Rules for SkyDeck {
    fn on_init(&mut self) {
        self.bind_ids();
        let i = self.ids;
        self.games = pin::persist_get(id(i.k_games));
        self.high = pin::persist_get(id(i.k_high));
        pin::persist_set(id(i.k_ball), 0);
        pin::persist_set(id(i.k_state), ATTRACT);
        pin::persist_set(id(i.k_mode), 0);
        pin::persist_set(id(i.k_bridge), 0);
        pin::persist_set(id(i.k_ready), 0);
        pin::set_gi_tint(1.0, 1.0, 1.0, 1.0);
        pin::show_scene(id(i.scene_attract));
    }

    fn on_switch_edge(&mut self, sw: u16, closed: bool) {
        self.bind_ids();
        let s = self.ids;
        let is = |x: Option<u16>| x == Some(sw);
        self.ticks = pin::tick();
        if !closed {
            return;
        }
        if is(s.start) {
            if self.state != GAME {
                self.start_game();
            }
            return;
        }
        if self.state != GAME {
            return;
        }
        if is(s.trough_1) || is(s.trough_2) {
            self.drained();
        } else if is(s.launch) {
            pin::pulse_coil(id(s.coil_plunger), 300);
        } else if is(s.action) {
            if self.od_ready && !self.od_active {
                self.start_overdrive();
            } else if !self.od_active && !self.od_ready && self.bridge_until < 0 {
                self.open_bridge();
            }
        } else if is(s.tilt) {
            self.tilted = true;
        } else if is(s.sl) {
            pin::pulse_coil(id(s.coil_sl), 25);
            pin::play_sound(id(s.snd_sling), 1.0);
            self.add(1000);
        } else if is(s.sr) {
            pin::pulse_coil(id(s.coil_sr), 25);
            pin::play_sound(id(s.snd_sling), 1.0);
            self.add(1000);
        } else if is(s.loop_entry) {
            self.add(1000);
            self.last_loop_entry = self.ticks;
        } else if is(s.loop_top) {
            self.add(1000);
            if self.ticks - self.last_loop_entry <= LOOP_WINDOW {
                self.add(50000);
            }
            self.last_loop_entry = LONG_AGO;
        } else if is(s.ramp_entry) {
            self.add(1000);
        } else if is(s.ramp_made) {
            if self.ticks - self.last_ramp_made > RAMP_DEBOUNCE {
                self.add(25000);
            }
            self.last_ramp_made = self.ticks;
        } else if is(s.vuk) {
            self.add(100);
            pin::play_sound(id(s.snd_bumper), 1.0);
            if self.vuk_at < 0 {
                self.vuk_at = self.ticks + self.vuk_hold();
            }
            if self.od_ready && !self.od_active {
                self.start_overdrive();
            }
        } else if is(s.deck) {
            self.add_deck(100);
        } else if is(s.newton) {
            self.add(100);
            self.newton_hit = true;
            self.check_armed();
        } else if is(s.d1) {
            self.drop_down(1);
        } else if is(s.d2) {
            self.drop_down(2);
        } else if is(s.d3) {
            self.drop_down(4);
        }
    }

    fn on_tick(&mut self) {
        self.bind_ids();
        let s = self.ids;
        self.ticks = pin::tick();
        if self.vuk_at >= 0 && self.ticks >= self.vuk_at {
            self.vuk_at = -1;
            pin::pulse_coil(id(s.coil_vuk), 20);
            if self.glo_out {
                self.glo_rising = false;
                self.glo_out = false;
            }
            if self.glo_after_eject {
                self.glo_after_eject = false;
                self.glo_at = self.ticks + GLO_DELAY;
            }
        }
        if self.glo_at >= 0 && self.ticks >= self.glo_at {
            self.glo_at = -1;
            pin::spawn_ball(id(s.disp_glo), id(s.prof_glo));
            self.glo_out = true;
        }
        if self.drop_at >= 0 && self.ticks >= self.drop_at {
            self.reset_bank();
        }
        if self.bridge_until >= 0 && self.ticks >= self.bridge_until {
            self.close_bridge();
        }
        if self.attract_at >= 0 && self.ticks >= self.attract_at {
            self.attract_at = -1;
            if self.state == OVER {
                self.state = ATTRACT;
                pin::persist_set(id(s.k_state), ATTRACT);
                pin::show_scene(id(s.scene_attract));
            }
        }
        if self.ticks.rem_euclid(CHECK_EVERY) == 0 && self.state == GAME {
            // A ball held in the scoop with no eject pending: eject it.
            if self.vuk_at < 0 && pin::switch(id(s.vuk)) {
                self.vuk_at = self.ticks + self.vuk_hold();
            }
            // Nothing on the table for two checks in a row while the rules
            // still count a ball: end it.
            if self.live > 0
                && self.glo_at < 0
                && !self.glo_after_eject
                && pin::balls_in_play() == 0
            {
                self.empty_checks += 1;
                if self.empty_checks >= 2 {
                    self.live = 0;
                    self.end_ball();
                }
            } else {
                self.empty_checks = 0;
            }
        }
        if self.dirty {
            pin::set_score(1, self.score);
            self.dirty = false;
        }
    }

    /// Lua `on_save_state`: every rule-state local.
    fn save_state(&mut self) -> &[u8] {
        let mut buf = [0u8; BLOB_LEN];
        let n = self.encode_state(&mut buf);
        self.blob = buf;
        &self.blob[..n]
    }

    /// Lua `on_restore_state`: binds the ids, takes every local from the
    /// blob (a blob of another length or layout fingerprint is ignored) and
    /// redraws the score.
    fn restore_state(&mut self, blob: &[u8]) {
        self.bind_ids();
        if !self.decode_state(blob) {
            return;
        }
        self.dirty = true;
    }
}

pin_guest_sdk::export_rules!(SkyDeck = SkyDeck::INITIAL);

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
        let mut a = SkyDeck::INITIAL;
        a.score = 987_654;
        a.drops = 5;
        a.od_active = true;
        a.glo_out = true;
        a.last_drain = -1;
        a.empty_checks = 1;
        let mut buf = [0u8; 512];
        let n = a.encode_state(&mut buf);
        assert_eq!(n, SkyDeck::SAVED_BYTES);
        // The layout fingerprint slot plus 27 saved slots.
        assert_eq!(n, (1 + 27) * 8);
        assert_eq!(buf[..8], SkyDeck::LAYOUT_FINGERPRINT.to_le_bytes());
        let mut b = SkyDeck::INITIAL;
        assert!(b.decode_state(&buf[..n]));
        assert_eq!((b.score, b.drops, b.empty_checks), (987_654, 5, 1));
        assert!(b.od_active && b.glo_out && !b.glo_rising);
        assert_eq!(b.last_ramp_made, LONG_AGO);
    }

    #[test]
    fn a_blob_of_the_wrong_length_changes_nothing() {
        let mut b = SkyDeck::INITIAL;
        assert!(!b.decode_state(&[3u8; 16]));
        assert_eq!(b.score, 0);
        assert_eq!(b.vuk_at, -1);
        // A valid blob one slot short or one slot long, fingerprint intact.
        let mut a = SkyDeck::INITIAL;
        a.score = 99;
        a.vuk_at = 5;
        let mut buf = [0u8; BLOB_LEN];
        let n = a.encode_state(&mut buf);
        assert!(!b.decode_state(&buf[..n - 8]));
        assert!(!b.decode_state(&buf[..n + 8]));
        // Arbitrary bytes of exactly the right length: wrong fingerprint.
        assert!(!b.decode_state(&[1u8; SkyDeck::SAVED_BYTES]));
        assert_eq!((b.score, b.vuk_at), (0, -1));
    }

    #[test]
    fn a_blob_with_a_wrong_layout_fingerprint_changes_nothing() {
        let mut a = SkyDeck::INITIAL;
        a.score = 99;
        a.vuk_at = 5;
        let mut buf = [0u8; BLOB_LEN];
        let n = a.encode_state(&mut buf);
        assert_eq!(n, SkyDeck::SAVED_BYTES);
        let mut b = SkyDeck::INITIAL;
        for bit in 0..64 {
            let mut bad = buf;
            bad[bit / 8] ^= 1 << (bit % 8);
            assert!(!b.decode_state(&bad[..n]), "fingerprint bit {bit}");
            assert_eq!((b.score, b.vuk_at), (0, -1));
        }
        // The untouched blob still restores.
        assert!(b.decode_state(&buf[..n]));
        assert_eq!((b.score, b.vuk_at), (a.score, a.vuk_at));
    }

    #[test]
    fn uv_tint_matches_the_host_conversion_of_the_lua_literals() {
        // The Lua host converts a Lua number with `n as f32`.
        let lua = [0.35_f64, 0.15, 1.0, 0.8].map(|n| n as f32);
        assert_eq!([UV_R, UV_G, UV_B, UV_I], lua);
    }

    #[test]
    fn vuk_hold_is_quick_while_the_globall_rises() {
        let mut a = SkyDeck::INITIAL;
        assert_eq!(a.vuk_hold(), VUK_HOLD);
        a.glo_rising = true;
        assert_eq!(a.vuk_hold(), VUK_QUICK);
        assert!(std::panic::catch_unwind(|| id(None)).is_err());
    }
}
