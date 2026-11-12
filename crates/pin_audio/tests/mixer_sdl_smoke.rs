//! SDL smoke test (dummy audio driver): `SdlOutput` opens, its callback runs
//! the mixer at least once, and `close` hands the mixer back.
//!
//! SDL may only be initialised once per process and on the main thread
//! (macOS), so this is a `harness = false` binary. The only wait is a bounded
//! poll on the callback counter (not a timing assertion).

use std::sync::Arc;
use std::time::{Duration, Instant};

use pin_audio::mixer::{Bus, Mixer, MixerConfig, Sample, SdlOutput, SoundTrigger, trigger_ring};
use pin_core::SoundId;

fn main() {
    // SAFETY: set before any SDL call or other thread exists.
    unsafe { std::env::set_var("SDL_AUDIO_DRIVER", "dummy") };
    let sdl = sdl3::init().expect("SDL init");
    let audio = sdl.audio().expect("audio subsystem");
    let (mut tx, rx) = trigger_ring(16);
    let mut mixer = Mixer::new(MixerConfig::default(), rx);
    mixer.register(
        SoundId(1),
        Arc::new(Sample::constant(48_000, 0.1, 0.1)),
        Bus::Sfx,
    );
    assert!(tx.send(SoundTrigger {
        sound: SoundId(1),
        gain: 1.0
    }));

    let out = SdlOutput::open(&audio, mixer).expect("open SdlOutput");
    let deadline = Instant::now() + Duration::from_secs(20);
    while out.callbacks() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let calls = out.callbacks();
    let mixer = out.close();
    assert!(calls >= 1, "audio callback never ran");
    assert!(mixer.frames_rendered() > 0, "mixer rendered nothing");
    assert_eq!(mixer.active_voices(Bus::Sfx), 1);
    println!(
        "[mixer_sdl_smoke] ok: {calls} callbacks, {} frames",
        mixer.frames_rendered()
    );
}
