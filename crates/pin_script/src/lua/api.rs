//! The `pinball` module functions and the host-PCG `math.random` (§3.4).
//!
//! Every function validates its arguments strictly (ids must be integers in
//! `0..=65535`, flags must be booleans, `0..1` quantities must lie in
//! `[0, 1]`); a violation raises a Lua error, which faults the script.
//! Commands that do not fit the fixed-capacity queue are dropped and counted
//! (`overflow_count`), never raised.

use super::bridge::Bridge;
use mlua::{Function, Lua, Table, Value};
use pin_core::ids::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId, SwitchId,
};
use pin_core::script::MAX_PERSIST;
use pin_core::{Pcg32, Rng};
use std::rc::Rc;

fn arg_err(f: &str, msg: &str) -> mlua::Error {
    mlua::Error::runtime(format!("pinball.{f}: {msg}"))
}

/// An integer argument: a Lua integer or an integral float.
fn int(v: &Value, f: &str, what: &str) -> mlua::Result<i64> {
    match *v {
        Value::Integer(i) => Ok(i),
        Value::Number(n) if n.fract() == 0.0 && n >= -(2f64.powi(63)) && n < 2f64.powi(63) => {
            Ok(n as i64)
        }
        _ => Err(arg_err(f, &format!("{what} must be an integer"))),
    }
}

fn id(v: &Value, f: &str, what: &str) -> mlua::Result<u16> {
    let i = int(v, f, what)?;
    u16::try_from(i).map_err(|_| arg_err(f, &format!("{what} id {i} out of range 0..=65535")))
}

fn num(v: &Value, f: &str, what: &str) -> mlua::Result<f64> {
    match *v {
        Value::Integer(i) => Ok(i as f64),
        Value::Number(n) if n.is_finite() => Ok(n),
        _ => Err(arg_err(f, &format!("{what} must be a finite number"))),
    }
}

fn unit(v: &Value, f: &str, what: &str) -> mlua::Result<f32> {
    let n = num(v, f, what)?;
    if (0.0..=1.0).contains(&n) {
        Ok(n as f32)
    } else {
        Err(arg_err(f, &format!("{what} {n} outside 0..1")))
    }
}

fn boolean(v: &Value, f: &str, what: &str) -> mlua::Result<bool> {
    match *v {
        Value::Boolean(b) => Ok(b),
        _ => Err(arg_err(f, &format!("{what} must be a boolean"))),
    }
}

fn persist_key(v: &Value, f: &str) -> mlua::Result<PersistKey> {
    let k = id(v, f, "persist key")?;
    if usize::from(k) < MAX_PERSIST {
        Ok(PersistKey(k))
    } else {
        Err(arg_err(f, &format!("persist key {k} out of range 0..{MAX_PERSIST}")))
    }
}

/// A 64-bit draw from the host PCG (two 32-bit outputs).
fn next_u64(rng: &mut Pcg32) -> u64 {
    rng.next_u64()
}

/// Lua 5.4's `project`: a uniform integer in `[0, n]` by masked rejection.
pub(crate) fn project(mut ran: u64, n: u64, rng: &mut Pcg32) -> u64 {
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
        ran = next_u64(rng);
    }
}

/// A uniform integer in `[lo, hi]` (`lo <= hi`).
pub(crate) fn range(lo: i64, hi: i64, rng: &mut Pcg32) -> i64 {
    let r = next_u64(rng);
    let span = hi.wrapping_sub(lo) as u64;
    lo.wrapping_add(project(r, span, rng) as i64)
}

/// A float in `[0, 1)` with 53 random bits (Lua 5.4's `I2d`).
pub(crate) fn unit_float(rng: &mut Pcg32) -> f64 {
    (next_u64(rng) >> 11) as f64 * (0.5 / (1u64 << 52) as f64)
}

/// `math.random` with Lua 5.4 semantics, drawing from the host PCG.
fn math_random(
    b: &Bridge,
    m: Option<Value>,
    n: Option<Value>,
    f: &str,
    allow_float: bool,
) -> mlua::Result<Value> {
    let m = m.filter(|v| !v.is_nil());
    let n = n.filter(|v| !v.is_nil());
    let (lo, hi) = match (&m, &n) {
        (None, None) if allow_float => {
            return b.with_ctx(|_, rng, _| Value::Number(unit_float(rng)));
        }
        (None, None) => return Err(arg_err(f, "expected random(n) or random(m, n)")),
        (Some(m), None) => {
            let m = int(m, f, "upper bound")?;
            if m == 0 && allow_float {
                return b.with_ctx(|_, rng, _| Value::Integer(next_u64(rng) as i64));
            }
            (1, m)
        }
        (Some(m), Some(n)) => (int(m, f, "lower bound")?, int(n, f, "upper bound")?),
        (None, Some(_)) => return Err(arg_err(f, "lower bound must be an integer")),
    };
    if lo > hi {
        return Err(arg_err(f, &format!("interval is empty ([{lo}, {hi}])")));
    }
    b.with_ctx(|_, rng, _| Value::Integer(range(lo, hi, rng)))
}

/// Builds the (writable, private) table of API functions; the sandbox wraps it
/// in a read-only proxy as `pinball`.
pub(crate) fn build(lua: &Lua, bridge: &Rc<Bridge>) -> mlua::Result<Table> {
    let api = lua.create_table()?;
    macro_rules! reg {
        ($name:literal, |$b:ident, $args:pat_param| $body:expr) => {{
            let $b = Rc::clone(bridge);
            api.raw_set($name, lua.create_function(move |_, $args| $body)?)?;
        }};
    }
    reg!("pulse_coil", |b, (c, ms): (Value, Value)| {
        let c = CoilId(id(&c, "pulse_coil", "coil")?);
        let ms = int(&ms, "pulse_coil", "duration")?;
        let ms = u16::try_from(ms)
            .map_err(|_| arg_err("pulse_coil", &format!("duration {ms} out of range 0..=65535")))?;
        b.with_ctx(|_, _, q| b.note(q.pulse_coil(c, ms))).map(drop)
    });
    reg!("hold_coil", |b, (c, on): (Value, Value)| {
        let c = CoilId(id(&c, "hold_coil", "coil")?);
        let on = boolean(&on, "hold_coil", "on")?;
        b.with_ctx(|_, _, q| {
            if b.note(q.hold_coil(c, on)) {
                b.set_held(c, on);
            }
        })
    });
    reg!("set_magnet", |b, (m, p): (Value, Value)| {
        let m = MagnetId(id(&m, "set_magnet", "magnet")?);
        let p = unit(&p, "set_magnet", "power")?;
        b.with_ctx(|_, _, q| b.note(q.set_magnet(m, p))).map(drop)
    });
    reg!("set_zipper", |b, (fl, closed): (Value, Value)| {
        let fl = FlipperId(id(&fl, "set_zipper", "flipper")?);
        let closed = boolean(&closed, "set_zipper", "closed")?;
        b.with_ctx(|_, _, q| b.note(q.set_zipper(fl, closed))).map(drop)
    });
    reg!("set_diverter", |b, (d, on): (Value, Value)| {
        let d = DiverterId(id(&d, "set_diverter", "diverter")?);
        let on = boolean(&on, "set_diverter", "on")?;
        b.with_ctx(|_, _, q| b.note(q.set_diverter(d, on))).map(drop)
    });
    reg!("spawn_ball", |b, (d, p): (Value, Value)| {
        let d = DispenserId(id(&d, "spawn_ball", "dispenser")?);
        let p = ProfileId(id(&p, "spawn_ball", "profile")?);
        b.with_ctx(|_, _, q| b.note(q.spawn_ball(d, p))).map(drop)
    });
    reg!("set_lamp", |b, (l, i): (Value, Value)| {
        let l = LampId(id(&l, "set_lamp", "lamp")?);
        let i = unit(&i, "set_lamp", "intensity")?;
        b.with_ctx(|_, _, q| b.note(q.set_lamp(l, i))).map(drop)
    });
    reg!("set_gi_tint", |b, (r, g, bl, a): (Value, Value, Value, Value)| {
        let tint = [
            unit(&r, "set_gi_tint", "r")?,
            unit(&g, "set_gi_tint", "g")?,
            unit(&bl, "set_gi_tint", "b")?,
            unit(&a, "set_gi_tint", "intensity")?,
        ];
        b.with_ctx(|_, _, q| q.set_gi_tint(tint))
    });
    reg!("set_score", |b, (p, s): (Value, Value)| {
        let p = int(&p, "set_score", "player")?;
        if !(1..=4).contains(&p) {
            return Err(arg_err("set_score", &format!("player {p} out of range 1..=4")));
        }
        let s = int(&s, "set_score", "score")?;
        // `p` is 1..=4, so `p - 1` fits a u8.
        b.with_ctx(|_, _, q| b.note(q.set_score((p - 1) as u8, s))).map(drop)
    });
    reg!("show_scene", |b, s: Value| {
        let s = SceneId(id(&s, "show_scene", "scene")?);
        b.with_ctx(|_, _, q| b.note(q.show_scene(s))).map(drop)
    });
    reg!("play_sound", |b, (s, g): (Value, Value)| {
        let s = SoundId(id(&s, "play_sound", "sound")?);
        let g = num(&g, "play_sound", "gain")?;
        if g < 0.0 {
            return Err(arg_err("play_sound", &format!("gain {g} is negative")));
        }
        b.with_ctx(|_, _, q| b.note(q.play_sound(s, g as f32))).map(drop)
    });
    reg!("persist_set", |b, (k, v): (Value, Value)| {
        let k = persist_key(&k, "persist_set")?;
        let v = int(&v, "persist_set", "value")?;
        b.with_ctx(|_, _, q| b.note(q.write_persist(k, v))).map(drop)
    });
    reg!("persist_get", |b, k: Value| {
        let k = persist_key(&k, "persist_get")?;
        b.with_ctx(|v, _, _| v.persist_value(k))
    });
    reg!("switch", |b, s: Value| {
        let s = SwitchId(id(&s, "switch", "switch")?);
        b.with_ctx(|v, _, _| v.switch_closed(s))
    });
    reg!("tick", |b, (): ()| b.with_ctx(|v, _, _| v.tick as i64));
    reg!("balls_in_play", |b, (): ()| {
        b.with_ctx(|v, _, _| i64::from(v.balls_in_play))
    });
    reg!("random", |b, (m, n): (Option<Value>, Option<Value>)| {
        math_random(&b, m, n, "random", false)
    });
    Ok(api)
}

/// The replacement for `math.random`, routed to the host PCG.
pub(crate) fn math_random_fn(lua: &Lua, bridge: &Rc<Bridge>) -> mlua::Result<Function> {
    let b = Rc::clone(bridge);
    lua.create_function(move |_, (m, n): (Option<Value>, Option<Value>)| {
        math_random(&b, m, n, "math.random", true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::SeedableRng;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn range_stays_in_bounds(seed in any::<u64>(), a in any::<i64>(), b in any::<i64>()) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let mut rng = Pcg32::seed_from_u64(seed);
            for _ in 0..8 {
                let r = range(lo, hi, &mut rng);
                prop_assert!(lo <= r && r <= hi);
            }
        }

        #[test]
        fn small_range_is_deterministic(seed in any::<u64>(), n in 1i64..1000) {
            let mut r1 = Pcg32::seed_from_u64(seed);
            let mut r2 = Pcg32::seed_from_u64(seed);
            for _ in 0..16 {
                let x = range(1, n, &mut r1);
                prop_assert!((1..=n).contains(&x));
                prop_assert_eq!(x, range(1, n, &mut r2));
            }
        }

        #[test]
        fn unit_float_in_unit_interval(seed in any::<u64>()) {
            let mut rng = Pcg32::seed_from_u64(seed);
            for _ in 0..16 {
                let f = unit_float(&mut rng);
                prop_assert!((0.0..1.0).contains(&f));
            }
        }

        #[test]
        fn int_accepts_only_integral_values(f in any::<f64>()) {
            let r = int(&Value::Number(f), "t", "x");
            if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e18 {
                prop_assert_eq!(r.unwrap(), f as i64);
            } else if !f.is_finite() || f.fract() != 0.0 {
                prop_assert!(r.is_err());
            }
        }
    }

    #[test]
    fn project_full_range_and_powers_of_two() {
        let mut rng = Pcg32::seed_from_u64(9);
        assert_eq!(project(u64::MAX, u64::MAX, &mut rng), u64::MAX);
        assert_eq!(project(0b1011, 7, &mut rng), 0b011);
        assert_eq!(range(5, 5, &mut rng), 5);
        // Full-range draws never panic or overflow.
        let _ = range(i64::MIN, i64::MAX, &mut rng);
        // Rejection path: n = 5 masks to 7 and redraws until <= 5.
        for _ in 0..64 {
            assert!(project(rng.next_u64(), 5, &mut rng) <= 5);
        }
    }

    #[test]
    fn converters_reject_bad_types() {
        assert!(id(&Value::Integer(-1), "t", "x").is_err());
        assert!(id(&Value::Integer(65536), "t", "x").is_err());
        assert_eq!(id(&Value::Number(7.0), "t", "x").unwrap(), 7);
        assert!(num(&Value::Number(f64::NAN), "t", "x").is_err());
        assert!(num(&Value::Boolean(true), "t", "x").is_err());
        assert!(unit(&Value::Number(1.5), "t", "x").is_err());
        assert_eq!(unit(&Value::Integer(1), "t", "x").unwrap(), 1.0);
        assert!(boolean(&Value::Integer(1), "t", "x").is_err());
        assert!(persist_key(&Value::Integer(64), "t").is_err());
        assert_eq!(persist_key(&Value::Integer(63), "t").unwrap(), PersistKey(63));
    }
}
