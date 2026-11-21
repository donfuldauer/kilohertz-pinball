//! File formats of the harness (little-endian, FNV-1a 64 trailer).
//!
//! Scenario (`.scn`): magic `S1SCEN\0\0`, version u32 = 1, seed u64, table
//! hash u64, ticks u64, op count u64, ops of 40 bytes: tick u64 (the tick
//! the op belongs to: bot operations and spawns are applied before it,
//! input events are fed with it), kind u8, code u8, 2 zero bytes, 28
//! payload bytes; then the FNV-1a 64 of everything before.
//!
//! Script stream (`.scr`): magic `S1SCRPT\0`, version u32 = 1, step count
//! u64, steps: tick u64 (0 = `on_init`), RNG draws u32 (`next_u32`
//! equivalents), digest u64 (switch edges delivered then the `HostView` at
//! `on_tick`), queue byte length u32, the encoded `HostCommandQueue`; then
//! the FNV-1a 64 trailer.

use pin_core::ids::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId,
};
use pin_core::input::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use pin_core::{HostCommandQueue, HostView};
use pin_sim::hash::{Fnv64, fnv1a64};
use pin_sim::replay::GameOp;

/// Scenario magic.
pub const SCN_MAGIC: [u8; 8] = *b"S1SCEN\0\0";
/// Script stream magic.
pub const SCR_MAGIC: [u8; 8] = *b"S1SCRPT\0";
/// Size of one scenario op.
pub const OP_BYTES: usize = 40;

/// One scenario operation.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Op {
    /// A recorded bot operation, applied before the tick.
    Game(GameOp),
    /// An input event fed with the tick.
    Input(InputEvent),
    /// A ball spawned through the test API before the tick.
    Spawn {
        /// Index in `BUILTIN_PROFILE_NAMES`.
        profile: u16,
        /// Collision layer.
        layer: u8,
        /// Centre, m.
        pos: [f32; 3],
        /// Velocity, m/s.
        vel: [f32; 3],
    },
}

/// A recorded scenario.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Scenario {
    /// Sim seed.
    pub seed: u64,
    /// Table hash of the bundle.
    pub table_hash: u64,
    /// Ticks to run.
    pub ticks: u64,
    /// `(tick, op)`, ticks non-decreasing.
    pub ops: Vec<(u64, Op)>,
}

fn put_f32s(p: &mut [u8], at: usize, v: [f32; 3]) {
    for (i, x) in v.iter().enumerate() {
        p[at + 4 * i..at + 4 * i + 4].copy_from_slice(&x.to_bits().to_le_bytes());
    }
}

fn get_u64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

fn get_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn get_f32s(b: &[u8], at: usize) -> [f32; 3] {
    [
        f32::from_bits(get_u32(b, at)),
        f32::from_bits(get_u32(b, at + 4)),
        f32::from_bits(get_u32(b, at + 8)),
    ]
}

fn check_trailer<'a>(b: &'a [u8], magic: &[u8; 8]) -> Result<&'a [u8], String> {
    if b.len() < 20 || &b[..8] != magic {
        return Err("bad magic".into());
    }
    let body = &b[..b.len() - 8];
    if fnv1a64(body) != get_u64(b, b.len() - 8) {
        return Err("checksum mismatch".into());
    }
    if get_u32(b, 8) != 1 {
        return Err("unsupported version".into());
    }
    Ok(body)
}

impl Scenario {
    /// File bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(44 + OP_BYTES * self.ops.len() + 8);
        out.extend_from_slice(&SCN_MAGIC);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&self.seed.to_le_bytes());
        out.extend_from_slice(&self.table_hash.to_le_bytes());
        out.extend_from_slice(&self.ticks.to_le_bytes());
        out.extend_from_slice(&(self.ops.len() as u64).to_le_bytes());
        for (tick, op) in &self.ops {
            let mut rec = [0u8; OP_BYTES];
            rec[..8].copy_from_slice(&tick.to_le_bytes());
            let p = 12;
            match *op {
                Op::Game(GameOp::SetSwitch { input, closed }) => {
                    rec[8] = 0;
                    rec[9] = input.index() as u8;
                    rec[p] = u8::from(closed);
                }
                Op::Game(GameOp::ReleaseSwitch { input }) => {
                    rec[8] = 1;
                    rec[9] = input.index() as u8;
                }
                Op::Game(GameOp::Nudge { dv }) => {
                    rec[8] = 2;
                    put_f32s(&mut rec, p, dv);
                }
                Op::Input(e) => {
                    rec[p..p + 8].copy_from_slice(&e.timestamp_ns.to_le_bytes());
                    match e.kind {
                        InputEventKind::Button { input, pressed } => {
                            rec[8] = 3;
                            rec[9] = input.index() as u8;
                            rec[p + 8] = u8::from(pressed);
                        }
                        InputEventKind::Axis { axis, value } => {
                            rec[8] = 4;
                            rec[9] = CabinetAxis::ALL
                                .iter()
                                .position(|&a| a == axis)
                                .unwrap_or(0) as u8;
                            rec[p + 8..p + 12].copy_from_slice(&value.to_bits().to_le_bytes());
                        }
                    }
                }
                Op::Spawn {
                    profile,
                    layer,
                    pos,
                    vel,
                } => {
                    rec[8] = 5;
                    rec[p..p + 2].copy_from_slice(&profile.to_le_bytes());
                    rec[p + 2] = layer;
                    put_f32s(&mut rec, p + 4, pos);
                    put_f32s(&mut rec, p + 16, vel);
                }
            }
            out.extend_from_slice(&rec);
        }
        let sum = fnv1a64(&out);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// Parses file bytes.
    ///
    /// # Errors
    /// A message for malformed bytes.
    pub fn from_bytes(b: &[u8]) -> Result<Scenario, String> {
        let body = check_trailer(b, &SCN_MAGIC)?;
        if body.len() < 44 {
            return Err("short scenario".into());
        }
        let n = get_u64(body, 36) as usize;
        if body.len() != 44 + n * OP_BYTES {
            return Err("scenario length mismatch".into());
        }
        let mut ops = Vec::with_capacity(n);
        for i in 0..n {
            let r = &body[44 + i * OP_BYTES..44 + (i + 1) * OP_BYTES];
            let tick = get_u64(r, 0);
            let code = usize::from(r[9]);
            let p = 12;
            let input = || {
                CabinetInput::ALL
                    .get(code)
                    .copied()
                    .ok_or_else(|| format!("op {i}: bad input"))
            };
            let op = match r[8] {
                0 => Op::Game(GameOp::SetSwitch {
                    input: input()?,
                    closed: r[p] != 0,
                }),
                1 => Op::Game(GameOp::ReleaseSwitch { input: input()? }),
                2 => Op::Game(GameOp::Nudge { dv: get_f32s(r, p) }),
                3 => Op::Input(InputEvent::button(get_u64(r, p), input()?, r[p + 8] != 0)),
                4 => Op::Input(InputEvent::axis(
                    get_u64(r, p),
                    *CabinetAxis::ALL
                        .get(code)
                        .ok_or_else(|| format!("op {i}: bad axis"))?,
                    f32::from_bits(get_u32(r, p + 8)),
                )),
                5 => Op::Spawn {
                    profile: u16::from_le_bytes([r[p], r[p + 1]]),
                    layer: r[p + 2],
                    pos: get_f32s(r, p + 4),
                    vel: get_f32s(r, p + 16),
                },
                k => return Err(format!("op {i}: bad kind {k}")),
            };
            ops.push((tick, op));
        }
        Ok(Scenario {
            seed: get_u64(body, 12),
            table_hash: get_u64(body, 20),
            ticks: get_u64(body, 28),
            ops,
        })
    }
}

/// Feeds a `HostView` into a digest.
pub fn digest_view(h: &mut Fnv64, v: &HostView) {
    h.write_u64(v.tick);
    for w in v.switches {
        h.write_u64(w);
    }
    h.write_u8(v.balls_in_play);
    for p in v.persist {
        h.write_i64(p);
    }
}

/// Appends the encoding of `q`.
pub fn encode_queue(q: &HostCommandQueue, out: &mut Vec<u8>) {
    fn n(out: &mut Vec<u8>, len: usize) {
        out.extend_from_slice(&(len as u16).to_le_bytes());
    }
    fn u16v(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    fn f(out: &mut Vec<u8>, v: f32) {
        out.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    n(out, q.coil_pulses.len());
    for &(c, ms) in &q.coil_pulses {
        u16v(out, c.0);
        u16v(out, ms);
    }
    n(out, q.coil_holds.len());
    for &(c, on) in &q.coil_holds {
        u16v(out, c.0);
        out.push(u8::from(on));
    }
    n(out, q.magnets.len());
    for &(m, p) in &q.magnets {
        u16v(out, m.0);
        f(out, p);
    }
    n(out, q.zippers.len());
    for &(z, on) in &q.zippers {
        u16v(out, z.0);
        out.push(u8::from(on));
    }
    n(out, q.diverters.len());
    for &(d, on) in &q.diverters {
        u16v(out, d.0);
        out.push(u8::from(on));
    }
    n(out, q.ball_spawns.len());
    for &(d, p) in &q.ball_spawns {
        u16v(out, d.0);
        u16v(out, p.0);
    }
    n(out, q.lamps.len());
    for &(l, i) in &q.lamps {
        u16v(out, l.0);
        f(out, i);
    }
    match q.gi_tint {
        Some(t) => {
            out.push(1);
            for v in t {
                f(out, v);
            }
        }
        None => out.push(0),
    }
    n(out, q.scores.len());
    for &(p, s) in &q.scores {
        out.push(p);
        out.extend_from_slice(&s.to_le_bytes());
    }
    n(out, q.display_scenes.len());
    for s in &q.display_scenes {
        u16v(out, s.0);
    }
    n(out, q.sounds.len());
    for &(s, g) in &q.sounds {
        u16v(out, s.0);
        f(out, g);
    }
    n(out, q.persist_writes.len());
    for &(k, v) in &q.persist_writes {
        u16v(out, k.0);
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// A bounds-checked little-endian reader (no allocation).
pub struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Reads `b` from `pos`.
    #[must_use]
    pub fn new(b: &'a [u8], pos: usize) -> Self {
        Self { b, pos }
    }
    /// Current offset.
    #[must_use]
    pub fn pos(&self) -> usize {
        self.pos
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }
    /// A byte.
    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    /// A u16.
    pub fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_le_bytes([s[0], s[1]]))
    }
    /// A u32.
    pub fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    }
    /// A u64.
    pub fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
    }
    /// An f32 by bits.
    pub fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
}

/// Decodes a queue written by [`encode_queue`] into `q` (cleared first; no
/// allocation: every list is fixed-capacity).
pub fn decode_queue(r: &mut Reader<'_>, q: &mut HostCommandQueue) -> Option<()> {
    q.clear();
    for _ in 0..r.u16()? {
        q.pulse_coil(CoilId(r.u16()?), r.u16()?).ok()?;
    }
    for _ in 0..r.u16()? {
        q.hold_coil(CoilId(r.u16()?), r.u8()? != 0).ok()?;
    }
    for _ in 0..r.u16()? {
        q.set_magnet(MagnetId(r.u16()?), r.f32()?).ok()?;
    }
    for _ in 0..r.u16()? {
        q.set_zipper(FlipperId(r.u16()?), r.u8()? != 0).ok()?;
    }
    for _ in 0..r.u16()? {
        q.set_diverter(DiverterId(r.u16()?), r.u8()? != 0).ok()?;
    }
    for _ in 0..r.u16()? {
        q.spawn_ball(DispenserId(r.u16()?), ProfileId(r.u16()?))
            .ok()?;
    }
    for _ in 0..r.u16()? {
        q.set_lamp(LampId(r.u16()?), r.f32()?).ok()?;
    }
    if r.u8()? == 1 {
        q.set_gi_tint([r.f32()?, r.f32()?, r.f32()?, r.f32()?]);
    }
    for _ in 0..r.u16()? {
        let p = r.u8()?;
        q.set_score(p, r.u64()? as i64).ok()?;
    }
    for _ in 0..r.u16()? {
        q.show_scene(SceneId(r.u16()?)).ok()?;
    }
    for _ in 0..r.u16()? {
        q.play_sound(SoundId(r.u16()?), r.f32()?).ok()?;
    }
    for _ in 0..r.u16()? {
        let k = PersistKey(r.u16()?);
        q.write_persist(k, r.u64()? as i64).ok()?;
    }
    Some(())
}

/// Writes a script-output stream.
#[derive(Debug, Default)]
pub struct ScriptStreamWriter {
    body: Vec<u8>,
    steps: u64,
    scratch: Vec<u8>,
}

impl ScriptStreamWriter {
    /// Appends one script step.
    pub fn step(&mut self, tick: u64, draws: u32, digest: u64, q: &HostCommandQueue) {
        self.scratch.clear();
        if !q.is_empty() {
            encode_queue(q, &mut self.scratch);
        }
        self.body.extend_from_slice(&tick.to_le_bytes());
        self.body.extend_from_slice(&draws.to_le_bytes());
        self.body.extend_from_slice(&digest.to_le_bytes());
        self.body
            .extend_from_slice(&(self.scratch.len() as u32).to_le_bytes());
        self.body.extend_from_slice(&self.scratch);
        self.steps += 1;
    }

    /// The file bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(20 + self.body.len() + 8);
        out.extend_from_slice(&SCR_MAGIC);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&self.steps.to_le_bytes());
        out.extend_from_slice(&self.body);
        let sum = fnv1a64(&out);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }
}

/// Validates a script stream; returns (steps, offset of the first step,
/// end of the body).
///
/// # Errors
/// A message for malformed bytes.
pub fn check_script_stream(b: &[u8]) -> Result<(u64, usize, usize), String> {
    let body = check_trailer(b, &SCR_MAGIC)?;
    Ok((get_u64(body, 12), 20, body.len()))
}
