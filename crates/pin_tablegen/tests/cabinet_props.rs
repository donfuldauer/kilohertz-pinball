//! KP-029 (#110) property tests (R3, R11, REQ-CAB-001, REQ-CAB-003):
//! random valid geometries give deterministic, loadable, finite models with
//! the stable node list and legs on the floor; random `cabinet.toml` bytes
//! and random field values never make the generator panic.

#[path = "cabinet_common.rs"]
mod common;

use common::*;
use pin_core::schema::cabinet::{CabinetGeometry, CabinetManifest};
use pin_tablegen::cabinet::{NODE_NAMES, generate_cabinet};
use proptest::prelude::*;

/// A preset with its body and legs overridden within sane ranges, the
/// controls moved with the body width.
fn geometry() -> impl Strategy<Value = CabinetGeometry> {
    (
        0usize..3,
        450.0f32..700.0,
        1000.0f32..1500.0,
        prop::array::uniform4(600.0f32..800.0),
        prop::array::uniform4(0.0f32..=1.0),
        350.0f32..450.0,
        550.0f32..700.0,
    )
        .prop_map(|(p, width, length, lens, ext, front, rear)| {
            let mut g = preset(PRESETS[p]);
            let dw = (width - g.body.width) * 0.5;
            g.body.width = width;
            g.body.length = length;
            g.body.front_height = front;
            g.body.rear_height = rear;
            g.glass_panel.width = pin_core::fmath::min_f32(g.glass_panel.width, width - 30.0);
            g.controls.left_flipper_button[0] -= dw;
            g.controls.right_flipper_button[0] += dw;
            let legs = [
                &mut g.legs.front_left,
                &mut g.legs.front_right,
                &mut g.legs.rear_left,
                &mut g.legs.rear_right,
            ];
            for (i, leg) in legs.into_iter().enumerate() {
                leg.length = lens[i];
                let lv = &mut leg.leveler;
                lv.extension = lv.extension_min + (lv.extension_max - lv.extension_min) * ext[i];
            }
            g
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// Acceptance + R3: deterministic, loads, finite floats, UVs in
    /// [0, 1], the stable node list, every leg assembly on z = 0 and the
    /// file's lowest point on the floor; the shaft z extent is the leg
    /// length.
    #[test]
    fn random_geometries_are_well_formed(g in geometry()) {
        let a = generate_cabinet(&g).unwrap();
        let b = generate_cabinet(&g).unwrap();
        prop_assert_eq!(&a.glb, &b.glb);
        prop_assert_eq!(a.nodes.clone(), NODE_NAMES.map(String::from).to_vec());
        let scene = load(&a.glb);
        let names: Vec<&str> = scene.meshes.iter().map(|m| m.name.as_str()).collect();
        prop_assert_eq!(names, NODE_NAMES.to_vec());
        for m in &scene.meshes {
            prop_assert!(m.positions.iter().flatten().all(|v| v.is_finite()));
            prop_assert!(m.normals.iter().flatten().all(|v| v.is_finite()));
            prop_assert!(m.uvs.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
        }
        for (name, leg) in g.legs.all() {
            let [shaft, bracket, leveler] = leg_nodes(name);
            let (lo, _) = bounds(&scene, &[&shaft, &bracket, &leveler]);
            prop_assert!(lo[2].abs() < 1e-4, "{} min z {}", name, lo[2]);
            let (slo, shi) = bounds(&scene, &[&shaft]);
            prop_assert!((f64::from(shi[2] - slo[2]) - f64::from(leg.length) / 1000.0).abs() < 1e-3);
        }
        let (lo, _) = scene_bounds(&scene);
        prop_assert!(lo[2].abs() < 1e-4, "global min z {}", lo[2]);
    }

    /// R11: random bytes as a `cabinet.toml` never panic the parser, and
    /// anything that resolves also generates without panicking.
    #[test]
    fn random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let text = String::from_utf8_lossy(&bytes);
        if let Ok(m) = CabinetManifest::from_toml_str(&text)
            && let Ok(g) = m.resolve()
        {
            let _ = generate_cabinet(&g);
        }
    }

    /// R11: structurally valid files with random (possibly extreme)
    /// values: whatever resolves generates (or names an error) without
    /// panicking.
    #[test]
    fn random_values_never_panic(
        width in prop_oneof![Just(-1.0f64), Just(0.0), 1.0f64..1e6, Just(1e30)],
        length in prop_oneof![Just(1.0f64), 100.0f64..1e6],
        leg in prop_oneof![Just(0.5f64), 300.0f64..2e5],
        ext in 0.0f64..44.0,
        inset in 0.0f64..800.0,
        plunger in prop_oneof![Just(-1e7f64), -1e3f64..1e3],
    ) {
        let text = format!(
            "schema_version = 1\npreset = \"classic_standard\"\n\
             [body]\nwidth = {width:?}\nlength = {length:?}\n\
             [legs.default]\nlength = {leg:?}\ncorner_inset = [{inset:?}, {inset:?}]\n\
             [legs.default.leveler]\nextension = {ext:?}\n\
             [controls]\nplunger = [{plunger:?}, 0.0, 200.0]\n"
        );
        if let Ok(m) = CabinetManifest::from_toml_str(&text)
            && let Ok(g) = m.resolve()
        {
            match generate_cabinet(&g) {
                Ok(out) => prop_assert_eq!(out.nodes.len(), 28),
                Err(e) => prop_assert!(!e.element.is_empty()),
            }
        }
    }
}
