//! Independent Tester, KP-086 (#180): real-browser evidence for the web
//! audio worklet (REQ-WEB-022, D43; R12).
//!
//! Each test computes the expected FNV-1a 64 digests natively here (own
//! driver of `pin_audio::Mixer`, `spec_kp086/oracle.rs`, never the crate
//! under test), writes the PCM and a config into a fixture directory, builds
//! the wasm (`--target wasm32-unknown-unknown`, own target dir under
//! `$CARGO_TARGET_DIR/wasm_ver`) and runs `spec_kp086/run.cjs` in Node, which
//! serves `www/`, the wasm and the test page from 127.0.0.1 under the D43
//! CSP and drives Chromium (OfflineAudioContext, 128-frame quanta) through
//! Playwright. The digests the page computed over the rendered
//! `AudioBuffer` are asserted here against the native ones.
//!
//! Skip rule: without node, `require('playwright')`, a Chromium launch or a
//! wasm build the tests print why and pass; with `PIN_REQUIRE_BROWSER=1`
//! they fail instead.

#[path = "spec_kp086/oracle.rs"]
mod oracle;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use oracle::*;

const QUANTA: u32 = 600;
/// The protocol test's late record: posted while the offline context is
/// suspended at quantum 375 (exactly 1.0 s at 48 kHz) with `at = 10`, i.e.
/// in the past, so it applies at quantum 375.
const LATE_QUANTUM: u32 = 375;
const LATE_AT: u32 = 10;
const LATE_ID: u16 = 1;
const LATE_GAIN: f32 = 0.8;

fn require_browser() -> bool {
    std::env::var("PIN_REQUIRE_BROWSER").is_ok_and(|v| v == "1")
}

/// Skips (prints and returns) or fails per the skip rule.
fn skip_or_fail(why: &str) {
    assert!(
        !require_browser(),
        "PIN_REQUIRE_BROWSER=1 but the browser test cannot run: {why}"
    );
    println!("SKIP (REQ-WEB-022 browser test): {why}");
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest().join("../../target"))
}

/// Builds the wasm once per process; `Err` explains why it is unavailable.
fn wasm() -> Result<PathBuf, String> {
    static W: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    W.get_or_init(|| {
        let tdir = target_dir().join("wasm_ver");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut cmd = Command::new(cargo);
        cmd.current_dir(manifest().join("../.."))
            .args([
                "build",
                "-p",
                "pin_web_audio",
                "--release",
                "--target",
                "wasm32-unknown-unknown",
                "--target-dir",
            ])
            .arg(&tdir);
        // Coverage or other host flags must not leak into the wasm build.
        for v in [
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_BUILD_RUSTFLAGS",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "LLVM_PROFILE_FILE",
            "CARGO_LLVM_COV",
            "CARGO_LLVM_COV_TARGET_DIR",
        ] {
            cmd.env_remove(v);
        }
        let o = cmd
            .output()
            .map_err(|e| format!("cargo not runnable: {e}"))?;
        if !o.status.success() {
            return Err(format!(
                "wasm build failed: {}",
                String::from_utf8_lossy(&o.stderr)
            ));
        }
        let p = tdir.join("wasm32-unknown-unknown/release/pin_web_audio.wasm");
        if p.is_file() {
            Ok(p)
        } else {
            Err(format!("wasm artefact missing at {}", p.display()))
        }
    })
    .clone()
}

fn hex(d: u64) -> String {
    format!("{d:016x}")
}

fn json_str(p: &Path) -> String {
    format!("{:?}", p.display().to_string())
}

/// Writes PCM files and config.json; returns the config path.
fn write_fixture(mode: &str, wasm: &Path) -> PathBuf {
    let dir = target_dir().join("spec_kp086_fixture").join(mode);
    std::fs::create_dir_all(&dir).unwrap();
    let mut sounds = String::new();
    for (i, s) in s6_sounds().iter().enumerate() {
        let file = format!("pcm_{}.bin", s.id);
        let bytes: Vec<u8> = s.pcm.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(dir.join(&file), bytes).unwrap();
        if i > 0 {
            sounds.push(',');
        }
        write!(
            sounds,
            r#"{{"id":{},"bus":{},"channels":{},"file":"{file}"}}"#,
            s.id, s.bus, s.channels
        )
        .unwrap();
    }
    let triggers: Vec<String> = s6_triggers()
        .iter()
        .map(|(q, id, g)| format!("[{q},{id},{}]", g.to_bits()))
        .collect();
    let cfg = format!(
        r#"{{"mode":"{mode}","quanta":{QUANTA},"pageDir":{},"wwwDir":{},"fixtureDir":{},"wasmPath":{},"sounds":[{sounds}],"triggers":[{}],"late":{{"quantum":{LATE_QUANTUM},"at":{LATE_AT},"id":{LATE_ID},"gainBits":{}}}}}"#,
        json_str(&manifest().join("tests/spec_kp086/page")),
        json_str(&manifest().join("www")),
        json_str(&dir),
        json_str(wasm),
        triggers.join(","),
        LATE_GAIN.to_bits(),
    );
    let path = dir.join("config.json");
    std::fs::write(&path, cfg).unwrap();
    path
}

/// Output of one runner invocation, or `None` when skipped.
struct Run {
    stdout: String,
}

impl Run {
    /// `(name, actual digest)` pairs printed by the runner.
    fn digests(&self) -> Vec<(String, String)> {
        self.stdout
            .lines()
            .filter_map(|l| l.strip_prefix("KP086 digest "))
            .map(|j| (field(j, "name"), field(j, "actual")))
            .collect()
    }
}

fn field(json: &str, key: &str) -> String {
    let pat = format!("\"{key}\":\"");
    let s = json
        .find(&pat)
        .map(|i| &json[i + pat.len()..])
        .unwrap_or("");
    s.split('"').next().unwrap_or("").to_string()
}

fn run_browser(mode: &str) -> Option<Run> {
    let wasm = match wasm() {
        Ok(p) => p,
        Err(e) => {
            skip_or_fail(&e);
            return None;
        }
    };
    if Command::new("node").arg("--version").output().is_err() {
        skip_or_fail("node is not installed");
        return None;
    }
    let cfg = write_fixture(mode, &wasm);
    let o = Command::new("node")
        .arg(manifest().join("tests/spec_kp086/run.cjs"))
        .arg(&cfg)
        .output()
        .expect("spawn node");
    let stdout = String::from_utf8_lossy(&o.stdout).to_string();
    let stderr = String::from_utf8_lossy(&o.stderr).to_string();
    println!("{stdout}");
    if !stderr.is_empty() {
        println!("node stderr:\n{stderr}");
    }
    match o.status.code() {
        Some(77) => {
            skip_or_fail(&format!("browser environment unavailable:\n{stdout}"));
            None
        }
        Some(0) => Some(Run { stdout }),
        c => panic!("browser runner ({mode}) failed with {c:?}:\n{stdout}\n{stderr}"),
    }
}

fn assert_digests(run: &Run, expected: &[(&str, u64)]) {
    let got = run.digests();
    for (name, want) in expected {
        let actual = got
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d.clone())
            .unwrap_or_else(|| panic!("no digest printed for {name}"));
        println!(
            "REQ-WEB-022 digest {name}: native expected {} browser actual {actual}",
            hex(*want)
        );
        assert_eq!(actual, hex(*want), "{name}: browser digest != native");
    }
}

/// REQ-WEB-022, D43: the S6 script rendered by the worklet in a Chromium
/// OfflineAudioContext (600 quanta of 128 frames, stereo) has the FNV-1a
/// digest of native `Mixer::render` at 48 000 Hz and at 44 100 Hz; the same
/// through `createWebAudio` (worklet mode), through the forced fallback
/// (`prerender(600)`) and through an own main-thread ABI driver posting
/// planar blocks to `pin-web-audio-fallback`. The wasm has no imports and
/// exports `memory` and the `pwa_*` ABI; zero console and page errors under
/// the D43 CSP.
#[test]
fn req_web_022_d43_browser_worklet_digest_equals_native() {
    let e48 = fnv1a(&native_script(48_000, &s6_sounds(), &s6_triggers(), QUANTA));
    let e44 = fnv1a(&native_script(44_100, &s6_sounds(), &s6_triggers(), QUANTA));
    assert_ne!(e48, e44);
    println!("native S6 digest 48000 Hz: {}", hex(e48));
    println!("native S6 digest 44100 Hz: {}", hex(e44));
    let Some(run) = run_browser("digest") else {
        return;
    };
    assert_digests(
        &run,
        &[
            ("worklet_48000", e48),
            ("worklet_44100", e44),
            ("helper_worklet_48000", e48),
            ("helper_fallback_48000", e48),
            ("own_fallback_48000", e48),
        ],
    );
}

/// REQ-WEB-022: the message protocol in Chromium. A `cmds` record with `at`
/// in the past (posted while the offline context is suspended at quantum
/// 375) applies at the current quantum: digest equals native S6 plus that
/// trigger at 375. Malformed and unknown messages do not throw and do not
/// change the digest. `register` replies with the `pwa_register` codes
/// (-3 channels 3, -5 over the `budgetMiB` budget, 0 exactly at it, -2 bus
/// 3, -1 id 65536); budget used and limit are 1 MiB.
#[test]
fn req_web_022_browser_message_protocol() {
    let e48 = fnv1a(&native_script(48_000, &s6_sounds(), &s6_triggers(), QUANTA));
    let mut late_trig = s6_triggers();
    late_trig.push((LATE_QUANTUM, LATE_ID, LATE_GAIN));
    let late = fnv1a(&native_script(48_000, &s6_sounds(), &late_trig, QUANTA));
    assert_ne!(late, e48);
    let Some(run) = run_browser("protocol") else {
        return;
    };
    assert_digests(&run, &[("late_48000", late), ("junk_48000", e48)]);
}

/// REQ-WEB-022 (W22 fallback): a garbage wasm makes the processor post one
/// `{type:'error', error: String}` (never `ready`) and output silence, and
/// `createWebAudio` rejects with a non-empty string message (bounded wait; the
/// fallback runs the same module, so it cannot rescue invalid wasm); with good
/// wasm it selects `worklet`. When `worklet.js` cannot load (served 404) the helper falls
/// back and the fallback output still equals native S6 bit for bit.
#[test]
fn req_web_022_browser_bad_wasm_rejects() {
    let e48 = fnv1a(&native_script(48_000, &s6_sounds(), &s6_triggers(), QUANTA));
    let Some(run) = run_browser("badwasm") else {
        return;
    };
    assert_digests(&run, &[("noworklet_fallback_48000", e48)]);
}

/// REQ-WEB-022 (starts on the first user gesture): with a realtime
/// `AudioContext` under Chromium's default autoplay policy,
/// `resumeOnGesture(ctx, document)` leaves it `suspended` before input and
/// `running` after a real `page.mouse.click`; `logLatency(ctx)` logs one
/// `pin_web_audio latency: baseLatency=` line and returns `baseLatency` and
/// `outputLatency` (number or null).
#[test]
fn req_web_022_browser_gesture_and_latency() {
    let Some(run) = run_browser("gesture") else {
        return;
    };
    assert!(run.stdout.contains("KP086 failures []"));
}
