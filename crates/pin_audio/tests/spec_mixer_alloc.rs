//! Spec test (Independent Tester, Subphase 3.3), criterion 6: `Mixer::render`,
//! `SoundSender::send` and `SoundSender::send_queue` never allocate.
//!
//! A counting global allocator, armed per thread so libtest's own threads
//! cannot disturb the count. Also holds the (ignored, non-gating) timing
//! report for a 256-frame render with 30 voices.

#[path = "spec_fixtures/util.rs"]
mod util;

use pin_audio::mixer::{Bus, MixerConfig, SoundTrigger};
use pin_core::{HostCommandQueue, SoundId};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use util::*;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}
static EVENTS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

impl Counting {
    fn note() {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            EVENTS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// SAFETY: defers to `System`; only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::note();
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        Self::note();
        // SAFETY: forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::note();
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::note();
        // SAFETY: forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Runs `f` and returns how many allocator calls (alloc, realloc, dealloc)
/// this thread made inside it.
fn allocator_calls(f: impl FnOnce()) -> usize {
    // Tests in this binary run on separate threads; serialise the shared counter.
    static GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = GATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    EVENTS.store(0, Ordering::Relaxed);
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    EVENTS.load(Ordering::Relaxed)
}

#[test]
fn the_counter_sees_allocations() {
    let n = allocator_calls(|| {
        let v = std::hint::black_box(vec![1u8; 100]);
        drop(v);
    });
    assert!(n >= 2, "alloc + dealloc must be counted, saw {n}");
    assert_eq!(allocator_calls(|| {}), 0);
}

/// A mixer with every bus registered and long enough samples to stay busy.
fn busy_rig() -> Rig {
    let mut rig = Rig::with_ring(MixerConfig::default(), 256);
    rig.reg(1, &dc(0.01, -0.01, 48_000, RATE), Bus::Music);
    rig.reg(2, &dc(0.02, 0.02, 3_000, RATE), Bus::Callout);
    rig.reg(3, &dc(0.005, 0.005, 20_000, RATE), Bus::Sfx);
    rig.reg(4, &dc(0.004, -0.004, 100, RATE), Bus::Sfx); // short: ends mid-buffer
    rig
}

#[test]
fn render_does_not_allocate() {
    let mut rig = busy_rig();
    let mut out = vec![0.0f32; 2 * 256];
    let mut odd = vec![0.0f32; 2 * 1_000];
    let mut q = HostCommandQueue::default();
    for _ in 0..16 {
        q.play_sound(SoundId(3), 0.5).unwrap();
    }
    let Rig { mixer, tx } = &mut rig;
    let n = allocator_calls(|| {
        // First render ever, with no voices.
        mixer.render(&mut out);
        // 30 voices at once: music 2, callout 4, sfx 24.
        for _ in 0..2 {
            assert!(tx.send(SoundTrigger { sound: SoundId(1), gain: 1.0 }));
        }
        for _ in 0..4 {
            assert!(tx.send(SoundTrigger { sound: SoundId(2), gain: 1.0 }));
        }
        for _ in 0..24 {
            assert!(tx.send(SoundTrigger { sound: SoundId(3), gain: 0.5 }));
        }
        mixer.render(&mut out);
        // Steals on every bus, unknown sounds, a full command queue, short
        // voices ending, ducking attack and release, odd buffer sizes.
        for round in 0..200u16 {
            assert!(tx.send(SoundTrigger { sound: SoundId(1 + round % 4), gain: 0.25 }));
            assert!(tx.send(SoundTrigger { sound: SoundId(1_000 + round), gain: 1.0 }));
            if round % 50 == 0 {
                assert_eq!(tx.send_queue(&q), 16);
            }
            if round % 7 == 0 {
                mixer.render(&mut odd);
            } else {
                mixer.render(&mut out);
            }
        }
        // Let everything run out (voices end, duck releases).
        for _ in 0..80 {
            mixer.render(&mut odd);
        }
    });
    assert_eq!(n, 0, "render/send made {n} allocator calls");
    // The scenario really exercised what it claims.
    assert!(mixer.stolen_voices() > 0);
    assert_eq!(mixer.unknown_sounds(), 200);
    assert_eq!(mixer.duck_level(), 1.0);
    assert_eq!(tx.dropped(), 0);
}

#[test]
fn send_does_not_allocate_even_when_the_ring_is_full() {
    let mut rig = Rig::with_ring(MixerConfig::default(), 8);
    let mut q = HostCommandQueue::default();
    for i in 0..16 {
        q.play_sound(SoundId(i), 1.0).unwrap();
    }
    let tx = &mut rig.tx;
    let mut sent = 0;
    let mut sent_from_queue = 0;
    let n = allocator_calls(|| {
        for i in 0..20u16 {
            if tx.send(SoundTrigger { sound: SoundId(i), gain: 1.0 }) {
                sent += 1;
            }
        }
        sent_from_queue = tx.send_queue(&q);
        let _ = std::hint::black_box(tx.dropped());
    });
    assert_eq!(n, 0, "send/send_queue made {n} allocator calls");
    assert_eq!(sent, 8);
    assert_eq!(sent_from_queue, 0);
    assert_eq!(tx.dropped(), 12 + 16);
}

/// Non-gating report (criterion 6): time for a 256-frame render with 30
/// active voices. Run with
/// `cargo test -p pin_audio --release --test spec_mixer_alloc -- --ignored --nocapture`.
/// Prints only; never asserts on time.
#[test]
#[ignore = "timing report, non-gating"]
fn report_render_time_256_frames_30_voices() {
    let mut rig = Rig::with_ring(MixerConfig::default(), 64);
    let frames = 256 * 2_100;
    rig.reg(1, &dc(0.01, -0.01, frames, RATE), Bus::Music);
    rig.reg(2, &dc(0.02, 0.02, frames, RATE), Bus::Callout);
    rig.reg(3, &dc(0.005, 0.005, frames, RATE), Bus::Sfx);
    for (id, count) in [(1, 2), (2, 4), (3, 24)] {
        for _ in 0..count {
            rig.play(id, 1.0);
        }
    }
    let mut out = vec![0.0f32; 2 * 256];
    for _ in 0..50 {
        rig.mixer.render(&mut out); // warm-up
    }
    assert_eq!(rig.mixer.active_voices(Bus::Sfx), 24);
    let mut times: Vec<u128> = (0..2_000)
        .map(|_| {
            let t = std::time::Instant::now();
            rig.mixer.render(std::hint::black_box(&mut out));
            t.elapsed().as_nanos()
        })
        .collect();
    assert_eq!(
        rig.mixer.active_voices(Bus::Music) + rig.mixer.active_voices(Bus::Callout) + 24,
        30,
        "30 voices were active for every timed render"
    );
    times.sort_unstable();
    println!(
        "[spec_mixer] render 256 frames, 30 voices: median {} ns, p99 {} ns, max {} ns (target ≤ 50 000 ns in release)",
        times[times.len() / 2],
        times[times.len() * 99 / 100],
        times[times.len() - 1]
    );
}
