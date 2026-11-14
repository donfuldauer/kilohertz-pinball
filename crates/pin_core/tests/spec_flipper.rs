//! Black-box spec tests for `pin_core::flipper` (PLAN.md §3.3).
//!
//! Every struct literal below names every field of the contract, so a renamed,
//! missing or extra field is a compile error.

use pin_core::flipper::{FlipperConfig, FlipperShape, FlipperStagingMode};
use pin_core::ids::{CoilId, FlipperId, SwitchId};
use pin_core::input::{CabinetAxis, CabinetInput};

fn straight() -> FlipperShape {
    FlipperShape::StraightCapsule {
        length_mm: 76.0,
        base_radius_mm: 11.0,
        tip_radius_mm: 6.5,
    }
}

fn banana() -> FlipperShape {
    FlipperShape::CurvedBanana {
        length_mm: 80.0,
        arc_radius_mm: 120.0,
        arc_sweep_deg: 35.0,
        tube_radius_mm: 7.0,
    }
}

/// Compile-time shape check for the variant that needs a `CabinetAxis`, whose
/// variants are the implementer's choice.
#[allow(dead_code)]
fn dual_stage(axis: CabinetAxis) -> FlipperStagingMode {
    FlipperStagingMode::DualStageThreshold {
        axis,
        lower_threshold: 0.3,
        upper_threshold: 0.8,
    }
}

fn config(shape: FlipperShape, staging: FlipperStagingMode, zipper: bool) -> FlipperConfig {
    FlipperConfig {
        id: FlipperId(3),
        coil: CoilId(1),
        eos_switch: SwitchId(17),
        collision_layer: 0u8,
        base_position_mm: [180.0, 120.0, 0.0],
        surface_normal: [0.0, 0.0, 1.0],
        rest_angle_deg: -30.0,
        sweep_angle_deg: 50.0,
        eos_angle_fraction: 0.92,
        shape,
        inertia_kg_m2: 1.0e-4,
        power_torque_nm: 1.2,
        hold_torque_nm: 0.15,
        return_spring_torque_nm: 0.03,
        stop_restitution: 0.15,
        rubber_restitution: 0.65,
        rubber_damping: 0.35,
        staging,
        zipper_slide_axis_mm: if zipper { Some([12.5, 0.0, 0.0]) } else { None },
        zipper_slide_speed_mm_s: 250.0,
    }
}

fn all_configs() -> Vec<FlipperConfig> {
    vec![
        config(
            straight(),
            FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
            false,
        ),
        config(
            straight(),
            FlipperStagingMode::ParallelLinked(CabinetInput::RightUpper),
            true,
        ),
        config(
            banana(),
            FlipperStagingMode::AutoSensorThingFlip {
                trigger_switch: SwitchId(40),
                delay_ms: 12.0,
                hold_duration_ms: 150.0,
            },
            false,
        ),
        config(
            banana(),
            FlipperStagingMode::DedicatedButton(CabinetInput::Action),
            true,
        ),
    ]
}

fn assert_contract_derives<T>()
where
    T: Clone + std::fmt::Debug + serde::Serialize + serde::de::DeserializeOwned,
{
}

#[test]
fn contract_types_derive_clone_debug_and_serde() {
    assert_contract_derives::<FlipperShape>();
    assert_contract_derives::<FlipperStagingMode>();
    assert_contract_derives::<FlipperConfig>();
}

#[test]
fn field_types_match_the_contract() {
    let c = config(
        straight(),
        FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
        true,
    );
    let _: FlipperId = c.id;
    let _: CoilId = c.coil;
    let _: SwitchId = c.eos_switch;
    let _: u8 = c.collision_layer;
    let _: [f32; 3] = c.base_position_mm;
    let _: [f32; 3] = c.surface_normal;
    let _: f32 = c.rest_angle_deg;
    let _: f32 = c.sweep_angle_deg;
    let _: f32 = c.eos_angle_fraction;
    let _: f32 = c.inertia_kg_m2;
    let _: f32 = c.power_torque_nm;
    let _: f32 = c.hold_torque_nm;
    let _: f32 = c.return_spring_torque_nm;
    let _: f32 = c.stop_restitution;
    let _: f32 = c.rubber_restitution;
    let _: f32 = c.rubber_damping;
    let _: Option<[f32; 3]> = c.zipper_slide_axis_mm;
    let _: f32 = c.zipper_slide_speed_mm_s;
    let _: &FlipperShape = &c.shape;
    let _: &FlipperStagingMode = &c.staging;
}

#[test]
fn shape_has_exactly_two_variants_with_the_contract_fields() {
    fn describe(s: &FlipperShape) -> (f32, f32) {
        match s {
            FlipperShape::StraightCapsule {
                length_mm,
                base_radius_mm,
                tip_radius_mm,
            } => (*length_mm, *base_radius_mm + *tip_radius_mm),
            FlipperShape::CurvedBanana {
                length_mm,
                arc_radius_mm,
                arc_sweep_deg,
                tube_radius_mm,
            } => (
                *length_mm,
                *arc_radius_mm + *arc_sweep_deg + *tube_radius_mm,
            ),
        }
    }
    assert_eq!(describe(&straight()), (76.0, 17.5));
    assert_eq!(describe(&banana()), (80.0, 162.0));
}

#[test]
fn staging_has_exactly_four_variants_with_the_contract_fields() {
    fn tag(s: &FlipperStagingMode) -> u8 {
        match s {
            FlipperStagingMode::DedicatedButton(_input) => 0,
            FlipperStagingMode::ParallelLinked(_input) => 1,
            FlipperStagingMode::DualStageThreshold {
                axis: _,
                lower_threshold,
                upper_threshold,
            } => {
                let _: (&f32, &f32) = (lower_threshold, upper_threshold);
                2
            }
            FlipperStagingMode::AutoSensorThingFlip {
                trigger_switch,
                delay_ms,
                hold_duration_ms,
            } => {
                let _: (&SwitchId, &f32, &f32) = (trigger_switch, delay_ms, hold_duration_ms);
                3
            }
        }
    }
    assert_eq!(
        tag(&FlipperStagingMode::DedicatedButton(CabinetInput::Start)),
        0
    );
    assert_eq!(
        tag(&FlipperStagingMode::ParallelLinked(CabinetInput::LeftUpper)),
        1
    );
    assert_eq!(
        tag(&FlipperStagingMode::AutoSensorThingFlip {
            trigger_switch: SwitchId(1),
            delay_ms: 0.0,
            hold_duration_ms: 1.0,
        }),
        3
    );
}

#[test]
fn clone_preserves_every_field() {
    for c in all_configs() {
        let d = c.clone();
        assert_eq!(format!("{c:?}"), format!("{d:?}"));
    }
}

#[test]
fn debug_output_names_the_contract_fields() {
    let c = config(
        banana(),
        FlipperStagingMode::ParallelLinked(CabinetInput::RightUpper),
        true,
    );
    let text = format!("{c:?}");
    for field in [
        "id",
        "coil",
        "eos_switch",
        "collision_layer",
        "base_position_mm",
        "surface_normal",
        "rest_angle_deg",
        "sweep_angle_deg",
        "eos_angle_fraction",
        "shape",
        "CurvedBanana",
        "arc_radius_mm",
        "arc_sweep_deg",
        "tube_radius_mm",
        "inertia_kg_m2",
        "power_torque_nm",
        "hold_torque_nm",
        "return_spring_torque_nm",
        "stop_restitution",
        "rubber_restitution",
        "rubber_damping",
        "staging",
        "ParallelLinked",
        "zipper_slide_axis_mm",
        "zipper_slide_speed_mm_s",
    ] {
        assert!(text.contains(field), "Debug output lacks {field}: {text}");
    }
}

#[test]
fn serde_round_trip_through_toml() {
    for c in all_configs() {
        let text = toml::to_string(&c).expect("FlipperConfig must serialise to TOML");
        let back: FlipperConfig =
            toml::from_str(&text).expect("serialised FlipperConfig must deserialise");
        assert_eq!(
            format!("{c:?}"),
            format!("{back:?}"),
            "round trip changed the value; TOML was:\n{text}"
        );
    }
}

#[test]
fn shape_and_staging_round_trip_inside_a_document() {
    #[derive(serde::Serialize, serde::Deserialize, Debug)]
    struct Doc {
        shapes: Vec<FlipperShape>,
        staging: Vec<FlipperStagingMode>,
    }
    let doc = Doc {
        shapes: vec![straight(), banana()],
        staging: vec![
            FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
            FlipperStagingMode::ParallelLinked(CabinetInput::RightUpper),
            FlipperStagingMode::AutoSensorThingFlip {
                trigger_switch: SwitchId(40),
                delay_ms: 12.0,
                hold_duration_ms: 150.0,
            },
        ],
    };
    let text = toml::to_string(&doc).expect("serialise");
    let back: Doc = toml::from_str(&text).expect("deserialise");
    assert_eq!(format!("{doc:?}"), format!("{back:?}"));
}
