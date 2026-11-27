//! REQ-WEB-022 / D43 real-browser evidence (R12): Chromium renders the S6
//! script through the `pin-web-audio` AudioWorklet (and through the
//! main-thread fallback) in an OfflineAudioContext; the FNV-1a digest of the
//! 600 × 128 stereo frames must equal native `Mixer::render`. The page is
//! served from 127.0.0.1 under the strict CSP; the wasm has no imports; every
//! quantum is 128 frames; no console or page errors; the realtime context
//! starts only on the first gesture and the latency line is logged.
//!
//! Skips with a printed reason when node, playwright, Chromium or the wasm
//! build is unavailable, unless `PIN_REQUIRE_BROWSER=1`, which turns every
//! skip into a failure.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use pin_audio::{Bus, Mixer, MixerConfig, Sample, SoundTrigger, trigger_ring};
use pin_core::SoundId;

fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 40) as i32 - (1 << 23)) as f32 / (1u32 << 23) as f32 * 0.5
        })
        .collect()
}

const SCRIPT: &[(u32, u16, f32)] = &[
    (0, 2, 0.6),
    (3, 1, 0.9),
    (10, 1, 0.7),
    (11, 3, 1.0),
    (40, 1, 1.2),
    (41, 1, 0.3),
    (42, 1, 0.5),
    (90, 3, 0.8),
    (91, 2, 0.4),
    (200, 1, 1.0),
    (201, 7, 1.0),
    (300, 1, f32::NAN),
];

/// The native digest of the S6 script (48 kHz, 128-frame quanta, 600 quanta).
fn native_digest(pcm: &[(u16, Bus, Sample)]) -> u64 {
    let (mut tx, rx) = trigger_ring(64);
    let cfg = MixerConfig {
        sample_rate: 48_000,
        buffer_frames: 128,
        ..MixerConfig::default()
    };
    let mut m = Mixer::new(cfg, rx);
    for (id, bus, s) in pcm {
        m.register(SoundId(*id), Arc::new(s.clone()), *bus);
    }
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut out = vec![0.0f32; 256];
    for q in 0..600 {
        for &(at, id, gain) in SCRIPT {
            if at == q {
                tx.send(SoundTrigger {
                    sound: SoundId(id),
                    gain,
                });
            }
        }
        m.render(&mut out);
        for x in &out {
            for b in x.to_bits().to_le_bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x100000001b3);
            }
        }
    }
    h
}

fn require() -> bool {
    std::env::var("PIN_REQUIRE_BROWSER").is_ok_and(|v| v == "1")
}

/// Skips (returns) with a message, or panics under `PIN_REQUIRE_BROWSER=1`.
fn unavailable(why: &str) {
    if require() {
        panic!("PIN_REQUIRE_BROWSER=1 but the browser test cannot run: {why}");
    }
    println!("SKIPPED wa_browser (set PIN_REQUIRE_BROWSER=1 to fail instead): {why}");
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Builds the release wasm into its own target dir (no outer lock).
fn build_wasm() -> Result<PathBuf, String> {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest().join("../../target"))
        .join("wasm");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(manifest())
        .args([
            "build",
            "-p",
            "pin_web_audio",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--target-dir",
        ])
        .arg(&target)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_LLVM_COV")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("wasm build failed ({status})"));
    }
    let wasm = target.join("wasm32-unknown-unknown/release/pin_web_audio.wasm");
    if wasm.is_file() {
        Ok(wasm)
    } else {
        Err(format!("{} missing after the build", wasm.display()))
    }
}

fn write_f32(path: &Path, xs: &[f32]) {
    let bytes: Vec<u8> = xs.iter().flat_map(|x| x.to_le_bytes()).collect();
    std::fs::write(path, bytes).unwrap();
}

/// REQ-WEB-022 / D43: worklet and fallback digests equal native in Chromium;
/// CSP, no imports, 128-frame quanta, zero errors, gesture start, latency.
#[test]
fn req_web_022_chromium_offline_digest_and_gesture() {
    let pcm = vec![
        (1u16, Bus::Sfx, Sample::from_mono_vec(noise(9_600, 7))),
        (2, Bus::Music, Sample::from_mono_vec(noise(24_000, 11))),
        (3, Bus::Callout, Sample::constant(4_800, 0.25, -0.125)),
    ];
    let expected = format!("{:016x}", native_digest(&pcm));
    println!("native digest: {expected}");

    match Command::new("node").arg("--version").output() {
        Ok(o) if o.status.success() => {}
        _ => return unavailable("node is not on PATH"),
    }
    let wasm = match build_wasm() {
        Ok(w) => w,
        Err(e) => return unavailable(&e),
    };
    let data =
        std::env::temp_dir().join(format!("pin_web_audio_wa_browser_{}", std::process::id()));
    std::fs::create_dir_all(&data).unwrap();
    std::fs::copy(&wasm, data.join("pin_web_audio.wasm")).unwrap();
    for (id, _, s) in &pcm {
        write_f32(&data.join(format!("pcm{id}.f32")), s.data());
    }
    let out = Command::new("node")
        .arg(manifest().join("tests/browser/wa_run.cjs"))
        .arg(manifest().join("www"))
        .arg(manifest().join("tests/browser"))
        .arg(&data)
        .arg(&expected)
        .output()
        .expect("spawn node");
    let _ = std::fs::remove_dir_all(&data);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    println!("{stdout}");
    eprintln!("{stderr}");
    match out.status.code() {
        Some(0) => assert!(stdout.contains("PASS")),
        Some(2) => unavailable(stdout.trim()),
        _ => panic!("Chromium test failed:\n{stdout}\n{stderr}"),
    }
}
