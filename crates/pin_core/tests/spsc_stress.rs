//! SPSC ring: order and timestamps survive a cross-thread hand-off (PLAN 1.3
//! verification), plus model-based property tests.

use pin_core::spsc::channel;
use pin_core::{CabinetAxis, CabinetInput, InputEvent};
use proptest::prelude::*;
use std::collections::VecDeque;

const N: u64 = 100_000;

fn event_for(i: u64) -> InputEvent {
    let ts = 1_000_000_000 + i * 997;
    if i % 3 == 0 {
        InputEvent::axis(ts, CabinetAxis::Plunger, (i % 1000) as f32 / 1000.0)
    } else {
        InputEvent::button(ts, CabinetInput::ALL[(i % 8) as usize], i % 2 == 0)
    }
}

#[test]
fn hundred_thousand_events_cross_threads_in_order() {
    for cap in [1usize, 7, 64, 1024] {
        let (mut tx, mut rx) = channel::<InputEvent>(cap);
        let producer = std::thread::spawn(move || {
            for i in 0..N {
                let mut ev = event_for(i);
                loop {
                    match tx.push(ev) {
                        Ok(()) => break,
                        Err(back) => {
                            ev = back;
                            std::hint::spin_loop();
                        }
                    }
                }
            }
        });
        let consumer = std::thread::spawn(move || {
            let mut i = 0u64;
            let mut last_ts = 0u64;
            while i < N {
                assert!(rx.len() <= cap);
                if let Some(ev) = rx.pop() {
                    assert_eq!(ev, event_for(i), "item {i} (cap {cap})");
                    assert!(ev.timestamp_ns > last_ts);
                    last_ts = ev.timestamp_ns;
                    i += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
            assert_eq!(rx.pop(), None);
        });
        producer.join().unwrap();
        consumer.join().unwrap();
    }
}

#[derive(Debug, Clone)]
enum Op {
    Push(u32),
    Pop,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![any::<u32>().prop_map(Op::Push), Just(Op::Pop)]
}

proptest! {
    /// The ring behaves exactly like a bounded `VecDeque`.
    #[test]
    fn matches_bounded_queue_model(cap in 1usize..20, ops in prop::collection::vec(op(), 0..400)) {
        let (mut tx, mut rx) = channel::<u32>(cap);
        let mut model = VecDeque::new();
        for o in ops {
            match o {
                Op::Push(v) => {
                    let r = tx.push(v);
                    if model.len() < cap {
                        prop_assert_eq!(r, Ok(()));
                        model.push_back(v);
                    } else {
                        prop_assert_eq!(r, Err(v));
                    }
                }
                Op::Pop => prop_assert_eq!(rx.pop(), model.pop_front()),
            }
            prop_assert_eq!(rx.len(), model.len());
            prop_assert_eq!(tx.len(), model.len());
            prop_assert_eq!(rx.is_empty(), model.is_empty());
            prop_assert_eq!(tx.is_full(), model.len() == cap);
        }
    }
}
