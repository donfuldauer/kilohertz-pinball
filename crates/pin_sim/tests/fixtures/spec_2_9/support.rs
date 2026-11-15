//! Shared helpers for the Subphase 2.9 spec tests (Independent Tester):
//! fixture bundles and plain-`Simulation` drivers. Black box: only the merged
//! `pin_sim` API of Subphase 2.5 (`Simulation`, `SimConfig`) is used here; the
//! autoplay helpers live in `soak_support.rs`.

#![allow(dead_code)]

use pin_sim::{SimConfig, Simulation};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Stack of the thread every simulation runs on.
pub const SIM_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Persist keys of the fixture rules (`[persist] keys` order).
pub const KEY_GAME_STATE: u16 = 0;
pub const KEY_BALL_TICKS: u16 = 1;
pub const KEY_GAMES_DONE: u16 = 2;
pub const KEY_BALLS_SERVED: u16 = 3;

/// Declared (non-cabinet) switches of the clean fixture.
pub const CLEAN_SWITCHES: [&str; 6] = [
    "top_opto",
    "mid_opto",
    "trough_1",
    "lower_left_eos",
    "lower_right_eos",
    "bumper_1_ring",
];

/// Centre of the trap pocket (trap geometry, `walls[5]`), mm: where a ball
/// rests against the pocket's bottom wall (y = 880 mm + ball radius).
pub const POCKET_MM: [f32; 2] = [255.0, 893.5];
/// The trap pocket's layout element (`walls[5]` of `trap_layout.toml`).
pub const POCKET_ELEMENT: &str = "walls[5]";

/// One second of simulated time, ticks.
pub const SECOND: u64 = 1000;
/// Stuck threshold of the brief: 1 cm/s.
pub const STUCK_SPEED_M_S: f32 = 0.01;

/// Runs `f` on a thread with a large stack and returns its result
/// (propagating panics).
pub fn on_sim_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let h = std::thread::Builder::new()
        .name("spec-2-9".into())
        .stack_size(SIM_STACK_BYTES)
        .spawn(f)
        .expect("spawn sim thread");
    match h.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    }
}

pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spec_2_9")
}

/// The committed bundle of a geometry (`clean`, `trap`, `gap`, `hidden`),
/// all with the clean rules.
pub fn geometry(name: &str) -> PathBuf {
    fixture_dir().join(name)
}

/// The committed clean bundle.
pub fn clean() -> PathBuf {
    geometry("clean")
}

/// The source of `scripts/<name>.lua`.
pub fn script(name: &str) -> String {
    std::fs::read_to_string(fixture_dir().join("scripts").join(format!("{name}.lua")))
        .unwrap_or_else(|e| panic!("read script {name}: {e}"))
}

/// A temporary directory removed when dropped. Derefs to its [`Path`].
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<std::ffi::OsStr> for TempDir {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.as_os_str()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh, empty directory under the cargo test tmpdir.
pub fn temp_dir(tag: &str) -> TempDir {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("spec_2_9-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

/// A temp copy of geometry `geo` whose `rules.lua` is `scripts/<script>.lua`.
pub fn variant(geo: &str, script_name: &str) -> TempDir {
    let dir = temp_dir(&format!("{geo}-{script_name}"));
    for entry in std::fs::read_dir(geometry(geo)).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    std::fs::write(dir.join("rules.lua"), script(script_name)).unwrap();
    dir
}

/// The impossible-shot defect: clean bundle plus `[[shots]] impossible_shot`,
/// `mid_opto` then `top_opto` (110 mm up the table) within 1 ms.
pub fn bad_shot_bundle() -> TempDir {
    let dir = variant("clean", "clean");
    let path = dir.join("table.toml");
    let toml = std::fs::read_to_string(&path).unwrap();
    let anchor = "[[modes]]";
    assert!(toml.contains(anchor), "clean table.toml has no [[modes]]");
    let shot = "[[shots]]\nid = \"impossible_shot\"\nsequence = [\"mid_opto\", \"top_opto\"]\nwindow_ms = 1\n\n";
    std::fs::write(&path, toml.replacen(anchor, &format!("{shot}{anchor}"), 1)).unwrap();
    dir
}

/// Every planted-defect bundle: (name, bundle).
pub fn defect_bundles() -> Vec<(&'static str, TempDir)> {
    vec![
        ("trap", variant("trap", "clean")),
        ("gap", variant("gap", "clean")),
        ("hidden", variant("hidden", "clean")),
        ("mode_error", variant("clean", "mode_error")),
        ("flood", variant("clean", "flood")),
        ("bad_shot", bad_shot_bundle()),
    ]
}

/// Loads a bundle with `seed`, panicking with the error text.
pub fn load(bundle: &Path, seed: u64) -> Simulation {
    match Simulation::load(
        bundle,
        SimConfig {
            seed,
            ..Default::default()
        },
    ) {
        Ok(s) => s,
        Err(e) => panic!("Simulation::load({}) failed: {e}", bundle.display()),
    }
}

/// Speed of a ball state, m/s.
pub fn speed(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Presses and releases cabinet switch `name` (one tick closed).
pub fn tap(sim: &mut Simulation, name: &str) {
    sim.set_switch(name, true).unwrap();
    sim.advance(1);
    sim.set_switch(name, false).unwrap();
    sim.advance(1);
    sim.release_switch(name).unwrap();
}

/// All numbers (integers or decimals, optionally signed) in `s`, in order.
pub fn numbers(s: &str) -> Vec<f64> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        if (b[i] == b'-' || b[i] == b'+') && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            i += 1;
        }
        if i < b.len() && b[i].is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if let Ok(v) = s[start..i].trim_end_matches('.').parse::<f64>() {
                out.push(v);
            }
        } else {
            i = start + 1;
        }
    }
    out
}

/// Whether `subject` locates the trap pocket: it names the pocket's layout
/// element, or contains an (x, y) pair within 40 mm of the pocket's resting
/// point, in mm or in m.
pub fn names_pocket(subject: &str) -> bool {
    if subject.contains(POCKET_ELEMENT) {
        return true;
    }
    let n = numbers(subject);
    n.windows(2).any(|w| {
        let near = |scale: f64| {
            let dx = w[0] * scale - f64::from(POCKET_MM[0]);
            let dy = w[1] * scale - f64::from(POCKET_MM[1]);
            dx.hypot(dy) <= 40.0
        };
        near(1.0) || near(1000.0)
    })
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
