//! 5.2 shaders (post chain, probe remap, and the PBR forward module with its
//! reflection include): naga parses, validates and translates them to
//! SPIR-V, MSL and HLSL (GPU-free).

use naga::back::{hlsl, msl, spv};
use naga::valid::{Capabilities, ValidationFlags, Validator};

fn all_sources() -> Vec<(&'static str, &'static str)> {
    let mut v: Vec<_> = pin_render::post::shader_sources().to_vec();
    v.extend_from_slice(pin_render::pbr::shader_sources());
    v
}

fn validated(name: &str, src: &str) -> (naga::Module, naga::valid::ModuleInfo) {
    let module = naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|e| panic!("{name}: WGSL parse error:\n{}", e.emit_to_string(src)));
    let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: validation error:\n{}", e.emit_to_string(src)));
    (module, info)
}

fn entry_points(name: &str) -> Vec<String> {
    let src = all_sources()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("module {name} missing"))
        .1;
    let (module, _) = validated(name, src);
    module.entry_points.iter().map(|e| e.name.clone()).collect()
}

#[test]
fn modules_validate_and_translate_everywhere() {
    for (name, src) in all_sources() {
        assert!(!src.trim().is_empty(), "{name} empty");
        let (module, info) = validated(name, src);
        let words = spv::write_vec(&module, &info, &spv::Options::default(), None)
            .unwrap_or_else(|e| panic!("{name}: SPIR-V: {e}"));
        assert_eq!(words.first(), Some(&0x0723_0203), "{name}: SPIR-V magic");
        let (text, _) = msl::write_string(
            &module,
            &info,
            &msl::Options::default(),
            &msl::PipelineOptions::default(),
        )
        .unwrap_or_else(|e| panic!("{name}: MSL: {e}"));
        assert!(text.contains("metal"), "{name}: MSL output");
        let mut hl = String::new();
        hlsl::Writer::new(
            &mut hl,
            &hlsl::Options::default(),
            &hlsl::PipelineOptions::default(),
        )
        .write(&module, &info, None)
        .unwrap_or_else(|e| panic!("{name}: HLSL: {e}"));
        assert!(!hl.is_empty(), "{name}: HLSL output");
    }
}

#[test]
fn entry_points_are_present() {
    let post = entry_points("post_chain");
    for ep in [
        "vs_fullscreen",
        "fs_blur",
        "fs_prefilter",
        "fs_down",
        "fs_up",
        "fs_composite",
    ] {
        assert!(post.iter().any(|e| e == ep), "post_chain lacks {ep}");
    }
    let remap = entry_points("probe_remap");
    assert!(remap.iter().any(|e| e == "fs_remap"));
    let forward = entry_points("pbr_forward");
    assert!(forward.iter().any(|e| e == "fs_opaque"));
}

/// The CPU references and the shaders state the same constants.
#[test]
fn shaders_match_the_cpu_references() {
    let post = pin_render::post::shader_sources()[0].1;
    for needle in [
        "2.51 * v + 0.03",
        "2.43 * v + 0.59) + 0.14",
        "vec3<f32>(0.2126, 0.7152, 0.0722)",
        &format!("MAX_BLUR_BALLS: u32 = {}u", pin_render::post::MAX_BLUR_BALLS),
        &format!("array<BlurBall, {}>", pin_render::post::MAX_BLUR_BALLS),
    ] {
        assert!(post.contains(needle), "post shader lacks `{needle}`");
    }
    let forward = pin_render::pbr::shader_sources()
        .iter()
        .find(|(n, _)| *n == "pbr_forward")
        .unwrap()
        .1;
    assert!(forward.contains("fn oct_encode"));
    assert!(forward.contains("@group(2) @binding(0) var probe_atlas: texture_2d_array<f32>"));
}
