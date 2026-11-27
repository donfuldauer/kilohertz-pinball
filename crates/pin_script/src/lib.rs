//! `pin_script`: Table scripting backends: Lua 5.4 and WebAssembly.
//!
//! * [`lua`]: the Lua 5.4 backend (Subphase 4.2): sandbox, memory cap,
//!   instruction budget and hot reload (PLAN.md §3.4, D10, D11, D25).
//! * [`wasm`]: the WebAssembly backend (Subphase 4.5): wasmtime with fuel
//!   metering, a memory cap and the same host API and fault semantics as Lua.
//!   Rules written in Rust use the `pin_guest_sdk` crate
//!   (`crates/pin_script/guest-sdk`), which documents the ABI.
//! * [`load_backend`] / [`load_backend_with`]: pick a backend from the
//!   extension of `script_entry`.
//! * [`NameTables`]: the name → id tables scripts see as `pinball.sw`, `pinball.coil`, …
//! * Engine-neutral modules, built with every feature set: [`config`] (the
//!   backend limits), [`rng`] (the random helpers both backends draw
//!   through) and [`budget`] (the per-tick budget shared by a tick's
//!   callbacks, D45).
//!
//! # Features (D10, REQ-WEB-001)
//!
//! * `lua` (default): the Lua 5.4 backend (mlua, libm).
//! * `wasmtime` (default): the native Wasm backend (wasmtime), WAT text
//!   input and the file watcher of [`lua::reload`] (which also needs `lua`).
//! * `wasmi`: the web Wasm engine dependency (its backend is a later unit).
//!
//! With a backend's feature off, [`lua`] / [`wasm`] keep only their
//! engine-neutral items ([`lua::LuaConfig`]; [`wasm::WasmConfig`], the
//! pre-compile scan and its limits), and [`load_backend`] returns
//! [`ScriptError::UnsupportedBackend`] for that backend's extension.
//! `--no-default-features` builds for `wasm32-unknown-unknown`.

pub mod budget;
pub mod config;
pub mod guest_sdk;
#[cfg(feature = "lua")]
pub mod lua;
pub mod rng;
pub mod wasm;

/// Lua 5.4 backend: not built in (the `lua` feature is off). Only the
/// engine-neutral limits remain.
#[cfg(not(feature = "lua"))]
pub mod lua {
    pub use crate::config::LuaConfig;
}

use pin_core::ids::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId, SwitchId,
};
use pin_core::{ScriptError, TableScriptBackend};
use std::path::Path;

/// Name → id tables built once at table load (§3.1: no strings cross into the
/// sim thread). The Lua backend exposes them as read-only tables
/// `pinball.sw`, `pinball.coil`, `pinball.lamp`, `pinball.magnet`,
/// `pinball.diverter`, `pinball.flipper`, `pinball.dispenser`,
/// `pinball.profile`, `pinball.sound`, `pinball.scene` and
/// `pinball.persist_key`, each mapping a name to its integer id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameTables {
    /// Switches (`pinball.sw`).
    pub switches: Vec<(String, SwitchId)>,
    /// Coils (`pinball.coil`).
    pub coils: Vec<(String, CoilId)>,
    /// Lamps (`pinball.lamp`).
    pub lamps: Vec<(String, LampId)>,
    /// Magnets (`pinball.magnet`).
    pub magnets: Vec<(String, MagnetId)>,
    /// Diverters (`pinball.diverter`).
    pub diverters: Vec<(String, DiverterId)>,
    /// Flippers (`pinball.flipper`).
    pub flippers: Vec<(String, FlipperId)>,
    /// Ball dispensers (`pinball.dispenser`).
    pub dispensers: Vec<(String, DispenserId)>,
    /// Ball profiles (`pinball.profile`).
    pub profiles: Vec<(String, ProfileId)>,
    /// Sounds (`pinball.sound`).
    pub sounds: Vec<(String, SoundId)>,
    /// Display scenes (`pinball.scene`).
    pub scenes: Vec<(String, SceneId)>,
    /// Persistent slots (`pinball.persist_key`).
    pub persist: Vec<(String, PersistKey)>,
}

impl NameTables {
    /// Every table as `(lua field name, [(name, id)])`, in a fixed order.
    #[cfg(any(feature = "lua", feature = "wasmtime"))]
    pub(crate) fn lua_tables(&self) -> [(&'static str, Vec<(&str, u16)>); 11] {
        fn conv<I: Copy>(v: &[(String, I)], id: impl Fn(I) -> u16) -> Vec<(&str, u16)> {
            v.iter().map(|(n, i)| (n.as_str(), id(*i))).collect()
        }
        [
            ("sw", conv(&self.switches, |i| i.0)),
            ("coil", conv(&self.coils, |i| i.0)),
            ("lamp", conv(&self.lamps, |i| i.0)),
            ("magnet", conv(&self.magnets, |i| i.0)),
            ("diverter", conv(&self.diverters, |i| i.0)),
            ("flipper", conv(&self.flippers, |i| i.0)),
            ("dispenser", conv(&self.dispensers, |i| i.0)),
            ("profile", conv(&self.profiles, |i| i.0)),
            ("sound", conv(&self.sounds, |i| i.0)),
            ("scene", conv(&self.scenes, |i| i.0)),
            ("persist_key", conv(&self.persist, |i| i.0)),
        ]
    }
}

/// Builds the rules backend named by the extension of `script_entry` (§3.4)
/// with the default limits: `.lua` → [`lua::LuaScriptBackend`] with
/// [`lua::LuaConfig::default`]; `.wasm` → [`wasm::WasmScriptBackend`] with
/// [`wasm::WasmConfig::default`]; anything else → `Err(UnsupportedBackend)`.
/// Extensions are matched case-insensitively.
pub fn load_backend(
    script_entry: &str,
    source: &[u8],
    names: &NameTables,
) -> Result<Box<dyn TableScriptBackend>, ScriptError> {
    load_backend_with(
        script_entry,
        source,
        names,
        lua::LuaConfig::default(),
        wasm::WasmConfig::default(),
    )
}

/// Like [`load_backend`], with explicit limits for each backend (the engine
/// passes [`lua::LuaConfig::engine`] and [`wasm::WasmConfig::engine`]).
///
/// A backend whose feature is off (`lua`, `wasmtime`) is not built in: its
/// extension gives [`ScriptError::UnsupportedBackend`] saying so.
pub fn load_backend_with(
    script_entry: &str,
    source: &[u8],
    names: &NameTables,
    lua: lua::LuaConfig,
    wasm: wasm::WasmConfig,
) -> Result<Box<dyn TableScriptBackend>, ScriptError> {
    #[cfg(not(all(feature = "lua", feature = "wasmtime")))]
    let _ = (source, names, lua, wasm);
    let ext = Path::new(script_entry)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        #[cfg(feature = "lua")]
        Some("lua") => Ok(Box::new(lua::LuaScriptBackend::new(source, names, lua)?)),
        #[cfg(feature = "wasmtime")]
        Some("wasm") => Ok(Box::new(wasm::WasmScriptBackend::new(source, names, wasm)?)),
        #[cfg(not(feature = "lua"))]
        Some("lua") => Err(not_built_in(script_entry, "lua", "lua")),
        #[cfg(not(feature = "wasmtime"))]
        Some("wasm") => Err(not_built_in(script_entry, "wasm", "wasmtime")),
        Some(other) => Err(ScriptError::UnsupportedBackend(format!(
            "{script_entry}: unknown script extension .{other} (expected .lua or .wasm)"
        ))),
        None => Err(ScriptError::UnsupportedBackend(format!(
            "{script_entry}: script_entry has no extension (expected .lua or .wasm)"
        ))),
    }
}

/// The error for a known extension whose backend feature is off.
#[cfg(not(all(feature = "lua", feature = "wasmtime")))]
fn not_built_in(script_entry: &str, ext: &str, feature: &str) -> ScriptError {
    ScriptError::UnsupportedBackend(format!(
        "{script_entry}: the .{ext} backend is not built in (pin_script feature `{feature}` is off)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-WEB-001: unknown or missing extensions are refused under every
    /// feature set.
    #[test]
    fn unknown_extension_is_unsupported_req_web_001() {
        let names = NameTables::default();
        for entry in ["rules.py", "rules", "dir.lua/rules"] {
            match load_backend(entry, b"", &names) {
                Err(ScriptError::UnsupportedBackend(m)) => assert!(m.contains(entry), "{m}"),
                Err(e) => panic!("{entry}: {e:?}"),
                Ok(_) => panic!("{entry}: loaded"),
            }
        }
    }

    /// REQ-WEB-001: with the `lua` feature off, a `.lua` entry (any case) is
    /// `UnsupportedBackend` naming the feature, never a panic.
    #[cfg(not(feature = "lua"))]
    #[test]
    fn lua_off_is_unsupported_req_web_001() {
        let names = NameTables::default();
        for entry in ["rules.lua", "scripts/RULES.LUA"] {
            match load_backend(entry, b"function on_tick() end", &names) {
                Err(ScriptError::UnsupportedBackend(m)) => {
                    assert!(m.contains(entry), "{m}");
                    assert!(m.contains("not built in"), "{m}");
                    assert!(m.contains("`lua`"), "{m}");
                }
                Err(e) => panic!("{entry}: {e:?}"),
                Ok(_) => panic!("{entry}: loaded without the lua backend"),
            }
        }
    }

    /// REQ-WEB-001: with the `wasmtime` feature off, a `.wasm` entry (any
    /// case) is `UnsupportedBackend` naming the feature, never a panic.
    #[cfg(not(feature = "wasmtime"))]
    #[test]
    fn wasm_off_is_unsupported_req_web_001() {
        let names = NameTables::default();
        for entry in ["rules.wasm", "RULES.Wasm"] {
            let r = load_backend_with(
                entry,
                b"\0asm\x01\0\0\0",
                &names,
                lua::LuaConfig::engine(),
                wasm::WasmConfig::engine(),
            );
            match r {
                Err(ScriptError::UnsupportedBackend(m)) => {
                    assert!(m.contains(entry), "{m}");
                    assert!(m.contains("not built in"), "{m}");
                    assert!(m.contains("`wasmtime`"), "{m}");
                }
                Err(e) => panic!("{entry}: {e:?}"),
                Ok(_) => panic!("{entry}: loaded without the wasmtime backend"),
            }
        }
    }

    /// The name tables keep their fixed order and field names.
    #[cfg(any(feature = "lua", feature = "wasmtime"))]
    #[test]
    fn lua_tables_order() {
        let names = NameTables {
            switches: vec![("start".into(), pin_core::ids::SwitchId(4))],
            persist: vec![("hi".into(), pin_core::ids::PersistKey(2))],
            ..NameTables::default()
        };
        let t = names.lua_tables();
        let fields: Vec<&str> = t.iter().map(|(f, _)| *f).collect();
        assert_eq!(
            fields,
            [
                "sw",
                "coil",
                "lamp",
                "magnet",
                "diverter",
                "flipper",
                "dispenser",
                "profile",
                "sound",
                "scene",
                "persist_key"
            ]
        );
        assert_eq!(t[0].1, vec![("start", 4)]);
        assert_eq!(t[10].1, vec![("hi", 2)]);
        assert!(t[1].1.is_empty());
    }
}
