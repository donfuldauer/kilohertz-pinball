//! Shared helpers for the Subphase 2.5 spec tests (Independent Tester).
//!
//! Black box: only the brief's pinned API (`pin_sim::{Simulation, SimConfig,
//! BallHandle, SimError}`, `pin_sim::replay::{Recording, replay}`, the
//! `pin_sim` binary) and merged dependency contracts (`pin_core`) are used.

#![allow(dead_code)]

use pin_core::input::{CabinetInput, InputEvent};
use pin_core::sim::{SimStep, tick_end_ns};
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::replay::Recording;
use pin_sim::{SimConfig, Simulation};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Stack of the thread every simulation runs on (≥ `MIN_HOST_STACK_BYTES`).
pub const SIM_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Default seed pinned by the brief.
pub const DEFAULT_SEED: u64 = 0x4B485A;

/// Player numbers of the fixture script's four reports (see rules.lua).
/// `Simulation::score` uses the script's 1-based numbering (DECISIONS "2.5 —
/// tick order details, switch composition and test API semantics").
pub const P_SCORE: u8 = 1;
pub const P_LANE_AT: u8 = 2;
pub const P_ACTION_AT: u8 = 3;
pub const P_RNG: u8 = 4;

/// Ball radius of `standard_steel`, m.
pub const R: f32 = 0.0135;
/// Dispenser `feed` exit position / velocity (SI).
pub const FEED_POS: [f32; 3] = [0.250, 0.950, 0.0135];
pub const FEED_VEL: [f32; 3] = [0.01, -0.6, 0.0];

/// Runs `f` on a thread with a large stack and returns its result
/// (propagating panics).
pub fn on_sim_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let h = std::thread::Builder::new()
        .name("spec-sim".into())
        .stack_size(SIM_STACK_BYTES)
        .spawn(f)
        .expect("spawn sim thread");
    match h.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    }
}

pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spec_2_5")
}

/// The committed fixture bundle (directory form).
pub fn bundle() -> PathBuf {
    fixture_dir().join("table")
}

/// A fresh, empty directory under the cargo test tmpdir.
pub fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("spec_2_5-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copies the fixture bundle into a fresh temp directory and returns it.
pub fn bundle_copy(tag: &str) -> PathBuf {
    let dir = temp_dir(tag);
    for entry in std::fs::read_dir(bundle()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    dir
}

/// A copy of the fixture bundle with `rules.lua` replaced.
pub fn bundle_with_script(tag: &str, lua: &str) -> PathBuf {
    let dir = bundle_copy(tag);
    std::fs::write(dir.join("rules.lua"), lua).unwrap();
    dir
}

/// A copy of the fixture bundle whose rules are the given WAT text
/// (`script_entry = "rules.wasm"`).
pub fn bundle_with_wasm(tag: &str, wat: &str) -> PathBuf {
    let dir = bundle_copy(tag);
    std::fs::remove_file(dir.join("rules.lua")).unwrap();
    std::fs::write(dir.join("rules.wasm"), wat).unwrap();
    let toml = std::fs::read_to_string(dir.join("table.toml")).unwrap();
    assert!(toml.contains("script_entry = \"rules.lua\""));
    std::fs::write(
        dir.join("table.toml"),
        toml.replace(
            "script_entry = \"rules.lua\"",
            "script_entry = \"rules.wasm\"",
        ),
    )
    .unwrap();
    dir
}

pub fn cfg(seed: u64) -> SimConfig {
    SimConfig {
        seed,
        ..Default::default()
    }
}

/// Loads the bundle, panicking with the error's Display text.
pub fn load_at(bundle: &Path, seed: u64) -> Simulation {
    match Simulation::load(bundle, cfg(seed)) {
        Ok(s) => s,
        Err(e) => panic!("Simulation::load({}) failed: {e}", bundle.display()),
    }
}

/// The fixture with the default configuration.
pub fn load() -> Simulation {
    match Simulation::load(&bundle(), SimConfig::default()) {
        Ok(s) => s,
        Err(e) => panic!("Simulation::load(fixture) failed: {e}"),
    }
}

/// A button event stamped inside tick `tick` (epoch 0).
pub fn button(tick: u64, input: CabinetInput, pressed: bool) -> InputEvent {
    InputEvent::button(tick_end_ns(0, tick) - 500_000, input, pressed)
}

/// Steps one tick through the `SimStep` contract with the given inputs.
pub fn step(sim: &mut Simulation, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
    let tick = sim.tick() + 1;
    sim.step_1ms(tick, inputs, out);
}

/// Deterministic input script used by the replay tests: `start` pressed at
/// tick 5 and then every 1500 ticks (a ball is dispensed each time), flippers
/// pressed pseudo-randomly, `action` now and then. Events are sorted by tick,
/// at most a few per tick, every tick in `1..=ticks`.
pub fn input_script(ticks: u64, salt: u64) -> Vec<(u64, InputEvent)> {
    let mut ev = Vec::new();
    let mut lcg = 0x9E37_79B9_7F4A_7C15u64 ^ salt;
    let mut next = || {
        lcg = lcg
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (lcg >> 33) as u32
    };
    // (tick, input, pressed)
    let mut raw: Vec<(u64, CabinetInput, bool)> = Vec::new();
    let mut t = 5;
    while t + 60 <= ticks {
        raw.push((t, CabinetInput::Start, true));
        raw.push((t + 40, CabinetInput::Start, false));
        t += 1500;
    }
    for (input, phase) in [
        (CabinetInput::LeftFlipper, 0u64),
        (CabinetInput::RightFlipper, 37),
    ] {
        let mut t = 100 + phase;
        loop {
            let hold = 20 + u64::from(next() % 180);
            if t + hold >= ticks {
                break;
            }
            raw.push((t, input, true));
            raw.push((t + hold, input, false));
            t += hold + 30 + u64::from(next() % 400);
        }
    }
    let mut t = 777;
    while t + 10 <= ticks {
        raw.push((t, CabinetInput::Action, true));
        raw.push((t + 9, CabinetInput::Action, false));
        t += 2111;
    }
    raw.sort_by_key(|r| r.0);
    for (tick, input, pressed) in raw {
        ev.push((tick, button(tick, input, pressed)));
    }
    ev
}

/// Drives a fresh simulation through `events` for `ticks` ticks via
/// `SimStep::step_1ms` (the events of tick `t` are passed to the step of tick
/// `t`) and returns it.
pub fn drive(bundle: &Path, seed: u64, ticks: u64, events: &[(u64, InputEvent)]) -> Simulation {
    let mut sim = load_at(bundle, seed);
    let mut out = Box::new(FrameStateSnapshot::default());
    let mut buf: Vec<InputEvent> = Vec::with_capacity(16);
    let mut i = 0;
    for tick in 1..=ticks {
        buf.clear();
        while i < events.len() && events[i].0 == tick {
            buf.push(events[i].1);
            i += 1;
        }
        sim.step_1ms(tick, &buf, &mut out);
    }
    assert_eq!(i, events.len(), "input script has events outside 1..=ticks");
    sim
}

/// The recording equivalent to [`drive`] with the same arguments.
pub fn recording(
    table_hash: u64,
    seed: u64,
    ticks: u64,
    events: &[(u64, InputEvent)],
) -> Recording {
    Recording {
        seed,
        table_hash,
        ticks,
        events: events.to_vec(),
    }
}

// ------------------------------------------------------------------ CLI

/// Extracts the hash from a `state_hash=<16 hex>` line of `stdout`.
pub fn parse_state_hash(stdout: &str) -> Option<u64> {
    for line in stdout.lines() {
        if let Some(hex) = line.trim().strip_prefix("state_hash=")
            && hex.len() == 16
            && hex.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return u64::from_str_radix(hex, 16).ok();
        }
    }
    None
}

// ------------------------------------------------------------------ zip

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Writes the files of directory `dir` (flat) as a stored (uncompressed) zip.
pub fn zip_dir(dir: &Path, zip_path: &Path) {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for name in &names {
        let data = std::fs::read(dir.join(name)).unwrap();
        let crc = crc32(&data);
        let offset = out.len() as u32;
        let len = data.len() as u32;
        // Local file header.
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0x21u16.to_le_bytes()); // date 1980-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&data);
        // Central directory entry.
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        central.extend_from_slice(&20u16.to_le_bytes()); // needed
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(names.len() as u16).to_le_bytes());
    out.extend_from_slice(&(names.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    std::fs::write(zip_path, out).unwrap();
}
