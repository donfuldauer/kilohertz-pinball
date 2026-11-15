use super::*;
use crate::pbr::math::{IDENTITY, scale_translation};
use crate::pbr::{MaterialKind, MaterialParams, MeshGeometry, NO_PROBE};
use crate::reflections::{MAX_PROBES, ReflectiveBall};
use proptest::prelude::*;

// ---------------------------------------------------------------------
// ReflectionQuality
// ---------------------------------------------------------------------

#[test]
fn quality_defaults_to_off() {
    assert_eq!(ReflectionQuality::default(), ReflectionQuality::Off);
    assert!(!ReflectionQuality::Off.traces_balls());
    assert!(ReflectionQuality::BallsOnly.traces_balls());
    assert!(!ReflectionQuality::BallsOnly.traces_clear_coat());
    assert!(ReflectionQuality::BallsAndClearCoat.traces_clear_coat());
}

#[test]
fn quality_text_roundtrip() {
    let names = ["off", "balls", "balls_and_clearcoat"];
    for (q, n) in ReflectionQuality::ALL.into_iter().zip(names) {
        assert_eq!(q.to_string(), n);
        assert_eq!(n.parse::<ReflectionQuality>(), Ok(q));
        // Exact match, like the serde form: no trimming, no case folding.
        for bad in [
            format!("  {n} "),
            format!("{n} "),
            format!(" {n}"),
            n.to_uppercase(),
            format!("  {} ", n.to_uppercase()),
        ] {
            assert_eq!(
                bad.parse::<ReflectionQuality>(),
                Err(ParseReflectionQualityError(bad.clone()))
            );
        }
    }
    let e = "ball".parse::<ReflectionQuality>().unwrap_err();
    assert_eq!(e, ParseReflectionQualityError("ball".into()));
    assert!(e.to_string().contains("balls_and_clearcoat"));
    assert!("".parse::<ReflectionQuality>().is_err());
}

#[test]
fn quality_serde_roundtrip() {
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct S {
        q: ReflectionQuality,
    }
    for q in ReflectionQuality::ALL {
        let text = toml::to_string(&S { q }).unwrap();
        assert_eq!(text.trim(), format!("q = \"{q}\""));
        assert_eq!(toml::from_str::<S>(&text).unwrap(), S { q });
    }
    assert!(toml::from_str::<S>("q = \"BallsOnly\"").is_err());
}

proptest! {
    #[test]
    fn quality_parse_is_total_and_canonical(s in ".{0,24}") {
        match s.parse::<ReflectionQuality>() {
            Ok(q) => prop_assert!(q.as_str() == s),
            Err(e) => prop_assert_eq!(e.0, s),
        }
    }

    #[test]
    fn quality_display_parse_roundtrip(i in 0usize..3) {
        let q = ReflectionQuality::ALL[i];
        prop_assert_eq!(q.to_string().parse::<ReflectionQuality>(), Ok(q));
    }

    #[test]
    fn cutoff_choice_is_monotone(a in -1.0f32..2.0, b in -1.0f32..2.0, cutoff in 0.0f32..1.0) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if reflection_choice(lo, cutoff) == ReflectionChoice::Probe {
            prop_assert_eq!(reflection_choice(hi, cutoff), ReflectionChoice::Probe);
        }
        if reflection_choice(hi, cutoff) == ReflectionChoice::Ray {
            prop_assert_eq!(reflection_choice(lo, cutoff), ReflectionChoice::Ray);
        }
        let want = if a > cutoff { ReflectionChoice::Probe } else { ReflectionChoice::Ray };
        prop_assert_eq!(reflection_choice(a, cutoff), want);
    }
}

#[test]
fn cutoff_edges() {
    let c = DEFAULT_RAY_ROUGHNESS_CUTOFF;
    assert_eq!(c, 0.3);
    assert_eq!(RayQueryConfig::default().ray_roughness_cutoff, 0.3);
    assert_eq!(reflection_choice(c, c), ReflectionChoice::Ray);
    assert_eq!(
        reflection_choice(f32::from_bits(c.to_bits() + 1), c),
        ReflectionChoice::Probe
    );
    assert_eq!(reflection_choice(0.0, c), ReflectionChoice::Ray);
    assert_eq!(reflection_choice(f32::NAN, c), ReflectionChoice::Probe);
    assert_eq!(reflection_choice(0.1, f32::NAN), ReflectionChoice::Probe);
    assert_eq!(reflection_choice(f32::INFINITY, c), ReflectionChoice::Probe);
}

// ---------------------------------------------------------------------
// Path selection
// ---------------------------------------------------------------------

#[test]
fn path_selection_with_fake_capability() {
    use ReflectionQuality::*;
    let caps = |ray_query, device_lost| RayCaps {
        ray_query,
        device_lost,
    };
    for rq in [false, true] {
        for lost in [false, true] {
            assert_eq!(
                choose_reflection_path(Off, &caps(rq, lost)),
                ReflectionPath::Probes(FallbackReason::QualityOff)
            );
        }
    }
    for q in [BallsOnly, BallsAndClearCoat] {
        assert_eq!(
            choose_reflection_path(q, &caps(false, false)),
            ReflectionPath::Probes(FallbackReason::FeatureAbsent)
        );
        assert_eq!(
            choose_reflection_path(q, &caps(true, true)),
            ReflectionPath::Probes(FallbackReason::DeviceLost)
        );
        assert_eq!(
            choose_reflection_path(q, &caps(false, true)),
            ReflectionPath::Probes(FallbackReason::DeviceLost)
        );
        assert_eq!(
            choose_reflection_path(q, &caps(true, false)),
            ReflectionPath::RayQuery
        );
    }
    assert!(!RayCaps::default().supports_ray_query());
    assert!(!RayCaps::default().device_lost());
}

#[test]
fn fallback_reasons_read_well() {
    for r in [
        FallbackReason::QualityOff,
        FallbackReason::FeatureAbsent,
        FallbackReason::DeviceLost,
        FallbackReason::RuntimeError,
    ] {
        assert!(!r.to_string().is_empty());
    }
    assert!(
        FallbackReason::FeatureAbsent
            .to_string()
            .contains("EXPERIMENTAL_RAY_QUERY")
    );
    let e = RayQueryError::FeatureAbsent {
        adapter: "x".into(),
    };
    assert_eq!(e.reason(), FallbackReason::FeatureAbsent);
    assert!(e.to_string().contains("EXPERIMENTAL_RAY_QUERY"));
    assert_eq!(
        RayQueryError::QualityOff.reason(),
        FallbackReason::QualityOff
    );
    let m = RayQueryError::InvalidMesh("bad".into());
    assert_eq!(m.reason(), FallbackReason::RuntimeError);
    assert!(m.to_string().contains("bad"));
    let g = RayQueryError::Gpu(crate::gpu::GpuError::InvalidArgument("x".into()));
    assert_eq!(g.reason(), FallbackReason::RuntimeError);
    assert!(g.to_string().contains("setup"));
}

#[test]
fn fallback_is_logged_once() {
    let printed = (0..3).filter(|_| select::log_fallback_once("test")).count();
    assert!(printed <= 1);
    assert!(!select::log_fallback_once("again"));
}

// ---------------------------------------------------------------------
// Layer planning
// ---------------------------------------------------------------------

fn ball_draw(kind: MaterialKind, roughness: f32) -> PlanDraw {
    PlanDraw {
        material: MaterialParams {
            roughness,
            ..kind.params()
        },
        model: IDENTITY,
        transparent: false,
        mesh_top: Some(0.0135),
    }
}

fn playfield(cc_roughness: f32, model: crate::pbr::math::Mat4) -> PlanDraw {
    PlanDraw {
        material: MaterialParams {
            clearcoat_roughness: cc_roughness,
            ..MaterialKind::PlayfieldWood.params()
        },
        model,
        transparent: false,
        mesh_top: Some(0.0),
    }
}

fn ball(draw: usize, x: f32) -> ReflectiveBall {
    ReflectiveBall {
        position: [x, 0.0, 0.0135],
        radius: 0.0135,
        draw,
    }
}

#[test]
fn plan_off_traces_nothing() {
    let draws = [ball_draw(MaterialKind::ChromeSteel, 0.06)];
    let mut plan = LayerPlan::default();
    plan_layers(
        ReflectionQuality::Off,
        0.3,
        &draws,
        &[ball(0, 0.0)],
        &mut plan,
    );
    assert!(!plan.traced);
    assert!(plan.jobs.is_empty());
    assert_eq!(plan.draw_layers, [NO_PROBE]);
}

#[test]
fn plan_balls_only() {
    let draws = [
        ball_draw(MaterialKind::ChromeSteel, 0.06),
        ball_draw(MaterialKind::ChromeSteel, 0.5),
        playfield(0.06, IDENTITY),
    ];
    let balls = [ball(1, 0.03), ball(0, -0.03)];
    let mut plan = LayerPlan::default();
    plan_layers(ReflectionQuality::BallsOnly, 0.3, &draws, &balls, &mut plan);
    assert!(plan.traced);
    assert_eq!(plan.draw_layers, [1, 0, NO_PROBE]);
    assert_eq!(plan.jobs.len(), 2);
    assert_eq!(plan.jobs[0].layer, 0);
    assert!(matches!(
        plan.jobs[0].kind,
        RayJobKind::BallCentre { radius, .. } if radius == 0.0135
    ));
    assert_eq!(plan.jobs[1].layer, 1);
    assert!(matches!(plan.jobs[1].kind, RayJobKind::BallView { .. }));
    assert!(plan.jobs.iter().all(|j| !j.half_res));
}

#[test]
fn plan_all_rough_uses_probes() {
    let draws = [
        ball_draw(MaterialKind::ChromeSteel, 0.31),
        ball_draw(MaterialKind::ChromeSteel, 0.9),
        playfield(0.5, IDENTITY),
    ];
    let mut plan = LayerPlan::default();
    for q in [
        ReflectionQuality::BallsOnly,
        ReflectionQuality::BallsAndClearCoat,
    ] {
        plan_layers(q, 0.3, &draws, &[ball(0, 0.0), ball(1, 0.1)], &mut plan);
        assert!(!plan.traced, "{q}");
        assert!(
            plan.jobs
                .iter()
                .all(|j| matches!(j.kind, RayJobKind::BallCentre { .. }))
        );
    }
}

#[test]
fn plan_clear_coat() {
    let lifted = scale_translation(1.0, [0.0, 0.0, 0.0005]);
    let draws = [
        ball_draw(MaterialKind::ChromeSteel, 0.06),
        playfield(0.06, IDENTITY),
        // Same plane, 0.5 mm up: shares the map.
        playfield(0.06, lifted),
        // 5 cm up: a different plane.
        playfield(0.06, scale_translation(1.0, [0.0, 0.0, 0.05])),
        // Too rough, transparent, unregistered: environment.
        playfield(0.6, IDENTITY),
        PlanDraw {
            transparent: true,
            ..playfield(0.06, IDENTITY)
        },
        PlanDraw {
            mesh_top: None,
            ..playfield(0.06, IDENTITY)
        },
        // No clear coat.
        PlanDraw {
            material: MaterialParams::DEFAULT,
            ..playfield(0.06, IDENTITY)
        },
    ];
    let mut plan = LayerPlan::default();
    plan_layers(
        ReflectionQuality::BallsAndClearCoat,
        0.3,
        &draws,
        &[ball(0, 0.0)],
        &mut plan,
    );
    let c = CLEARCOAT_LAYER;
    let n = NO_PROBE;
    assert_eq!(plan.draw_layers, [0, c, c, n, n, n, n, n]);
    let last = plan.jobs.last().unwrap();
    assert_eq!(last.layer, CLEARCOAT_LAYER);
    assert!(last.half_res);
    assert_eq!(
        last.kind,
        RayJobKind::Plane {
            point: [0.0; 3],
            normal: [0.0, 0.0, 1.0]
        }
    );
    // Balls only: the playfield keeps the environment.
    plan_layers(
        ReflectionQuality::BallsOnly,
        0.3,
        &draws,
        &[ball(0, 0.0)],
        &mut plan,
    );
    assert_eq!(plan.draw_layers[1], NO_PROBE);
    // A rough ball and a smooth clear coat still trace.
    let draws2 = [ball_draw(MaterialKind::ChromeSteel, 0.8), draws[1]];
    plan_layers(
        ReflectionQuality::BallsAndClearCoat,
        0.3,
        &draws2,
        &[ball(0, 0.0)],
        &mut plan,
    );
    assert!(plan.traced);
    assert_eq!(plan.draw_layers, [0, CLEARCOAT_LAYER]);
}

#[test]
fn plan_skips_bad_balls_and_caps_layers() {
    let draws: Vec<PlanDraw> = (0..MAX_PROBES + 3)
        .map(|_| ball_draw(MaterialKind::ChromeSteel, 0.06))
        .collect();
    let mut balls: Vec<ReflectiveBall> = (0..draws.len()).map(|i| ball(i, i as f32)).collect();
    balls[1].draw = 999;
    balls[2].position[0] = f32::NAN;
    balls[3].radius = f32::INFINITY;
    let mut plan = LayerPlan::default();
    plan_layers(ReflectionQuality::BallsOnly, 0.3, &draws, &balls, &mut plan);
    assert_eq!(plan.jobs.len(), MAX_PROBES - 2);
    assert_eq!(plan.draw_layers[1], NO_PROBE);
    assert_eq!(plan.draw_layers[2], NO_PROBE);
    assert_eq!(plan.draw_layers[3], 3);
    assert!(matches!(
        plan.jobs[1].kind,
        RayJobKind::BallView { radius, .. } if radius == 0.0
    ));
    assert!(
        plan.draw_layers[MAX_PROBES..]
            .iter()
            .all(|&l| l == NO_PROBE)
    );
    assert!(plan.jobs.iter().all(|j| j.layer < MAX_PROBES as u32));
}

proptest! {
    /// Every planned layer is valid and unique, every draw maps to a layer
    /// that is traced or to the environment, and `traced` matches.
    #[test]
    fn plan_invariants(
        rough in prop::collection::vec(0.0f32..1.0, 0..20),
        cc in prop::collection::vec(0.0f32..1.0, 0..4),
        ball_draws in prop::collection::vec(0usize..30, 0..20),
        cutoff in 0.0f32..1.0,
        q in 0usize..3,
    ) {
        let mut draws: Vec<PlanDraw> = rough
            .iter()
            .map(|&r| ball_draw(MaterialKind::ChromeSteel, r))
            .collect();
        draws.extend(cc.iter().map(|&r| playfield(r, IDENTITY)));
        let balls: Vec<ReflectiveBall> = ball_draws
            .iter()
            .enumerate()
            .map(|(i, &d)| ball(d, i as f32 * 0.1))
            .collect();
        let quality = ReflectionQuality::ALL[q];
        let mut plan = LayerPlan::default();
        plan_layers(quality, cutoff, &draws, &balls, &mut plan);
        prop_assert_eq!(plan.draw_layers.len(), draws.len());
        let mut layers: Vec<u32> = plan.jobs.iter().map(|j| j.layer).collect();
        layers.dedup();
        prop_assert_eq!(layers.len(), plan.jobs.len());
        prop_assert!(plan.jobs.len() <= RAY_ATLAS_LAYERS as usize);
        for &l in &plan.draw_layers {
            prop_assert!(l == NO_PROBE || plan.jobs.iter().any(|j| j.layer == l));
        }
        let any_ray = plan.jobs.iter().any(|j| !matches!(j.kind, RayJobKind::BallCentre { .. }));
        prop_assert_eq!(plan.traced, any_ray);
        if quality == ReflectionQuality::Off {
            prop_assert!(plan.jobs.is_empty());
        }
    }
}

// ---------------------------------------------------------------------
// Geometry and instances
// ---------------------------------------------------------------------

#[test]
fn tlas_transform_is_row_major() {
    let m = scale_translation(2.0, [1.0, 2.0, 3.0]);
    assert_eq!(
        tlas_transform(&m),
        [2.0, 0.0, 0.0, 1.0, 0.0, 2.0, 0.0, 2.0, 0.0, 0.0, 2.0, 3.0]
    );
    let mut r = IDENTITY;
    r[0][1] = 5.0; // column 0, row 1
    assert_eq!(tlas_transform(&r)[4], 5.0);
}

#[test]
fn clear_coat_plane_follows_the_model() {
    let (p, n) = clearcoat_plane(&scale_translation(2.0, [0.0, 0.0, 1.0]), 0.5).unwrap();
    assert_eq!(p, [0.0, 0.0, 2.0]);
    assert_eq!(n, [0.0, 0.0, 1.0]);
    // A model flipping z flips the normal.
    let mut flip = IDENTITY;
    flip[2][2] = -1.0;
    let (_, n) = clearcoat_plane(&flip, 0.0).unwrap();
    assert_eq!(n, [0.0, 0.0, -1.0]);
    let mut nan = IDENTITY;
    nan[3][0] = f32::NAN;
    assert!(clearcoat_plane(&nan, 0.0).is_none());
    let mut inf = IDENTITY;
    inf[2][2] = f32::INFINITY;
    assert!(clearcoat_plane(&inf, 0.0).is_none());
}

#[test]
fn geometry_pool_appends_meshes() {
    let mut pool = GeometryPool::default();
    let quad = MeshGeometry::quad(1.0);
    let a = pool.add(&quad).unwrap();
    assert_eq!(
        a,
        PoolRange {
            first_vertex: 0,
            first_index: 0,
            triangles: 2
        }
    );
    let mut no_normals = MeshGeometry::uv_sphere(1.0, 6, 4);
    no_normals.normals.truncate(1);
    no_normals.normals[0] = [f32::NAN; 3];
    let b = pool.add(&no_normals).unwrap();
    assert_eq!(b.first_vertex, 4);
    assert_eq!(b.first_index, 6);
    assert_eq!(pool.vertices.len(), 2 * (4 + no_normals.positions.len()));
    assert_eq!(pool.vertices[1], [0.0, 0.0, 1.0, 0.0]);
    assert_eq!(pool.vertices[9], [0.0; 4]);
    assert_eq!(pool.vertices[8][3], 1.0);
    assert_eq!(mesh_top(&quad), Some(0.0));
    assert_eq!(mesh_top(&MeshGeometry::default()), None);
}

#[test]
fn invalid_meshes_are_rejected() {
    let mut m = MeshGeometry::quad(1.0);
    m.indices.push(0);
    assert!(validate_mesh(&m).is_err());
    let mut m = MeshGeometry::quad(1.0);
    m.indices[0] = 4;
    assert!(validate_mesh(&m).unwrap_err().contains("out of range"));
    let mut m = MeshGeometry::quad(1.0);
    m.positions[0][1] = f32::INFINITY;
    assert!(validate_mesh(&m).is_err());
    assert!(validate_mesh(&MeshGeometry::default()).is_err());
    let mut pool = GeometryPool::default();
    assert!(pool.add(&MeshGeometry::default()).is_err());
    assert!(pool.vertices.is_empty());
}

proptest! {
    /// Arbitrary meshes never panic the validator or the pool, and an
    /// accepted mesh's indices stay in range.
    #[test]
    fn pool_survives_random_meshes(
        pos in prop::collection::vec(prop::array::uniform3(prop::num::f32::ANY), 0..12),
        idx in prop::collection::vec(0u32..16, 0..24),
        normals in 0usize..12,
    ) {
        let mesh = MeshGeometry {
            normals: vec![[0.0, 0.0, 1.0]; normals],
            positions: pos,
            indices: idx,
        };
        let mut pool = GeometryPool::default();
        if let Ok(r) = pool.add(&mesh) {
            prop_assert_eq!(r.triangles as usize * 3, mesh.indices.len());
            prop_assert!(pool.indices.iter().all(|&i| (i as usize) < mesh.positions.len()));
            prop_assert_eq!(pool.vertices.len(), mesh.positions.len() * 2);
        } else {
            prop_assert!(pool.vertices.is_empty());
        }
    }
}

#[test]
fn instance_records_are_sanitised() {
    let range = PoolRange {
        first_vertex: 7,
        first_index: 9,
        triangles: 3,
    };
    let chrome = instance_record(&MaterialKind::ChromeSteel.params(), 0.0, range);
    assert_eq!(chrome.emissive_metallic[3], 1.0);
    assert_eq!(chrome.surface[0], 0.06);
    assert_eq!(chrome.geometry, [9, 7, 3, 0]);
    let wood = instance_record(&MaterialKind::PlayfieldWood.params(), 0.0, range);
    assert_eq!(wood.coat, [1.0, 0.06, 0.0, 0.0]);
    let glass = instance_record(&MaterialKind::ClearAcrylic.params(), 0.0, range);
    assert_eq!(glass.coat[2], 0.98);
    let insert = MaterialKind::LightInsert.params();
    assert_eq!(instance_record(&insert, 0.5, range).surface[3], 2.0);
    assert_eq!(instance_record(&insert, 7.0, range).surface[3], 4.0);
    assert_eq!(instance_record(&insert, f32::NAN, range).surface[3], 0.0);
    let bad = MaterialParams {
        base_color: [f32::NAN, -1.0, f32::INFINITY, 1.0],
        emissive: [f32::NAN, 2.0, -3.0],
        metallic: 9.0,
        roughness: f32::NAN,
        clearcoat: 3.0,
        subsurface: f32::NEG_INFINITY,
        ..MaterialParams::DEFAULT
    };
    let r = instance_record(&bad, 1.0, range);
    assert_eq!(r.base_color, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(r.emissive_metallic, [0.0, 2.0, 0.0, 1.0]);
    assert_eq!(r.surface[0], 0.0);
    assert_eq!(r.coat, [1.0, 0.1, 0.0, 0.0]);
}

// ---------------------------------------------------------------------
// WGSL layout
// ---------------------------------------------------------------------

fn trace_module() -> naga::Module {
    naga::front::wgsl::parse_str(TRACE_SOURCE).expect("trace WGSL parses")
}

fn struct_size(module: &naga::Module, name: &str) -> u32 {
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).unwrap();
    let (h, _) = module
        .types
        .iter()
        .find(|(_, t)| t.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no WGSL struct {name}"));
    layouter[h].size
}

#[test]
fn wgsl_layouts_match_rust() {
    let m = trace_module();
    assert_eq!(
        struct_size(&m, "TraceParams") as usize,
        source::TRACE_PARAMS_SIZE
    );
    assert_eq!(
        struct_size(&m, "TraceInstance") as usize,
        core::mem::size_of::<InstanceRecord>()
    );
    assert!(TRACE_SOURCE.starts_with("enable wgpu_ray_query;"));
    assert!(TRACE_SOURCE.contains(&format!("TRACE_MAX_JOBS: u32 = {RAY_ATLAS_LAYERS}u")));
    assert!(TRACE_SOURCE.contains(&format!(
        "TRACE_MAX_LIGHTS: u32 = {}u",
        crate::pbr::MAX_LIGHTS
    )));
    assert!(TRACE_SOURCE.contains(&format!("array<TraceJob, {RAY_ATLAS_LAYERS}>")));
    assert!(TRACE_SOURCE.contains(&format!("array<TraceLight, {}>", crate::pbr::MAX_LIGHTS)));
    assert_eq!(shader_sources()[0].1, TRACE_SOURCE);
}

#[test]
fn survey_constants() {
    assert_eq!(WGPU_VERSION, "30.0.1");
    assert_eq!(RAY_QUERY_FEATURE, wgpu::Features::EXPERIMENTAL_RAY_QUERY);
    let c = RayQueryConfig::default();
    assert_eq!(c.probes, crate::reflections::ProbeConfig::default());
    assert!(c.max_distance > 1.0 && c.origin_offset > 0.0 && c.plane_offset > 0.0);
}

// ---------------------------------------------------------------------
// Trace uniform: tune
// ---------------------------------------------------------------------

#[test]
fn trace_tune_carries_the_roughness_cutoff_in_w() {
    let d = RayQueryConfig::default();
    let t = source::trace_tune(&d);
    assert_eq!(t, [d.max_distance, d.origin_offset, d.plane_offset, 0.3]);
    for c in [0.0, 0.2, 0.5, 1.0] {
        let cfg = RayQueryConfig {
            ray_roughness_cutoff: c,
            ..d
        };
        assert_eq!(source::trace_tune(&cfg)[3].to_bits(), c.to_bits());
    }
    // NaN passes through (the shader's `<=` is then false: never traces).
    let nan = RayQueryConfig {
        ray_roughness_cutoff: f32::NAN,
        ..d
    };
    assert!(source::trace_tune(&nan)[3].is_nan());
    // The shader reads the cutoff from tune.w.
    assert!(TRACE_SOURCE.contains("let cutoff = params.tune.w;"));
}

proptest! {
    #[test]
    fn trace_tune_w_roundtrips_any_cutoff(c in -1.0f32..2.0) {
        let cfg = RayQueryConfig { ray_roughness_cutoff: c, ..RayQueryConfig::default() };
        prop_assert_eq!(source::trace_tune(&cfg)[3].to_bits(), c.to_bits());
    }
}
