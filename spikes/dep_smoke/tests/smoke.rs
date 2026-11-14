use dep_smoke::{Sample, first_four, pcg_stream};
use proptest::prelude::*;

#[test]
fn serde_toml_round_trip() {
    let s = Sample {
        name: "kHz".into(),
        tilt_threshold: 0.75,
        balls: 3,
        switches: vec![1, 2, 42],
    };
    let text = toml::to_string(&s).unwrap();
    let back: Sample = toml::from_str(&text).unwrap();
    assert_eq!(s, back);
}

#[test]
fn arrayvec_caps_at_capacity() {
    let v = first_four(&[1, 2, 3, 4, 5, 6]);
    assert_eq!(v.as_slice(), &[1, 2, 3, 4]);
    assert!(v.is_full());
}

#[test]
fn pcg_is_seed_deterministic() {
    assert_eq!(pcg_stream::<8>(7), pcg_stream::<8>(7));
    assert_ne!(pcg_stream::<8>(7), pcg_stream::<8>(8));
}

#[test]
fn libm_is_usable() {
    assert_eq!(libm::sqrt(4.0), 2.0);
    assert_eq!(libm::sinf(0.5).to_bits(), libm::sinf(0.5).to_bits());
    assert!((libm::sin(std::f64::consts::FRAC_PI_2) - 1.0).abs() < 1e-15);
}

#[test]
fn gltf_parses_minimal_document() {
    let g =
        gltf::Gltf::from_slice(br#"{"asset":{"version":"2.0"},"scenes":[{"nodes":[]}]}"#).unwrap();
    assert_eq!(g.scenes().count(), 1);
}

#[test]
fn notify_watcher_constructs_and_watches() {
    use notify::Watcher;
    let dir = std::env::temp_dir().join(format!("khz-notify-spike-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut w = notify::recommended_watcher(|_res: notify::Result<notify::Event>| {}).unwrap();
    w.watch(&dir, notify::RecursiveMode::NonRecursive).unwrap();
    w.unwatch(&dir).unwrap();
    drop(w);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn egui_runs_a_frame_and_tessellates() {
    let ctx = egui::Context::default();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.label("Kilohertz Pinball");
    });
    // No painter here: discard texture uploads explicitly (epaint asserts otherwise).
    out.textures_delta.clear();
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    assert!(!prims.is_empty());
}

#[test]
fn egui_wgpu_renderer_builds_when_an_adapter_exists() {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        println!("[dep_smoke] no wgpu adapter; egui-wgpu checked at compile/link time only");
        return;
    };
    let (device, _queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))
    .unwrap();
    let _renderer = egui_wgpu::Renderer::new(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        egui_wgpu::RendererOptions::default(),
    );
    println!(
        "[dep_smoke] egui-wgpu Renderer created on {}",
        adapter.get_info().name
    );
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn proptest_runs(xs in proptest::collection::vec(any::<u32>(), 0..16)) {
        let v = first_four(&xs);
        prop_assert_eq!(v.len(), xs.len().min(4));
    }
}
