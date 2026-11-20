## w8h-libtest-args — Shared libtest argument helper lives in a dev-only crate

**Decision:**

- A new std-only workspace crate `crates/pin_testmain` (no dependencies, `publish = false` through
  the workspace) holds the one implementation of libtest-compatible argument handling for
  `harness = false` test targets: `Options`, `parse_args` (pure, never panics, never exits), `Plan`
  (`Run`, `List`, `SkipAll`), `plan` (pure) and `libtest_main(test_name, body)`, which parses
  `std::env::args().skip(1)`, prints libtest-style output and either runs `body` once on the calling
  thread or exits 0 (`--help`, `--list`, filtered out). It spawns no thread and touches no SDL or GPU
  state. The root `[workspace.dependencies]` gains `pin_testmain`; pin_app, pin_render and pin_audio
  take it under `[dev-dependencies]` only. No other crate depends on it.
- All eight `harness = false` targets of the workspace rename their old `main` to `run` (body
  unchanged) and get `fn main() { pin_testmain::libtest_main("<[[test]] name>", run) }`, so the
  helper runs before any SDL init, window or wgpu call, on the main thread: pin_app
  `platform_headless`, `orchestrator_window`, `presenter_thread_window`,
  `profiler_debug_view_window`; pin_render `gpu_window`, `gpu_surface_hooks_window`; pin_audio
  `mixer_sdl_smoke`, `spec_mixer_sdl`.
- `spikes/` (a separate, excluded workspace with `wgpu_probe`, `sdl_timestamp`,
  `sdl_virtual_joystick`) is out of scope and unchanged.
- Small choices the brief left open: a lone `--` is skipped and parsing continues (later arguments
  are still parsed as flags or filters); a value option or `--skip` at the end of the arguments with
  no value is accepted and ignored; `--opt=value` splitting applies only to `--` options (a `-x=y`
  argument is an unknown flag); none of the eight module docs said the target ignored its arguments,
  so each gets one added sentence ("The target accepts libtest filters and `--list` through
  `pin_testmain`.") next to its existing `harness = false` explanation, and no other doc changes.

**Rationale:** One implementation instead of eight copies. As a normal lib crate its parser unit
tests run once under libtest, in the GPU-free `core-tests` CI job (which excludes only pin_render,
pin_audio and pin_app). Nothing changes in `pin_core`, so no contract change request (D4), and no
product code (`src/` of an existing crate) changes. The unfiltered CI `gpu-tests` command still runs
every windowed body, while a filtered or `--list` run no longer opens a window or fails under
`PIN_REQUIRE_GPU=1`.

**Alternatives:**

- A `#[path]`-included file shared by the targets: its unit tests would never run (the including
  targets are `harness = false`), and each target would compile its own copy.
- A module in `pin_core`: needs a contract change request and puts test-harness code in the shared
  contract crate.
- Converting the targets back to libtest: SDL needs the process main thread (Cocoa) and one init per
  process, which libtest's worker threads break; that is why they are `harness = false`.
- Two copies (one per crate family): rejected by the owner.
