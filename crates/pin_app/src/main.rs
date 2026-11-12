//! `pin_app`: Kilohertz Pinball application binary.
//!
//! The modules live in the `pin_app` library (`src/lib.rs`). The full entry
//! point (thread orchestrator) is implemented by Subphase 1.4; Subphase 1.3
//! provides `--list-displays` to exercise the platform layer.

use pin_app::platform::{DisplayTopologyManager, Platform, PlatformConfig};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list-displays") {
        let config = PlatformConfig {
            headless: args.iter().any(|a| a == "--headless"),
        };
        let result = Platform::init(config)
            .and_then(|p| DisplayTopologyManager::query(p.video()).map(|m| (p, m)));
        match result {
            Ok((_platform, manager)) => {
                for d in manager.displays() {
                    println!(
                        "display {}: {}x{} at ({}, {}), {:.2} Hz, scale {:.2}",
                        d.index,
                        d.bounds.w,
                        d.bounds.h,
                        d.bounds.x,
                        d.bounds.y,
                        d.refresh_hz,
                        d.content_scale
                    );
                }
            }
            Err(e) => {
                eprintln!("pin_app: {e}");
                std::process::exit(1);
            }
        }
    }
    // Entry point is implemented by Subphase 1.4 (orchestrator).
}
