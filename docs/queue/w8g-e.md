# Human queue — w8g-e (std transcendental ban scope)

- [ ] [w8g-e-1] open: `crates/pin_app/examples/build_feature_lab.rs` builds bundle content but is covered by review only (pin_app is exempt, so it has no clippy.toml); it uses `libm::atan2f` / `libm::hypot` today. Possible later: move bundle building into a crate that has the ban (e.g. pin_tablegen or pin_assetgen).
- [ ] [w8g-e-2] open: pin_render, pin_audio and the pin_app runtime are exempt because their floats are presentation-only; if any of their float results ever feeds sim state, replays or bundled assets, copy `crates/pin_assets/clippy.toml` into that crate and migrate its sites to `libm`.
