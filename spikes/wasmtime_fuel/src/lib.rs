//! Spike 0.1/Q6b: `wasmtime` fuel metering.
//!
//! Minimal recipe: an engine with `consume_fuel(true)`; every call gets a fuel
//! budget; running out traps with [`wasmtime::Trap::OutOfFuel`].

use wasmtime::{Config, Engine, Instance, Module, Store};

/// Engine with fuel metering enabled.
pub fn fuel_engine() -> wasmtime::Result<Engine> {
    let mut config = Config::new();
    config.consume_fuel(true);
    Engine::new(&config)
}

/// Instantiate `wat` in a fresh store holding `fuel` units.
pub fn instantiate(
    engine: &Engine,
    wat: &str,
    fuel: u64,
) -> wasmtime::Result<(Store<()>, Instance)> {
    let module = Module::new(engine, wat)?;
    let mut store = Store::new(engine, ());
    store.set_fuel(fuel)?;
    let instance = Instance::new(&mut store, &module, &[])?;
    Ok((store, instance))
}
