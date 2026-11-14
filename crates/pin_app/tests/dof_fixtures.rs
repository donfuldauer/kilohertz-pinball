//! Subphase 3.5: generates (and guards) the recorded telemetry fixtures the
//! .NET DOF bridge tests replay, using `pin_audio::telemetry` as the sender.
//!
//! Files in `tools/dof_bridge/tests/fixtures/`:
//! * `map.json` — `ResolvedDofMap::resolve(table.toml).to_json()`;
//! * `<name>.datagrams.bin` — records `u32 LE length` + datagram bytes;
//! * `<name>.expected.txt` — `<record index> <type> <number> <value>` per
//!   forwarded update, from the reference model below (the forwarding rules
//!   of DECISIONS.md "3.5 — DOF bridge public API").
//!
//! The test regenerates everything in memory and fails if a checked-in file
//! differs; `KHZ_REGEN_DOF_FIXTURES=1` rewrites the files instead.

use pin_app::dof::{DofSourceKind, ResolvedDofMap};
use pin_audio::telemetry::{
    DatagramBatch, HapticsTelemetryEvent, TelemetryEncoder, TelemetryKind, TelemetryState,
    decode_datagram,
};
use pin_core::schema::table::{DofElementType, TableManifest};
use std::fmt::Write as _;
use std::path::PathBuf;

const TABLE_TOML: &str = include_str!("../../../tools/dof_bridge/tests/fixtures/table.toml");

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/dof_bridge/tests/fixtures")
}

fn fixture_map() -> ResolvedDofMap {
    let table = TableManifest::from_toml_str(TABLE_TOML).expect("fixture table parses");
    ResolvedDofMap::resolve(&table)
        .expect("fixture table resolves")
        .expect("fixture table has [dof]")
}

/// The cabinet state of the fixture scenario at `t`. `variant` shifts the
/// patterns so a restarted session differs from the first one.
fn scenario(t: u64, variant: u64, st: &mut TelemetryState, imp: &mut Vec<HapticsTelemetryEvent>) {
    imp.clear();
    let t2 = t + variant * 37;
    // Flippers.
    st.coils[1] = (100..400).contains(&t2) || (1500..1600).contains(&t2);
    st.coils[2] = (120..150).contains(&t2) || (2210..2260).contains(&t2);
    // Knocker.
    st.coils[9] = (2500..2530).contains(&t2);
    // Bumper: ring switch closes 2 ticks before the coil fires, every 300 ticks.
    let b = (t2 + 100) % 300;
    st.switches[30] = (0..12).contains(&b);
    st.coils[12] = (2..22).contains(&b);
    // Sling switch.
    st.switches[21] = (700..705).contains(&t2) || (1730..1736).contains(&t2);
    // Lamp fade 0 → 1 over 1000 ticks: a change every tick, ~256 distinct DOF values.
    st.lamps[40] = if t2 < 500 {
        0.0
    } else if t2 < 1500 {
        (t2 - 500) as f32 / 1000.0
    } else {
        1.0
    };
    // Blinking insert.
    st.lamps[41] = if (t2 / 250) % 2 == 1 { 1.0 } else { 0.0 };
    // Flasher pulses (analog value to an S output).
    st.lamps[60] = if t2 % 400 < 30 { 0.7 } else { 0.0 };
    // GI strings.
    st.gi[0] = if (1800..2200).contains(&t2) { 0.5 } else { 1.0 };
    st.gi[2] = if t2 >= 50 { 1.0 } else { 0.0 };
    // Unmapped activity the bridge must ignore.
    st.coils[5] = (t2 / 50) % 2 == 1;
    st.switches[50] = (t2 / 70) % 2 == 1;
    st.lamps[100] = ((t2 * 7919) % 1000) as f32 / 1000.0;
    st.magnets[0] = if (t2 / 90).is_multiple_of(3) {
        0.25
    } else {
        0.0
    };
    if t2 % 100 == 7 {
        imp.push(HapticsTelemetryEvent {
            tick: t,
            source: 12,
            strength: 0.8,
        });
    }
}

/// Encoded datagrams of `ticks` of the scenario: `(tick, bytes)`.
fn session(ticks: std::ops::Range<u64>, variant: u64) -> Vec<(u64, Vec<u8>)> {
    let mut enc = TelemetryEncoder::new();
    let mut st = TelemetryState::new();
    let mut imp = Vec::new();
    let mut batch = DatagramBatch::new();
    let mut out = Vec::new();
    for t in ticks {
        scenario(t, variant, &mut st, &mut imp);
        enc.encode_tick(t, &st, &imp, &mut batch);
        for d in batch.iter() {
            out.push((t, d.to_vec()));
        }
    }
    out
}

fn clean_records() -> Vec<Vec<u8>> {
    session(0..3201, 0).into_iter().map(|(_, d)| d).collect()
}

fn header_bytes(magic: &[u8; 4], version: u8, flags: u8, count: u16, tick: u64) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(magic);
    v.push(version);
    v.push(flags);
    v.extend_from_slice(&count.to_le_bytes());
    v.extend_from_slice(&tick.to_le_bytes());
    v
}

/// Garbage datagrams, all of which a receiver must reject.
fn garbage(seed: u64) -> Vec<Vec<u8>> {
    let mut lamp_entry = vec![2u8, 0, 40, 0];
    lamp_entry.extend_from_slice(&1.0f32.to_le_bytes());
    let mut bad_kind = header_bytes(b"PNTL", 1, 0, 1, 5000 + seed);
    bad_kind.extend_from_slice(&[6, 0, 40, 0, 0, 0, 128, 63]);
    let mut bad_magic = header_bytes(b"PNTX", 1, 0, 1, 5000 + seed);
    bad_magic.extend_from_slice(&lamp_entry);
    let mut bad_version = header_bytes(b"PNTL", 2, 0, 1, 5000 + seed);
    bad_version.extend_from_slice(&lamp_entry);
    let mut short_len = header_bytes(b"PNTL", 1, 0, 2, 5000 + seed);
    short_len.extend_from_slice(&lamp_entry);
    let too_many = header_bytes(b"PNTL", 1, 0, 181, 5000 + seed);
    let mut noise = Vec::new();
    let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ seed;
    for _ in 0..40 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        noise.push(x as u8);
    }
    vec![
        b"PNTL\x01".to_vec(),
        bad_magic,
        bad_version,
        short_len,
        too_many,
        bad_kind,
        noise,
    ]
}

/// The bridge joins mid-session, the network loses, reorders and duplicates
/// datagrams, garbage arrives, and the sender restarts.
fn faulty_records() -> Vec<Vec<u8>> {
    let a = session(0..1700, 0);
    // Join late: the first full state (tick 0) and early deltas are missed.
    let mut a: Vec<(u64, Vec<u8>)> = a.into_iter().filter(|(t, _)| *t >= 250).collect();
    // Reordering: swap neighbours.
    let mut i = 5;
    while i + 1 < a.len() {
        a.swap(i, i + 1);
        i += 23;
    }
    let mut out = Vec::new();
    for (n, (_, d)) in a.iter().enumerate() {
        if n % 37 == 11 {
            continue; // lost
        }
        out.push(d.clone());
        if n % 53 == 3 {
            out.push(d.clone()); // duplicated
        }
        if n % 211 == 100 {
            out.extend(garbage(n as u64));
        }
    }
    // Sender restart (new session from tick 0, different pattern phase).
    let mut b = session(0..1300, 1);
    // A delta of tick 990..999 arrives after the first datagram of the
    // tick-1000 full state: stale, must be ignored.
    let full_1000 = b
        .iter()
        .position(|(t, _)| *t == 1000)
        .expect("full state at 1000");
    let late = (0..full_1000)
        .rev()
        .find(|&k| b[k].0 >= 990)
        .expect("a delta just before the full state");
    let moved = b.remove(late);
    b.insert(full_1000, moved); // after the first full-state datagram (which shifted down by one)
    out.extend(b.into_iter().map(|(_, d)| d));
    out
}

/// Reference model of the bridge's forwarding rules.
struct Model {
    map: Vec<(u8, u16, char, u16)>,
    last_sent: Vec<Option<i32>>,
    resync_sent: Vec<bool>,
    elem_tick: std::collections::BTreeMap<(u8, u16), u64>,
    has_full: bool,
    last_full: u64,
    resync_tick: Option<u64>,
}

fn convert(kind: u8, v: f32) -> i32 {
    if kind <= 1 {
        i32::from(v >= 0.5)
    } else if v.is_nan() {
        0
    } else {
        (f64::from(v).clamp(0.0, 1.0) * 255.0 + 0.5).floor() as i32
    }
}

fn type_char(t: DofElementType) -> char {
    match t {
        DofElementType::S => 'S',
        DofElementType::W => 'W',
        DofElementType::L => 'L',
        DofElementType::G => 'G',
        DofElementType::E => 'E',
    }
}

impl Model {
    fn new(map: &ResolvedDofMap) -> Self {
        let m: Vec<_> = map
            .entries
            .iter()
            .map(|e| {
                (
                    e.source.telemetry_kind(),
                    e.id,
                    type_char(e.element_type),
                    e.number,
                )
            })
            .collect();
        Self {
            last_sent: vec![None; m.len()],
            resync_sent: vec![false; m.len()],
            map: m,
            elem_tick: Default::default(),
            has_full: false,
            last_full: 0,
            resync_tick: None,
        }
    }

    fn process(&mut self, record: usize, bytes: &[u8], out: &mut String) {
        let Ok((h, entries)) = decode_datagram(bytes) else {
            return;
        };
        let full = h.is_full_state();
        if full {
            if !self.has_full || h.tick < self.last_full {
                self.has_full = true;
                self.last_full = h.tick;
                self.resync_tick = Some(h.tick);
                self.elem_tick.clear();
                self.resync_sent.iter_mut().for_each(|s| *s = false);
            } else if h.tick > self.last_full {
                self.last_full = h.tick;
            }
        } else if self.has_full && h.tick < self.last_full {
            return;
        }
        let forcing = full && self.resync_tick == Some(h.tick);
        for e in entries {
            let kind = match e.kind {
                TelemetryKind::Switch => 0,
                TelemetryKind::Coil => 1,
                TelemetryKind::Lamp => 2,
                TelemetryKind::Gi => 3,
                _ => continue,
            };
            if !self.map.iter().any(|m| m.0 == kind && m.1 == e.id) {
                continue;
            }
            if self
                .elem_tick
                .get(&(kind, e.id))
                .is_some_and(|&t| h.tick < t)
            {
                continue;
            }
            self.elem_tick.insert((kind, e.id), h.tick);
            let v = convert(kind, e.value);
            for (i, m) in self.map.iter().enumerate() {
                if m.0 != kind || m.1 != e.id {
                    continue;
                }
                if (forcing && !self.resync_sent[i]) || self.last_sent[i] != Some(v) {
                    writeln!(out, "{record} {} {} {v}", m.2, m.3).unwrap();
                    self.last_sent[i] = Some(v);
                }
                if forcing {
                    self.resync_sent[i] = true;
                }
            }
        }
    }
}

fn expected(map: &ResolvedDofMap, records: &[Vec<u8>], name: &str) -> String {
    let mut out = format!(
        "# {name}: forwarded updates `<record index> <type> <number> <value>` (generated by crates/pin_app/tests/dof_fixtures.rs)\n"
    );
    let mut model = Model::new(map);
    for (i, r) in records.iter().enumerate() {
        model.process(i, r, &mut out);
    }
    out
}

fn bin(records: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in records {
        out.extend_from_slice(&(r.len() as u32).to_le_bytes());
        out.extend_from_slice(r);
    }
    out
}

fn check_or_write(name: &str, data: &[u8]) {
    let path = fixtures_dir().join(name);
    if std::env::var_os("KHZ_REGEN_DOF_FIXTURES").is_some() {
        std::fs::write(&path, data).unwrap();
        return;
    }
    let on_disk = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (run with KHZ_REGEN_DOF_FIXTURES=1 to generate)",
            path.display()
        )
    });
    assert!(
        on_disk == data,
        "{} is out of date (run with KHZ_REGEN_DOF_FIXTURES=1 to regenerate)",
        path.display()
    );
}

#[test]
fn fixture_map_matches() {
    let map = fixture_map();
    assert_eq!(map.rom_name, "dof_fixture_rom");
    assert_eq!(map.entries.len(), 12);
    assert_eq!(map.entries[10].source, DofSourceKind::Gi);
    assert_eq!(map.entries[10].id, 0);
    check_or_write("map.json", map.to_json().as_bytes());
}

#[test]
fn clean_fixture_matches() {
    let map = fixture_map();
    let records = clean_records();
    // Every record is a valid datagram; 4 full states of 5 datagrams each.
    let fulls = records
        .iter()
        .filter(|r| decode_datagram(r).unwrap().0.is_full_state())
        .count();
    assert_eq!(fulls, 20);
    let exp = expected(&map, &records, "clean");
    // The resync sends all 12 mappings once; the fade forwards 255 steps.
    assert!(exp.lines().filter(|l| l.contains(" L 40 ")).count() >= 255);
    check_or_write("clean.datagrams.bin", &bin(&records));
    check_or_write("clean.expected.txt", exp.as_bytes());
}

#[test]
fn faulty_fixture_matches() {
    let map = fixture_map();
    let records = faulty_records();
    let rejected = records
        .iter()
        .filter(|r| decode_datagram(r).is_err())
        .count();
    assert_eq!(rejected, 7 * 7, "garbage records: 7 kinds at 7 places");
    let exp = expected(&map, &records, "faulty");
    check_or_write("faulty.datagrams.bin", &bin(&records));
    check_or_write("faulty.expected.txt", exp.as_bytes());
}

#[test]
fn model_conversion_rules() {
    assert_eq!(convert(0, 1.0), 1);
    assert_eq!(convert(1, 0.0), 0);
    assert_eq!(convert(1, f32::NAN), 0);
    assert_eq!(convert(2, 0.5), 128);
    assert_eq!(convert(3, 2.0), 255);
    assert_eq!(convert(2, -1.0), 0);
    assert_eq!(convert(2, f32::NAN), 0);
}
