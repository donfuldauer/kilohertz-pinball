//! The `pinball` import module: host functions, their argument validation and
//! the per-store host state ([`HostState`]).
//!
//! Every function validates its arguments exactly as the Lua API does
//! (`crate::lua` `api.rs`): ids must lie in `0..=65535`, booleans must be `0`
//! or `1`, `0..1` quantities must lie in `[0, 1]`, gains must be finite and
//! non-negative, players `1..=4`, persist keys `< 64`, durations
//! `0..=65535`, and `random(m, n)` needs `m <= n`. A violation traps, which
//! faults the script. Commands that do not fit the fixed-capacity queue are
//! dropped and counted (`overflow_count`), never trapped.
//!
//! Functions that read or write the host context (everything except `id_of`)
//! trap when called outside `on_init` / `on_tick` / `on_switch_edge` (in the
//! start function or the state hooks), as the Lua API raises there.
//!
//! The success paths never allocate: errors (which fault the script) do.

use crate::NameTables;
use crate::lua::api::range;
use pin_core::ids::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId, SwitchId,
};
use pin_core::script::{MAX_PERSIST, QueueOverflow};
use pin_core::{HostCommandQueue, HostCtx, HostView, Pcg32};
use std::collections::BTreeMap;
use std::fmt::Display;
use std::sync::Arc;
use wasmtime::{Caller, Extern, Linker, Memory, StoreLimits, Trap};

/// Import module name.
pub(crate) const MODULE: &str = "pinball";

/// Number of name kinds `id_of` accepts (`0..KINDS`).
pub(crate) const KINDS: usize = 11;

/// Words in the coil-hold bitset (one bit per possible `CoilId`).
const HOLD_WORDS: usize = (u16::MAX as usize + 1) / 64;

/// Value types used by the ABI.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum V {
    I32,
    I64,
    F32,
}

use V::{F32, I32, I64};

/// The complete import ABI: `(name, params, results)`.
pub(crate) const IMPORTS: &[(&str, &[V], &[V])] = &[
    ("pulse_coil", &[I32, I32], &[]),
    ("hold_coil", &[I32, I32], &[]),
    ("set_magnet", &[I32, F32], &[]),
    ("set_zipper", &[I32, I32], &[]),
    ("set_diverter", &[I32, I32], &[]),
    ("spawn_ball", &[I32, I32], &[]),
    ("set_lamp", &[I32, F32], &[]),
    ("set_gi_tint", &[F32, F32, F32, F32], &[]),
    ("set_score", &[I32, I64], &[]),
    ("show_scene", &[I32], &[]),
    ("play_sound", &[I32, F32], &[]),
    ("persist_set", &[I32, I64], &[]),
    ("persist_get", &[I32], &[I64]),
    ("switch", &[I32], &[I32]),
    ("tick", &[], &[I64]),
    ("balls_in_play", &[], &[I32]),
    ("random", &[I64, I64], &[I64]),
    ("id_of", &[I32, I32, I32], &[I32]),
];

/// Name → id lookup per kind, sorted by name for allocation-free binary
/// search. Built from [`NameTables`] in its fixed kind order; when a name
/// repeats within a kind the last entry wins (as in the Lua tables).
#[derive(Debug, Default)]
pub(crate) struct IdTables {
    kinds: [Vec<(Box<[u8]>, u16)>; KINDS],
}

impl IdTables {
    pub(crate) fn new(names: &NameTables) -> Self {
        let mut kinds: [Vec<(Box<[u8]>, u16)>; KINDS] = Default::default();
        for (slot, (_, entries)) in kinds.iter_mut().zip(names.lua_tables()) {
            let mut map = BTreeMap::new();
            for (name, id) in entries {
                map.insert(name.as_bytes(), id);
            }
            *slot = map.into_iter().map(|(n, id)| (n.into(), id)).collect();
        }
        Self { kinds }
    }

    /// The id of `name` in table `kind` (`kind < KINDS`).
    pub(crate) fn lookup(&self, kind: usize, name: &[u8]) -> Option<u16> {
        let t = self.kinds.get(kind)?;
        t.binary_search_by(|(n, _)| n.as_ref().cmp(name))
            .ok()
            .map(|i| t[i].1)
    }
}

#[derive(Copy, Clone)]
struct RawCtx {
    view: *const HostView,
    rng: *mut Pcg32,
    cmd: *mut HostCommandQueue,
}

// SAFETY: the pointers are only set for the duration of one guest call made
// through `&mut WasmScriptBackend` (see `HostState::enter`) and cleared
// before that call returns, so they never outlive the borrow they came from
// nor are they dereferenced from another thread.
unsafe impl Send for RawCtx {}

/// Host bookkeeping that survives hot swaps: dropped-command counter and the
/// set of coils the script holds (for the tilt release).
#[derive(Clone)]
pub(crate) struct Book {
    pub(crate) overflow: u32,
    holds: [u64; HOLD_WORDS],
}

impl Book {
    fn new() -> Self {
        Self {
            overflow: 0,
            holds: [0; HOLD_WORDS],
        }
    }

    /// Counts a dropped command; true if the command was queued.
    pub(crate) fn note(&mut self, r: Result<(), QueueOverflow>) -> bool {
        match r {
            Ok(()) => true,
            Err(QueueOverflow) => {
                self.overflow = self.overflow.saturating_add(1);
                false
            }
        }
    }

    /// Records the hold state of `coil`.
    pub(crate) fn set_held(&mut self, coil: CoilId, on: bool) {
        let i = coil.index();
        let bit = 1u64 << (i % 64);
        let w = &mut self.holds[i / 64];
        *w = if on { *w | bit } else { *w & !bit };
    }

    /// True if the script currently holds `coil`.
    #[cfg(test)]
    pub(crate) fn is_held(&self, coil: CoilId) -> bool {
        let i = coil.index();
        self.holds[i / 64] & (1u64 << (i % 64)) != 0
    }

    /// True while any coil is marked held.
    pub(crate) fn any_held(&self) -> bool {
        self.holds.iter().any(|w| *w != 0)
    }

    /// Queues `(coil, false)` for every held coil while the queue has room;
    /// coils that do not fit stay marked and are released by the next call.
    pub(crate) fn release_holds(&mut self, cmd: &mut HostCommandQueue) {
        for (wi, w) in self.holds.iter_mut().enumerate() {
            while *w != 0 {
                let b = w.trailing_zeros() as usize;
                // `wi * 64 + b` ≤ u16::MAX by construction of HOLD_WORDS.
                let coil = CoilId((wi * 64 + b) as u16);
                if cmd.hold_coil(coil, false).is_err() {
                    return;
                }
                *w &= *w - 1;
            }
        }
    }
}

/// The data of one wasmtime `Store`.
pub(crate) struct HostState {
    ctx: Option<RawCtx>,
    pub(crate) book: Book,
    pub(crate) limits: StoreLimits,
    pub(crate) names: Arc<IdTables>,
    /// The instance's exported memory, once instantiation finished.
    pub(crate) memory: Option<Memory>,
}

impl HostState {
    pub(crate) fn new(names: Arc<IdTables>, limits: StoreLimits) -> Self {
        Self {
            ctx: None,
            book: Book::new(),
            limits,
            names,
            memory: None,
        }
    }

    /// Makes `ctx` visible to the imports until [`Self::leave`]. The caller
    /// must not touch `ctx` until then and must call `leave` before the
    /// borrow of `ctx` ends.
    pub(crate) fn enter(&mut self, ctx: &mut HostCtx<'_>) {
        self.ctx = Some(RawCtx {
            view: std::ptr::from_ref(ctx.view),
            rng: std::ptr::from_mut(&mut *ctx.rng),
            cmd: std::ptr::from_mut(&mut *ctx.cmd),
        });
    }

    /// Hides the context again.
    pub(crate) fn leave(&mut self) {
        self.ctx = None;
    }

    /// Runs `f` on the current callback's context, or fails when the API is
    /// used outside a callback.
    fn with_ctx<R>(
        &mut self,
        f: impl FnOnce(&HostView, &mut Pcg32, &mut HostCommandQueue, &mut Book) -> R,
    ) -> wasmtime::Result<R> {
        match self.ctx {
            // SAFETY: the pointers come from a live `&mut HostCtx` in `enter`
            // and are cleared by `leave` before that borrow ends; the backend
            // does not touch the context in between, and imports run one at a
            // time on the calling thread.
            Some(raw) => Ok(unsafe { f(&*raw.view, &mut *raw.rng, &mut *raw.cmd, &mut self.book) }),
            None => Err(wasmtime::Error::msg(
                "the pinball API is only available inside on_init, on_tick and on_switch_edge",
            )),
        }
    }
}

fn err(f: &str, msg: impl Display) -> wasmtime::Error {
    wasmtime::Error::msg(format!("pinball.{f}: {msg}"))
}

fn id(v: i32, f: &str, what: &str) -> wasmtime::Result<u16> {
    u16::try_from(v).map_err(|_| err(f, format_args!("{what} id {v} out of range 0..=65535")))
}

fn flag(v: i32, f: &str, what: &str) -> wasmtime::Result<bool> {
    match v {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(err(
            f,
            format_args!("{what} must be a boolean (0 or 1), got {v}"),
        )),
    }
}

fn finite(v: f32, f: &str, what: &str) -> wasmtime::Result<f32> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(err(f, format_args!("{what} must be a finite number")))
    }
}

fn unit(v: f32, f: &str, what: &str) -> wasmtime::Result<f32> {
    let v = finite(v, f, what)?;
    if (0.0..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(err(f, format_args!("{what} {v} outside 0..1")))
    }
}

fn persist_key(v: i32, f: &str) -> wasmtime::Result<PersistKey> {
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

/// Charges `cost` fuel units for host work done on the guest's behalf.
fn charge(c: &mut Caller<'_, HostState>, cost: u64) -> wasmtime::Result<()> {
    let left = c.get_fuel()?;
    match left.checked_sub(cost) {
        Some(rest) => c.set_fuel(rest),
        None => {
            c.set_fuel(0)?;
            Err(Trap::OutOfFuel.into())
        }
    }
}

/// `id_of(kind, name_ptr, name_len)`.
fn id_of(mut c: Caller<'_, HostState>, kind: i32, ptr: i32, len: i32) -> wasmtime::Result<i32> {
    let k = usize::try_from(kind)
        .ok()
        .filter(|k| *k < KINDS)
        .ok_or_else(|| {
            err(
                "id_of",
                format_args!("kind {kind} out of range 0..={}", KINDS - 1),
            )
        })?;
    let mem = match c.data().memory {
        Some(m) => m,
        None => match c.get_export("memory") {
            Some(Extern::Memory(m)) => m,
            _ => return Err(err("id_of", "the module exports no memory")),
        },
    };
    // Pointers and lengths are unsigned 32-bit values in the guest.
    let (start, n) = (ptr as u32 as usize, len as u32 as usize);
    let in_bounds = start
        .checked_add(n)
        .is_some_and(|end| end <= mem.data_size(&c));
    if !in_bounds {
        return Err(err(
            "id_of",
            format_args!("name at {start}+{n} is out of bounds of guest memory"),
        ));
    }
    // One fuel unit per name byte plus one for the call.
    charge(&mut c, n as u64 + 1)?;
    let (data, st) = mem.data_and_store_mut(&mut c);
    let name = &data[start..start + n];
    Ok(st.names.lookup(k, name).map_or(-1, i32::from))
}

/// Registers every `pinball` import in `linker`.
pub(crate) fn define(l: &mut Linker<HostState>) -> wasmtime::Result<()> {
    type C<'a> = Caller<'a, HostState>;
    type R<T = ()> = wasmtime::Result<T>;
    l.func_wrap(MODULE, "pulse_coil", |mut c: C, coil: i32, ms: i32| -> R {
        let coil = CoilId(id(coil, "pulse_coil", "coil")?);
        let ms = u16::try_from(ms).map_err(|_| {
            err(
                "pulse_coil",
                format_args!("duration {ms} out of range 0..=65535"),
            )
        })?;
        c.data_mut()
            .with_ctx(|_, _, q, b| b.note(q.pulse_coil(coil, ms)))
            .map(drop)
    })?;
    l.func_wrap(MODULE, "hold_coil", |mut c: C, coil: i32, on: i32| -> R {
        let coil = CoilId(id(coil, "hold_coil", "coil")?);
        let on = flag(on, "hold_coil", "on")?;
        c.data_mut().with_ctx(|_, _, q, b| {
            if b.note(q.hold_coil(coil, on)) {
                b.set_held(coil, on);
            }
        })
    })?;
    l.func_wrap(MODULE, "set_magnet", |mut c: C, m: i32, p: f32| -> R {
        let m = MagnetId(id(m, "set_magnet", "magnet")?);
        let p = unit(p, "set_magnet", "power")?;
        c.data_mut()
            .with_ctx(|_, _, q, b| b.note(q.set_magnet(m, p)))
            .map(drop)
    })?;
    l.func_wrap(
        MODULE,
        "set_zipper",
        |mut c: C, fl: i32, closed: i32| -> R {
            let fl = FlipperId(id(fl, "set_zipper", "flipper")?);
            let closed = flag(closed, "set_zipper", "closed")?;
            c.data_mut()
                .with_ctx(|_, _, q, b| b.note(q.set_zipper(fl, closed)))
                .map(drop)
        },
    )?;
    l.func_wrap(MODULE, "set_diverter", |mut c: C, d: i32, on: i32| -> R {
        let d = DiverterId(id(d, "set_diverter", "diverter")?);
        let on = flag(on, "set_diverter", "on")?;
        c.data_mut()
            .with_ctx(|_, _, q, b| b.note(q.set_diverter(d, on)))
            .map(drop)
    })?;
    l.func_wrap(MODULE, "spawn_ball", |mut c: C, d: i32, p: i32| -> R {
        let d = DispenserId(id(d, "spawn_ball", "dispenser")?);
        let p = ProfileId(id(p, "spawn_ball", "profile")?);
        c.data_mut()
            .with_ctx(|_, _, q, b| b.note(q.spawn_ball(d, p)))
            .map(drop)
    })?;
    l.func_wrap(MODULE, "set_lamp", |mut c: C, lamp: i32, i: f32| -> R {
        let lamp = LampId(id(lamp, "set_lamp", "lamp")?);
        let i = unit(i, "set_lamp", "intensity")?;
        c.data_mut()
            .with_ctx(|_, _, q, b| b.note(q.set_lamp(lamp, i)))
            .map(drop)
    })?;
    l.func_wrap(
        MODULE,
        "set_gi_tint",
        |mut c: C, r: f32, g: f32, bl: f32, a: f32| -> R {
            let tint = [
                unit(r, "set_gi_tint", "r")?,
                unit(g, "set_gi_tint", "g")?,
                unit(bl, "set_gi_tint", "b")?,
                unit(a, "set_gi_tint", "intensity")?,
            ];
            c.data_mut().with_ctx(|_, _, q, _| q.set_gi_tint(tint))
        },
    )?;
    l.func_wrap(MODULE, "set_score", |mut c: C, p: i32, s: i64| -> R {
        if !(1..=4).contains(&p) {
            return Err(err(
                "set_score",
                format_args!("player {p} out of range 1..=4"),
            ));
        }
        // `p` is 1..=4, so `p - 1` fits a u8.
        c.data_mut()
            .with_ctx(|_, _, q, b| b.note(q.set_score((p - 1) as u8, s)))
            .map(drop)
    })?;
    l.func_wrap(MODULE, "show_scene", |mut c: C, s: i32| -> R {
        let s = SceneId(id(s, "show_scene", "scene")?);
        c.data_mut()
            .with_ctx(|_, _, q, b| b.note(q.show_scene(s)))
            .map(drop)
    })?;
    l.func_wrap(MODULE, "play_sound", |mut c: C, s: i32, g: f32| -> R {
        let s = SoundId(id(s, "play_sound", "sound")?);
        let g = finite(g, "play_sound", "gain")?;
        if g < 0.0 {
            return Err(err("play_sound", format_args!("gain {g} is negative")));
        }
        c.data_mut()
            .with_ctx(|_, _, q, b| b.note(q.play_sound(s, g)))
            .map(drop)
    })?;
    l.func_wrap(MODULE, "persist_set", |mut c: C, k: i32, v: i64| -> R {
        let k = persist_key(k, "persist_set")?;
        c.data_mut()
            .with_ctx(|_, _, q, b| b.note(q.write_persist(k, v)))
            .map(drop)
    })?;
    l.func_wrap(MODULE, "persist_get", |mut c: C, k: i32| -> R<i64> {
        let k = persist_key(k, "persist_get")?;
        c.data_mut().with_ctx(|v, _, _, _| v.persist_value(k))
    })?;
    l.func_wrap(MODULE, "switch", |mut c: C, s: i32| -> R<i32> {
        let s = SwitchId(id(s, "switch", "switch")?);
        c.data_mut()
            .with_ctx(|v, _, _, _| i32::from(v.switch_closed(s)))
    })?;
    l.func_wrap(MODULE, "tick", |mut c: C| -> R<i64> {
        c.data_mut().with_ctx(|v, _, _, _| v.tick as i64)
    })?;
    l.func_wrap(MODULE, "balls_in_play", |mut c: C| -> R<i32> {
        c.data_mut()
            .with_ctx(|v, _, _, _| i32::from(v.balls_in_play))
    })?;
    l.func_wrap(MODULE, "random", |mut c: C, m: i64, n: i64| -> R<i64> {
        if m > n {
            return Err(err(
                "random",
                format_args!("interval is empty ([{m}, {n}])"),
            ));
        }
        c.data_mut().with_ctx(|_, rng, _, _| range(m, n, rng))
    })?;
    l.func_wrap(MODULE, "id_of", id_of)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::SeedableRng;

    #[test]
    fn holds_release_in_order_and_resume_after_overflow() {
        let mut b = Book::new();
        for c in 0..40u16 {
            b.set_held(CoilId(c * 3), true);
        }
        b.set_held(CoilId(u16::MAX), true);
        assert!(b.is_held(CoilId(u16::MAX)));
        let mut q = HostCommandQueue::default();
        b.release_holds(&mut q);
        assert_eq!(q.coil_holds.len(), 32);
        assert_eq!(q.coil_holds[0], (CoilId(0), false));
        assert_eq!(q.coil_holds[31], (CoilId(93), false));
        assert!(b.any_held());
        q.clear();
        b.release_holds(&mut q);
        assert_eq!(q.coil_holds.len(), 9);
        assert_eq!(q.coil_holds[8], (CoilId(u16::MAX), false));
        assert!(!b.any_held());
        b.set_held(CoilId(7), true);
        b.set_held(CoilId(7), false);
        assert!(!b.is_held(CoilId(7)));
        assert!(b.note(Ok(())));
        assert!(!b.note(Err(QueueOverflow)));
        assert_eq!(b.overflow, 1);
    }

    #[test]
    fn id_tables_sort_and_last_duplicate_wins() {
        let names = NameTables {
            switches: vec![("b".into(), SwitchId(2)), ("a".into(), SwitchId(1))],
            coils: vec![("kick".into(), CoilId(4)), ("kick".into(), CoilId(9))],
            persist: vec![("hs".into(), PersistKey(63))],
            ..NameTables::default()
        };
        let t = IdTables::new(&names);
        assert_eq!(t.lookup(0, b"a"), Some(1));
        assert_eq!(t.lookup(0, b"b"), Some(2));
        assert_eq!(t.lookup(0, b"c"), None);
        assert_eq!(t.lookup(1, b"kick"), Some(9));
        assert_eq!(t.lookup(10, b"hs"), Some(63));
        assert_eq!(t.lookup(2, b"kick"), None);
        assert_eq!(t.lookup(KINDS, b"a"), None);
    }

    #[test]
    fn validators() {
        assert!(id(-1, "t", "x").is_err());
        assert!(id(65536, "t", "x").is_err());
        assert_eq!(id(65535, "t", "x").unwrap(), 65535);
        assert!(flag(2, "t", "x").is_err());
        assert!(flag(-1, "t", "x").is_err());
        assert!(flag(1, "t", "x").unwrap());
        assert!(unit(f32::NAN, "t", "x").is_err());
        assert!(unit(1.0001, "t", "x").is_err());
        assert!(unit(-0.0001, "t", "x").is_err());
        assert_eq!(unit(1.0, "t", "x").unwrap(), 1.0);
        assert!(finite(f32::INFINITY, "t", "x").is_err());
        assert!(persist_key(64, "t").is_err());
        assert!(persist_key(-1, "t").is_err());
        assert_eq!(persist_key(63, "t").unwrap(), PersistKey(63));
        let e = err("pulse_coil", "boom").to_string();
        assert_eq!(e, "pinball.pulse_coil: boom");
    }

    #[test]
    fn ctx_only_inside_callbacks() {
        let mut st = HostState::new(Arc::default(), StoreLimits::default());
        assert!(st.with_ctx(|_, _, _, _| ()).is_err());
        let view = HostView::default();
        let mut rng = Pcg32::seed_from_u64(0);
        let mut cmd = HostCommandQueue::default();
        let mut ctx = HostCtx {
            view: &view,
            rng: &mut rng,
            cmd: &mut cmd,
        };
        st.enter(&mut ctx);
        st.with_ctx(|_, _, q, b| b.note(q.pulse_coil(CoilId(1), 5)))
            .unwrap();
        st.leave();
        assert!(st.with_ctx(|_, _, _, _| ()).is_err());
        assert_eq!(cmd.coil_pulses.as_slice(), &[(CoilId(1), 5)]);
    }
}
