//! The limits of each backend, [`LuaConfig`] and [`WasmConfig`] (§3.4, D25).
//!
//! Built under every feature combination (REQ-WEB-001), so code that only
//! passes limits around (`pin_sim`'s configuration, the web build) keeps its
//! shape whether or not a backend is compiled in. They are re-exported as
//! [`crate::lua::LuaConfig`] and [`crate::wasm::WasmConfig`], the paths the
//! engine uses.

/// Limits of one Lua backend (§3.4, D25).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LuaConfig {
    /// Hard cap on the Lua heap, bytes.
    pub memory_limit_bytes: usize,
    /// VM instructions allowed per callback (granularity `lua::HOOK_INTERVAL`).
    pub instruction_budget: u32,
}

impl LuaConfig {
    /// The configuration the engine runs table rules with (DECISIONS.md
    /// "Orchestrator — 4.2 re-review round 3"): 100 000 instructions per
    /// callback and a 2 MiB heap. Under it no script can make a single tick
    /// take more than 8 ms (release build; measured in DECISIONS.md "4.2 — fix
    /// round 3"). [`LuaConfig::default`] keeps the brief's larger limits for
    /// tools and tests.
    pub const fn engine() -> Self {
        Self {
            memory_limit_bytes: 2 * 1024 * 1024,
            instruction_budget: 100_000,
        }
    }
}

impl Default for LuaConfig {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 16 * 1024 * 1024,
            instruction_budget: 1_000_000,
        }
    }
}

/// Limits of one Wasm backend (§3.4, D25).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WasmConfig {
    /// Hard cap on the guest's linear memory, bytes.
    pub memory_limit_bytes: usize,
    /// Fuel (≈ Wasm operators) allowed per callback.
    pub fuel_per_callback: u64,
}

impl WasmConfig {
    /// The configuration the engine runs table rules with, matching
    /// [`crate::lua::LuaConfig::engine`]: a 2 MiB memory cap and 100 000 fuel
    /// per callback. [`WasmConfig::default`] keeps the larger tool/test
    /// limits.
    pub const fn engine() -> Self {
        Self {
            memory_limit_bytes: 2 * 1024 * 1024,
            fuel_per_callback: 100_000,
        }
    }
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 16 * 1024 * 1024,
            fuel_per_callback: 1_000_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::TypeId;

    /// D25 / SPEC §3.4 (REQ-WEB-001: built with every feature set): the
    /// engine limits are 2 MiB / 100 000 for both backends.
    #[test]
    fn engine_limits_req_web_001() {
        const LUA: LuaConfig = LuaConfig::engine();
        const WASM: WasmConfig = WasmConfig::engine();
        assert_eq!(LUA.memory_limit_bytes, 2_097_152);
        assert_eq!(LUA.instruction_budget, 100_000);
        assert_eq!(WASM.memory_limit_bytes, 2_097_152);
        assert_eq!(WASM.fuel_per_callback, 100_000);
    }

    /// The tool/test defaults are 16 MiB / 1 000 000 for both backends.
    #[test]
    fn default_limits() {
        let lua = LuaConfig::default();
        assert_eq!(lua.memory_limit_bytes, 16_777_216);
        assert_eq!(lua.instruction_budget, 1_000_000);
        let wasm = WasmConfig::default();
        assert_eq!(wasm.memory_limit_bytes, 16_777_216);
        assert_eq!(wasm.fuel_per_callback, 1_000_000);
        assert_ne!(lua, LuaConfig::engine());
        assert_ne!(wasm, WasmConfig::engine());
        assert!(format!("{lua:?} {wasm:?}").contains("instruction_budget"));
    }

    /// REQ-WEB-001: `lua::LuaConfig` and `wasm::WasmConfig` are these types
    /// under the features this test binary was built with.
    #[test]
    fn backend_paths_are_the_same_types_req_web_001() {
        assert_eq!(
            TypeId::of::<crate::lua::LuaConfig>(),
            TypeId::of::<LuaConfig>()
        );
        assert_eq!(
            TypeId::of::<crate::wasm::WasmConfig>(),
            TypeId::of::<WasmConfig>()
        );
        // Interchangeable without conversion.
        let l: crate::lua::LuaConfig = LuaConfig::engine();
        let w: crate::wasm::WasmConfig = WasmConfig::engine();
        assert_eq!((l, w), (LuaConfig::engine(), WasmConfig::engine()));
    }
}
