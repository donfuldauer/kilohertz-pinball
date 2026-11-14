//! Independent spec tests for `pin_render::gpu::{GpuOptions, GpuError, require_gpu}`
//! (Subphase 1.4 FIXED API). No adapter is needed: only option parsing and the error path.
//!
//! Environment variables are process-global, so everything that touches them lives in ONE
//! test function in this binary (no parallel env races).

use pin_render::gpu::{GpuContext, GpuError, GpuOptions, require_gpu};
use std::fmt::Display;

fn set(key: &str, value: Option<&str>) {
    // SAFETY: this test binary has a single test that touches the environment, and the
    // other tests in it never read these variables.
    unsafe {
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}

#[test]
fn env_driven_options() {
    let saved_backend = std::env::var("WGPU_BACKEND").ok();
    let saved_require = std::env::var("PIN_REQUIRE_GPU").ok();

    // PIN_REQUIRE_GPU=1 means "a missing adapter is a failure".
    set("PIN_REQUIRE_GPU", None);
    assert!(!require_gpu(), "unset PIN_REQUIRE_GPU");
    set("PIN_REQUIRE_GPU", Some("1"));
    assert!(require_gpu(), "PIN_REQUIRE_GPU=1");
    set("PIN_REQUIRE_GPU", Some("0"));
    assert!(!require_gpu(), "PIN_REQUIRE_GPU=0");
    set("PIN_REQUIRE_GPU", Some(""));
    assert!(!require_gpu(), "empty PIN_REQUIRE_GPU");
    set("PIN_REQUIRE_GPU", None);

    // WGPU_BACKEND selects the backend set.
    for (name, want) in [
        ("vulkan", wgpu::Backends::VULKAN),
        ("dx12", wgpu::Backends::DX12),
        ("metal", wgpu::Backends::METAL),
        ("gl", wgpu::Backends::GL),
    ] {
        set("WGPU_BACKEND", Some(name));
        let opts = GpuOptions::from_env();
        assert_eq!(opts.backends, want, "WGPU_BACKEND={name}");
    }

    // Without the variable the result is still usable (some backend is selected).
    set("WGPU_BACKEND", None);
    let opts = GpuOptions::from_env();
    assert!(
        !opts.backends.is_empty(),
        "no WGPU_BACKEND: default backends"
    );

    set("WGPU_BACKEND", saved_backend.as_deref());
    set("PIN_REQUIRE_GPU", saved_require.as_deref());
}

#[test]
fn default_options_select_some_backend() {
    let opts = GpuOptions::default();
    assert!(!opts.backends.is_empty());
    let _fallback: bool = opts.force_fallback_adapter;
}

#[test]
fn gpu_error_is_displayable() {
    fn assert_display<T: Display>() {}
    assert_display::<GpuError>();
}

/// With no backend enabled there is no adapter: the constructor must report `NoAdapter`
/// (an `Err`, not a panic), and the message must say something.
#[test]
fn no_backends_gives_no_adapter_error() {
    let opts = GpuOptions {
        backends: wgpu::Backends::empty(),
        ..GpuOptions::default()
    };
    match GpuContext::new_headless(&opts) {
        Ok(_) => panic!("an adapter was found with no backends enabled"),
        Err(e) => {
            assert!(
                matches!(e, GpuError::NoAdapter { .. }),
                "expected NoAdapter, got: {e}"
            );
            assert!(!e.to_string().trim().is_empty());
        }
    }
}
