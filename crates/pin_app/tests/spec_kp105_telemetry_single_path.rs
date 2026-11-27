//! KP-105 / #216 (SPEC §4 Subphase 3.4, `docs/TELEMETRY_PROTOCOL.md` §4.6):
//! telemetry has a single path (encoded on the sim thread, sent by one
//! sender thread). The audio thread only feeds haptic impulses into the SSF
//! exciter mixer. `pin_app::ui` re-exports the telemetry and DOF config
//! types.
//!
//! Time is simulated (R6): a monotonically increasing fake clock and loops
//! bounded by poll count; nothing sleeps or asserts on wall-clock time.

mod support;

use pin_app::orchestrator::{
    AudioConfig, TableSim, TelemetryFeed, TelemetryMap, TelemetrySources, ThreadsConfig,
    load_validated, open_telemetry, spawn_threads,
};
use pin_app::ui::{
    AppConfig, DEFAULT_TELEMETRY_ENDPOINT, DofConfig, MAX_ENDPOINT_BYTES, TelemetryConfig,
};
use pin_core::input::{CabinetInput, HapticsTelemetryEvent, InputEvent};
use pin_core::sim::TICK_NS;
use pin_core::snapshot::FrameStateSnapshot;
use pin_core::spsc;
use pin_sim::SimConfig;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use support::{greybox_bundle, on_big_stack};

/// A simulated clock: 250 microseconds per call, never wall time (R6).
fn sim_clock() -> u64 {
    static NOW: AtomicU64 = AtomicU64::new(0);
    NOW.fetch_add(250_000, Ordering::Relaxed)
}

/// SPEC §4 3.4, #216 deliverable 3: the config types are reachable from
/// `pin_app::ui` (no `config::` path) with their documented defaults.
#[test]
fn ui_reexports_telemetry_and_dof_config() {
    assert_eq!(DEFAULT_TELEMETRY_ENDPOINT, "127.0.0.1:35120");
    assert_eq!(MAX_ENDPOINT_BYTES, 255);
    let t = TelemetryConfig::default();
    assert_eq!(
        t,
        TelemetryConfig {
            enabled: true,
            endpoint: DEFAULT_TELEMETRY_ENDPOINT.to_string(),
        }
    );
    assert!(DofConfig::default().enabled);
    let app = AppConfig::default();
    assert_eq!(app.telemetry, t);
    assert!(app.dof.enabled);

    // An over-long endpoint differs from the default and round-trips.
    let long = TelemetryConfig {
        enabled: false,
        endpoint: "a".repeat(MAX_ENDPOINT_BYTES + 1),
    };
    assert_ne!(long, t);
    let text = toml::to_string(&long).expect("serialize");
    let back: TelemetryConfig = toml::from_str(&text).expect("deserialize");
    assert_eq!(back, long);
}

/// SPEC §4 3.4, TELEMETRY_PROTOCOL §4.6, #216 deliverable 1: the audio
/// thread feeds the SSF mixer from the haptics ring but opens no socket and
/// sends nothing. No telemetry endpoint is configured for these threads, so
/// the zero datagrams on the loopback listener is the black-box result of
/// "the audio thread does not open a socket or send".
#[test]
fn audio_thread_sends_no_telemetry_but_still_feeds_ssf() {
    let dir = greybox_bundle().to_path_buf();
    let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    on_big_stack(move || {
        let sim = load_validated(&dir, SimConfig::default()).unwrap();
        let (htx, hrx) = pin_input::haptics::haptics_ring::<HapticsTelemetryEvent>(256);
        let (mut tx, rx) = spsc::channel::<InputEvent>(64);
        let mut cfg = ThreadsConfig::with_sim(Box::new(TableSim::new(sim, Some(htx))), rx);
        cfg.now_ns = sim_clock;
        cfg.audio = AudioConfig {
            haptics: Some(hrx),
            period: Duration::from_millis(1),
            ..AudioConfig::default()
        };
        let mut push = |tick: u64, i: CabinetInput, p: bool| {
            tx.push(InputEvent::button(tick * TICK_NS - TICK_NS / 2, i, p))
                .unwrap();
        };
        push(50, CabinetInput::Start, true);
        push(60, CabinetInput::Start, false);
        for k in 0..4u64 {
            push(2000 + k * 300, CabinetInput::LeftFlipper, true);
            push(2060 + k * 300, CabinetInput::LeftFlipper, false);
        }
        let threads = spawn_threads(cfg).unwrap();
        let mut ok = false;
        for _ in 0..50_000_000u64 {
            let a = threads.audio_stats();
            if a.haptic_events > 0 && a.frames > 0 && threads.stats().sim_ticks >= 200 {
                ok = true;
                break;
            }
            std::thread::yield_now();
        }
        let (a, ticks) = (threads.audio_stats(), threads.stats().sim_ticks);
        threads.shutdown().unwrap();
        assert!(
            ok,
            "haptics never reached the audio thread: haptic_events={} frames={} sim_ticks={ticks}",
            a.haptic_events, a.frames
        );
    });
    let mut buf = [0u8; 2048];
    match listener.recv(&mut buf) {
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::WouldBlock),
        Ok(n) => panic!("unexpected {n}-byte datagram from the audio thread"),
    }
}

/// SPEC §4 3.4, TELEMETRY_PROTOCOL §4.6, #216 acceptance: the sim-thread
/// telemetry path still delivers a full-state datagram over loopback.
#[test]
fn sim_thread_telemetry_path_still_delivers() {
    let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    listener
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, link) = open_telemetry(&addr.to_string()).unwrap();
    let mut feed = TelemetryFeed::new(tx, TelemetryMap::default());
    feed.tick(
        0,
        &FrameStateSnapshot::default(),
        &TelemetrySources::default(),
        &[],
    );
    let mut buf = [0u8; 2048];
    let n = listener.recv(&mut buf).expect("a datagram arrives");
    assert!(n > 0);
    let report = link.stop();
    assert!(report.sent >= 1, "{report:?}");
}
