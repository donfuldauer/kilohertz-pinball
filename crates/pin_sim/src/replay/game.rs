//! Recorded bot games (gate-a2-cr1): the effective cabinet input of one
//! autoplay game, replayable without the bot.
//!
//! The autoplay [`Bot`](crate::autoplay::Bot) does not feed
//! [`InputEvent`](pin_core::input::InputEvent)s: between ticks it forces
//! the cabinet switches with [`Simulation::set_switch`] /
//! [`Simulation::release_switch`] (which update the switch state at once)
//! and applies nudges as a cabinet velocity change on the mechs. A
//! [`Recording`](super::Recording) of input events fed through ingestion
//! would therefore not reproduce the game. A [`GameRecording`] holds
//! exactly those operations instead ([`GameOp`]), each with the tick it was
//! applied before, and [`replay_game`] applies the same operations through
//! the same calls at the same tick boundaries, then advances one tick, with
//! no bot running. Same binary and machine (D9) gives the same final
//! [`state_hash`](Simulation::state_hash) and score as the bot game.
//!
//! The format is a sibling of the [`Recording`](super::Recording) file
//! (which is unchanged): its own magic, the same header layout, checksum
//! trailer and [`MAX_TICKS`] bound.
//!
//! # File format (version 1, little-endian)
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 8 | magic `b"KHZGAME\0"` |
//! | 8 | 4 | version `u32` = 1 |
//! | 12 | 8 | seed `u64` |
//! | 20 | 8 | table hash `u64` |
//! | 28 | 8 | ticks `u64` |
//! | 36 | 8 | operation count `n` `u64` |
//! | 44 | 24·n | operations |
//! | 44 + 24·n | 8 | FNV-1a 64 of every byte before it |
//!
//! An operation is 24 bytes: tick `u64` (the tick the operation is applied
//! before, 1-based, non-decreasing, at most `ticks`), kind `u8`, code `u8`,
//! two zero bytes, then 12 payload bytes:
//!
//! * kind 0, set switch: code is the `CabinetInput` index (in
//!   `CabinetInput::ALL` order), payload `u32` 0/1 (open/closed) then eight
//!   zero bytes;
//! * kind 1, release switch: code as kind 0, payload all zero;
//! * kind 2, nudge: code 0, payload the cabinet velocity change, three
//!   finite `f32` (x, y, z, m/s).
//!
//! [`GameRecording::from_bytes`] never panics, and allocates at most one
//! operation vector sized from a count it has checked against the input
//! length.

use core::fmt;
use glam::Vec3;
use pin_core::input::CabinetInput;
use std::path::Path;

use super::{MAX_TICKS, ReplayError, read_capped, u32_at, u64_at};
use crate::hash::fnv1a64;
use crate::{SimConfig, SimError, Simulation};

/// File magic of a [`GameRecording`] (distinct from [`super::MAGIC`]).
pub const GAME_MAGIC: [u8; 8] = *b"KHZGAME\0";
/// Current [`GameRecording`] format version.
pub const GAME_VERSION: u32 = 1;
/// Header size, bytes.
pub const GAME_HEADER_BYTES: usize = 44;
/// Size of one operation, bytes.
pub const GAME_OP_BYTES: usize = 24;
/// Size of the checksum trailer, bytes.
pub const GAME_TRAILER_BYTES: usize = 8;
/// File-name extension of a recorded game.
pub const GAME_EXTENSION: &str = "khzrec";
/// Largest file [`GameRecording::load`] reads, bytes (1 GiB).
pub const GAME_MAX_FILE_BYTES: u64 = 1 << 30;

/// One operation the bot applied to the simulation between two ticks.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum GameOp {
    /// [`Simulation::set_switch`] of the cabinet switch `input`.
    SetSwitch {
        /// The cabinet switch.
        input: CabinetInput,
        /// Forced closed (`true`) or open.
        closed: bool,
    },
    /// [`Simulation::release_switch`] of the cabinet switch `input`.
    ReleaseSwitch {
        /// The cabinet switch.
        input: CabinetInput,
    },
    /// A cabinet nudge: the velocity change the bot applies to the mechs
    /// (m/s, finite).
    Nudge {
        /// Cabinet velocity change `[x, y, z]`, m/s.
        dv: [f32; 3],
    },
}

/// Receives the operations a [`Bot`](crate::autoplay::Bot) applies, with the
/// tick they are applied before (`sim.tick() + 1`).
///
/// `()` is the no-op sink (it compiles to nothing); a [`GameRecording`]
/// appends them.
pub trait GameOpSink {
    /// Called once per applied operation, in application order.
    fn op(&mut self, tick: u64, op: GameOp);
}

impl GameOpSink for () {
    #[inline(always)]
    fn op(&mut self, _tick: u64, _op: GameOp) {}
}

impl GameOpSink for GameRecording {
    fn op(&mut self, tick: u64, op: GameOp) {
        self.ops.push((tick, op));
    }
}

/// A recorded bot game (see the [module documentation](self)).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct GameRecording {
    /// Simulation seed of the game ([`SimConfig::seed`]).
    pub seed: u64,
    /// [`Simulation::table_hash`] of the bundle.
    pub table_hash: u64,
    /// Number of ticks the game ran.
    pub ticks: u64,
    /// `(tick, operation)` in application order (ticks non-decreasing).
    pub ops: Vec<(u64, GameOp)>,
}

/// The result of [`replay_game`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GameReplay {
    /// Final [`Simulation::state_hash`].
    pub state_hash: u64,
    /// Final [`Simulation::score`] of player 0.
    pub score: Option<i64>,
    /// Final [`Simulation::tick`].
    pub ticks: u64,
}

impl fmt::Display for GameReplay {
    /// The two stdout lines of `pin_sim <bundle> --replay-game FILE`:
    /// `state_hash=<16 hex>` and `score=<i64 or none>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "state_hash={:016x}\nscore=", self.state_hash)?;
        match self.score {
            Some(s) => write!(f, "{s}"),
            None => f.write_str("none"),
        }
    }
}

impl GameRecording {
    /// An empty recording for `seed` and `table_hash`.
    #[must_use]
    pub fn new(seed: u64, table_hash: u64) -> Self {
        Self {
            seed,
            table_hash,
            ticks: 0,
            ops: Vec::new(),
        }
    }

    /// The file bytes (format in the [module documentation](self)).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            GAME_HEADER_BYTES + GAME_OP_BYTES * self.ops.len() + GAME_TRAILER_BYTES,
        );
        out.extend_from_slice(&GAME_MAGIC);
        out.extend_from_slice(&GAME_VERSION.to_le_bytes());
        out.extend_from_slice(&self.seed.to_le_bytes());
        out.extend_from_slice(&self.table_hash.to_le_bytes());
        out.extend_from_slice(&self.ticks.to_le_bytes());
        out.extend_from_slice(&(self.ops.len() as u64).to_le_bytes());
        for (tick, op) in &self.ops {
            out.extend_from_slice(&tick.to_le_bytes());
            let mut payload = [0u32; 3];
            let (kind, code) = match *op {
                GameOp::SetSwitch { input, closed } => {
                    payload[0] = u32::from(closed);
                    (0u8, input.index() as u8)
                }
                GameOp::ReleaseSwitch { input } => (1u8, input.index() as u8),
                GameOp::Nudge { dv } => {
                    payload = dv.map(f32::to_bits);
                    (2u8, 0u8)
                }
            };
            out.extend_from_slice(&[kind, code, 0, 0]);
            for p in payload {
                out.extend_from_slice(&p.to_le_bytes());
            }
        }
        let sum = fnv1a64(&out);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// Parses file bytes. Never panics; the only allocation is the
    /// operation vector, sized from a count already checked against
    /// `bytes.len()`.
    ///
    /// # Errors
    /// [`ReplayError::BadMagic`], [`ReplayError::BadLength`],
    /// [`ReplayError::UnsupportedVersion`], [`ReplayError::Checksum`],
    /// [`ReplayError::TooManyTicks`], and [`ReplayError::BadEvent`] for a
    /// malformed operation (unknown kind or code, nonzero padding, a
    /// switch value other than 0/1, a non-finite nudge, a tick of 0, above
    /// `ticks` or decreasing).
    pub fn from_bytes(bytes: &[u8]) -> Result<GameRecording, ReplayError> {
        if bytes.len() < GAME_MAGIC.len() || bytes[..GAME_MAGIC.len()] != GAME_MAGIC {
            return Err(ReplayError::BadMagic);
        }
        if bytes.len() < GAME_HEADER_BYTES + GAME_TRAILER_BYTES {
            return Err(ReplayError::BadLength {
                expected: (GAME_HEADER_BYTES + GAME_TRAILER_BYTES) as u64,
                actual: bytes.len() as u64,
            });
        }
        let version = u32_at(bytes, 8);
        if version != GAME_VERSION {
            return Err(ReplayError::UnsupportedVersion(version));
        }
        let seed = u64_at(bytes, 12);
        let table_hash = u64_at(bytes, 20);
        let ticks = u64_at(bytes, 28);
        let count = u64_at(bytes, 36);
        let expected = count
            .saturating_mul(GAME_OP_BYTES as u64)
            .saturating_add((GAME_HEADER_BYTES + GAME_TRAILER_BYTES) as u64);
        if expected != bytes.len() as u64 {
            return Err(ReplayError::BadLength {
                expected,
                actual: bytes.len() as u64,
            });
        }
        let body = bytes.len() - GAME_TRAILER_BYTES;
        if fnv1a64(&bytes[..body]) != u64_at(bytes, body) {
            return Err(ReplayError::Checksum);
        }
        if ticks > MAX_TICKS {
            return Err(ReplayError::TooManyTicks(ticks));
        }
        // `count` operations fit in `bytes`, so this capacity is bounded by
        // the input length.
        let n = (body - GAME_HEADER_BYTES) / GAME_OP_BYTES;
        let mut ops = Vec::with_capacity(n);
        let mut last_tick = 0u64;
        for index in 0..n {
            let at = GAME_HEADER_BYTES + index * GAME_OP_BYTES;
            let bad = || ReplayError::BadEvent {
                index: index as u64,
            };
            let tick = u64_at(bytes, at);
            if tick == 0 || tick > ticks || tick < last_tick {
                return Err(bad());
            }
            last_tick = tick;
            let (kind, code) = (bytes[at + 8], usize::from(bytes[at + 9]));
            if bytes[at + 10] != 0 || bytes[at + 11] != 0 {
                return Err(bad());
            }
            let p = [
                u32_at(bytes, at + 12),
                u32_at(bytes, at + 16),
                u32_at(bytes, at + 20),
            ];
            let op = match kind {
                0 | 1 => {
                    let input = *CabinetInput::ALL.get(code).ok_or_else(bad)?;
                    if p[1] != 0 || p[2] != 0 {
                        return Err(bad());
                    }
                    if kind == 0 {
                        if p[0] > 1 {
                            return Err(bad());
                        }
                        GameOp::SetSwitch {
                            input,
                            closed: p[0] == 1,
                        }
                    } else {
                        if p[0] != 0 {
                            return Err(bad());
                        }
                        GameOp::ReleaseSwitch { input }
                    }
                }
                2 => {
                    let dv = p.map(f32::from_bits);
                    if code != 0 || !dv.iter().all(|v| v.is_finite()) {
                        return Err(bad());
                    }
                    GameOp::Nudge { dv }
                }
                _ => return Err(bad()),
            };
            ops.push((tick, op));
        }
        Ok(GameRecording {
            seed,
            table_hash,
            ticks,
            ops,
        })
    }

    /// Writes [`to_bytes`](Self::to_bytes) to `path`.
    ///
    /// # Errors
    /// [`ReplayError::Io`].
    pub fn save(&self, path: &Path) -> Result<(), ReplayError> {
        std::fs::write(path, self.to_bytes())
            .map_err(|e| ReplayError::Io(format!("{}: {e}", path.display())))
    }

    /// Reads and parses `path` (at most [`GAME_MAX_FILE_BYTES`]).
    ///
    /// # Errors
    /// [`ReplayError::Io`] and the [`from_bytes`](Self::from_bytes) errors.
    pub fn load(path: &Path) -> Result<GameRecording, ReplayError> {
        let file = std::fs::File::open(path)
            .map_err(|e| ReplayError::Io(format!("{}: {e}", path.display())))?;
        GameRecording::from_bytes(&read_capped(file, GAME_MAX_FILE_BYTES)?)
    }

    /// Applies `rec`'s operations to `sim` (normally freshly loaded with
    /// `rec.seed` and [`SimConfig::default`]'s limits) and advances it to
    /// `rec.ticks`: before each tick `t` it applies the operations recorded
    /// for `t`, in order, then calls `sim.advance(1)`; returns the result.
    ///
    /// # Errors
    /// [`ReplayError::BadEvent`] when an operation cannot be applied (its
    /// cabinet switch does not exist on this table).
    pub fn play(sim: &mut Simulation, rec: &GameRecording) -> Result<GameReplay, ReplayError> {
        let mut next = 0usize;
        while sim.tick() < rec.ticks {
            let now = sim.tick() + 1;
            while let Some(&(tick, op)) = rec.ops.get(next) {
                if tick > now {
                    break;
                }
                // Ticks are non-decreasing and start at 1, so an operation
                // for an earlier tick only exists when `sim` did not start
                // at tick 0; it is skipped like `Recorder::play` does.
                if tick == now {
                    apply(sim, op).map_err(|_| ReplayError::BadEvent { index: next as u64 })?;
                }
                next += 1;
            }
            sim.advance(1);
        }
        Ok(GameReplay {
            state_hash: sim.state_hash(),
            score: sim.score(0),
            ticks: sim.tick(),
        })
    }
}

/// Applies one operation exactly as the bot does.
fn apply(sim: &mut Simulation, op: GameOp) -> Result<(), SimError> {
    match op {
        GameOp::SetSwitch { input, closed } => sim.set_switch(input.name(), closed),
        GameOp::ReleaseSwitch { input } => sim.release_switch(input.name()),
        GameOp::Nudge { dv } => {
            sim.mechs.nudge(Vec3::from_array(dv));
            Ok(())
        }
    }
}

/// Checks the bundle's table hash against `rec` (before building anything or
/// running `on_init`), loads it with the recorded seed and
/// [`SimConfig::default`]'s script limits, replays the recorded bot
/// operations with no bot ([`GameRecording::play`]) and returns the final
/// state hash, score and tick.
///
/// Run it on a thread with enough stack for a Wasm script (see
/// [`Simulation`]).
///
/// # Errors
/// [`ReplayError::TooManyTicks`] above [`MAX_TICKS`];
/// [`ReplayError::Sim`] when the bundle cannot be opened or read;
/// [`ReplayError::TableMismatch`] when it is not the recorded table;
/// [`ReplayError::Sim`] when the recorded table does not load;
/// [`ReplayError::BadEvent`] when an operation cannot be applied.
pub fn replay_game(bundle: &Path, rec: &GameRecording) -> Result<GameReplay, ReplayError> {
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
    GameRecording::play(&mut sim, rec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample() -> GameRecording {
        GameRecording {
            seed: 7,
            table_hash: 0xABCD,
            ticks: 100,
            ops: vec![
                (
                    1,
                    GameOp::SetSwitch {
                        input: CabinetInput::Start,
                        closed: true,
                    },
                ),
                (
                    5,
                    GameOp::Nudge {
                        dv: [0.01, 0.0, -0.0],
                    },
                ),
                (
                    5,
                    GameOp::ReleaseSwitch {
                        input: CabinetInput::Start,
                    },
                ),
                (
                    100,
                    GameOp::SetSwitch {
                        input: CabinetInput::Tilt,
                        closed: false,
                    },
                ),
            ],
        }
    }

    #[test]
    fn round_trips_and_differs_from_recording_magic() {
        let r = sample();
        let b = r.to_bytes();
        assert_eq!(&b[..8], b"KHZGAME\0");
        assert_ne!(GAME_MAGIC, super::super::MAGIC);
        assert_eq!(b.len(), GAME_HEADER_BYTES + 4 * GAME_OP_BYTES + 8);
        assert_eq!(GameRecording::from_bytes(&b).unwrap(), r);
        // The input-recording parser refuses it.
        assert_eq!(
            super::super::Recording::from_bytes(&b),
            Err(ReplayError::BadMagic)
        );
    }

    #[test]
    fn rejects_malformed_operations_with_a_valid_checksum() {
        let reseal = |mut b: Vec<u8>| {
            let body = b.len() - 8;
            let sum = fnv1a64(&b[..body]);
            b[body..].copy_from_slice(&sum.to_le_bytes());
            b
        };
        let good = sample().to_bytes();
        let op0 = GAME_HEADER_BYTES;
        let cases: Vec<(usize, u8)> = vec![
            (op0 + 8, 3),      // unknown kind
            (op0 + 9, 8),      // unknown cabinet input
            (op0 + 10, 1),     // padding
            (op0 + 12, 2),     // switch value 2
            (op0 + 16, 1),     // switch payload tail
            (op0, 0),          // tick 0
            (op0 + 24 + 9, 1), // nudge code
        ];
        for (at, v) in cases {
            let mut b = good.clone();
            b[at] = v;
            assert!(
                matches!(
                    GameRecording::from_bytes(&reseal(b)),
                    Err(ReplayError::BadEvent { .. })
                ),
                "byte {at} = {v}"
            );
        }
        // Non-finite nudge.
        let mut b = good.clone();
        b[op0 + 24 + 12..op0 + 24 + 16].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
        assert!(matches!(
            GameRecording::from_bytes(&reseal(b)),
            Err(ReplayError::BadEvent { index: 1 })
        ));
        // Decreasing tick.
        let mut r = sample();
        r.ops.swap(0, 1);
        r.ops[0].0 = 9;
        assert!(matches!(
            GameRecording::from_bytes(&r.to_bytes()),
            Err(ReplayError::BadEvent { index: 1 })
        ));
        // Above `ticks`.
        let mut r = sample();
        r.ticks = 99;
        assert!(matches!(
            GameRecording::from_bytes(&r.to_bytes()),
            Err(ReplayError::BadEvent { index: 3 })
        ));
        // Too many ticks.
        let r = GameRecording {
            ticks: MAX_TICKS + 1,
            ..GameRecording::default()
        };
        assert_eq!(
            GameRecording::from_bytes(&r.to_bytes()),
            Err(ReplayError::TooManyTicks(MAX_TICKS + 1))
        );
        // Checksum, length, version.
        let mut b = good.clone();
        b[13] ^= 1;
        assert_eq!(GameRecording::from_bytes(&b), Err(ReplayError::Checksum));
        assert!(matches!(
            GameRecording::from_bytes(&good[..good.len() - 1]),
            Err(ReplayError::BadLength { .. })
        ));
        let mut b = good;
        b[8] = 2;
        assert_eq!(
            GameRecording::from_bytes(&b),
            Err(ReplayError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn display_is_the_two_cli_lines() {
        let r = GameReplay {
            state_hash: 0x1f,
            score: Some(-3),
            ticks: 1,
        };
        assert_eq!(r.to_string(), "state_hash=000000000000001f\nscore=-3");
        let r = GameReplay { score: None, ..r };
        assert_eq!(r.to_string(), "state_hash=000000000000001f\nscore=none");
    }

    fn arb_op() -> impl Strategy<Value = GameOp> {
        prop_oneof![
            (0usize..8, any::<bool>()).prop_map(|(i, closed)| GameOp::SetSwitch {
                input: CabinetInput::ALL[i],
                closed
            }),
            (0usize..8).prop_map(|i| GameOp::ReleaseSwitch {
                input: CabinetInput::ALL[i]
            }),
            prop::array::uniform3(-1.0e3f32..1.0e3).prop_map(|dv| GameOp::Nudge { dv }),
        ]
    }

    proptest! {
        #[test]
        fn format_round_trips(
            seed in any::<u64>(),
            table_hash in any::<u64>(),
            mut ops in prop::collection::vec((1u64..10_000, arb_op()), 0..40),
        ) {
            ops.sort_by_key(|(t, _)| *t);
            let ticks = ops.last().map_or(0, |(t, _)| *t);
            let r = GameRecording { seed, table_hash, ticks, ops };
            prop_assert_eq!(GameRecording::from_bytes(&r.to_bytes()), Ok(r));
        }

        #[test]
        fn parser_never_panics_on_random_bytes(
            bytes in prop::collection::vec(any::<u8>(), 0..512),
            magic in any::<bool>(),
        ) {
            let mut b = bytes;
            if magic && b.len() >= 8 {
                b[..8].copy_from_slice(&GAME_MAGIC);
            }
            let _ = GameRecording::from_bytes(&b);
        }

        #[test]
        fn parser_never_panics_on_mutated_files(
            at in any::<prop::sample::Index>(),
            v in any::<u8>(),
            cut in any::<prop::sample::Index>(),
        ) {
            let mut b = sample().to_bytes();
            let i = at.index(b.len());
            b[i] = v;
            let _ = GameRecording::from_bytes(&b);
            let c = cut.index(b.len());
            let _ = GameRecording::from_bytes(&b[..c]);
        }
    }
}
