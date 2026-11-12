//! Spec (4.5 criterion 1): an integer-only ruleset written independently in
//! Lua (`parity_rules.lua`) and in hand-written WAT against the fixed ABI
//! (`parity_rules.wat`) receives the same switch sequence and must yield
//! identical `HostCommandQueue` contents after EVERY callback, consume the host
//! PCG identically and count the same overflows. Also: a faulting ruleset tilts
//! identically in both backends, and the Wasm backend is replay-stable.

#[path = "spec_fixtures/wasm/support.rs"]
mod support;

use pin_core::{
    HostCommandQueue, HostCtx, HostView, Pcg32, Rng, SeedableRng, SwitchEdge, SwitchId,
    TableScriptBackend,
};
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use proptest::prelude::*;
use std::hash::{Hash, Hasher};
use support::names;

const RULES_LUA: &[u8] = include_bytes!("spec_fixtures/wasm/parity_rules.lua");
const RULES_WAT: &[u8] = include_bytes!("spec_fixtures/wasm/parity_rules.wat");
const FAULT_LUA: &[u8] = include_bytes!("spec_fixtures/wasm/parity_fault.lua");
const FAULT_WAT: &[u8] = include_bytes!("spec_fixtures/wasm/parity_fault.wat");

/// Switches the generated input sequence toggles.
const SWITCHES: [u16; 8] = [0, 7, 8, 20, 63, 255, 5, 100];

/// One backend with its own generator and queue; the view is shared.
struct Side<B> {
    b: B,
    rng: Pcg32,
    cmd: HostCommandQueue,
}

impl<B: TableScriptBackend> Side<B> {
    fn new(b: B, seed: u64) -> Self {
        Self {
            b,
            rng: Pcg32::seed_from_u64(seed),
            cmd: HostCommandQueue::default(),
        }
    }
    fn ctx<'a>(&'a mut self, view: &'a HostView) -> (&'a mut B, HostCtx<'a>) {
        (
            &mut self.b,
            HostCtx {
                view,
                rng: &mut self.rng,
                cmd: &mut self.cmd,
            },
        )
    }
    fn init(&mut self, view: &HostView) {
        let (b, mut ctx) = self.ctx(view);
        b.on_init(&mut ctx);
    }
    fn tick(&mut self, view: &HostView) {
        let (b, mut ctx) = self.ctx(view);
        b.on_tick(&mut ctx);
    }
    fn edge(&mut self, view: &HostView, sw: u16, closed: bool) {
        let (b, mut ctx) = self.ctx(view);
        let e = if closed {
            SwitchEdge::Closed
        } else {
            SwitchEdge::Opened
        };
        b.on_switch_edge(SwitchId(sw), e, &mut ctx);
    }
}

struct Stats {
    ticks: u64,
    edges: u64,
    draws_seen: bool,
    pulses: usize,
    holds: usize,
    lamps: usize,
    sounds: usize,
    scores: usize,
    spawns: usize,
    persist: usize,
    magnets: usize,
    scenes: usize,
    zippers: usize,
    diverters: usize,
    overflow: u32,
    hash: u64,
}

fn hash_queue(h: &mut impl Hasher, q: &HostCommandQueue) {
    // HostCommandQueue has no Hash (it carries f32); its Debug form is exact enough.
    format!("{q:?}").hash(h);
}

/// Drives both backends with the same generated inputs and asserts equality
/// after every callback.
fn run_parity(seed: u64, input_seed: u64, ticks: u64) -> Stats {
    let lua = LuaScriptBackend::new(RULES_LUA, &names(), LuaConfig::default())
        .unwrap_or_else(|e| panic!("lua parity rules failed to load: {e}"));
    let wasm = WasmScriptBackend::new(RULES_WAT, &names(), WasmConfig::default())
        .unwrap_or_else(|e| panic!("wasm parity rules failed to load: {e}"));
    let mut l = Side::new(lua, seed);
    let mut w = Side::new(wasm, seed);
    let mut view = HostView::default();
    // Some pre-existing persistent state.
    view.persist[16] = 3;
    view.persist[63] = -7;
    let mut input = Pcg32::seed_from_u64(input_seed);
    let mut st = Stats {
        ticks: 0,
        edges: 0,
        draws_seen: false,
        pulses: 0,
        holds: 0,
        lamps: 0,
        sounds: 0,
        scores: 0,
        spawns: 0,
        persist: 0,
        magnets: 0,
        scenes: 0,
        zippers: 0,
        diverters: 0,
        overflow: 0,
        hash: 0,
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let fresh = Pcg32::seed_from_u64(seed);

    macro_rules! check {
        ($what:expr) => {{
            assert_eq!(l.cmd, w.cmd, "queues differ after {}", $what);
            assert_eq!(l.rng, w.rng, "host PCG state differs after {}", $what);
            assert_eq!(
                l.b.overflow_count(),
                w.b.overflow_count(),
                "overflow counts differ after {}",
                $what
            );
            assert!(!l.b.is_faulted(), "lua rules faulted at {}: {:?}", $what, l.b.last_error());
            assert!(!w.b.is_faulted(), "wasm rules faulted at {}: {:?}", $what, w.b.last_error());
        }};
    }

    l.init(&view);
    w.init(&view);
    check!("on_init");
    assert_eq!(l.cmd.display_scenes.len(), 1, "on_init must have run");
    hash_queue(&mut hasher, &w.cmd);
    for &(k, v) in &l.cmd.persist_writes {
        view.persist[k.index()] = v;
    }

    for t in 1..=ticks {
        view.tick = t;
        view.balls_in_play = ((t / 400) % 3) as u8;
        l.cmd.clear();
        w.cmd.clear();
        // Edges collected "in the previous tick": 0..=3 toggles on ~1/5 of ticks.
        if input.next_u32() % 5 == 0 {
            let count = 1 + input.next_u32() % 3;
            for _ in 0..count {
                let sw = SWITCHES[(input.next_u32() % 8) as usize];
                let closed = !view.switch_closed(SwitchId(sw));
                view.set_switch(SwitchId(sw), closed);
                l.edge(&view, sw, closed);
                w.edge(&view, sw, closed);
                st.edges += 1;
                check!(format_args!("edge sw {sw} closed {closed} in tick {t}"));
            }
        }
        l.tick(&view);
        w.tick(&view);
        check!(format_args!("on_tick {t}"));
        st.ticks += 1;
        let q = &w.cmd;
        st.pulses += q.coil_pulses.len();
        st.holds += q.coil_holds.len();
        st.lamps += q.lamps.len();
        st.sounds += q.sounds.len();
        st.scores += q.scores.len();
        st.spawns += q.ball_spawns.len();
        st.persist += q.persist_writes.len();
        st.magnets += q.magnets.len();
        st.scenes += q.display_scenes.len();
        st.zippers += q.zippers.len();
        st.diverters += q.diverters.len();
        hash_queue(&mut hasher, q);
        // The sim applies persistent writes; both backends see the same view.
        for &(k, v) in &l.cmd.persist_writes {
            view.persist[k.index()] = v;
        }
    }
    st.draws_seen = w.rng != fresh;
    st.overflow = w.b.overflow_count();
    st.hash = hasher.finish();
    st
}

#[test]
fn lua_and_wasm_rules_emit_identical_queues_on_every_tick() {
    let st = run_parity(0xC0FFEE, 0xBADC0DE, 3000);
    // The run must actually exercise everything criterion 1 lists.
    assert!(st.ticks >= 2000);
    assert!(st.edges >= 200, "only {} edges", st.edges);
    assert!(st.draws_seen, "no random draws happened");
    assert!(st.pulses > 200, "pulses {}", st.pulses);
    assert!(st.holds >= 200, "holds {}", st.holds);
    assert!(st.lamps > 400, "lamps {}", st.lamps);
    assert!(st.sounds >= 30, "sounds {}", st.sounds);
    assert!(st.scores > 200, "scores {}", st.scores);
    assert!(st.spawns >= 9, "spawns {}", st.spawns);
    assert!(st.persist >= 20, "persist {}", st.persist);
    assert!(st.magnets >= 10, "magnets {}", st.magnets);
    assert!(st.scenes >= 9 && st.zippers >= 9 && st.diverters >= 9);
    assert_eq!(st.overflow, 6 * 8, "six 40-pulse bursts drop 8 commands each");
}

#[test]
fn parity_holds_for_several_seeds() {
    for (seed, input_seed) in [(0u64, 0u64), (1, 2), (u64::MAX, 77), (0xDEAD_BEEF, 0xFEED)] {
        let st = run_parity(seed, input_seed, 2100);
        assert!(st.edges >= 200);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    #[test]
    fn parity_holds_for_random_seeds(seed in any::<u64>(), input_seed in any::<u64>()) {
        run_parity(seed, input_seed, 700);
    }
}

#[test]
fn wasm_command_stream_is_stable_over_ten_runs() {
    let first = run_parity(42, 4242, 2000).hash;
    for _ in 0..9 {
        assert_eq!(run_parity(42, 4242, 2000).hash, first);
    }
    assert_ne!(run_parity(43, 4242, 2000).hash, first, "the seed must matter");
}

/// Fault parity: 39 coils held, then an argument violation after queueing
/// commands. Both backends must roll back, release in the same order over the
/// same number of ticks, and stay silent afterwards.
#[test]
fn a_faulting_ruleset_tilts_identically_in_lua_and_wasm() {
    let lua = LuaScriptBackend::new(FAULT_LUA, &names(), LuaConfig::default()).unwrap();
    let wasm = WasmScriptBackend::new(FAULT_WAT, &names(), WasmConfig::default())
        .unwrap_or_else(|e| panic!("wasm fault rules failed to load: {e}"));
    let mut l = Side::new(lua, 5);
    let mut w = Side::new(wasm, 5);
    let mut view = HostView::default();
    l.init(&view);
    w.init(&view);
    let mut released = Vec::new();
    for t in 1..=8u64 {
        view.tick = t;
        l.cmd.clear();
        w.cmd.clear();
        if t >= 3 {
            l.edge(&view, 9, true);
            w.edge(&view, 9, true);
            assert_eq!(l.cmd, w.cmd, "queues differ after the edge in tick {t}");
        }
        l.tick(&view);
        w.tick(&view);
        assert_eq!(l.cmd, w.cmd, "queues differ after tick {t}");
        assert_eq!(l.b.is_faulted(), w.b.is_faulted(), "tick {t}");
        assert_eq!(l.b.fault_count(), w.b.fault_count(), "tick {t}");
        assert_eq!(l.b.overflow_count(), w.b.overflow_count(), "tick {t}");
        let q = &w.cmd;
        match t {
            1 => assert_eq!(q.coil_holds.len(), 32),
            2 => assert_eq!(q.coil_holds.len(), 9),
            3 => {
                assert!(w.b.is_faulted());
                // The edge before the faulting tick ran normally and stays queued.
                assert_eq!(q.scores.as_slice(), &[(0, 9)]);
                assert!(q.coil_pulses.is_empty(), "faulting tick's pulse was not rolled back");
                assert!(q.lamps.is_empty(), "faulting tick's lamp was not rolled back");
                assert_eq!(q.gi_tint, None, "faulting tick's GI tint was not rolled back");
                assert_eq!(q.coil_holds.len(), 32, "the hold queue fills with releases");
            }
            _ => {
                assert!(q.scores.is_empty() && q.display_scenes.is_empty());
                assert!(q.coil_pulses.is_empty() && q.lamps.is_empty());
            }
        }
        if t >= 3 {
            for &(c, on) in &q.coil_holds {
                assert!(!on, "a tilted script only releases coils");
                released.push(c.0);
            }
        }
        if t >= 5 {
            assert!(q.is_empty(), "tick {t} after the tilt release must be a no-op");
        }
    }
    released.sort_unstable();
    // 101..=131 (100 was released by the script), 200..=207, and 300: the hold
    // queued by the faulting callback is released too (Lua rollback semantics).
    let mut want: Vec<u16> = (101..=131).chain(200..=207).collect();
    want.push(300);
    assert_eq!(released, want);
    assert_eq!(w.b.fault_count(), 1);
}
