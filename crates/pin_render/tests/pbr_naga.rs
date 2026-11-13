//! Verification 1 (5.1): naga parses and validates every PBR WGSL module and
//! translates it to SPIR-V, MSL and HLSL (GPU-free).

use naga::back::{hlsl, msl, spv};
use naga::valid::{Capabilities, ValidationFlags, Validator};
use pin_render::pbr::shader_sources;

fn validated(name: &str, src: &str) -> (naga::Module, naga::valid::ModuleInfo) {
    let module = naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|e| panic!("{name}: WGSL parse error:\n{}", e.emit_to_string(src)));
    let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: validation error:\n{}", e.emit_to_string(src)));
    (module, info)
}

#[test]
fn every_module_is_listed_once_and_non_empty() {
    let sources = shader_sources();
    assert!(sources.len() >= 2);
    let mut names: Vec<_> = sources.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), sources.len(), "duplicate module names");
    for (name, src) in sources {
        assert!(!src.trim().is_empty(), "{name} is empty");
    }
}

#[test]
fn every_module_validates() {
    for (name, src) in shader_sources() {
        validated(name, src);
    }
}

#[test]
fn every_module_translates_to_spirv() {
    for (name, src) in shader_sources() {
        let (module, info) = validated(name, src);
        let words = spv::write_vec(&module, &info, &spv::Options::default(), None)
            .unwrap_or_else(|e| panic!("{name}: SPIR-V: {e}"));
        assert_eq!(words.first(), Some(&0x0723_0203), "{name}: SPIR-V magic");
    }
}

#[test]
fn every_module_translates_to_msl() {
    for (name, src) in shader_sources() {
        let (module, info) = validated(name, src);
        let (text, _) = msl::write_string(
            &module,
            &info,
            &msl::Options::default(),
            &msl::PipelineOptions::default(),
        )
        .unwrap_or_else(|e| panic!("{name}: MSL: {e}"));
        assert!(text.contains("metal"), "{name}: MSL output");
    }
}

#[test]
fn every_module_translates_to_hlsl() {
    for (name, src) in shader_sources() {
        let (module, info) = validated(name, src);
        let mut text = String::new();
        let options = hlsl::Options::default();
        let pipeline = hlsl::PipelineOptions::default();
        hlsl::Writer::new(&mut text, &options, &pipeline)
            .write(&module, &info, None)
            .unwrap_or_else(|e| panic!("{name}: HLSL: {e}"));
        if !module.entry_points.is_empty() {
            assert!(!text.is_empty(), "{name}: HLSL output");
        }
    }
}

#[test]
fn forward_module_has_the_renderer_entry_points() {
    let (_, src) = shader_sources()
        .iter()
        .find(|(n, _)| *n == "pbr_forward")
        .expect("pbr_forward");
    let (module, _) = validated("pbr_forward", src);
    let names: Vec<_> = module.entry_points.iter().map(|e| e.name.as_str()).collect();
    for ep in ["vs_main", "fs_opaque", "fs_blended"] {
        assert!(names.contains(&ep), "missing entry point {ep}: {names:?}");
    }
}
