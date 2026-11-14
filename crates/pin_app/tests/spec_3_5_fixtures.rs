//! Subphase 3.5 spec helper (Independent Tester): generates the recorded
//! telemetry session the .NET spec tests (`tools/dof_bridge/tests/Spec.DofBridge`)
//! replay into the bridge, together with the exact DOF update sequence the
//! brief demands, and checks that the checked-in copies are up to date.
//!
//! The sender here is an independent model of `docs/TELEMETRY_PROTOCOL.md`
//! section 4 (diff, order, full state, split) on top of the merged wire
//! encoder `pin_core::telemetry::encode_datagram`. The oracle is an
//! independent model of the listener guidance (section 6) plus the brief's
//! value mapping (0/1 for switches and coils, 0..=255 for lamps and GI) and
//! change-only forwarding.
//!
//! Regenerate the fixtures with `SPEC_3_5_WRITE_FIXTURES=1 cargo test -p pin_app
//! --test spec_3_5_fixtures`.

use std::path::PathBuf;

use pin_core::telemetry::{
    FLAG_FULL_STATE, MAX_DATAGRAM_LEN, MAX_ENTRIES_PER_DATAGRAM, TelemetryEntry, TelemetryKind,
    decode_datagram, encode_datagram,
};

const SWITCHES: usize = 256;
const COILS: usize = 256;
const LAMPS: usize = 256;
const GI: usize = 32;
const MAGNETS: usize = 32;
const FULL_INTERVAL: u64 = 1000;

#[derive(Clone, PartialEq)]
struct State {
    switches: [f32; SWITCHES],
    coils: [f32; COILS],
    lamps: [f32; LAMPS],
    gi: [f32; GI],
    magnets: [f32; MAGNETS],
}

impl State {
    fn new() -> Self {
        State {
            switches: [0.0; SWITCHES],
            coils: [0.0; COILS],
            lamps: [0.0; LAMPS],
            gi: [0.0; GI],
            magnets: [0.0; MAGNETS],
        }
    }

    fn groups(&self) -> [(TelemetryKind, &[f32]); 5] {
        [
            (TelemetryKind::Switch, &self.switches[..]),
            (TelemetryKind::Coil, &self.coils[..]),
            (TelemetryKind::Lamp, &self.lamps[..]),
            (TelemetryKind::Gi, &self.gi[..]),
            (TelemetryKind::Magnet, &self.magnets[..]),
        ]
    }
}

/// Protocol section 4 sender model.
struct Sender {
    prev: State,
    last_full: Option<u64>,
    last_tick: Option<u64>,
}

impl Sender {
    fn new() -> Self {
        Sender {
            prev: State::new(),
            last_full: None,
            last_tick: None,
        }
    }

    /// Datagrams of one tick (empty when nothing changed and no full state is due).
    fn tick(&mut self, tick: u64, state: &State, impulses: &[(u16, f32)]) -> Vec<Vec<u8>> {
        let full = match (self.last_full, self.last_tick) {
            (None, _) => true,
            (Some(_), Some(t)) if tick < t => true,
            (Some(f), _) => tick - f >= FULL_INTERVAL,
        };
        let mut entries = Vec::new();
        for ((kind, now), (_, before)) in state.groups().into_iter().zip(self.prev.groups()) {
            for (id, (&v, &p)) in now.iter().zip(before).enumerate() {
                if full || v.to_bits() != p.to_bits() {
                    entries.push(TelemetryEntry {
                        kind,
                        id: id as u16,
                        value: v,
                    });
                }
            }
        }
        for &(id, value) in impulses {
            entries.push(TelemetryEntry {
                kind: TelemetryKind::HapticImpulse,
                id,
                value,
            });
        }
        self.prev = state.clone();
        self.last_tick = Some(tick);
        if full {
            self.last_full = Some(tick);
        }
        let flags = if full { FLAG_FULL_STATE } else { 0 };
        entries
            .chunks(MAX_ENTRIES_PER_DATAGRAM)
            .map(|chunk| {
                let mut buf = [0u8; MAX_DATAGRAM_LEN];
                let n = encode_datagram(tick, flags, chunk, &mut buf).expect("encodes");
                buf[..n].to_vec()
            })
            .collect()
    }
}

/// The mapping the .NET spec tests hand to the bridge for this fixture:
/// (telemetry kind, engine id, DOF type char, DOF number).
const MAPPING: &[(TelemetryKind, u16, char, u16)] = &[
    (TelemetryKind::Coil, 3, 'S', 1),
    (TelemetryKind::Coil, 7, 'S', 12),
    (TelemetryKind::Switch, 10, 'W', 20),
    (TelemetryKind::Switch, 200, 'W', 21),
    (TelemetryKind::Lamp, 5, 'L', 30),
    (TelemetryKind::Lamp, 250, 'L', 31),
    (TelemetryKind::Gi, 2, 'G', 1),
    (TelemetryKind::Gi, 31, 'G', 2),
];

/// Listener model: protocol section 6 + the brief's value mapping and
/// change-only forwarding.
struct Oracle {
    last: Vec<Option<i32>>,
    last_full_tick: Option<u64>,
    out: Vec<(char, u16, i32)>,
}

impl Oracle {
    fn new() -> Self {
        Oracle {
            last: vec![None; MAPPING.len()],
            last_full_tick: None,
            out: Vec::new(),
        }
    }

    fn receive(&mut self, bytes: &[u8]) {
        let Ok((header, entries)) = decode_datagram(bytes) else {
            return;
        };
        if header.is_full_state() {
            self.last_full_tick = Some(header.tick);
        } else if self.last_full_tick.is_some_and(|f| header.tick < f) {
            return;
        }
        for e in &entries {
            for (slot, &(kind, id, ty, number)) in MAPPING.iter().enumerate() {
                if kind != e.kind || id != e.id {
                    continue;
                }
                let v = match kind {
                    TelemetryKind::Switch | TelemetryKind::Coil => i32::from(e.value != 0.0),
                    _ => (f64::from(e.value) * 255.0).round() as i32,
                };
                if self.last[slot] != Some(v) {
                    self.last[slot] = Some(v);
                    self.out.push((ty, number, v));
                }
            }
        }
    }
}

struct Fixture {
    /// Datagrams as the listener receives them.
    delivered: Vec<Vec<u8>>,
    /// Expected `(type, number, value)` sink calls, in order.
    expected: Vec<(char, u16, i32)>,
}

/// The recorded session. One datagram (tick 700) is lost on the wire; the
/// periodic full state at tick 1000 repairs the mirror.
fn session() -> Fixture {
    let mut tx = Sender::new();
    let mut st = State::new();
    let mut sent: Vec<(u64, Vec<Vec<u8>>)> = Vec::new();
    let mut step = |tick: u64, st: &State, impulses: &[(u16, f32)], tx: &mut Sender| {
        sent.push((tick, tx.tick(tick, st, impulses)));
    };

    // Tick 0: table load, first full state.
    st.switches[10] = 1.0;
    st.switches[11] = 1.0; // unmapped
    st.lamps[5] = 0.2;
    st.lamps[6] = 0.5; // unmapped
    st.gi[2] = 1.0;
    st.magnets[1] = 0.7; // magnets are never mapped
    step(0, &st, &[], &mut tx);
    // Tick 5: a coil fires, with its haptic impulse.
    st.coils[3] = 1.0;
    step(5, &st, &[(3, 1.0)], &mut tx);
    // Tick 6: no change, no datagram.
    step(6, &st, &[], &mut tx);
    // Tick 45: switch + coil in one datagram (switch entries come first).
    st.coils[3] = 0.0;
    st.switches[200] = 1.0;
    step(45, &st, &[], &mut tx);
    // Tick 100: levels.
    st.lamps[5] = 0.6;
    st.lamps[6] = 0.1;
    st.gi[31] = 0.4;
    step(100, &st, &[], &mut tx);
    st.lamps[250] = 1.0;
    st.coils[4] = 1.0; // unmapped
    step(101, &st, &[], &mut tx);
    // Tick 102: only unmapped elements and an impulse.
    st.magnets[1] = 0.25;
    st.switches[11] = 0.0;
    step(102, &st, &[(0x2001, 0.5)], &mut tx);
    st.switches[10] = 0.0;
    st.coils[7] = 1.0;
    st.magnets[1] = 0.0;
    step(300, &st, &[], &mut tx);
    // Tick 700: this datagram is lost.
    st.lamps[5] = 0.8;
    st.switches[200] = 0.0;
    step(700, &st, &[], &mut tx);
    st.coils[7] = 0.0;
    step(701, &st, &[], &mut tx);
    // Tick 1000: periodic full state, nothing changed at the sender.
    step(1000, &st, &[], &mut tx);
    st.gi[2] = 0.0;
    step(1001, &st, &[], &mut tx);
    st.lamps[250] = 0.2;
    step(1500, &st, &[], &mut tx);
    // Tick 2000: periodic full state that coincides with a change.
    st.coils[3] = 1.0;
    step(2000, &st, &[], &mut tx);
    st.coils[3] = 0.0;
    step(2001, &st, &[], &mut tx);

    let mut delivered = Vec::new();
    for (tick, datagrams) in sent {
        match tick {
            0 | 1000 | 2000 => assert_eq!(datagrams.len(), 5, "full state at {tick}"),
            6 => assert!(datagrams.is_empty()),
            _ => assert_eq!(datagrams.len(), 1, "delta at {tick}"),
        }
        if tick != 700 {
            delivered.extend(datagrams);
        }
    }
    let mut oracle = Oracle::new();
    for d in &delivered {
        oracle.receive(d);
    }
    Fixture {
        delivered,
        expected: oracle.out,
    }
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/dof_bridge/tests/Spec.DofBridge/fixtures")
}

/// `[u16 LE length][datagram]` records.
fn recording_bytes(datagrams: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for d in datagrams {
        out.extend_from_slice(&(d.len() as u16).to_le_bytes());
        out.extend_from_slice(d);
    }
    out
}

fn expected_text(expected: &[(char, u16, i32)]) -> String {
    expected
        .iter()
        .map(|(t, n, v)| format!("{t} {n} {v}\n"))
        .collect()
}

fn mapping_text() -> String {
    MAPPING
        .iter()
        .map(|(kind, id, t, n)| format!("{} {id} {t} {n}\n", *kind as u8))
        .collect()
}

#[test]
fn hand_checked_expected_sequence() {
    // The oracle output, written out by hand from the scenario, so the oracle
    // itself is pinned.
    let want: &[(char, u16, i32)] = &[
        // first full state: every mapped element, in datagram entry order
        ('W', 20, 1),
        ('W', 21, 0),
        ('S', 1, 0),
        ('S', 12, 0),
        ('L', 30, 51),
        ('L', 31, 0),
        ('G', 1, 255),
        ('G', 2, 0),
        // tick 5
        ('S', 1, 1),
        // tick 45
        ('W', 21, 1),
        ('S', 1, 0),
        // tick 100, 101
        ('L', 30, 153),
        ('G', 2, 102),
        ('L', 31, 255),
        // tick 300
        ('W', 20, 0),
        ('S', 12, 1),
        // tick 701 (700 was lost)
        ('S', 12, 0),
        // full state at 1000 repairs the two lost changes and nothing else
        ('W', 21, 0),
        ('L', 30, 204),
        // tick 1001, 1500
        ('G', 1, 0),
        ('L', 31, 51),
        // full state at 2000 carries one real change
        ('S', 1, 1),
        ('S', 1, 0),
    ];
    assert_eq!(session().expected, want);
}

#[test]
fn every_recorded_datagram_is_valid_protocol_v1() {
    let f = session();
    assert_eq!(f.delivered.len(), 5 + 6 + 1 + 5 + 2 + 5 + 1);
    let mut last_tick = 0;
    for d in &f.delivered {
        let (header, entries) = decode_datagram(d).expect("valid datagram");
        assert_eq!(d.len(), 16 + 8 * entries.len());
        assert!(header.tick >= last_tick);
        last_tick = header.tick;
    }
}

#[test]
fn checked_in_fixtures_match_the_generator() {
    let f = session();
    let files = [
        ("session.bin", recording_bytes(&f.delivered)),
        (
            "session.expected.txt",
            expected_text(&f.expected).into_bytes(),
        ),
        ("session.mapping.txt", mapping_text().into_bytes()),
    ];
    let dir = fixture_dir();
    if std::env::var_os("SPEC_3_5_WRITE_FIXTURES").is_some() {
        std::fs::create_dir_all(&dir).expect("fixture dir");
        for (name, bytes) in &files {
            std::fs::write(dir.join(name), bytes).expect("write fixture");
        }
    }
    for (name, bytes) in &files {
        let on_disk = std::fs::read(dir.join(name))
            .unwrap_or_else(|e| panic!("{name}: {e} (run with SPEC_3_5_WRITE_FIXTURES=1)"));
        assert!(
            on_disk == *bytes,
            "{name} is stale; regenerate with SPEC_3_5_WRITE_FIXTURES=1"
        );
    }
}
