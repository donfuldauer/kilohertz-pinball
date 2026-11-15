//! Subphase 6.0 wiring (PR #21 review #12): the sim's sound commands reach
//! the 3.3 mixer and the mixed 7.1 output (mixer + SSF) reaches the audio
//! sink. Offline only (D31); simulated scheduling and joins, no sleeps (D8).

mod support;

use pin_app::orchestrator::{
    AUDIO_BLOCK_FRAMES, AudioConfig, AudioEngine, IdleSim, OfflineSink, SOUND_RING_CAPACITY,
    TableSim, ThreadsConfig, load_checked, spawn_threads, table_samples, validate_table,
};
use pin_audio::mixer::{Bus, MixerConfig, Sample, SoundTrigger, trigger_ring};
use pin_audio::ssf::{CH_FL, CH_FR, CHANNELS, SsfConfig};
use pin_core::{FrameStateSnapshot, SimStep, SoundId};
use pin_sim::SimConfig;
use std::sync::Arc;
use support::{greybox_bundle, on_big_stack};

fn step(t: &mut TableSim, n: u64, out: &mut FrameStateSnapshot) {
    for _ in 0..n {
        let tick = t.sim().tick() + 1;
        t.step_1ms(tick, &[], out);
    }
}

fn peak(block: &[f32]) -> f32 {
    block.iter().fold(0.0f32, |m, &x| m.max(x.abs()))
}

#[test]
fn a_bumper_sound_command_is_audible_within_one_block() {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        let checked = validate_table(&dir).unwrap();
        let (samples, errors) = table_samples(&checked, MixerConfig::default().sample_rate);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(samples.len(), 2, "sfx_bumper and sfx_sling decode");
        let bumper = checked.table.ids.sound("sfx_bumper").unwrap();
        let (stx, srx) = trigger_ring(SOUND_RING_CAPACITY);
        let mut table = TableSim::new(load_checked(&checked, SimConfig::default()).unwrap(), None)
            .with_sounds(stx);
        let mut engine = AudioEngine::new(
            SsfConfig::default(),
            MixerConfig::default(),
            Some(srx),
            &samples,
        );
        let mut out = FrameStateSnapshot::default();
        // Attract mode: no sound yet, the output is exact silence.
        step(&mut table, 50, &mut out);
        assert_eq!(peak(engine.render_block()), 0.0);
        // Start a game, then hit bumper 1.
        table.sim_mut().set_switch("start", true).unwrap();
        step(&mut table, 5, &mut out);
        table.sim_mut().release_switch("start").unwrap();
        step(&mut table, 1500, &mut out);
        assert_eq!(peak(engine.render_block()), 0.0, "no sound before the hit");
        table.sim_mut().set_switch("bumper_1_ring", true).unwrap();
        let mut heard_at = None;
        for tick in 0..10 {
            step(&mut table, 1, &mut out);
            if table
                .sim()
                .sound_commands()
                .iter()
                .any(|&(s, _)| s == bumper)
            {
                // The trigger is in the ring: the next block plays it.
                let b = engine.render_block();
                let front = b
                    .chunks_exact(CHANNELS)
                    .fold(0.0f32, |m, f| m.max(f[CH_FL].abs()).max(f[CH_FR].abs()));
                heard_at = Some((tick, front));
                break;
            }
        }
        let (_, front) = heard_at.expect("bumper sound commanded within 10 ticks");
        assert!(front > 1e-3, "front channels silent: {front}");
        assert!(engine.peak_voices() >= 1);
        assert_eq!(table.sounds_dropped(), 0);
        assert_eq!(engine.mixer().unknown_sounds(), 0);
        assert_eq!(
            engine.mixer().frames_rendered(),
            3 * AUDIO_BLOCK_FRAMES as u64
        );
        assert!(!table.sim().script_faulted());
    });
}

#[test]
fn the_audio_thread_mixes_triggers_into_the_offline_sink() {
    let (mut stx, srx) = trigger_ring(8);
    let (sink, recording) = OfflineSink::new(64 * AUDIO_BLOCK_FRAMES);
    let (_in_tx, in_rx) = pin_core::spsc::channel(4);
    let mut cfg = ThreadsConfig::with_sim(Box::new(IdleSim), in_rx);
    cfg.audio = AudioConfig {
        sounds: Some(srx),
        samples: vec![(
            SoundId(0),
            Arc::new(Sample::constant(4 * AUDIO_BLOCK_FRAMES, 0.25, 0.25)),
            Bus::Sfx,
        )],
        sink: Some(Box::new(sink)),
        ..AudioConfig::default()
    };
    // Queued before the thread starts: its first block plays it.
    assert!(stx.send(SoundTrigger {
        sound: SoundId(0),
        gain: 1.0
    }));
    let threads = spawn_threads(cfg).unwrap();
    // Wait (no sleep, no clock) until the audio thread rendered a block.
    while threads.audio_stats().mixer_peak_voices == 0 || threads.audio_stats().frames == 0 {
        std::thread::yield_now();
    }
    threads.shutdown().unwrap();
    let samples = recording.samples();
    assert!(samples.len() >= AUDIO_BLOCK_FRAMES * CHANNELS);
    // The first block holds the sample on FL/FR, mixer-exact.
    assert_eq!(samples[CH_FL], 0.25);
    assert_eq!(samples[CH_FR], 0.25);
    assert!(recording.peak() >= 0.25);
}

#[test]
fn audio_stats_count_mixer_frames_and_voices() {
    let (mut stx, srx) = trigger_ring(8);
    let (_in_tx, in_rx) = pin_core::spsc::channel(4);
    let mut cfg = ThreadsConfig::with_sim(Box::new(IdleSim), in_rx);
    cfg.audio = AudioConfig {
        sounds: Some(srx),
        samples: vec![(
            SoundId(1),
            Arc::new(Sample::constant(48_000, 0.1, 0.1)),
            Bus::Music,
        )],
        ..AudioConfig::default()
    };
    stx.send(SoundTrigger {
        sound: SoundId(1),
        gain: 1.0,
    });
    let threads = spawn_threads(cfg).unwrap();
    while threads.audio_stats().mixer_frames < 2 * AUDIO_BLOCK_FRAMES as u64 {
        std::thread::yield_now();
    }
    let st = threads.audio_stats();
    threads.shutdown().unwrap();
    assert_eq!(st.mixer_peak_voices, 1);
    assert!(st.mixer_frames >= 2 * AUDIO_BLOCK_FRAMES as u64);
    assert!(st.frames >= st.mixer_frames - AUDIO_BLOCK_FRAMES as u64);
}
