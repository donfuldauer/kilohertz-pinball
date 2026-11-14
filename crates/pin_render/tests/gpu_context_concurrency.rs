//! Many short-lived [`GpuContext`]s created, used and dropped on several
//! threads at once (gpu-tests).
//!
//! Test harnesses run every test on its own thread, and each test usually
//! makes its own context: lavapipe crashed (SIGSEGV) under exactly that load
//! in CI (DECISIONS "1.4 — CI fix: lavapipe SIGSEGV, macOS occlusion"). This
//! test reproduces the pattern on purpose and keeps it in the suite.

use pin_render::debug::{Camera, DebugLine, DebugLines, render_offscreen};
use pin_render::gpu::{GpuContext, GpuOptions, require_gpu};

const THREADS: usize = 8;
const ROUNDS: usize = 6;

fn frame(ctx: &GpuContext) -> Vec<u8> {
    let mut lines = DebugLines::new(4);
    lines.push(DebugLine::new(
        [0.0, 0.5, 0.0],
        [1.0, 0.5, 0.0],
        [1.0, 0.0, 0.0, 1.0],
    ));
    let camera = Camera::ortho_top_down([0.0, 0.0], [1.0, 1.0]);
    render_offscreen(ctx, 32, 32, [0.0, 0.0, 0.0, 1.0], &camera, &lines).expect("render")
}

#[test]
fn contexts_created_used_and_dropped_concurrently() {
    let opts = GpuOptions::from_env();
    match GpuContext::new_headless(&opts) {
        Ok(_) => {}
        Err(e) if require_gpu() => panic!("PIN_REQUIRE_GPU=1 but {e}"),
        Err(e) => return println!("SKIP: {e}"),
    }
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            std::thread::spawn(move || {
                let mut first: Option<Vec<u8>> = None;
                for _ in 0..ROUNDS {
                    let ctx = GpuContext::new_headless(&opts).expect("context");
                    let px = frame(&ctx);
                    drop(ctx);
                    // Rows 15 and 16 straddle y = 0.5: lit red, the rest black.
                    let lit = px.chunks_exact(4).filter(|p| p[0] > 128).count();
                    assert_eq!(lit % 32, 0, "whole rows lit");
                    assert!((32..=64).contains(&lit), "lit {lit}");
                    match &first {
                        None => first = Some(px),
                        Some(f) => assert_eq!(f, &px, "same frame every round"),
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("worker");
    }
}
