//! Engine-neutral `pinball` host functions: the argument validation, queue
//! bookkeeping and tilt release of `crates/pin_script/src/wasm/host.rs`,
//! without wasmtime types, so wasmi (design A) and the browser-JIT backend
//! (design B) share one copy. Every function behaves exactly like its
//! wasmtime counterpart: same checks in the same order, same queue calls,
//! same error texts (`pinball.<f>: ...`).
//!
//! The success paths never allocate; errors (which fault the script) do.

use crate::abi::IMPORTS;
use pin_core::ids::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId, SwitchId,
};
use pin_core::script::{MAX_PERSIST, QueueOverflow};
use pin_core::{HostCommandQueue, HostCtx, HostView, Pcg32, Rng};
use pin_script::NameTables;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::sync::Arc;

/// Number of name kinds `id_of` accepts (`0..KINDS`).
pub const KINDS: usize = 11;

/// Words in the coil-hold bitset (one bit per possible `CoilId`).
const HOLD_WORDS: usize = (u16::MAX as usize + 1) / 64;

/// Lua 5.4's `project` (pin_script `lua/api.rs`, verbatim).
fn project(mut ran: u64, n: u64, rng: &mut Pcg32) -> u64 {
    if n & n.wrapping_add(1) == 0 {
        return ran & n;
    }
    let mut lim = n;
    lim |= lim >> 1;
    lim |= lim >> 2;
    lim |= lim >> 4;
    lim |= lim >> 8;
    lim |= lim >> 16;
    lim |= lim >> 32;
    loop {
        ran &= lim;
        if ran <= n {
            return ran;
        }
        ran = rng.next_u64();
    }
}

/// A uniform integer in `[lo, hi]` (pin_script `lua::api::range`, verbatim).
pub fn range(lo: i64, hi: i64, rng: &mut Pcg32) -> i64 {
    let r = rng.next_u64();
    let span = hi.wrapping_sub(lo) as u64;
    lo.wrapping_add(project(r, span, rng) as i64)
}

/// Name → id lookup per kind (pin_script host.rs `IdTables`).
#[derive(Debug, Default)]
pub struct IdTables {
    kinds: [Vec<(Box<[u8]>, u16)>; KINDS],
}

impl IdTables {
    /// Built from the name tables in pin_script's fixed kind order; the
    /// last duplicate wins.
    #[must_use]
    pub fn new(names: &NameTables) -> Self {
        fn conv<I: Copy>(v: &[(String, I)], id: impl Fn(I) -> u16) -> Vec<(&str, u16)> {
            v.iter().map(|(n, i)| (n.as_str(), id(*i))).collect()
        }
        let tables = [
            conv(&names.switches, |i| i.0),
            conv(&names.coils, |i| i.0),
            conv(&names.lamps, |i| i.0),
            conv(&names.magnets, |i| i.0),
            conv(&names.diverters, |i| i.0),
            conv(&names.flippers, |i| i.0),
            conv(&names.dispensers, |i| i.0),
            conv(&names.profiles, |i| i.0),
            conv(&names.sounds, |i| i.0),
            conv(&names.scenes, |i| i.0),
            conv(&names.persist, |i| i.0),
        ];
        let mut kinds: [Vec<(Box<[u8]>, u16)>; KINDS] = Default::default();
        for (slot, entries) in kinds.iter_mut().zip(tables) {
            let mut map = BTreeMap::new();
            for (name, id) in entries {
                map.insert(name.as_bytes(), id);
            }
            *slot = map.into_iter().map(|(n, id)| (n.into(), id)).collect();
        }
        Self { kinds }
    }

    /// The id of `name` in table `kind`.
    #[must_use]
    pub fn lookup(&self, kind: usize, name: &[u8]) -> Option<u16> {
        let t = self.kinds.get(kind)?;
        t.binary_search_by(|(n, _)| n.as_ref().cmp(name))
            .ok()
            .map(|i| t[i].1)
    }
}

/// Dropped-command counter and hold bookkeeping (pin_script host.rs `Book`).
#[derive(Clone)]
pub struct Book {
    /// Commands dropped because their queue was full.
    pub overflow: u32,
    holds: [u64; HOLD_WORDS],
    releasing: [u64; HOLD_WORDS],
}

impl Default for Book {
    fn default() -> Self {
        Self {
            overflow: 0,
            holds: [0; HOLD_WORDS],
            releasing: [0; HOLD_WORDS],
        }
    }
}

impl Book {
    /// Counts a dropped command; true if queued.
    pub fn note(&mut self, r: Result<(), QueueOverflow>) -> bool {
        match r {
            Ok(()) => true,
            Err(QueueOverflow) => {
                self.overflow = self.overflow.saturating_add(1);
                false
            }
        }
    }

    /// Records the hold state of `coil`.
    pub fn set_held(&mut self, coil: CoilId, on: bool) {
        let i = coil.index();
        let bit = 1u64 << (i % 64);
        let w = &mut self.holds[i / 64];
        *w = if on { *w | bit } else { *w & !bit };
        self.releasing[i / 64] &= !bit;
    }

    /// True while tilt releases wait for queue room.
    #[must_use]
    pub fn releases_pending(&self) -> bool {
        self.releasing.iter().any(|w| *w != 0)
    }

    /// Starts the tilt release.
    pub fn release_holds(&mut self, cmd: &mut HostCommandQueue) {
        for (r, h) in self.releasing.iter_mut().zip(self.holds.iter_mut()) {
            *r |= *h;
            *h = 0;
        }
        self.drain_releases(cmd);
    }

    /// Queues pending releases while the queue has room.
    pub fn drain_releases(&mut self, cmd: &mut HostCommandQueue) {
        for (wi, w) in self.releasing.iter_mut().enumerate() {
            while *w != 0 {
                let b = w.trailing_zeros() as usize;
                let coil = CoilId((wi * 64 + b) as u16);
                if cmd.hold_coil(coil, false).is_err() {
                    return;
                }
                *w &= *w - 1;
            }
        }
    }
}

/// Command-list lengths at the start of a callback (pin_script mod.rs
/// `QueueMark`): a faulting callback's commands are dropped.
pub struct QueueMark {
    lens: [usize; 11],
    gi_tint: Option<[f32; 4]>,
    overflow: u32,
}

impl QueueMark {
    /// Marks `q`.
    #[must_use]
    pub fn new(q: &HostCommandQueue, overflow: u32) -> Self {
        Self {
            lens: [
                q.coil_pulses.len(),
                q.coil_holds.len(),
                q.magnets.len(),
                q.zippers.len(),
                q.diverters.len(),
                q.ball_spawns.len(),
                q.lamps.len(),
                q.scores.len(),
                q.display_scenes.len(),
                q.sounds.len(),
                q.persist_writes.len(),
            ],
            gi_tint: q.gi_tint,
            overflow,
        }
    }

    /// Drops what the callback queued; dropped holds count as held.
    pub fn rollback(&self, q: &mut HostCommandQueue, book: &mut Book) {
        let l = &self.lens;
        for &(coil, _) in &q.coil_holds[l[1].min(q.coil_holds.len())..] {
            book.set_held(coil, true);
        }
        book.overflow = self.overflow;
        q.coil_pulses.truncate(l[0]);
        q.coil_holds.truncate(l[1]);
        q.magnets.truncate(l[2]);
        q.zippers.truncate(l[3]);
        q.diverters.truncate(l[4]);
        q.ball_spawns.truncate(l[5]);
        q.lamps.truncate(l[6]);
        q.scores.truncate(l[7]);
        q.display_scenes.truncate(l[8]);
        q.sounds.truncate(l[9]);
        q.persist_writes.truncate(l[10]);
        q.gi_tint = self.gi_tint;
    }
}

#[derive(Copy, Clone)]
struct RawCtx {
    view: *const HostView,
    rng: *mut Pcg32,
    cmd: *mut HostCommandQueue,
}

// SAFETY: set only for the duration of one guest call made through `&mut`
// backend and cleared before it returns (as pin_script's RawCtx).
unsafe impl Send for RawCtx {}

/// A host-side failure: the guest traps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostTrap {
    /// `id_of` ran out of fuel (wasmtime's `Trap::OutOfFuel`).
    OutOfFuel,
    /// Validation or misuse; the message (wasmtime-identical text).
    Msg(String),
}

/// Per-instance host state.
pub struct HostState {
    ctx: Option<RawCtx>,
    /// Hold bookkeeping, survives the callback.
    pub book: Book,
    /// Name tables.
    pub names: Arc<IdTables>,
}

fn err(f: &str, msg: impl Display) -> HostTrap {
    HostTrap::Msg(format!("pinball.{f}: {msg}"))
}

fn id(v: i32, f: &str, what: &str) -> Result<u16, HostTrap> {
    u16::try_from(v).map_err(|_| err(f, format_args!("{what} id {v} out of range 0..=65535")))
}

fn flag(v: i32, f: &str, what: &str) -> Result<bool, HostTrap> {
    match v {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(err(
            f,
            format_args!("{what} must be a boolean (0 or 1), got {v}"),
        )),
    }
}

fn finite(v: f32, f: &str, what: &str) -> Result<f32, HostTrap> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(err(f, format_args!("{what} must be a finite number")))
    }
}

fn unit(v: f32, f: &str, what: &str) -> Result<f32, HostTrap> {
    let v = finite(v, f, what)?;
    if (0.0..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(err(f, format_args!("{what} {v} outside 0..1")))
    }
}

fn persist_key(v: i32, f: &str) -> Result<PersistKey, HostTrap> {
    let k = id(v, f, "persist key")?;
    if usize::from(k) < MAX_PERSIST {
        Ok(PersistKey(k))
    } else {
        Err(err(
            f,
            format_args!("persist key {k} out of range 0..{MAX_PERSIST}"),
        ))
    }
}

/// `id_of`'s kind check (first, as in wasmtime's host function).
///
/// # Errors
/// A kind outside `0..KINDS`.
pub fn id_kind(kind: i32) -> Result<usize, HostTrap> {
    usize::try_from(kind)
        .ok()
        .filter(|k| *k < KINDS)
        .ok_or_else(|| {
            err(
                "id_of",
                format_args!("kind {kind} out of range 0..={}", KINDS - 1),
            )
        })
}

/// A raw argument/result value of a host call (bit patterns).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct Raw(pub u64);

impl Raw {
    /// As i32.
    #[must_use]
    pub fn i32(self) -> i32 {
        self.0 as u32 as i32
    }
    /// As i64.
    #[must_use]
    pub fn i64(self) -> i64 {
        self.0 as i64
    }
    /// As f32 (bits).
    #[must_use]
    pub fn f32(self) -> f32 {
        f32::from_bits(self.0 as u32)
    }
    /// From i32.
    #[must_use]
    pub fn from_i32(v: i32) -> Self {
        Raw(u64::from(v as u32))
    }
    /// From i64.
    #[must_use]
    pub fn from_i64(v: i64) -> Self {
        Raw(v as u64)
    }
}

impl HostState {
    /// Fresh state.
    #[must_use]
    pub fn new(names: Arc<IdTables>) -> Self {
        Self {
            ctx: None,
            book: Book::default(),
            names,
        }
    }

    /// Makes `ctx` visible to the host functions until [`Self::leave`].
    pub fn enter(&mut self, ctx: &mut HostCtx<'_>) {
        self.ctx = Some(RawCtx {
            view: std::ptr::from_ref(ctx.view),
            rng: std::ptr::from_mut(&mut *ctx.rng),
            cmd: std::ptr::from_mut(&mut *ctx.cmd),
        });
    }

    /// Hides the context again.
    pub fn leave(&mut self) {
        self.ctx = None;
    }

    fn with_ctx<R>(
        &mut self,
        f: impl FnOnce(&HostView, &mut Pcg32, &mut HostCommandQueue, &mut Book) -> R,
    ) -> Result<R, HostTrap> {
        match self.ctx {
            // SAFETY: as pin_script host.rs: pointers from a live `&mut
            // HostCtx`, cleared before that borrow ends.
            Some(raw) => Ok(unsafe { f(&*raw.view, &mut *raw.rng, &mut *raw.cmd, &mut self.book) }),
            None => Err(HostTrap::Msg(
                "the pinball API is only available inside on_init, on_tick and on_switch_edge"
                    .to_owned(),
            )),
        }
    }

    /// Runs import `k` (index into [`IMPORTS`], not `id_of`) with `args`;
    /// returns its result (0 when it has none).
    ///
    /// # Errors
    /// The trap the wasmtime host function raises for the same call.
    pub fn call(&mut self, k: u8, a: &[Raw]) -> Result<Raw, HostTrap> {
        match k {
            0 => {
                let coil = CoilId(id(a[0].i32(), "pulse_coil", "coil")?);
                let ms = a[1].i32();
                let ms = u16::try_from(ms).map_err(|_| {
                    err(
                        "pulse_coil",
                        format_args!("duration {ms} out of range 0..=65535"),
                    )
                })?;
                self.with_ctx(|_, _, q, b| b.note(q.pulse_coil(coil, ms)))
                    .map(|_| Raw(0))
            }
            1 => {
                let coil = CoilId(id(a[0].i32(), "hold_coil", "coil")?);
                let on = flag(a[1].i32(), "hold_coil", "on")?;
                self.with_ctx(|_, _, q, b| {
                    if b.note(q.hold_coil(coil, on)) {
                        b.set_held(coil, on);
                    }
                })
                .map(|()| Raw(0))
            }
            2 => {
                let m = MagnetId(id(a[0].i32(), "set_magnet", "magnet")?);
                let p = unit(a[1].f32(), "set_magnet", "power")?;
                self.with_ctx(|_, _, q, b| b.note(q.set_magnet(m, p)))
                    .map(|_| Raw(0))
            }
            3 => {
                let fl = FlipperId(id(a[0].i32(), "set_zipper", "flipper")?);
                let closed = flag(a[1].i32(), "set_zipper", "closed")?;
                self.with_ctx(|_, _, q, b| b.note(q.set_zipper(fl, closed)))
                    .map(|_| Raw(0))
            }
            4 => {
                let d = DiverterId(id(a[0].i32(), "set_diverter", "diverter")?);
                let on = flag(a[1].i32(), "set_diverter", "on")?;
                self.with_ctx(|_, _, q, b| b.note(q.set_diverter(d, on)))
                    .map(|_| Raw(0))
            }
            5 => {
                let d = DispenserId(id(a[0].i32(), "spawn_ball", "dispenser")?);
                let p = ProfileId(id(a[1].i32(), "spawn_ball", "profile")?);
                self.with_ctx(|_, _, q, b| b.note(q.spawn_ball(d, p)))
                    .map(|_| Raw(0))
            }
            6 => {
                let lamp = LampId(id(a[0].i32(), "set_lamp", "lamp")?);
                let i = unit(a[1].f32(), "set_lamp", "intensity")?;
                self.with_ctx(|_, _, q, b| b.note(q.set_lamp(lamp, i)))
                    .map(|_| Raw(0))
            }
            7 => {
                let tint = [
                    unit(a[0].f32(), "set_gi_tint", "r")?,
                    unit(a[1].f32(), "set_gi_tint", "g")?,
                    unit(a[2].f32(), "set_gi_tint", "b")?,
                    unit(a[3].f32(), "set_gi_tint", "intensity")?,
                ];
                self.with_ctx(|_, _, q, _| q.set_gi_tint(tint))
                    .map(|()| Raw(0))
            }
            8 => {
                let p = a[0].i32();
                if !(1..=4).contains(&p) {
                    return Err(err(
                        "set_score",
                        format_args!("player {p} out of range 1..=4"),
                    ));
                }
                let s = a[1].i64();
                self.with_ctx(|_, _, q, b| b.note(q.set_score((p - 1) as u8, s)))
                    .map(|_| Raw(0))
            }
            9 => {
                let s = SceneId(id(a[0].i32(), "show_scene", "scene")?);
                self.with_ctx(|_, _, q, b| b.note(q.show_scene(s)))
                    .map(|_| Raw(0))
            }
            10 => {
                let s = SoundId(id(a[0].i32(), "play_sound", "sound")?);
                let g = finite(a[1].f32(), "play_sound", "gain")?;
                if g < 0.0 {
                    return Err(err("play_sound", format_args!("gain {g} is negative")));
                }
                self.with_ctx(|_, _, q, b| b.note(q.play_sound(s, g)))
                    .map(|_| Raw(0))
            }
            11 => {
                let k = persist_key(a[0].i32(), "persist_set")?;
                let v = a[1].i64();
                self.with_ctx(|_, _, q, b| b.note(q.write_persist(k, v)))
                    .map(|_| Raw(0))
            }
            12 => {
                let k = persist_key(a[0].i32(), "persist_get")?;
                self.with_ctx(|v, _, _, _| Raw::from_i64(v.persist_value(k)))
            }
            13 => {
                let s = SwitchId(id(a[0].i32(), "switch", "switch")?);
                self.with_ctx(|v, _, _, _| Raw::from_i32(i32::from(v.switch_closed(s))))
            }
            14 => self.with_ctx(|v, _, _, _| Raw::from_i64(v.tick as i64)),
            15 => self.with_ctx(|v, _, _, _| Raw::from_i32(i32::from(v.balls_in_play))),
            16 => {
                let (m, n) = (a[0].i64(), a[1].i64());
                if m > n {
                    return Err(err(
                        "random",
                        format_args!("interval is empty ([{m}, {n}])"),
                    ));
                }
                self.with_ctx(|_, rng, _, _| Raw::from_i64(range(m, n, rng)))
            }
            _ => Err(HostTrap::Msg(format!(
                "pinball.{}: not a plain import",
                IMPORTS.get(usize::from(k)).map_or("?", |i| i.0)
            ))),
        }
    }

    /// `id_of(kind, ptr, len)` against guest memory `mem`, charging
    /// `len + 1` from `fuel_left` (the fuel the guest has left, saturated
    /// at 0, as wasmtime's `get_fuel`). Returns the id (or −1) and the fuel
    /// left after the charge.
    ///
    /// # Errors
    /// wasmtime's: bad kind, out-of-bounds name, out of fuel.
    pub fn id_of(
        &self,
        kind: i32,
        ptr: i32,
        len: i32,
        mem: &[u8],
        fuel_left: u64,
    ) -> Result<(i32, u64), HostTrap> {
        let k = id_kind(kind)?;
        let (start, n) = (ptr as u32 as usize, len as u32 as usize);
        let name = start.checked_add(n).and_then(|end| mem.get(start..end));
        self.id_of_parts(k, start, n, name, fuel_left)
    }

    /// `id_of` after the kind check, with the name already looked up in
    /// guest memory (`None`: out of bounds). Used by the JIT host, whose
    /// guest memory lives in another instance.
    ///
    /// # Errors
    /// Out-of-bounds name, out of fuel.
    pub fn id_of_parts(
        &self,
        k: usize,
        start: usize,
        n: usize,
        name: Option<&[u8]>,
        fuel_left: u64,
    ) -> Result<(i32, u64), HostTrap> {
        let Some(name) = name else {
            return Err(err(
                "id_of",
                format_args!("name at {start}+{n} is out of bounds of guest memory"),
            ));
        };
        let cost = n as u64 + 1;
        let Some(rest) = fuel_left.checked_sub(cost) else {
            return Err(HostTrap::OutOfFuel);
        };
        Ok((self.names.lookup(k, name).map_or(-1, i32::from), rest))
    }
}
