//! Implementer allocation guard: `Mixer::render` and `SoundSender::send`
//! never allocate (counting global allocator, per thread).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use pin_audio::mixer::{Bus, Mixer, MixerConfig, Sample, SoundTrigger, trigger_ring};
use pin_core::SoundId;
use pin_core::script::HostCommandQueue;

struct Counting;

thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn count<R>(f: impl FnOnce() -> R) -> (R, u64) {
    let before = COUNT.with(Cell::get);
    let r = f();
    (r, COUNT.with(Cell::get) - before)
}

#[test]
fn render_and_send_do_not_allocate() {
    let (mut tx, rx) = trigger_ring(40);
    let mut m = Mixer::new(MixerConfig::default(), rx);
    for (i, bus) in Bus::ALL.into_iter().enumerate() {
        m.register(
            SoundId(i as u16),
            Arc::new(Sample::constant(3000, 0.01, -0.01)),
            bus,
        );
    }
    m.register(
        SoundId(9),
        Arc::new(Sample::constant(10, 0.1, 0.1)),
        Bus::Sfx,
    );
    let mut out = vec![0.0f32; 2 * 256];
    let mut q = HostCommandQueue::default();
    for i in 0..16u16 {
        q.play_sound(SoundId(i % 4), 0.5).unwrap();
    }
    let mut total = 0;
    for round in 0..40u16 {
        let (_, a) = count(|| {
            for k in 0..30u16 {
                // Includes unknown ids, steals and short voices that end.
                tx.send(SoundTrigger {
                    sound: SoundId((k + round) % 11),
                    gain: 1.0,
                });
            }
            tx.send_queue(&q);
            m.render(&mut out);
            m.render(&mut out[..2 * 100 + 1]);
        });
        total += a;
    }
    assert!(m.stolen_voices() > 0 && m.unknown_sounds() > 0 && tx.dropped() > 0);
    assert_eq!(total, 0, "allocations in render/send");
}
