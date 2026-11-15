//! Grey-box rules (`tables/greybox/rules.lua`) against the Lua backend
//! directly, in simulated ticks (no sleeps, no wall clock):
//!
//! * Hot reload (PR #21 finding #14): `apply_reload` of the same source in
//!   the middle of a game keeps the switch / coil / scene / sound bindings
//!   working (the new state never runs `on_init`) and carries the game state,
//!   ball number, score and Powerball mode through `on_save_state` /
//!   `on_restore_state`; the game then continues to game over and a new
//!   START.
//! * A magnet-opto event still pending when a ball ends does not start the
//!   Powerball mode on the next ball (finding #15).
//! * The `mode:powerball` persist key (the 2.9 soak's `mode:<id>` signal)
//!   counts Powerball mode starts next to `powerball_modes` (finding #16).

mod support;

use pin_assets::{Vfs, load_table};
use pin_core::ids::{CoilId, PersistKey, SceneId, SoundId, SwitchId};
use pin_core::script::{HostCommandQueue, HostCtx, HostView, SwitchEdge, TableScriptBackend};
use pin_core::{Pcg32, SeedableRng};
use pin_script::NameTables;
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use support::greybox_bundle;

/// The rules plus a minimal host: ticks, applies persist writes to the view
/// and collects what the rules asked for.
struct Rig {
    b: LuaScriptBackend,
    names: NameTables,
    src: Vec<u8>,
    view: HostView,
    rng: Pcg32,
    cmd: HostCommandQueue,
    /// Last score set for player 1.
    score: i64,
    scenes: Vec<SceneId>,
    spawns: usize,
    pulses: Vec<CoilId>,
    sounds: Vec<SoundId>,
}

impl Rig {
    fn new() -> Self {
        let vfs = Vfs::open(greybox_bundle()).unwrap();
        let table = load_table(&vfs).unwrap();
        let names = pin_sim::load::name_tables(&table.ids);
        let src = vfs.read("rules.lua").unwrap();
        let b = LuaScriptBackend::new(&src, &names, LuaConfig::engine()).unwrap();
        let mut r = Rig {
            b,
            names,
            src,
            view: HostView::default(),
            rng: Pcg32::seed_from_u64(7),
            cmd: HostCommandQueue::default(),
            score: 0,
            scenes: Vec::new(),
            spawns: 0,
            pulses: Vec::new(),
            sounds: Vec::new(),
        };
        r.call(|b, c| b.on_init(c));
        r
    }

    fn call(&mut self, f: impl FnOnce(&mut LuaScriptBackend, &mut HostCtx)) {
        self.cmd.clear();
        let mut ctx = HostCtx {
            view: &self.view,
            rng: &mut self.rng,
            cmd: &mut self.cmd,
        };
        f(&mut self.b, &mut ctx);
        for &(k, v) in &self.cmd.persist_writes {
            self.view.persist[k.index()] = v;
        }
        for &(p, s) in &self.cmd.scores {
            if p == 0 {
                self.score = s;
            }
        }
        self.scenes.extend(self.cmd.display_scenes.iter().copied());
        self.spawns += self.cmd.ball_spawns.len();
        self.pulses
            .extend(self.cmd.coil_pulses.iter().map(|(c, _)| *c));
        self.sounds.extend(self.cmd.sounds.iter().map(|(s, _)| *s));
        assert!(!self.b.is_faulted(), "script fault");
    }

    /// One tick with no edge.
    fn tick(&mut self) {
        self.view.tick += 1;
        self.call(|b, c| b.on_tick(c));
    }

    fn ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.tick();
        }
    }

    /// One tick in which switch `name` closes (and reopens next tick).
    fn hit(&mut self, name: &str) {
        let s = self.sw(name);
        self.view.tick += 1;
        self.call(|b, c| {
            b.on_switch_edge(s, SwitchEdge::Closed, c);
            b.on_tick(c);
        });
        self.view.tick += 1;
        self.call(|b, c| {
            b.on_switch_edge(s, SwitchEdge::Opened, c);
            b.on_tick(c);
        });
    }

    fn sw(&self, n: &str) -> SwitchId {
        find(&self.names.switches, n)
    }

    fn key(&self, n: &str) -> PersistKey {
        find(&self.names.persist, n)
    }

    fn persist(&self, n: &str) -> i64 {
        self.view.persist_value(self.key(n))
    }

    fn reload(&mut self) {
        let src = self.src.clone();
        self.b
            .apply_reload(&src)
            .expect("reload of the same source");
        assert!(!self.b.is_faulted());
    }
}

fn find<I: Copy>(v: &[(String, I)], n: &str) -> I {
    v.iter()
        .find(|(s, _)| s == n)
        .map(|(_, i)| *i)
        .unwrap_or_else(|| panic!("no id named {n}"))
}

const ATTRACT: i64 = 0;
const GAME: i64 = 1;
const OVER: i64 = 2;

/// The Powerball crosses the magnet detector: opto, no eddy; the detection
/// window (30 ticks) then elapses.
fn powerball_crossing(r: &mut Rig) {
    r.hit("magnet_opto");
    r.ticks(40);
}

#[test]
fn reload_in_attract_keeps_start_working() {
    let mut r = Rig::new();
    r.ticks(10);
    r.reload();
    r.ticks(10);
    let spawns = r.spawns;
    r.hit("start");
    assert_eq!(r.persist("game_state"), GAME, "START matched after reload");
    assert_eq!(r.persist("ball"), 1);
    assert_eq!(r.spawns, spawns + 1, "ball 1 served");
    assert_eq!(
        r.scenes.last(),
        Some(&find(&r.names.scenes, "scene_score")),
        "score scene shown"
    );
}

#[test]
fn reload_mid_game_preserves_state_and_the_game_continues() {
    let mut r = Rig::new();
    r.ticks(5);
    r.hit("start");
    r.hit("bumper_1_ring");
    assert_eq!(r.score, 100);
    // Ball 1 drains: ball 2 (the Powerball) starts its mode.
    r.hit("trough_1");
    assert_eq!(r.persist("ball"), 2);
    powerball_crossing(&mut r);
    assert_eq!(r.persist("powerball_modes"), 1);
    // A VUK capture and a drop target leave timers pending across the reload.
    r.hit("vuk_sw");
    r.hit("drop_1_sw");
    let score = r.score;
    assert_eq!(score, 100 + 2 * (2500 + 750));

    r.reload();
    // Restored, no re-init: host persist untouched, no scene re-shown.
    let scenes = r.scenes.len();
    r.tick();
    assert_eq!(r.scenes.len(), scenes, "on_init not re-run");
    assert_eq!(r.persist("game_state"), GAME);
    assert_eq!(r.persist("ball"), 2);
    assert_eq!(r.score, score, "score redrawn from the restored state");

    // Id tables bound: switches match, coils and sounds resolve, and the
    // Powerball mode still doubles.
    let bumper_coil = find(&r.names.coils, "bumper_2_coil");
    let sfx = find(&r.names.sounds, "sfx_bumper");
    let (pulses, sounds) = (r.pulses.len(), r.sounds.len());
    r.hit("bumper_2_ring");
    assert_eq!(r.score, score + 200, "bumper scored, doubled in the mode");
    assert_eq!(&r.pulses[pulses..], [bumper_coil]);
    assert_eq!(&r.sounds[sounds..], [sfx]);
    // The pending VUK eject and drop reset fire from the restored timers.
    let pulses = r.pulses.len();
    r.ticks(1100);
    let fired = &r.pulses[pulses..];
    assert!(
        fired.contains(&find(&r.names.coils, "vuk_coil")),
        "{fired:?}"
    );
    assert!(fired.contains(&find(&r.names.coils, "drop_reset_coil")));

    // Ball 2 and 3 drain: game over, then attract, then a new game.
    r.hit("trough_2");
    assert_eq!(r.persist("ball"), 3);
    assert_eq!(r.persist("powerball_modes"), 1, "mode ended with ball 2");
    r.hit("sling_left_sw");
    assert_eq!(r.score, score + 200 + 10, "mode off on ball 3");
    r.hit("trough_1");
    assert_eq!(r.persist("game_state"), OVER);
    assert_eq!(r.persist("games_played"), 1);
    assert_eq!(r.persist("high_score"), score + 210);
    r.ticks(4100);
    assert_eq!(r.persist("game_state"), ATTRACT);
    r.hit("start");
    assert_eq!(r.persist("game_state"), GAME);
    assert_eq!(r.persist("ball"), 1);
}

#[test]
fn reload_after_game_over_keeps_the_game_count_and_attract_timer() {
    let mut r = Rig::new();
    r.hit("start");
    r.hit("orbit_top");
    for _ in 0..3 {
        r.hit("trough_1");
    }
    assert_eq!(r.persist("game_state"), OVER);
    r.reload();
    r.ticks(4100);
    assert_eq!(r.persist("game_state"), ATTRACT, "restored attract timer");
    r.hit("start");
    r.hit("orbit_top");
    for _ in 0..3 {
        r.hit("trough_1");
    }
    assert_eq!(r.persist("games_played"), 2, "restored game count");
    assert_eq!(r.persist("high_score"), 5000);
}

#[test]
fn opto_pending_at_end_of_ball_does_not_leak_into_the_next_ball() {
    let mut r = Rig::new();
    r.hit("start");
    // Ball 1 (steel) closes the opto, then drains inside the detection
    // window, before the eddy edge could have been judged.
    r.hit("magnet_opto");
    r.ticks(3);
    r.hit("trough_1");
    assert_eq!(r.persist("ball"), 2);
    r.ticks(100);
    assert_eq!(
        r.persist("powerball_modes"),
        0,
        "no mode from ball 1's opto"
    );
    assert_eq!(r.persist("mode:powerball"), 0);
    let pb = find(&r.names.scenes, "scene_powerball_mania");
    assert!(!r.scenes.contains(&pb));
    // Ball 2's own crossing still starts the mode.
    powerball_crossing(&mut r);
    assert_eq!(r.persist("powerball_modes"), 1);
    assert!(r.scenes.contains(&pb));
}

#[test]
fn mode_powerball_persist_key_counts_starts() {
    let mut r = Rig::new();
    assert_eq!(r.persist("mode:powerball"), 0);
    for game in 1..=2 {
        r.hit("start");
        r.hit("trough_1");
        assert_eq!(r.persist("ball"), 2);
        powerball_crossing(&mut r);
        assert_eq!(r.persist("mode:powerball"), game, "game {game}");
        assert_eq!(r.persist("powerball_modes"), game, "game {game}");
        r.hit("trough_1");
        r.hit("trough_1");
        assert_eq!(r.persist("game_state"), OVER);
        r.ticks(4100);
    }
}
