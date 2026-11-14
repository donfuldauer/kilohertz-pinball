//! Record and replay.
//!
//! Owner: Subphase 2.5. Stub created by Subphase 1.1.
//!
//! A [`Recording`] is everything needed to reproduce a run on the same
//! binary and machine (D9): the RNG seed, the bundle's
//! [`table_hash`](crate::Simulation::table_hash), the number of ticks, and
//! every input event with the tick it was fed to. [`Recorder`] produces one
//! while stepping a [`Simulation`]; [`replay`] loads the bundle with the
//! recorded seed, refuses a different table, feeds the same events on the
//! same ticks and returns the final [`state_hash`](crate::Simulation::state_hash).
//! State injected through the test API while recording is not recorded.
//!
//! # File format (version 1, little-endian)
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 8 | magic `b"KHZREPL\0"` |
//! | 8 | 4 | version `u32` = 1 |
//! | 12 | 8 | seed `u64` |
//! | 20 | 8 | table hash `u64` |
//! | 28 | 8 | ticks `u64` |
//! | 36 | 8 | event count `n` `u64` |
//! | 44 | 24·n | events |
//! | 44 + 24·n | 8 | FNV-1a 64 of every byte before it |
//!
//! An event is 24 bytes: tick `u64`, timestamp (ns) `u64`, kind `u8`
//! (0 button, 1 axis), code `u8` (`CabinetInput` / `CabinetAxis` index in
//! their `ALL` order), two zero bytes, then for a button the `u32` 0/1 and
//! for an axis the `f32` bits (finite). Ticks are 1-based, non-decreasing
//! and at most `ticks`, and `ticks` is at most [`MAX_TICKS`] (2³² ticks,
//! about 49.7 days of simulated time) so a valid-looking file cannot ask for
//! a practically endless replay.
//!
//! [`Recording::from_bytes`] never panics, and allocates at most one
//! event vector sized from a count it has checked against the input length.

use core::fmt;
use pin_core::input::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use pin_core::sim::SimStep;
use std::path::Path;

use crate::hash::fnv1a64;
use crate::{SimConfig, SimError, Simulation};

/// File magic.
pub const MAGIC: [u8; 8] = *b"KHZREPL\0";
/// Current format version.
pub const VERSION: u32 = 1;
/// Header size, bytes.
pub const HEADER_BYTES: usize = 44;
/// Size of one event, bytes.
pub const EVENT_BYTES: usize = 24;
/// Largest `ticks` a recording may declare (2³² ticks ≈ 49.7 days of
/// simulated time); [`Recording::from_bytes`] and [`replay`] refuse more
/// with [`ReplayError::TooManyTicks`].
pub const MAX_TICKS: u64 = 1 << 32;
/// Size of the checksum trailer, bytes.
pub const TRAILER_BYTES: usize = 8;

/// A recorded run (see the [module documentation](self)).
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    /// RNG seed of the run ([`SimConfig::seed`]).
    pub seed: u64,
    /// [`Simulation::table_hash`] of the bundle.
    pub table_hash: u64,
    /// Number of ticks run.
    pub ticks: u64,
    /// `(tick, event)` in feeding order (ticks non-decreasing).
    pub events: Vec<(u64, InputEvent)>,
}

/// Why a recording could not be read or replayed.
#[derive(Clone, Debug, PartialEq)]
pub enum ReplayError {
    /// The bytes do not start with [`MAGIC`].
    BadMagic,
    /// A version this build cannot read.
    UnsupportedVersion(u32),
    /// The bytes end early, or carry trailing data, for the declared event
    /// count.
    BadLength {
        /// Bytes the header implies (saturating).
        expected: u64,
        /// Bytes given.
        actual: u64,
    },
    /// The checksum trailer does not match.
    Checksum,
    /// Event `index` is malformed (unknown kind or code, a non-0/1 button
    /// value, a non-finite axis value, a tick of 0, above `ticks` or
    /// decreasing).
    BadEvent {
        /// Event index.
        index: u64,
    },
    /// The recording declares more than [`MAX_TICKS`] ticks.
    TooManyTicks(u64),
    /// The bundle is not the recorded table.
    TableMismatch {
        /// The recording's table hash.
        recorded: u64,
        /// The bundle's table hash.
        bundle: u64,
    },
    /// Loading the bundle failed.
    Sim(SimError),
    /// Reading or writing a file failed.
    Io(String),
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::BadMagic => f.write_str("not a replay file (bad magic)"),
            ReplayError::UnsupportedVersion(v) => {
                write!(f, "unsupported replay version {v} (expected {VERSION})")
            }
            ReplayError::BadLength { expected, actual } => write!(
                f,
                "replay file length {actual} does not match its header ({expected} bytes)"
            ),
            ReplayError::Checksum => f.write_str("replay file checksum mismatch"),
            ReplayError::TooManyTicks(t) => {
                write!(f, "replay declares {t} ticks (at most {MAX_TICKS})")
            }
            ReplayError::BadEvent { index } => write!(f, "replay event {index} is malformed"),
            ReplayError::TableMismatch { recorded, bundle } => write!(
                f,
                "recording is for table {recorded:016x}, bundle is {bundle:016x}"
            ),
            ReplayError::Sim(e) => write!(f, "{e}"),
            ReplayError::Io(m) => write!(f, "I/O error: {m}"),
        }
    }
}

impl std::error::Error for ReplayError {}

impl From<SimError> for ReplayError {
    fn from(e: SimError) -> Self {
        ReplayError::Sim(e)
    }
}

fn input_code(i: CabinetInput) -> u8 {
    i.index() as u8
}

fn axis_code(a: CabinetAxis) -> u8 {
    CabinetAxis::ALL.iter().position(|&x| x == a).unwrap_or(0) as u8
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(a)
}

impl Recording {
    /// An empty recording for `seed` and `table_hash`.
    #[must_use]
    pub fn new(seed: u64, table_hash: u64) -> Self {
        Self {
            seed,
            table_hash,
            ticks: 0,
            events: Vec::new(),
        }
    }

    /// The file bytes (format in the [module documentation](self)).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_BYTES + EVENT_BYTES * self.events.len() + 8);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.seed.to_le_bytes());
        out.extend_from_slice(&self.table_hash.to_le_bytes());
        out.extend_from_slice(&self.ticks.to_le_bytes());
        out.extend_from_slice(&(self.events.len() as u64).to_le_bytes());
        for (tick, e) in &self.events {
            out.extend_from_slice(&tick.to_le_bytes());
            out.extend_from_slice(&e.timestamp_ns.to_le_bytes());
            let (kind, code, value) = match e.kind {
                InputEventKind::Button { input, pressed } => {
                    (0u8, input_code(input), u32::from(pressed))
                }
                InputEventKind::Axis { axis, value } => (1u8, axis_code(axis), value.to_bits()),
            };
            out.extend_from_slice(&[kind, code, 0, 0]);
            out.extend_from_slice(&value.to_le_bytes());
        }
        let sum = fnv1a64(&out);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// Parses file bytes. Never panics; the only allocation is the event
    /// vector, sized from a count already checked against `bytes.len()`.
    ///
    /// # Errors
    /// See [`ReplayError`] (`BadMagic`, `UnsupportedVersion`, `BadLength`,
    /// `Checksum`, `BadEvent`).
    pub fn from_bytes(bytes: &[u8]) -> Result<Recording, ReplayError> {
        if bytes.len() < MAGIC.len() || bytes[..MAGIC.len()] != MAGIC {
            return Err(ReplayError::BadMagic);
        }
        if bytes.len() < HEADER_BYTES + TRAILER_BYTES {
            return Err(ReplayError::BadLength {
                expected: (HEADER_BYTES + TRAILER_BYTES) as u64,
                actual: bytes.len() as u64,
            });
        }
        let version = u32_at(bytes, 8);
        if version != VERSION {
            return Err(ReplayError::UnsupportedVersion(version));
        }
        let seed = u64_at(bytes, 12);
        let table_hash = u64_at(bytes, 20);
        let ticks = u64_at(bytes, 28);
        let count = u64_at(bytes, 36);
        let expected = count
            .saturating_mul(EVENT_BYTES as u64)
            .saturating_add((HEADER_BYTES + TRAILER_BYTES) as u64);
        if expected != bytes.len() as u64 {
            return Err(ReplayError::BadLength {
                expected,
                actual: bytes.len() as u64,
            });
        }
        let body = bytes.len() - TRAILER_BYTES;
        if fnv1a64(&bytes[..body]) != u64_at(bytes, body) {
            return Err(ReplayError::Checksum);
        }
        if ticks > MAX_TICKS {
            return Err(ReplayError::TooManyTicks(ticks));
        }
        // `count` events fit in `bytes`, so this capacity is bounded by the
        // input length.
        let n = (body - HEADER_BYTES) / EVENT_BYTES;
        let mut events = Vec::with_capacity(n);
        let mut last_tick = 0u64;
        for index in 0..n {
            let at = HEADER_BYTES + index * EVENT_BYTES;
            let bad = || ReplayError::BadEvent {
                index: index as u64,
            };
            let tick = u64_at(bytes, at);
            let timestamp_ns = u64_at(bytes, at + 8);
            let (kind, code) = (bytes[at + 16], usize::from(bytes[at + 17]));
            if bytes[at + 18] != 0 || bytes[at + 19] != 0 {
                return Err(bad());
            }
            let value = u32_at(bytes, at + 20);
            if tick == 0 || tick > ticks || tick < last_tick {
                return Err(bad());
            }
            last_tick = tick;
            let ev = match kind {
                0 => {
                    let input = *CabinetInput::ALL.get(code).ok_or_else(bad)?;
                    if value > 1 {
                        return Err(bad());
                    }
                    InputEvent::button(timestamp_ns, input, value == 1)
                }
                1 => {
                    let axis = *CabinetAxis::ALL.get(code).ok_or_else(bad)?;
                    let v = f32::from_bits(value);
                    if !v.is_finite() {
                        return Err(bad());
                    }
                    InputEvent::axis(timestamp_ns, axis, v)
                }
                _ => return Err(bad()),
            };
            events.push((tick, ev));
        }
        Ok(Recording {
            seed,
            table_hash,
            ticks,
            events,
        })
    }

    /// Writes [`to_bytes`](Self::to_bytes) to `path`.
    ///
    /// # Errors
    /// [`ReplayError::Io`].
    pub fn save(&self, path: &Path) -> Result<(), ReplayError> {
        std::fs::write(path, self.to_bytes()).map_err(|e| ReplayError::Io(e.to_string()))
    }

    /// Reads and parses `path` (at most 1 GiB).
    ///
    /// # Errors
    /// [`ReplayError::Io`] and the [`from_bytes`](Self::from_bytes) errors.
    pub fn load(path: &Path) -> Result<Recording, ReplayError> {
        const MAX_FILE: u64 = 1 << 30;
        let file = std::fs::File::open(path).map_err(|e| ReplayError::Io(e.to_string()))?;
        Recording::from_bytes(&read_capped(file, MAX_FILE)?)
    }

    /// The events fed on `tick`, in order (`events` is sorted by tick).
    #[must_use]
    pub fn events_on(&self, tick: u64) -> &[(u64, InputEvent)] {
        let lo = self.events.partition_point(|(t, _)| *t < tick);
        let hi = self.events.partition_point(|(t, _)| *t <= tick);
        &self.events[lo..hi]
    }
}

/// Reads all of `r`, but at most `max + 1` bytes: more than `max` is an
/// error, whatever size the source claimed beforehand.
fn read_capped(r: impl std::io::Read, max: u64) -> Result<Vec<u8>, ReplayError> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    r.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| ReplayError::Io(e.to_string()))?;
    if bytes.len() as u64 > max {
        return Err(ReplayError::Io(format!(
            "more than {max} bytes is too large"
        )));
    }
    Ok(bytes)
}

/// Steps a [`Simulation`] and records every tick's input events.
///
/// Recording appends to a `Vec` (it allocates); it is a tool around the
/// simulation, not part of the sim-thread tick.
pub struct Recorder {
    sim: Simulation,
    rec: Recording,
}

impl Recorder {
    /// Starts recording `sim`, which must be at tick 0 (just loaded; the
    /// seed and table hash are taken from it). A recording holds only the
    /// seed and the inputs and [`replay`] starts from a fresh load, so a run
    /// that already advanced could not be reproduced.
    ///
    /// # Panics
    /// When `sim` has already run a tick.
    #[must_use]
    pub fn new(sim: Simulation) -> Self {
        assert_eq!(
            sim.tick(),
            0,
            "Recorder::new needs a simulation at tick 0 (a recording replays from load)"
        );
        let rec = Recording::new(sim.config().seed, sim.table_hash());
        Self { sim, rec }
    }

    /// Records `inputs` for the next tick, then runs that tick.
    pub fn step(&mut self, inputs: &[InputEvent]) {
        let tick = self.sim.tick() + 1;
        for e in inputs {
            self.rec.events.push((tick, *e));
        }
        self.rec.ticks = tick;
        let mut out = *self.sim.snapshot();
        self.sim.step_1ms(tick, inputs, &mut out);
    }

    /// The simulation.
    #[must_use]
    pub fn sim(&self) -> &Simulation {
        &self.sim
    }

    /// The recording so far.
    #[must_use]
    pub fn recording(&self) -> &Recording {
        &self.rec
    }

    /// Ends recording.
    #[must_use]
    pub fn finish(self) -> (Simulation, Recording) {
        (self.sim, self.rec)
    }

    /// Runs `rec`'s ticks on `sim` (normally freshly loaded with `rec.seed`),
    /// feeding the recorded events; returns the final state hash.
    pub fn play(sim: &mut Simulation, rec: &Recording) -> u64 {
        let mut scratch = Vec::new();
        let mut out = *sim.snapshot();
        let mut next = 0usize;
        for tick in (sim.tick() + 1)..=rec.ticks {
            scratch.clear();
            while next < rec.events.len() && rec.events[next].0 <= tick {
                if rec.events[next].0 == tick {
                    scratch.push(rec.events[next].1);
                }
                next += 1;
            }
            sim.step_1ms(tick, &scratch, &mut out);
        }
        sim.state_hash()
    }
}

/// Checks the bundle's table hash against `rec` (before building anything or
/// running `on_init`), loads it with the recorded seed, replays `rec` and
/// returns the final [`Simulation::state_hash`].
///
/// The script runs with the default engine limits
/// ([`SimConfig::default`]: `LuaConfig::engine()` / `WasmConfig::engine()`);
/// the recording stores only the seed, so a run recorded with other limits
/// does not replay identically.
///
/// Run it on a thread with enough stack for a Wasm script (see
/// [`Simulation`]).
///
/// # Errors
/// [`ReplayError::TooManyTicks`] above [`MAX_TICKS`];
/// [`ReplayError::Sim`] when the bundle cannot be opened or read;
/// [`ReplayError::TableMismatch`] when it is not the recorded table (even if
/// it would not load); [`ReplayError::Sim`] when the recorded table does not
/// load.
pub fn replay(bundle: &Path, rec: &Recording) -> Result<u64, ReplayError> {
    if rec.ticks > MAX_TICKS {
        return Err(ReplayError::TooManyTicks(rec.ticks));
    }
    let vfs = pin_assets::Vfs::open(bundle).map_err(SimError::from)?;
    let bundle_hash = crate::load::bundle_hash(&vfs)?;
    drop(vfs);
    if bundle_hash != rec.table_hash {
        return Err(ReplayError::TableMismatch {
            recorded: rec.table_hash,
            bundle: bundle_hash,
        });
    }
    let mut sim = Simulation::load(
        bundle,
        SimConfig {
            seed: rec.seed,
            ..SimConfig::default()
        },
    )?;
    if sim.table_hash() != rec.table_hash {
        return Err(ReplayError::TableMismatch {
            recorded: rec.table_hash,
            bundle: sim.table_hash(),
        });
    }
    Ok(Recorder::play(&mut sim, rec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_capped_enforces_the_limit_on_the_bytes_read() {
        // A source longer than the cap is rejected after reading cap + 1
        // bytes, whatever its size looked like beforehand.
        let big = std::io::repeat(7u8);
        assert!(matches!(read_capped(big, 16), Err(ReplayError::Io(_))));
        assert_eq!(read_capped(&[1u8; 16][..], 16).unwrap(), vec![1u8; 16]);
        assert!(read_capped(&[1u8; 17][..], 16).is_err());
        assert!(read_capped(&[][..], 16).unwrap().is_empty());
    }
}
