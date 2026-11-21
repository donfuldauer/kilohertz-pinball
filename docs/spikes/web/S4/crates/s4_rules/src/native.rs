//! Native baseline: the repo's own wasmtime backend
//! (`pin_script::wasm::WasmScriptBackend`, engine config untouched) as an
//! `ExternalScript`, with the read-only fuel probe of the scratch patch.

use crate::run::Rules;
use pin_core::ids::SwitchId;
use pin_core::{HostCtx, ScriptError, SwitchEdge, TableScriptBackend};
use pin_script::NameTables;
use pin_script::wasm::{WasmConfig, WasmScriptBackend};

/// wasmtime backend + fuel probe.
pub struct NativeRules {
    inner: WasmScriptBackend,
    budget: u64,
    last: u64,
}

impl NativeRules {
    /// `WasmScriptBackend::new`.
    ///
    /// # Errors
    /// Its load errors.
    pub fn new(src: &[u8], names: &NameTables, cfg: WasmConfig) -> Result<Self, ScriptError> {
        Ok(Self {
            inner: WasmScriptBackend::new(src, names, cfg)?,
            budget: cfg.fuel_per_callback,
            last: 0,
        })
    }

    /// The wrapped backend.
    #[must_use]
    pub fn inner(&self) -> &WasmScriptBackend {
        &self.inner
    }

    fn note(&mut self) {
        self.last = self.budget.saturating_sub(self.inner.fuel_left());
    }
}

impl TableScriptBackend for NativeRules {
    fn on_init(&mut self, ctx: &mut HostCtx) {
        let was = self.inner.is_faulted();
        self.inner.on_init(ctx);
        if !was {
            self.note();
        }
    }
    fn on_tick(&mut self, ctx: &mut HostCtx) {
        let was = self.inner.is_faulted();
        self.inner.on_tick(ctx);
        if !was {
            self.note();
        }
    }
    fn on_switch_edge(&mut self, s: SwitchId, e: SwitchEdge, ctx: &mut HostCtx) {
        let was = self.inner.is_faulted();
        self.inner.on_switch_edge(s, e, ctx);
        if !was {
            self.note();
        }
    }
    fn apply_reload(&mut self, src: &[u8]) -> Result<(), ScriptError> {
        self.inner.apply_reload(src)
    }
}

impl pin_sim::ExternalScript for NativeRules {
    fn overflow_count(&self) -> u32 {
        self.inner.overflow_count()
    }
    fn is_faulted(&self) -> bool {
        self.inner.is_faulted()
    }
    fn fault_count(&self) -> u32 {
        self.inner.fault_count()
    }
    fn last_error(&self) -> Option<&ScriptError> {
        self.inner.last_error()
    }
}

impl Rules for NativeRules {
    fn last_fuel(&self) -> u64 {
        self.last
    }
    fn memory(&self) -> Vec<u8> {
        self.inner.memory_bytes().to_vec()
    }
}
