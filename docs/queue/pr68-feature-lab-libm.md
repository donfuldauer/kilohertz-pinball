# Queue — pr68-feature-lab-libm (Feature Lab builder libm fix)

- [ ] [pr68-validate-lint-libm] crates/pin_assets/src/validate/guide_lint.rs:72 (f64::hypot) and rules.rs:1282 (f64::asin) use platform libm; lint warnings may differ across glibc versions. Switch to libm and consider clippy disallowed-methods for std f32/f64 transcendentals in pin_* build and sim crates.
