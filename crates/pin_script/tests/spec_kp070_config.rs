//! REQ-WEB-001, D10, D25: the engine/default limits keep their values and
//! the config types keep their paths under every feature combination.

use pin_script::config::{LuaConfig, WasmConfig};

const MIB: usize = 1024 * 1024;

fn take_lua_via_lua_path(c: pin_script::lua::LuaConfig) -> LuaConfig {
    c
}
fn take_wasm_via_wasm_path(c: pin_script::wasm::WasmConfig) -> WasmConfig {
    c
}
fn is_copy_clone_debug_eq<T: Copy + Clone + std::fmt::Debug + PartialEq + Eq>() {}

#[test]
fn req_web_001_lua_config_values() {
    let e = LuaConfig::engine();
    assert_eq!(e.memory_limit_bytes, 2 * MIB);
    assert_eq!(e.instruction_budget, 100_000);
    let d = LuaConfig::default();
    assert_eq!(d.memory_limit_bytes, 16 * MIB);
    assert_eq!(d.instruction_budget, 1_000_000);
    assert_ne!(e, d);
}

#[test]
fn req_web_001_wasm_config_values() {
    let e = WasmConfig::engine();
    assert_eq!(e.memory_limit_bytes, 2 * MIB);
    assert_eq!(e.fuel_per_callback, 100_000);
    let d = WasmConfig::default();
    assert_eq!(d.memory_limit_bytes, 16 * MIB);
    assert_eq!(d.fuel_per_callback, 1_000_000);
    assert_ne!(e, d);
}

#[test]
fn req_web_001_config_paths_are_the_same_types() {
    let l = take_lua_via_lua_path(LuaConfig::engine());
    assert_eq!(l, LuaConfig::engine());
    let w = take_wasm_via_wasm_path(WasmConfig::engine());
    assert_eq!(w, WasmConfig::engine());
    let l2: pin_script::lua::LuaConfig = pin_script::lua::LuaConfig::default();
    assert_eq!(l2, LuaConfig::default());
}

#[test]
fn req_web_001_config_traits() {
    is_copy_clone_debug_eq::<LuaConfig>();
    is_copy_clone_debug_eq::<WasmConfig>();
    let a = LuaConfig::engine();
    let b = a;
    assert_eq!(a.clone(), b);
    assert!(!format!("{a:?}").is_empty());
    let w = WasmConfig::engine();
    let w2 = w;
    assert_eq!(w.clone(), w2);
    assert!(!format!("{w:?}").is_empty());
}
