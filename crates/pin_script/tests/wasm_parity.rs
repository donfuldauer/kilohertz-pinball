//! Subphase 4.5 verification criterion 1: the integer-only ruleset in
//! `fixtures/wasm/rules.lua` and its Rust twin compiled to
//! `fixtures/wasm/rules.wasm` (built with `pin_guest_sdk`) emit identical
//! `HostCommandQueue` contents on every tick for the same inputs.

use pin_core::ids::{SoundId, SwitchId};
use pin_core::{
    HostCommandQueue, HostCtx, HostView, Pcg32, SeedableRng, SwitchEdge, TableScriptBackend,
};
use pin_script::NameTables;
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use std::path::{Path, PathBuf};
use std::process::Command;

const RULES_LUA: &[u8] = include_bytes!("fixtures/wasm/rules.lua");
const RULES_WASM: &[u8] = include_bytes!("fixtures/wasm/rules.wasm");

fn names() -> NameTables {
    NameTables {
        sounds: vec![("jackpot".into(), SoundId(9))],
        ..NameTables::default()
    }
}

/// A tiny deterministic input generator (independent of the script PCG).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

#[derive(Default)]
struct Totals {
    edges: usize,
    lamps: usize,
    sounds: usize,
    scores: usize,
    holds: usize,
    spawns: usize,
    persist: usize,
    pulses: usize,
    magnets: usize,
}

fn run_parity(seed: u64, ticks: u64) -> Totals {
    let mut lua = LuaScriptBackend::new(RULES_LUA, &names(), LuaConfig::engine()).unwrap();
    let mut wasm = WasmScriptBackend::new(RULES_WASM, &names(), WasmConfig::engine()).unwrap();
    let mut view = HostView::default();
    let (mut rng_l, mut rng_w) = (Pcg32::seed_from_u64(seed), Pcg32::seed_from_u64(seed));
    let (mut ql, mut qw) = (HostCommandQueue::default(), HostCommandQueue::default());
    let mut input = Lcg(seed ^ 0x5eed);
    let mut tot = Totals::default();

    macro_rules! both {
        (|$b:ident, $ctx:ident| $call:expr) => {{
            {
                let $b = &mut lua;
                let $ctx = &mut HostCtx {
                    view: &view,
                    rng: &mut rng_l,
                    cmd: &mut ql,
                };
                $call;
            }
            {
                let $b = &mut wasm;
                let $ctx = &mut HostCtx {
                    view: &view,
                    rng: &mut rng_w,
                    cmd: &mut qw,
                };
                $call;
            }
        }};
    }

    both!(|b, ctx| b.on_init(ctx));
    for t in 0..ticks {
        view.tick = t;
        // About one edge every 8 ticks, on switches 0..12.
        while input.next().is_multiple_of(8) {
            let sw = SwitchId((input.next() % 12) as u16);
            let closed = !view.switch_closed(sw);
            view.set_switch(sw, closed);
            let edge = if closed {
                SwitchEdge::Closed
            } else {
                SwitchEdge::Opened
            };
            tot.edges += 1;
            both!(|b, ctx| b.on_switch_edge(sw, edge, ctx));
        }
        both!(|b, ctx| b.on_tick(ctx));
        assert_eq!(ql, qw, "command queues differ at tick {t}");
        tot.lamps += ql.lamps.len();
        tot.sounds += ql.sounds.len();
        tot.scores += ql.scores.len();
        tot.holds += ql.coil_holds.len();
        tot.spawns += ql.ball_spawns.len();
        tot.persist += ql.persist_writes.len();
        tot.pulses += ql.coil_pulses.len();
        tot.magnets += ql.magnets.len();
        // Apply the effects the rules read back.
        for &(k, v) in &ql.persist_writes {
            view.persist[k.index()] = v;
        }
        view.balls_in_play = ((usize::from(view.balls_in_play) + ql.ball_spawns.len()) % 5) as u8;
        ql.clear();
        qw.clear();
    }
    assert!(!lua.is_faulted(), "{:?}", lua.last_error());
    assert!(!wasm.is_faulted(), "{:?}", wasm.last_error());
    assert_eq!(
        rng_l, rng_w,
        "the two backends drew differently from the PCG"
    );
    tot
}

#[test]
fn lua_and_wasm_emit_identical_commands_every_tick() {
    let tot = run_parity(0xC0FFEE, 3000);
    assert!(tot.edges >= 200, "only {} edges", tot.edges);
    for (what, n) in [
        ("lamps", tot.lamps),
        ("sounds", tot.sounds),
        ("scores", tot.scores),
        ("holds", tot.holds),
        ("spawns", tot.spawns),
        ("persist", tot.persist),
        ("pulses", tot.pulses),
        ("magnets", tot.magnets),
    ] {
        assert!(n > 0, "the fixture never exercised {what}");
    }
}

#[test]
fn parity_holds_for_other_seeds() {
    for seed in [1, 2, 3, 0xDEAD_BEEF] {
        run_parity(seed, 2000);
    }
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wasm")
}

/// True if the wasm32-unknown-unknown standard library is installed.
fn wasm_target_installed() -> bool {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    Command::new(rustc)
        .args(["--print", "sysroot"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .is_some_and(|root| {
            Path::new(&root)
                .join("lib/rustlib/wasm32-unknown-unknown")
                .is_dir()
        })
}

/// Rebuilds the committed `rules.wasm` from its source exactly as
/// `fixtures/wasm/rebuild.sh` does and compares the bytes. Skipped (with a
/// note) where the wasm32 target is not installed, e.g. in CI.
#[test]
fn committed_rules_wasm_matches_its_source() {
    if !wasm_target_installed() {
        println!("SKIP: wasm32-unknown-unknown is not installed; rules.wasm not rebuilt");
        return;
    }
    let dir = fixture_dir();
    let guest = dir.join("parity_guest");
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("guest-sdk");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("parity_guest");
    let canon = |p: &Path| p.canonicalize().unwrap().display().to_string();
    let flags = format!(
        "--remap-path-prefix={}=/pin_guest_sdk --remap-path-prefix={}=/parity_guest",
        canon(&sdk),
        canon(&guest)
    );
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "build",
            "--release",
            "--offline",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .arg("--manifest-path")
        .arg(guest.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .env("RUSTFLAGS", flags)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .expect("run cargo");
    assert!(status.success(), "building the parity guest failed");
    let built = std::fs::read(target.join("wasm32-unknown-unknown/release/parity_rules.wasm"))
        .expect("read the rebuilt module");
    assert!(
        built == RULES_WASM,
        "fixtures/wasm/rules.wasm is stale: run fixtures/wasm/rebuild.sh"
    );
}
