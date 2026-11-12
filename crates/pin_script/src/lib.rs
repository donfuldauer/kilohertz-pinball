//! `pin_script`: Table scripting backends: Lua 5.4 and WebAssembly.
//!
//! * [`lua`]: the Lua 5.4 backend (Subphase 4.2): sandbox, memory cap,
//!   instruction budget and hot reload (PLAN.md §3.4, D10, D11, D25).
//! * [`wasm`]: the WebAssembly backend (Subphase 4.5; stub).
//! * [`load_backend`]: picks a backend from the extension of `script_entry`.
//! * [`NameTables`]: the name → id tables scripts see as `pinball.sw`, `pinball.coil`, …

pub mod guest_sdk;
pub mod lua;
pub mod wasm;

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

/// Builds the rules backend named by the extension of `script_entry` (§3.4):
/// `.lua` → [`lua::LuaScriptBackend`] with [`lua::LuaConfig::default`];
/// `.wasm` → `Err(UnsupportedBackend)` until Subphase 4.5 lands the Wasm
/// backend; anything else → `Err(UnsupportedBackend)`. Extensions are matched
/// case-insensitively.
pub fn load_backend(
    script_entry: &str,
    source: &[u8],
    names: &NameTables,
) -> Result<Box<dyn TableScriptBackend>, ScriptError> {
    let ext = Path::new(script_entry)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("lua") => Ok(Box::new(lua::LuaScriptBackend::new(
            source,
            names,
            lua::LuaConfig::default(),
        )?)),
        Some("wasm") => Err(ScriptError::UnsupportedBackend(format!(
            "{script_entry}: the .wasm backend is unsupported until Subphase 4.5"
        ))),
        Some(other) => Err(ScriptError::UnsupportedBackend(format!(
            "{script_entry}: unknown script extension .{other} (expected .lua or .wasm)"
        ))),
        None => Err(ScriptError::UnsupportedBackend(format!(
            "{script_entry}: script_entry has no extension (expected .lua or .wasm)"
        ))),
    }
}
