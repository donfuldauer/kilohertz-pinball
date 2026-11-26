//! KP-006 (SPEC §4 Subphases 3.4 and 3.5): the app's cabinet feedback.
//!
//! * UDP telemetry over loopback: [`open_telemetry`] + [`TelemetryFeed`]
//!   send a full state, then deltas, and a listener mirroring the datagrams
//!   (`docs/TELEMETRY_PROTOCOL.md` §6) matches the snapshots; a full ring
//!   forces a full state and the listener converges.
//! * Rumble with a mock output: a session restarted within one second has no
//!   leftover envelope.
//! * The DOF bridge lifecycle with a stand-in bridge (Unix).
//! * The sim thread's tick with telemetry and rumble attached makes no heap
//!   allocation (R4), including a forced full state.
//! * The new `config.toml` keys survive bounded random bytes (R11).
//!
//! Time is simulated: ticks are fed in a loop. The only waits are for the
//! loopback listener thread to catch up (bounded, never asserted on).

#[cfg(unix)]
mod private_tmp;
mod support;

use pin_app::orchestrator::{
    TableSim, TelemetryFeed, TelemetryMap, TelemetrySources, load_app_table, open_telemetry,
};
use pin_app::platform::rumble::{RumbleCommand, RumbleDriver, RumbleOutput};
use pin_app::ui::AppConfig;
use pin_audio::mixer::MixerConfig;
use pin_audio::telemetry::{
    HapticsTelemetryEvent, TelemetryEntry, TelemetryKind, TelemetrySender, decode_datagram,
    telemetry_channel,
};
use pin_core::CoilId;
use pin_core::haptics::{HapticEvent, HapticKind};
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::sim::{SimStep, TICK_NS};
use pin_core::snapshot::FrameStateSnapshot;
use pin_input::haptics::{RumbleSettings, haptics_ring};
use pin_sim::alloc_guard::{CountingAlloc, count_allocs, is_installed};
use proptest::prelude::*;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// A listener's view of the lamps and GI strings (protocol §6): the last
/// value of each.
struct Mirror {
    lamps: [f32; 256],
    gi: [f32; 32],
}

impl Default for Mirror {
    fn default() -> Self {
        Mirror {
            lamps: [0.0; 256],
            gi: [0.0; 32],
        }
    }
}

impl Mirror {
    fn apply(&mut self, e: &TelemetryEntry) {
        let i = usize::from(e.id);
        match e.kind {
            TelemetryKind::Lamp => self.lamps[i] = e.value,
            TelemetryKind::Gi => self.gi[i] = e.value,
            _ => {}
        }
    }
}

/// One received datagram: tick, full-state flag, entries.
type Received = (u64, bool, Vec<TelemetryEntry>);

/// Decodes `bytes` (every datagram must be valid).
fn decode(bytes: &[u8]) -> Received {
    let (h, e) = decode_datagram(bytes).expect("valid datagram");
    (h.tick, h.is_full_state(), e)
}

/// A loopback listener on its own thread, collecting every datagram.
struct Listener {
    addr: std::net::SocketAddr,
    seen_tick: Arc<AtomicU64>,
    count: Arc<AtomicU64>,
    thread: std::thread::JoinHandle<Vec<Received>>,
    stop: Arc<AtomicU64>,
}

impl Listener {
    fn start() -> Listener {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let addr = socket.local_addr().unwrap();
        let seen_tick = Arc::new(AtomicU64::new(u64::MAX));
        let count = Arc::new(AtomicU64::new(0));
        // `u64::MAX`: keep reading; otherwise stop once that many arrived.
        let stop = Arc::new(AtomicU64::new(u64::MAX));
        let (t, c, s) = (seen_tick.clone(), count.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            let mut got = Vec::new();
            let mut buf = [0u8; 2048];
            let deadline_after_stop = Duration::from_secs(10);
            let mut stop_seen: Option<Instant> = None;
            loop {
                let want = s.load(Ordering::Acquire);
                if want != u64::MAX {
                    if got.len() as u64 >= want {
                        break;
                    }
                    let since = *stop_seen.get_or_insert_with(Instant::now);
                    if since.elapsed() > deadline_after_stop {
                        break;
                    }
                }
                if let Ok(n) = socket.recv(&mut buf) {
                    let d = decode(&buf[..n]);
                    t.store(d.0, Ordering::Release);
                    got.push(d);
                    c.fetch_add(1, Ordering::Release);
                }
            }
            got
        });
        Listener {
            addr,
            seen_tick,
            count,
            thread,
            stop,
        }
    }

    /// Waits (bounded) until a datagram of `tick` or later arrived.
    fn wait_for_tick(&self, tick: u64) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let t = self.seen_tick.load(Ordering::Acquire);
            if t != u64::MAX && t >= tick {
                return;
            }
            std::thread::yield_now();
        }
        panic!("tick {tick} never arrived");
    }

    /// Collects every datagram once `sent` arrived.
    fn finish(self, sent: u64) -> Vec<Received> {
        self.stop.store(sent, Ordering::Release);
        let got = self.thread.join().unwrap();
        assert_eq!(got.len() as u64, sent, "datagrams lost on loopback");
        assert_eq!(self.count.load(Ordering::Acquire), sent);
        got
    }
}

/// The lamp value of lamp `i` at tick `t` (changes every tick).
fn lamp_at(t: u64, i: usize) -> f32 {
    ((t * 7 + i as u64 * 13) % 101) as f32 / 100.0
}

/// The snapshot of tick `t`: lamps 0..8 and GI lamps 20, 21 change.
fn snapshot_at(t: u64) -> FrameStateSnapshot {
    let mut s = FrameStateSnapshot {
        tick: t,
        ..FrameStateSnapshot::default()
    };
    for i in 0..8 {
        s.lamps[i] = lamp_at(t, i);
    }
    s.lamps[20] = lamp_at(t / 10, 20);
    s.lamps[21] = if t % 300 < 150 { 1.0 } else { 0.25 };
    s
}

/// Lamps 0..8 on telemetry ids 100..108 (sim ids differ from the numbers),
/// GI lamps 20 and 21 on GI strings 0 and 3.
fn test_map() -> TelemetryMap {
    TelemetryMap {
        lamps: (0..8u16).map(|i| (100 + i as u8, i)).collect(),
        gi: vec![(0, 20), (3, 21)],
        ..TelemetryMap::default()
    }
}

/// What the mirror must hold after tick `t` for [`test_map`].
fn expected(t: u64) -> ([f32; 8], [f32; 2]) {
    let s = snapshot_at(t);
    let mut lamps = [0.0; 8];
    lamps.copy_from_slice(&s.lamps[..8]);
    (lamps, [s.lamps[20], s.lamps[21]])
}

fn mirrored(m: &Mirror) -> ([f32; 8], [f32; 2]) {
    let mut lamps = [0.0; 8];
    lamps.copy_from_slice(&m.lamps[100..108]);
    (lamps, [m.gi[0], m.gi[3]])
}

/// SPEC §4 3.4 / KP-006 acceptance: a loopback listener receives a full
/// state, then deltas, whose lamp and GI values match the snapshot over
/// 1,000 ticks of simulated time (checked at every tick), and a periodic
/// full state one second later.
#[test]
fn spec_3_4_kp006_loopback_full_state_then_deltas_match_the_snapshot() {
    let listener = Listener::start();
    let (tx, link) = open_telemetry(&listener.addr.to_string()).unwrap();
    assert_eq!(link.destination(), listener.addr);
    let mut feed = TelemetryFeed::new(tx, test_map());
    let stats = feed.stats();
    let sources = TelemetrySources::default();
    const TICKS: u64 = 1001;
    for t in 0..TICKS {
        feed.tick(t, &snapshot_at(t), &sources, &[]);
        // Pace the producer to the listener (every tick changes a lamp, so
        // every tick sends a datagram): nothing is dropped.
        listener.wait_for_tick(t);
    }
    assert_eq!(feed.ring_dropped(), 0);
    assert_eq!(feed.forced_full_states(), 0);
    let report = link.stop();
    assert_eq!(report.send_errors, 0);
    assert_eq!(report.ring_dropped, 0);
    assert!(report.sent >= TICKS, "{report:?}");
    let got = listener.finish(report.sent);
    assert_eq!(stats.ticks(), TICKS);

    // The first tick is a full state of 5 datagrams (832 entries).
    let first: Vec<&Received> = got.iter().take_while(|d| d.0 == 0).collect();
    assert_eq!(first.len(), 5);
    assert!(first.iter().all(|d| d.1), "flag bit 0 on every datagram");
    assert_eq!(first.iter().map(|d| d.2.len()).sum::<usize>(), 832);
    // Deltas until tick 1000, which is the periodic full state.
    for d in &got[5..] {
        assert_eq!(d.1, d.0 == 1000, "tick {} full={}", d.0, d.1);
        if !d.1 {
            assert!(d.2.len() <= 10, "a delta carries only changes");
        }
    }
    // Replay: after the last datagram of each tick the mirror equals that
    // tick's snapshot.
    let mut mirror = Mirror::default();
    for (i, d) in got.iter().enumerate() {
        for e in &d.2 {
            mirror.apply(e);
        }
        let last_of_tick = got.get(i + 1).is_none_or(|n| n.0 != d.0);
        if last_of_tick {
            assert_eq!(mirrored(&mirror), expected(d.0), "tick {}", d.0);
        }
    }
    assert_eq!(got.last().unwrap().0, TICKS - 1);
    assert_eq!(mirrored(&mirror), expected(TICKS - 1));
}

/// SPEC §4 3.4 / KP-006 acceptance: a full ring (minimum capacity, no
/// sender running) forces a full state; once a sender drains the ring the
/// next tick is a full state and the listener converges to the snapshot.
#[test]
fn spec_3_4_kp006_back_pressure_forces_a_full_state_and_the_listener_converges() {
    let (tx, rx) = telemetry_channel(5);
    let mut feed = TelemetryFeed::new(tx, test_map());
    let sources = TelemetrySources::default();
    let mut t = 0;
    while feed.forced_full_states() == 0 {
        feed.tick(t, &snapshot_at(t), &sources, &[]);
        t += 1;
        assert!(t < 100, "the ring never filled");
    }
    assert!(feed.ring_dropped() > 0);
    // More ticks while the ring stays full: each one is dropped again.
    for _ in 0..5 {
        feed.tick(t, &snapshot_at(t), &sources, &[]);
        t += 1;
    }
    let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    listener
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut sender = TelemetrySender::new(rx, listener.local_addr().unwrap()).unwrap();
    let mut mirror = Mirror::default();
    let mut buf = [0u8; 2048];
    let mut receive = |sender: &mut TelemetrySender, mirror: &mut Mirror| -> Vec<Received> {
        let n = sender.pump();
        (0..n)
            .map(|_| {
                let len = listener.recv(&mut buf).unwrap();
                let d = decode(&buf[..len]);
                for e in &d.2 {
                    mirror.apply(e);
                }
                d
            })
            .collect()
    };
    let stale = receive(&mut sender, &mut mirror);
    assert_eq!(stale.len(), 5, "the first full state filled the ring");
    assert_ne!(mirrored(&mirror), expected(t - 1), "the listener is behind");
    // The next tick after the drain is a full state.
    feed.tick(t, &snapshot_at(t), &sources, &[]);
    let resync = receive(&mut sender, &mut mirror);
    assert_eq!(resync.len(), 5);
    assert!(resync.iter().all(|d| d.0 == t && d.1), "full state at {t}");
    assert_eq!(mirrored(&mirror), expected(t), "converged");
    // Then deltas again, still converged.
    for _ in 0..50 {
        t += 1;
        feed.tick(t, &snapshot_at(t), &sources, &[]);
        for d in receive(&mut sender, &mut mirror) {
            assert!(!d.1);
        }
        assert_eq!(mirrored(&mirror), expected(t));
    }
    assert_eq!(sender.send_errors(), 0);
}

/// Records every command it is given.
#[derive(Default)]
struct MockPads(Vec<RumbleCommand>);

impl RumbleOutput for MockPads {
    fn set_gamepad_rumble(&mut self, cmd: RumbleCommand) -> usize {
        self.0.push(cmd);
        1
    }
}

fn coil_fire(tick: u64) -> HapticEvent {
    HapticEvent::new(tick, HapticKind::CoilFire { coil: CoilId(1) }, 1.0)
}

/// SPEC §4 3.4 / KP-006 acceptance: with a mock output, a session started
/// within 1 s of the previous one (the tick restarts at 0 while the old
/// envelope would still be active) produces no leftover envelope: after
/// the "off" at the restart, no non-zero command of the old session.
#[test]
fn spec_3_4_kp006_rumble_restart_within_one_second_has_no_leftover_envelope() {
    let (mut tx, rx) = haptics_ring::<HapticEvent>(16);
    let mut settings = RumbleSettings::default();
    settings.coil_fire.duration_ticks = 400;
    let mut driver = RumbleDriver::new(settings, rx);
    let mut pads = MockPads::default();
    for t in 0..=600 {
        if t == 500 {
            tx.push(coil_fire(500));
        }
        driver.service(t, &mut pads);
    }
    let on = pads.0.iter().filter(|c| **c != RumbleCommand::OFF).count();
    assert!(on > 0, "the old session rumbled");
    assert_ne!(*pads.0.last().unwrap(), RumbleCommand::OFF, "still running");
    // New session 100 ms later in wall time: its ticks restart at 0 and
    // pass the old envelope's ticks (500..900) again.
    pads.0.clear();
    for t in 0..1500 {
        driver.service(t, &mut pads);
    }
    assert_eq!(pads.0.first(), Some(&RumbleCommand::OFF), "stopped at once");
    let leftover = pads.0.iter().filter(|c| **c != RumbleCommand::OFF).count();
    assert_eq!(
        leftover,
        0,
        "leftover envelope: {leftover} non-zero command(s), first {:?}",
        pads.0.iter().find(|c| **c != RumbleCommand::OFF)
    );
    // The new session's own impulses still rumble.
    tx.push(coil_fire(1500));
    assert!(
        driver
            .service(1500, &mut pads)
            .is_some_and(|c| c != RumbleCommand::OFF)
    );
}

/// SPEC §4 3.4 / KP-006: `reset` (table load and swap) returns "off" only
/// when the motors were on, and drops what was queued.
#[test]
fn spec_3_4_kp006_rumble_reset_returns_off_only_when_on() {
    let (mut tx, rx) = haptics_ring::<HapticEvent>(16);
    let mut driver = RumbleDriver::new(RumbleSettings::default(), rx);
    assert_eq!(driver.reset(), None);
    tx.push(coil_fire(10));
    let mut pads = MockPads::default();
    assert!(driver.service(10, &mut pads).is_some());
    assert_eq!(driver.reset(), Some(RumbleCommand::OFF));
    assert_eq!(driver.reset(), None);
    tx.push(coil_fire(11));
    assert_eq!(driver.reset(), None, "queued but never sent");
    assert_eq!(
        driver.service(11, &mut pads),
        None,
        "the queued event is gone"
    );
}

/// SPEC §4 3.5 / KP-006 acceptance: the DOF bridge (a stand-in script) is
/// started on table load and stopped on exit; with the platform unsupported
/// or a table without `[dof]` nothing is spawned.
#[cfg(unix)]
#[test]
fn spec_3_5_kp006_dof_bridge_started_on_load_and_stopped_on_exit() {
    use pin_app::dof::{DofBridge, DofLaunchConfig};
    use pin_core::schema::table::TableManifest;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    let table = TableManifest::from_toml_str(include_str!(
        "../../../tools/dof_bridge/tests/fixtures/table.toml"
    ))
    .unwrap();
    let mut no_dof = table.clone();
    no_dof.dof = None;
    let dir = private_tmp::create("khz-kp006-dof");
    let starts = dir.join("starts");
    let exe = dir.join("bridge.sh");
    std::fs::write(
        &exe,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\ncat > /dev/null\nexit 0\n",
            starts.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut cfg = DofLaunchConfig::new(&exe, 35120);
    cfg.map_dir = dir.clone();
    let alive = |pid: u32| {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(
        DofBridge::start_for_table_on(false, &table, &cfg)
            .unwrap()
            .is_none()
    );
    assert!(
        DofBridge::start_for_table_on(true, &no_dof, &cfg)
            .unwrap()
            .is_none()
    );
    assert!(!starts.exists(), "nothing spawned");
    // Load.
    let mut bridge = DofBridge::start_for_table_on(true, &table, &cfg)
        .unwrap()
        .expect("bridge started");
    let pid = bridge.id();
    let map_file = bridge.map_file().to_path_buf();
    assert!(map_file.exists());
    assert!(alive(pid));
    assert!(bridge.try_wait().unwrap().is_none(), "running while loaded");
    // Exit.
    let status = bridge.stop().wait().expect("exit status");
    assert!(status.success(), "{status:?}");
    assert!(!alive(pid));
    assert!(!map_file.exists());
    let args = std::fs::read_to_string(&starts).unwrap();
    assert_eq!(args.lines().count(), 1, "started once: {args}");
    assert!(args.contains("--port 35120") && args.contains("--exit-on-stdin-eof"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// R4 / KP-006: the sim thread's tick with telemetry and rumble attached
/// (a real table) makes no heap allocation, including ticks whose push
/// fails and the forced full state after them.
#[test]
fn kp006_table_sim_with_telemetry_and_rumble_does_not_allocate() {
    let dir = support::greybox_bundle().to_path_buf();
    support::on_big_stack(move || {
        assert!(is_installed());
        let mut app = load_app_table(&dir, MixerConfig::default()).unwrap();
        assert!(
            !app.telemetry_map.switches.is_empty(),
            "greybox has switches"
        );
        let (tx, mut rx) = telemetry_channel(5);
        let feed = TelemetryFeed::new(tx, app.telemetry_map.clone());
        let stats = feed.stats();
        app.sim.set_telemetry(feed);
        let (htx, mut hrx) = haptics_ring::<HapticEvent>(256);
        app.sim.set_rumble(htx);
        let sim: &mut TableSim = &mut app.sim;
        let mut out = FrameStateSnapshot::default();
        let mut worst = 0u64;
        let mut impulses = 0usize;
        // The left flipper's coil (sim id) reads energised while held.
        let left_coil = {
            let m = &app.manifest;
            let f = m
                .flippers
                .iter()
                .find(|f| f.id.contains("left"))
                .expect("a left flipper");
            let def = m.coils.iter().find(|c| c.id == f.coil).unwrap();
            usize::from(def.number)
        };
        let mut coil_on_ticks = 0u64;
        let press = |t: u64, p: bool| {
            vec![InputEvent::button(
                t * TICK_NS,
                CabinetInput::LeftFlipper,
                p,
            )]
        };
        for t in 1..=4000u64 {
            // Built outside the guard.
            let inputs = match t {
                50 => vec![InputEvent::button(50 * TICK_NS, CabinetInput::Start, true)],
                t if t % 300 == 0 => press(t, true),
                t if t % 300 == 60 => press(t, false),
                _ => Vec::new(),
            };
            let ((), n) = count_allocs(|| sim.step_1ms(t, &inputs, &mut out));
            worst = worst.max(n);
            impulses += hrx.drain(|_| {});
            if sim.telemetry_sources().unwrap().coils[left_coil] {
                coil_on_ticks += 1;
            }
            // The "telemetry thread" drains the ring except during 20 ticks
            // of every 500 (back-pressure).
            if t % 500 >= 20 {
                while rx.pop().is_some() {}
            }
        }
        assert_eq!(worst, 0, "a tick allocated {worst} times");
        assert!(stats.forced_full_states() > 0, "no back-pressure exercised");
        assert!(impulses > 0, "the flipper presses gave impulses");
        let sources = sim.telemetry_sources().unwrap();
        assert_eq!(sources.switch_bits, sim.sim().switch_bits());
        assert_eq!(sim.rumble_dropped(), 0);
        assert!(sim.telemetry().is_some_and(|f| f.forced_full_states() > 0));
        // 13 presses held 60 ticks each (the coil follows the button).
        assert!(
            (13 * 50..=13 * 70).contains(&coil_on_ticks),
            "left flipper coil on for {coil_on_ticks} ticks"
        );
    });
}

/// R11 / KP-006: `config.toml` bytes around the new `[telemetry]` and
/// `[dof]` sections never panic the parser; the endpoint is never empty.
#[test]
fn kp006_config_parsing_survives_random_bytes() {
    const SEEDS: [&str; 4] = [
        "[telemetry]\nenabled = true\nendpoint = \"127.0.0.1:35120\"\n",
        "[dof]\nenabled = false\n",
        "telemetry = 1\ndof = [1]\n",
        "[telemetry]\nendpoint = \"[::1]:9\"\n[dof]\nenabled = \"x\"\n",
    ];
    let mut runner = proptest::test_runner::TestRunner::new(proptest::test_runner::Config {
        cases: 512,
        ..proptest::test_runner::Config::default()
    });
    let strategy = (
        0..SEEDS.len(),
        proptest::collection::vec(any::<u8>(), 0..256),
        any::<usize>(),
    );
    runner
        .run(&strategy, |(seed, noise, at)| {
            let mut bytes = SEEDS[seed].as_bytes().to_vec();
            let at = at % (bytes.len() + 1);
            bytes.splice(at..at, noise.iter().copied());
            let l = AppConfig::from_toml_bytes(&bytes);
            prop_assert!(!l.config.telemetry.endpoint.trim().is_empty());
            let l = AppConfig::from_toml_bytes(&noise);
            prop_assert!(!l.config.telemetry.endpoint.trim().is_empty());
            Ok(())
        })
        .unwrap();
}

/// KP-006: the impulse type the feed sends is the sim's.
#[test]
fn kp006_impulses_ride_along_as_kind_5() {
    let (tx, mut rx) = telemetry_channel(16);
    let mut feed = TelemetryFeed::new(tx, TelemetryMap::default());
    let s = FrameStateSnapshot::default();
    let src = TelemetrySources::default();
    feed.tick(0, &s, &src, &[]);
    while rx.pop().is_some() {}
    let imp = [HapticsTelemetryEvent {
        tick: 1,
        source: 0x2005,
        strength: 0.75,
    }; 70];
    feed.tick(1, &s, &src, &imp);
    let (_, _, e) = decode(rx.pop().unwrap().as_bytes());
    assert_eq!(e.len(), 64, "at most 64 impulses per tick");
    assert!(
        e.iter()
            .all(|e| e.kind == TelemetryKind::HapticImpulse && e.id == 0x2005)
    );
    assert_eq!(feed.dropped_impulses(), 6);
}

/// R3 / KP-006: attaching telemetry and rumble only reads the simulation:
/// the same inputs give the same state hash with and without them.
#[test]
fn kp006_telemetry_and_rumble_leave_the_simulation_unchanged() {
    let dir = support::greybox_bundle().to_path_buf();
    support::on_big_stack(move || {
        let run = |attach: bool| {
            let mut app = load_app_table(&dir, MixerConfig::default()).unwrap();
            let (tx, mut rx) = telemetry_channel(64);
            let (htx, mut hrx) = haptics_ring::<HapticEvent>(256);
            if attach {
                app.sim
                    .set_telemetry(TelemetryFeed::new(tx, app.telemetry_map.clone()));
                app.sim.set_rumble(htx);
            }
            let mut out = FrameStateSnapshot::default();
            for t in 1..=3000u64 {
                let inputs = match t {
                    50 => vec![InputEvent::button(50 * TICK_NS, CabinetInput::Start, true)],
                    t if t % 250 == 0 => vec![InputEvent::button(
                        t * TICK_NS,
                        CabinetInput::RightFlipper,
                        true,
                    )],
                    t if t % 250 == 40 => vec![InputEvent::button(
                        t * TICK_NS,
                        CabinetInput::RightFlipper,
                        false,
                    )],
                    _ => Vec::new(),
                };
                app.sim.step_1ms(t, &inputs, &mut out);
                while rx.pop().is_some() {}
                hrx.drain(|_| {});
            }
            app.sim.sim().state_hash()
        };
        assert_eq!(run(true), run(false));
    });
}
