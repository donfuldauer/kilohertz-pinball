# Spec fixtures for subphase 4.5 (Wasm scripting backend)

Written by the Independent Tester from the brief's fixed ABI alone.

| File | Purpose |
| --- | --- |
| `support.rs` | Shared harness, included by each `spec_wasm_*.rs` with `#[path]`. |
| `driver.wat` | Hand-written guest: one import call per `on_switch_edge`, arguments read from persist slots 0..=3. |
| `driver.wasm` | `driver.wat` assembled to binary (wat 1.263.0, `wat::parse_bytes`), committed so the binary path is tested without a toolchain. Reassemble with any WAT assembler (`wasm-tools parse driver.wat -o driver.wasm`) after editing `driver.wat`. |
| `parity_rules.lua` / `parity_rules.wat` | The same integer-only ruleset written independently for both backends (criterion 1). |
| `parity_fault.lua` / `parity_fault.wat` | The same faulting ruleset for both backends (tilt parity). |
