//! Black-box tests for REQ-LAT-006 (SPEC §7.5, §2.2, D19): playfield frame latency, software
//! adapter detection, the one-warning rule and the lean quality settings. GPU-free.

use pin_render::gpu::wgpu::{self, Backend, DeviceType};
use pin_render::gpu::{
    LeanQuality, PLAYFIELD_FRAME_LATENCY, SECONDARY_FRAME_LATENCY, frame_latency_for,
    is_cpu_adapter, lean_quality_for,
};
use pin_render::post::{AaMode, BloomConfig, MotionBlurConfig, PostConfig, ToneMap};
use pin_render::topology::SurfaceRole;
use proptest::prelude::*;

const ALL_TYPES: [DeviceType; 5] = [
    DeviceType::Other,
    DeviceType::IntegratedGpu,
    DeviceType::DiscreteGpu,
    DeviceType::VirtualGpu,
    DeviceType::Cpu,
];

fn info(name: &str, device_type: DeviceType, backend: Backend) -> wgpu::AdapterInfo {
    wgpu::AdapterInfo {
        name: name.to_string(),
        vendor: 0x10de,
        device: 0x1234,
        device_type,
        device_pci_bus_id: String::new(),
        driver: "driver".to_string(),
        driver_info: "driver info".to_string(),
        backend,
        subgroup_min_size: 0,
        subgroup_max_size: 0,
        transient_saves_memory: None,
        limit_bucket: Default::default(),
    }
}

#[test]
fn req_lat_006_frame_latency_per_surface_role() {
    assert_eq!(PLAYFIELD_FRAME_LATENCY, 1);
    assert_eq!(SECONDARY_FRAME_LATENCY, 2);
    assert_eq!(frame_latency_for(SurfaceRole::Playfield), 1);
    assert_eq!(
        frame_latency_for(SurfaceRole::Playfield),
        PLAYFIELD_FRAME_LATENCY
    );
    assert_eq!(frame_latency_for(SurfaceRole::Backglass), 2);
    assert_eq!(frame_latency_for(SurfaceRole::Score), 2);
    assert_eq!(
        frame_latency_for(SurfaceRole::Backglass),
        SECONDARY_FRAME_LATENCY
    );
    assert_eq!(
        frame_latency_for(SurfaceRole::Score),
        SECONDARY_FRAME_LATENCY
    );
}

#[test]
fn req_lat_006_is_cpu_adapter_by_device_type_only() {
    for t in ALL_TYPES {
        for backend in [Backend::Vulkan, Backend::Gl, Backend::Noop] {
            let i = info("some adapter", t, backend);
            assert_eq!(
                is_cpu_adapter(&i),
                t == DeviceType::Cpu,
                "{t:?} {backend:?}"
            );
        }
    }
}

#[test]
fn req_lat_006_is_cpu_adapter_ignores_name() {
    assert!(is_cpu_adapter(&info(
        "NVIDIA GeForce RTX 4090",
        DeviceType::Cpu,
        Backend::Vulkan
    )));
    assert!(!is_cpu_adapter(&info(
        "llvmpipe (LLVM 17.0.6, 256 bits)",
        DeviceType::DiscreteGpu,
        Backend::Vulkan
    )));
    assert!(!is_cpu_adapter(&info(
        "llvmpipe",
        DeviceType::Other,
        Backend::Vulkan
    )));
}

#[test]
fn req_lat_006_cpu_adapter_warns_once_and_goes_lean() {
    let i = info("Software Rasterizer XYZ", DeviceType::Cpu, Backend::Vulkan);
    let mut msgs: Vec<String> = Vec::new();
    let r = lean_quality_for(&i, &mut |m: &str| msgs.push(m.to_string()));
    assert_eq!(r, Some(LeanQuality::lean()));
    assert_eq!(msgs.len(), 1, "exactly one warning");
    assert!(
        msgs[0].contains("Software Rasterizer XYZ"),
        "message: {}",
        msgs[0]
    );
}

#[test]
fn req_lat_006_non_cpu_adapters_never_warn_and_stay_default() {
    for t in ALL_TYPES.into_iter().filter(|t| *t != DeviceType::Cpu) {
        let i = info("Real GPU", t, Backend::Vulkan);
        let mut count = 0u32;
        let r = lean_quality_for(&i, &mut |_| count += 1);
        assert_eq!(r, None, "{t:?}");
        assert_eq!(count, 0, "{t:?}");
    }
}

#[test]
fn req_lat_006_repeated_calls_have_no_hidden_once_state() {
    let i = info("cpu0", DeviceType::Cpu, Backend::Vulkan);
    let mut count = 0u32;
    let mut results = Vec::new();
    for _ in 0..5 {
        results.push(lean_quality_for(&i, &mut |_| count += 1));
    }
    assert_eq!(count, 5);
    assert!(results.iter().all(|r| *r == Some(LeanQuality::lean())));
}

#[test]
fn req_lat_006_lean_values_exact() {
    let l = LeanQuality::lean();
    assert_eq!(l.msaa_samples, 1);
    assert!(!l.bloom);
    assert!(!l.motion_blur);
    assert!(!l.probe_captures);
    assert_eq!(l.render_scale, 0.5);
}

#[test]
fn req_lat_006_apply_to_post_default() {
    let d = PostConfig::default();
    let r = LeanQuality::lean().apply_to_post(d);
    assert_eq!(r.aa, AaMode::Off);
    assert_eq!(r.aa.samples(), 1);
    assert!(!r.bloom.enabled);
    assert!(!r.motion_blur.enabled);
    assert_eq!(r.exposure, d.exposure);
    assert_eq!(r.tone_map, d.tone_map);
    assert_eq!(r.bloom.threshold, d.bloom.threshold);
    assert_eq!(r.bloom.knee, d.bloom.knee);
    assert_eq!(r.bloom.intensity, d.bloom.intensity);
    assert_eq!(r.bloom.radius, d.bloom.radius);
    assert_eq!(r.bloom.firefly_clamp, d.bloom.firefly_clamp);
    assert_eq!(r.bloom.max_levels, d.bloom.max_levels);
    assert_eq!(r.motion_blur.shutter_s, d.motion_blur.shutter_s);
    assert_eq!(r.motion_blur.samples, d.motion_blur.samples);
    assert_eq!(r.motion_blur.max_length_px, d.motion_blur.max_length_px);
}

#[test]
fn req_lat_006_apply_to_post_neutral_unchanged() {
    let n = PostConfig::neutral();
    assert_eq!(LeanQuality::lean().apply_to_post(n), n);
}

fn arb_post() -> impl Strategy<Value = PostConfig> {
    (
        (
            -100.0f32..100.0,
            any::<bool>(),
            0.0f32..50.0,
            0.0f32..5.0,
            0.0f32..1.0,
        ),
        (
            0.0f32..1.0,
            -10.0f32..1000.0,
            1u32..16,
            any::<bool>(),
            0.0f32..0.1,
        ),
        (
            1u32..=64,
            0.0f32..512.0,
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        ),
    )
        .prop_map(|(a, b, c)| PostConfig {
            exposure: a.0,
            bloom: BloomConfig {
                enabled: a.1,
                threshold: a.2,
                knee: a.3,
                intensity: a.4,
                radius: b.0,
                firefly_clamp: b.1,
                max_levels: b.2,
            },
            tone_map: if c.4 {
                ToneMap::AcesFilm
            } else {
                ToneMap::Linear
            },
            motion_blur: MotionBlurConfig {
                enabled: b.3,
                shutter_s: b.4,
                samples: c.0,
                max_length_px: c.1,
            },
            aa: if c.2 { AaMode::Msaa4 } else { AaMode::Off },
        })
}

proptest! {
    /// REQ-LAT-006: lean settings disable exactly AA, bloom and blur, keep all else, idempotent.
    #[test]
    fn req_lat_006_apply_to_post_properties(p in arb_post()) {
        let lean = LeanQuality::lean();
        let r = lean.apply_to_post(p);
        prop_assert_eq!(r.aa, AaMode::Off);
        prop_assert!(!r.bloom.enabled);
        prop_assert!(!r.motion_blur.enabled);
        let mut expect = p;
        expect.aa = AaMode::Off;
        expect.bloom.enabled = false;
        expect.motion_blur.enabled = false;
        prop_assert_eq!(r, expect);
        prop_assert_eq!(lean.apply_to_post(r), r);
    }
}
