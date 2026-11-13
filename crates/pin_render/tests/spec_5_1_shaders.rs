//! Independent spec tests for Subphase 5.1, verification criterion 1: `naga` validates
//! every WGSL module returned by `pin_render::pbr::shader_sources()` and translates it to
//! SPIR-V, MSL and HLSL. GPU-free.
//!
//! "Without warnings" is asserted as: the WGSL front end parses, the validator accepts the
//! module with every validation flag on, each back end returns `Ok`, and the per-entry-point
//! results of the MSL and HLSL writers (naga's soft-failure channel) are all `Ok`.
//! Bindings are not mapped here, so the back ends run with `fake_missing_bindings`.

use naga::back::{hlsl, msl, spv};
use naga::valid::{Capabilities, ModuleInfo, ValidationFlags, Validator};
use pin_render::pbr::shader_sources;

fn parse_and_validate(name: &str, src: &str) -> (naga::Module, ModuleInfo) {
    let module = naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|e| panic!("WGSL module `{name}` fails to parse:\n{}", e.emit_to_string(src)));
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| {
            panic!("WGSL module `{name}` fails validation:\n{}", e.emit_to_string(src))
        });
    // Back ends need overrides resolved; modules must be translatable with their defaults.
    let (module, info) = naga::back::pipeline_constants::process_overrides(
        &module,
        &info,
        None,
        &Default::default(),
    )
    .unwrap_or_else(|e| panic!("WGSL module `{name}`: overrides have no usable defaults: {e}"));
    (module.into_owned(), info.into_owned())
}

#[test]
fn shader_sources_is_non_empty_with_unique_non_empty_names() {
    let sources = shader_sources();
    assert!(!sources.is_empty(), "shader_sources() returned no modules");
    let mut names: Vec<&str> = sources.iter().map(|(n, _)| *n).collect();
    for (name, src) in sources {
        assert!(!name.trim().is_empty(), "a shader module has an empty name");
        assert!(!src.trim().is_empty(), "shader module `{name}` has an empty source");
    }
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "shader module names are not unique");
}

#[test]
fn shader_sources_is_stable_across_calls() {
    let a = shader_sources();
    let b = shader_sources();
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.0, y.0);
        assert_eq!(x.1, y.1);
    }
}

#[test]
fn every_module_parses_and_validates() {
    for (name, src) in shader_sources() {
        let _ = parse_and_validate(name, src);
    }
}

/// A forward renderer needs at least one vertex and one fragment entry point somewhere.
#[test]
fn modules_contain_vertex_and_fragment_entry_points() {
    let mut vertex = 0;
    let mut fragment = 0;
    for (name, src) in shader_sources() {
        let (module, _) = parse_and_validate(name, src);
        for ep in &module.entry_points {
            match ep.stage {
                naga::ShaderStage::Vertex => vertex += 1,
                naga::ShaderStage::Fragment => fragment += 1,
                _ => {}
            }
        }
    }
    assert!(vertex >= 1, "no vertex entry point in any PBR module");
    assert!(fragment >= 1, "no fragment entry point in any PBR module");
}

#[test]
fn every_module_translates_to_spirv() {
    for (name, src) in shader_sources() {
        let (module, info) = parse_and_validate(name, src);
        let words = spv::write_vec(&module, &info, &spv::Options::default(), None)
            .unwrap_or_else(|e| panic!("module `{name}` does not translate to SPIR-V: {e}"));
        assert!(words.len() > 5, "module `{name}`: SPIR-V output is empty");
        assert_eq!(words[0], 0x0723_0203, "module `{name}`: bad SPIR-V magic");
    }
}

#[test]
fn every_module_translates_to_msl() {
    for (name, src) in shader_sources() {
        let (module, info) = parse_and_validate(name, src);
        let options = msl::Options {
            lang_version: (2, 4),
            fake_missing_bindings: true,
            ..Default::default()
        };
        let (text, tr) = msl::write_string(&module, &info, &options, &msl::PipelineOptions::default())
            .unwrap_or_else(|e| panic!("module `{name}` does not translate to MSL: {e}"));
        assert!(!text.trim().is_empty(), "module `{name}`: MSL output is empty");
        assert_eq!(tr.entry_point_names.len(), module.entry_points.len());
        for (ep, res) in module.entry_points.iter().zip(&tr.entry_point_names) {
            assert!(
                res.is_ok(),
                "module `{name}` entry point `{}` fails MSL translation: {:?}",
                ep.name,
                res
            );
        }
    }
}

#[test]
fn every_module_translates_to_hlsl() {
    for (name, src) in shader_sources() {
        let (module, info) = parse_and_validate(name, src);
        let options = hlsl::Options {
            shader_model: hlsl::ShaderModel::V6_0,
            fake_missing_bindings: true,
            ..Default::default()
        };
        let pipeline = hlsl::PipelineOptions::default();
        let mut text = String::new();
        let refl = hlsl::Writer::new(&mut text, &options, &pipeline)
            .write(&module, &info, None)
            .unwrap_or_else(|e| panic!("module `{name}` does not translate to HLSL: {e}"));
        assert!(!text.trim().is_empty(), "module `{name}`: HLSL output is empty");
        assert_eq!(refl.entry_point_names.len(), module.entry_points.len());
        for (ep, res) in module.entry_points.iter().zip(&refl.entry_point_names) {
            assert!(
                res.is_ok(),
                "module `{name}` entry point `{}` fails HLSL translation: {:?}",
                ep.name,
                res
            );
        }
    }
}

/// The brief pins naga to the exact version wgpu 30.0.1 uses. The lock file must carry
/// exactly one naga, and it must be 30.0.1 (two versions would mean the test validates
/// with a different compiler than the one that ships).
#[test]
fn naga_is_pinned_to_the_wgpu_version() {
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
        .expect("workspace Cargo.lock");
    let mut versions = Vec::new();
    let mut lines = lock.lines();
    while let Some(l) = lines.next() {
        if l == "name = \"naga\"" {
            versions.push(lines.next().unwrap_or_default().to_owned());
        }
    }
    assert_eq!(versions, vec!["version = \"30.0.1\"".to_owned()], "naga versions in Cargo.lock");
    let root = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
        .expect("workspace Cargo.toml");
    let pin = root
        .lines()
        .find(|l| l.trim_start().starts_with("naga ") || l.trim_start().starts_with("naga="))
        .expect("root Cargo.toml has a `naga` workspace dependency");
    assert!(pin.contains("=30.0.1"), "naga is not pinned exactly: {pin}");
    for feat in ["wgsl-in", "spv-out", "msl-out", "hlsl-out"] {
        assert!(pin.contains(feat), "naga pin lacks the `{feat}` feature: {pin}");
    }
}
