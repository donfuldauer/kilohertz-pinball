//! Spec test (Independent Tester, Subphase 3.3), criterion 7: SDL smoke test.
//!
//! `harness = false`: SDL may only be initialised once per process and on the
//! main thread (macOS). Uses SDL's dummy audio driver, opens `SdlOutput`,
//! waits (bounded) until the audio callback has provably run, closes, and
//! checks the returned `Mixer`.
//!
//! "The callback ran" is observed through the fixed API only: `Mixer::render`
//! drains the trigger ring, so once more triggers have been accepted than the
//! ring can hold, a render call must have happened on the audio thread. The
//! bounded wait is the only non-simulated wait and is not a timing assertion.

#[path = "spec_fixtures/util.rs"]
mod util;

use pin_audio::mixer::{Bus, Mixer, MixerConfig, SdlOutput, SoundTrigger, trigger_ring};
use pin_core::SoundId;
use std::time::{Duration, Instant};
use util::*;

const RING: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(20);

fn check(name: &str, ok: bool) {
    assert!(ok, "check failed: {name}");
    println!("[spec_mixer_sdl] ok: {name}");
}

fn main() {
    sdl3::hint::set("SDL_AUDIO_DRIVER", "dummy");
    let sdl = sdl3::init().expect("SDL init");
    let audio = sdl.audio().expect("SDL audio subsystem (dummy driver)");
    println!(
        "[spec_mixer_sdl] audio driver: {}",
        audio.current_audio_driver()
    );
    check("dummy audio driver", audio.current_audio_driver() == "dummy");

    let cfg = MixerConfig::default();
    let (mut tx, rx) = trigger_ring(RING);
    let mut mixer = Mixer::new(cfg, rx);
    // 120 s of DC on the music bus: still playing when the mixer comes back.
    mixer.register(SoundId(1), dc(0.25, -0.125, 120 * RATE as usize, RATE), Bus::Music);
    check(
        "music trigger queued before open",
        tx.send(SoundTrigger {
            sound: SoundId(1),
            gain: 1.0,
        }),
    );

    let output = match SdlOutput::open(&audio, mixer) {
        Ok(o) => o,
        Err(e) => panic!("SdlOutput::open failed: {e}"),
    };
    println!("[spec_mixer_sdl] ok: SdlOutput opened");

    // Keep sending unknown sounds; the ring holds RING, so accepting more
    // than RING in total proves the callback drained it at least once.
    let start = Instant::now();
    let mut accepted = 1usize; // the music trigger
    let mut next = 100u16;
    while accepted <= 2 * RING {
        if tx.send(SoundTrigger {
            sound: SoundId(next),
            gain: 1.0,
        }) {
            accepted += 1;
            next += 1;
        } else {
            assert!(
                start.elapsed() < TIMEOUT,
                "audio callback did not run within {TIMEOUT:?} (accepted {accepted} triggers)"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    println!("[spec_mixer_sdl] ok: callback ran (ring drained, {accepted} triggers accepted)");

    let mut mixer: Mixer = output.close();
    println!("[spec_mixer_sdl] ok: closed, mixer returned");

    // The callback really went through Mixer::render: it consumed triggers.
    check(
        "callback counted the unknown sounds it drained",
        mixer.unknown_sounds() >= (accepted - 1 - RING) as u64 && mixer.unknown_sounds() >= 1,
    );
    check(
        "music voice started by the callback is still active",
        mixer.active_voices(Bus::Music) == 1,
    );

    // After close the callback is gone: the mixer is ours again and renders
    // offline, deterministically, with the voice the callback started.
    let unknown_before = mixer.unknown_sounds();
    let mut out = [9.0f32; 2 * 64];
    mixer.render(&mut out);
    assert_all_frames(&out, 0.25, -0.125, 1e-6, "offline render after close");
    println!("[spec_mixer_sdl] ok: offline render after close continues the music voice");
    check(
        "pending triggers are drained by the offline render",
        mixer.unknown_sounds() as usize == accepted - 1,
    );
    check("nothing dropped", tx.dropped() == 0);
    check("something was drained by the callback", unknown_before >= 1);

    // The subsystem is still usable: open and close a second time.
    let output = match SdlOutput::open(&audio, mixer) {
        Ok(o) => o,
        Err(e) => panic!("second SdlOutput::open failed: {e}"),
    };
    let mixer = output.close();
    check(
        "second open/close returns the same mixer",
        mixer.active_voices(Bus::Music) == 1,
    );
    println!("[spec_mixer_sdl] all checks passed");
}
