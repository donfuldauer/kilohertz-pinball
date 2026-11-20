# Queue: w8g-d

- [ ] [w8g-d-sdl-test-thread] The two new windowed tests (`pin_render/tests/gpu_surface_hooks_window.rs`, `pin_app/tests/profiler_debug_view_window.rs`) run SDL on a libtest thread because `Cargo.toml` (outside w8g-d's owned paths) has no `harness = false` entries for them. That works on X11/Wayland (the CI GPU job) but not on Cocoa; a later unit owning the two `Cargo.toml` files could add `[[test]] harness = false, required-features = ["gpu-tests"]` entries with a plain `main` (decision "w8g-d — debug-view profiling hook").
