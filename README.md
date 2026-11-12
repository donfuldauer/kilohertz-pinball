# Kilohertz Pinball

A cross-platform pinball simulation engine for Windows, Linux and macOS, written in Rust. The name comes from its 1,000 Hz fixed-timestep simulation loop.

**Status:** planning complete, no code yet.

## Plan

The full architecture and execution plan is in [`docs/PLAN.md`](docs/PLAN.md). It is written to be executed by autonomous coding agents with one human setup step before the run and one acceptance step after it.

Highlights:

- 1 kHz physics, input and scripting on a dedicated thread; `wgpu` rendering (Vulkan, DX12, Metal)
- `parry3d` collision with C² spline wireforms, torque-driven flippers, multi-material balls
- Table bundles of glTF, TOML and sandboxed rules in Lua 5.4 or WebAssembly
- Single-window or three-monitor cabinet output, X-Arcade profiles, SSF audio, UDP telemetry
- 37 subphases in 10 waves, each self-validated by an implementer, an independent tester and a reviewer

## Getting started

Execution begins with Subphase 0.1 (dependency spikes) and Subphase 1.1 (workspace and contracts) as described in the plan. The prerequisites for an unattended run are listed in section 2.6 of the plan.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
