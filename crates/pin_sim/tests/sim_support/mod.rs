//! Shared helpers of the Subphase 2.5 implementer tests (`sim_*.rs`,
//! `replay_*.rs`).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// The in-repo replay fixture bundle (a folder).
pub fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/replay_table")
}

/// Runs `f` on a thread with 16 MiB of stack (wasmtime needs at least
/// `MIN_HOST_STACK_BYTES`; see `Simulation`).
pub fn on_sim_thread<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(f)
        .expect("spawn sim thread")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A scratch directory removed (with its contents) when dropped, also when
/// the test panics. Derefs to its [`Path`].
#[derive(Debug)]
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl std::ops::Deref for ScratchDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<std::ffi::OsStr> for ScratchDir {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.as_os_str()
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh, empty scratch directory under the system temp dir, removed when
/// the returned guard drops.
pub fn scratch_dir(tag: &str) -> ScratchDir {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("pin_sim_2_5_{tag}_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    ScratchDir(dir)
}

/// A copy of the fixture bundle in a scratch directory, with `edit`
/// applied to its files (`(name, Some(contents))` writes, `None` removes).
pub fn fixture_variant(tag: &str, edits: &[(&str, Option<&[u8]>)]) -> ScratchDir {
    let dir = scratch_dir(tag);
    for entry in std::fs::read_dir(fixture()).expect("read fixture") {
        let entry = entry.expect("dir entry");
        std::fs::copy(entry.path(), dir.join(entry.file_name())).expect("copy fixture file");
    }
    for (name, contents) in edits {
        match contents {
            Some(c) => std::fs::write(dir.join(name), c).expect("write variant file"),
            None => std::fs::remove_file(dir.join(name)).expect("remove variant file"),
        }
    }
    dir
}

/// The fixture with `rules.lua` replaced.
pub fn with_rules(tag: &str, lua: &str) -> ScratchDir {
    fixture_variant(tag, &[("rules.lua", Some(lua.as_bytes()))])
}

/// The fixture's `table.toml` text.
pub fn fixture_table_toml() -> String {
    std::fs::read_to_string(fixture().join("table.toml")).expect("read table.toml")
}

/// A Wasm (WAT text) rules module equivalent in spirit to the fixture's
/// `rules.lua`: launches a ball at init and on every trough edge, pulses
/// the bumper coil on ring edges, scores integer points, blinks a lamp.
pub const RULES_WAT: &str = r#"(module
  (import "pinball" "spawn_ball" (func $spawn (param i32 i32)))
  (import "pinball" "pulse_coil" (func $pulse (param i32 i32)))
  (import "pinball" "set_score" (func $score (param i32 i64)))
  (import "pinball" "set_lamp" (func $lamp (param i32 f32)))
  (import "pinball" "random" (func $random (param i64 i64) (result i64)))
  (memory (export "memory") 1)
  (global $points (mut i64) (i64.const 0))
  (global $t (mut i32) (i32.const 0))
  (func $add (param $p i64)
    (global.set $points (i64.add (global.get $points) (local.get $p)))
    (call $score (i32.const 1) (global.get $points)))
  (func (export "on_init")
    (call $score (i32.const 1) (i64.const 0))
    (call $spawn (i32.const 0) (i32.const 0)))
  (func (export "on_tick")
    (global.set $t (i32.add (global.get $t) (i32.const 1)))
    (if (i32.eqz (i32.rem_u (global.get $t) (i32.const 250)))
      (then (call $lamp (i32.const 1) (f32.const 1)))))
  (func (export "on_switch_edge") (param $id i32) (param $closed i32)
    (if (i32.eqz (local.get $closed)) (then (return)))
    (if (i32.eq (local.get $id) (i32.const 12))
      (then (call $pulse (i32.const 3) (i32.const 20)) (call $add (i64.const 100))))
    (if (i32.eq (local.get $id) (i32.const 14))
      (then (call $add (i64.add (i64.const 10) (call $random (i64.const 0) (i64.const 9))))))
    (if (i32.eq (local.get $id) (i32.const 13))
      (then (call $add (i64.const 1)) (call $spawn (i32.const 0) (i32.const 0))))))
"#;

/// The fixture with a Wasm rules module (`rules.wasm`, WAT text).
pub fn wasm_fixture(tag: &str) -> ScratchDir {
    let table = fixture_table_toml().replace(
        "script_entry = \"rules.lua\"",
        "script_entry = \"rules.wasm\"",
    );
    assert!(table.contains("rules.wasm"));
    fixture_variant(
        tag,
        &[
            ("table.toml", Some(table.as_bytes())),
            ("rules.lua", None),
            ("rules.wasm", Some(RULES_WAT.as_bytes())),
        ],
    )
}
