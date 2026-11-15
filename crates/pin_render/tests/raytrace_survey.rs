//! 5.4 requirement 1: which backends / adapters report
//! `Features::EXPERIMENTAL_RAY_QUERY` on the pinned wgpu (gpu-tests; prints
//! one line per adapter, skips with a note when there is none unless
//! `PIN_REQUIRE_GPU=1`). The result is recorded in the `raytrace` module
//! docs and DECISIONS "5.4 — ray-query support on wgpu 30.0.1".
#![cfg(feature = "gpu-tests")]

use pin_render::gpu::{create_instance, require_gpu, wgpu};
use pin_render::raytrace::{RAY_QUERY_FEATURE, WGPU_VERSION, adapter_supports_ray_query};

#[test]
fn raytrace_survey_ray_query_support_per_backend() {
    assert_eq!(WGPU_VERSION, "30.0.1");
    let mut seen = 0;
    for (name, backend) in [
        ("vulkan", wgpu::Backends::VULKAN),
        ("dx12", wgpu::Backends::DX12),
        ("metal", wgpu::Backends::METAL),
        ("gl", wgpu::Backends::GL),
    ] {
        let instance = create_instance(backend);
        let adapters = pollster::block_on(instance.enumerate_adapters(backend));
        if adapters.is_empty() {
            println!("[raytrace survey] wgpu {WGPU_VERSION} {name}: no adapter");
        }
        for a in &adapters {
            seen += 1;
            let info = a.get_info();
            let rq = a.features().contains(RAY_QUERY_FEATURE);
            assert_eq!(rq, adapter_supports_ray_query(a));
            println!(
                "[raytrace survey] wgpu {WGPU_VERSION} {name}: \"{}\" ({:?}, driver \"{} {}\") \
                 EXPERIMENTAL_RAY_QUERY = {rq}",
                info.name, info.device_type, info.driver, info.driver_info
            );
        }
    }
    if seen == 0 {
        assert!(
            !require_gpu(),
            "PIN_REQUIRE_GPU=1 but no adapter on any backend"
        );
        println!("[raytrace survey] SKIP: no adapter on any backend");
    }
}
