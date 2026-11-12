//! Carrying `on_save_state` results from the old Lua state to the new one on
//! hot reload. Each reload builds a fresh state, so the saved value is copied
//! out as plain data: nil, booleans, integers, floats, strings and tables of
//! those (nested up to [`MAX_DEPTH`]). Table entries whose key or value is of
//! any other type (functions, userdata, threads) are skipped; a table reached
//! twice (shared or cyclic) is an error.

use mlua::{Lua, Table, Value};
use std::collections::HashSet;

/// Maximum table nesting carried across a reload.
pub(crate) const MAX_DEPTH: usize = 64;

/// A Lua value detached from any state.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Saved {
    Nil,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(Vec<u8>),
    Table(Vec<(Saved, Saved)>),
}

/// Copies `v` out of its state. `Ok(None)` means "unsupported type, skip".
pub(crate) fn save(v: &Value) -> mlua::Result<Option<Saved>> {
    let mut seen = HashSet::new();
    save_inner(v, 0, &mut seen)
}

fn save_inner(v: &Value, depth: usize, seen: &mut HashSet<usize>) -> mlua::Result<Option<Saved>> {
    Ok(Some(match v {
        Value::Nil => Saved::Nil,
        Value::Boolean(b) => Saved::Bool(*b),
        Value::Integer(i) => Saved::Int(*i),
        Value::Number(n) => Saved::Num(*n),
        Value::String(s) => Saved::Str(s.as_bytes().to_vec()),
        Value::Table(t) => {
            if depth >= MAX_DEPTH {
                return Err(mlua::Error::runtime(format!(
                    "on_save_state: tables nested deeper than {MAX_DEPTH}"
                )));
            }
            if !seen.insert(t.to_pointer() as usize) {
                return Err(mlua::Error::runtime(
                    "on_save_state: a table appears more than once (shared or cyclic)",
                ));
            }
            let mut entries = Vec::new();
            for pair in t.pairs::<Value, Value>() {
                let (k, val) = pair?;
                let (Some(k), Some(val)) = (
                    save_inner(&k, depth + 1, seen)?,
                    save_inner(&val, depth + 1, seen)?,
                ) else {
                    continue;
                };
                entries.push((k, val));
            }
            Saved::Table(entries)
        }
        _ => return Ok(None),
    }))
}

/// Recreates `s` inside `lua`.
pub(crate) fn restore(lua: &Lua, s: &Saved) -> mlua::Result<Value> {
    Ok(match s {
        Saved::Nil => Value::Nil,
        Saved::Bool(b) => Value::Boolean(*b),
        Saved::Int(i) => Value::Integer(*i),
        Saved::Num(n) => Value::Number(*n),
        Saved::Str(b) => Value::String(lua.create_string(b)?),
        Saved::Table(entries) => {
            let t: Table = lua.create_table_with_capacity(0, entries.len())?;
            for (k, v) in entries {
                t.raw_set(restore(lua, k)?, restore(lua, v)?)?;
            }
            Value::Table(t)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_between_states() {
        let a = Lua::new();
        let v: Value = a
            .load(
                r#"return { n = 3, f = 1.5, s = "hi", b = true, list = {1, 2, {x = 7}},
                      skip = print, [print] = 1 }"#,
            )
            .eval()
            .unwrap();
        let saved = save(&v).unwrap().unwrap();
        let b = Lua::new();
        let r = restore(&b, &saved).unwrap();
        b.globals().set("t", r).unwrap();
        let ok: bool = b
            .load(
                r#"return t.n == 3 and math.type(t.n) == "integer" and t.f == 1.5 and t.s == "hi"
                   and t.b == true and t.list[3].x == 7 and #t.list == 3 and t.skip == nil"#,
            )
            .eval()
            .unwrap();
        assert!(ok);
        assert_eq!(save(&Value::Nil).unwrap(), Some(Saved::Nil));
    }

    #[test]
    fn cycles_and_depth_are_rejected() {
        let a = Lua::new();
        let cyc: Value = a.load("local t = {} t.self = t return t").eval().unwrap();
        assert!(save(&cyc).is_err());
        let deep: Value = a
            .load("local t = {} for i = 1, 100 do t = {t} end return t")
            .eval()
            .unwrap();
        assert!(save(&deep).is_err());
        let f: Value = a.load("return print").eval().unwrap();
        assert_eq!(save(&f).unwrap(), None);
    }
}
