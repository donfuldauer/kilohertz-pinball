//! Verification criterion (Subphase 1.1): a writer and a reader thread over
//! 100,000 publishes see zero torn reads and monotonic tick numbers.

use pin_core::snapshot::{
    BallState, FlipperState, MAX_BALLS, MAX_DYNAMIC_MESHES, MAX_FLIPPERS, MAX_LAMPS,
    MeshTransform,
};
use pin_core::{FlipperId, FrameStateSnapshot, ProfileId, TripleBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const PUBLISHES: u64 = 100_000;

/// A snapshot whose every field is a function of `tick`, so any mix of two
/// publishes is detectable.
fn derived(tick: u64) -> FrameStateSnapshot {
    let t = tick as f32; // exact for tick < 2^24
    let mut s = FrameStateSnapshot {
        tick,
        ball_count: (tick % (MAX_BALLS as u64 + 1)) as u8,
        flipper_count: (tick % (MAX_FLIPPERS as u64 + 1)) as u8,
        dynamic_mesh_count: (tick % (MAX_DYNAMIC_MESHES as u64 + 1)) as u8,
        gi: [t, t + 1.0, t + 2.0, t + 3.0],
        ..FrameStateSnapshot::default()
    };
    for (i, b) in s.balls.iter_mut().enumerate() {
        let f = i as f32;
        *b = BallState {
            position_m: [t, t + f, t - f],
            orientation: [t, f, -t, f + 1.0],
            velocity_m_s: [t * 2.0, f, t],
            angular_velocity_rad_s: [f, t, -f],
            profile: ProfileId((tick as u16).wrapping_add(i as u16)),
            layer: (tick as u8).wrapping_add(i as u8),
        };
    }
    for (i, fl) in s.flippers.iter_mut().enumerate() {
        *fl = FlipperState {
            id: FlipperId((tick as u16) ^ i as u16),
            angle_rad: t + i as f32,
            angular_velocity_rad_s: -t,
            zipper_offset_m: t * 0.5,
        };
    }
    for (i, m) in s.dynamic_meshes.iter_mut().enumerate() {
        *m = MeshTransform {
            position_m: [t, i as f32, t],
            orientation: [i as f32, t, t, t],
        };
    }
    for (i, l) in s.lamps.iter_mut().enumerate() {
        *l = t + (i % MAX_LAMPS) as f32;
    }
    s
}

#[test]
fn hundred_thousand_publishes_no_torn_reads_monotonic_ticks() {
    let (mut writer, mut reader) = TripleBuffer::new(derived(0));
    let done = Arc::new(AtomicBool::new(false));

    let done_w = Arc::clone(&done);
    let w = std::thread::spawn(move || {
        for tick in 1..=PUBLISHES {
            writer.publish(derived(tick));
        }
        done_w.store(true, Ordering::Release);
    });

    let r = std::thread::spawn(move || {
        let mut last = 0u64;
        let mut distinct = 0u64;
        let mut reads = 0u64;
        loop {
            let finished = done.load(Ordering::Acquire);
            let s = *reader.read();
            reads += 1;
            assert_eq!(s, derived(s.tick), "torn read at tick {}", s.tick);
            assert!(s.tick >= last, "tick went backwards: {} -> {}", last, s.tick);
            if s.tick > last {
                distinct += 1;
            }
            last = s.tick;
            if s.tick == PUBLISHES {
                break;
            }
            // Once the writer has finished, the next read must see the last publish.
            assert!(!finished, "writer finished but reader saw tick {}", s.tick);
        }
        (reads, distinct)
    });

    w.join().unwrap();
    let (reads, distinct) = r.join().unwrap();
    assert!(reads >= 1 && distinct >= 1);
}

#[test]
fn small_payload_stress_every_value_is_a_real_publish() {
    // A 3-word payload with redundant encodings of the same counter.
    let (mut writer, mut reader) = TripleBuffer::new([0u64, !0u64, 0u64]);
    let w = std::thread::spawn(move || {
        for i in 1..=PUBLISHES {
            writer.publish([i, !i, i.wrapping_mul(0x9E37_79B9_7F4A_7C15)]);
        }
    });
    let mut last = 0;
    while last < PUBLISHES {
        let v = *reader.read();
        assert_eq!(v[1], !v[0]);
        assert_eq!(v[2], v[0].wrapping_mul(0x9E37_79B9_7F4A_7C15));
        assert!(v[0] >= last);
        last = v[0];
    }
    w.join().unwrap();
}
