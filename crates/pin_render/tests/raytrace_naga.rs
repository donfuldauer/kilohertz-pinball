//! 5.4 shader validation (GPU-free): naga 30.0.1 parses and validates every
//! ray-query WGSL module with the ray-query capability and translates it to
//! SPIR-V (the only back end wgpu uses for ray queries). MSL and HLSL are
//! tried and their outcome printed, not asserted (wgpu exposes
//! `EXPERIMENTAL_RAY_QUERY` on Vulkan only; see the `raytrace` docs).

use naga::back::{hlsl, msl, spv};
use naga::valid::{Capabilities, ValidationFlags, Validator};
use pin_render::raytrace::shader_sources;

fn parse(name: &str, src: &str) -> naga::Module {
    naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|e| panic!("{name}: WGSL parse error:\n{}", e.emit_to_string(src)))
}

fn validated(name: &str, src: &str) -> (naga::Module, naga::valid::ModuleInfo) {
    let module = parse(name, src);
    let info = Validator::new(ValidationFlags::all(), Capabilities::RAY_QUERY)
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: validation error:\n{}", e.emit_to_string(src)));
    (module, info)
}

#[test]
fn raytrace_every_module_is_listed_once_and_non_empty() {
    let sources = shader_sources();
    assert!(!sources.is_empty());
    let mut names: Vec<_> = sources.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), sources.len(), "duplicate module names");
    for (name, src) in sources {
        assert!(!src.trim().is_empty(), "{name} is empty");
        assert!(src.starts_with("enable wgpu_ray_query;"), "{name}");
    }
}

#[test]
fn raytrace_every_module_validates_with_the_ray_query_capability() {
    for (name, src) in shader_sources() {
        let (module, _) = validated(name, src);
        let ep: Vec<_> = module
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(ep, ["cs_trace"], "{name}");
        assert_eq!(module.entry_points[0].stage, naga::ShaderStage::Compute);
        assert_eq!(module.entry_points[0].workgroup_size, [8, 8, 1]);
    }
}

#[test]
fn raytrace_ray_queries_need_the_capability() {
    for (name, src) in shader_sources() {
        let module = parse(name, src);
        let err = Validator::new(ValidationFlags::all(), Capabilities::empty())
            .validate(&module)
            .expect_err("ray queries must not validate without Capabilities::RAY_QUERY");
        println!("[{name}] without RAY_QUERY: {}", err.as_inner());
    }
}

#[test]
fn raytrace_every_module_translates_to_spirv() {
    for (name, src) in shader_sources() {
        let (module, info) = validated(name, src);
        let words = spv::write_vec(&module, &info, &spv::Options::default(), None)
            .unwrap_or_else(|e| panic!("{name}: SPIR-V: {e}"));
        assert_eq!(words.first(), Some(&0x0723_0203), "{name}: SPIR-V magic");
        // OpCapability RayQueryKHR (4472) is declared.
        assert!(
            words.windows(2).any(|w| w == [0x0002_0011, 4472]),
            "{name}: SPIR-V lacks the RayQueryKHR capability"
        );
    }
}

/// Recorded, not asserted: naga's MSL / HLSL support of ray queries is not
/// part of the 5.4 contract (Vulkan only).
#[test]
fn raytrace_msl_and_hlsl_outcome_is_recorded() {
    for (name, src) in shader_sources() {
        let (module, info) = validated(name, src);
        let msl_with = |options: &msl::Options| {
            msl::write_string(&module, &info, options, &msl::PipelineOptions::default())
                .map(|_| ())
                .map_err(|e| e.to_string())
        };
        let msl = msl_with(&msl::Options::default());
        let msl_2_4 = msl_with(&msl::Options {
            lang_version: (2, 4),
            ..Default::default()
        });
        let mut text = String::new();
        let hlsl = hlsl::Writer::new(
            &mut text,
            &hlsl::Options::default(),
            &hlsl::PipelineOptions::default(),
        )
        .write(&module, &info, None)
        .map(|_| ())
        .map_err(|e| e.to_string());
        println!(
            "[{name}] naga 30.0.1 MSL (default): {msl:?}; MSL 2.4: {msl_2_4:?}; HLSL: {hlsl:?}"
        );
    }
}

proptest::proptest! {
    /// The WGSL front end never panics on bounded random edits of the trace
    /// module (truncation at a random byte).
    #[test]
    fn raytrace_truncated_sources_do_not_panic(cut in 0usize..12_000) {
        for (_, src) in shader_sources() {
            let mut end = cut.min(src.len());
            while !src.is_char_boundary(end) {
                end -= 1;
            }
            let _ = naga::front::wgsl::parse_str(&src[..end]);
        }
    }
}
