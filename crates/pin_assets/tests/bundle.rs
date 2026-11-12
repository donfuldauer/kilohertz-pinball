//! The mock bundle loads identically from folder and zip (criterion 1), and
//! interning / SI conversion follow the binding rules.

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{FlipperStageHint, IdKind, Vfs, ZipLimits, load_gltf, load_table};
use pin_core::profiles::BUILTIN_PROFILE_NAMES;
use pin_core::schema::table::SplineKind;
use pin_core::{
    CoilId, DispenserId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId, SoundId,
    SwitchId,
};
use support::*;

#[test]
fn folder_and_zip_load_identically() {
    let files = bundle_files();
    let dir = temp_dir("bundle");
    // Folder bundle wrapped in a single top-level folder; zip at the root.
    write_dir(&dir.join("apex_circuit"), &files);
    let folder = Vfs::open(&dir).unwrap();
    let zip_path = dir.join("apex.pinbundle");
    std::fs::write(&zip_path, zip_of(&files)).unwrap();
    let zipped = Vfs::open(&zip_path).unwrap();

    let names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
    assert_eq!(folder.list(), names);
    assert_eq!(zipped.list(), names);
    for (name, data) in &files {
        assert_eq!(&folder.read(name).unwrap(), data, "{name}");
        assert_eq!(&zipped.read(name).unwrap(), data, "{name}");
    }

    let a = load_table(&folder).unwrap();
    let b = load_table(&zipped).unwrap();
    assert_eq!(a, b);
    assert!(a.warnings.is_empty(), "{:?}", a.warnings);
    assert_eq!(
        load_gltf(&folder, "playfield.glb").unwrap(),
        load_gltf(&zipped, "playfield.glb").unwrap()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn interning_and_si() {
    let vfs = Vfs::from_zip_bytes(zip_of(&bundle_files()), ZipLimits::default()).unwrap();
    let t = load_table(&vfs).unwrap();
    let ids = &t.ids;
    assert_eq!(ids.switch("right_orbit_opto"), Some(SwitchId(24)));
    assert_eq!(ids.switch("lower_left_eos"), Some(SwitchId(25)));
    assert_eq!(ids.switch("start"), Some(SwitchId(6)));
    assert_eq!(ids.switch("tilt"), Some(SwitchId(8)));
    assert_eq!(ids.coil("bumper_1_coil"), Some(CoilId(2)));
    assert_eq!(ids.lamp("overdrive_ready"), Some(LampId(40)));
    assert_eq!(ids.magnet("orbit_magnet"), Some(MagnetId(1)));
    assert_eq!(ids.flipper("lower_left"), Some(FlipperId(0)));
    assert_eq!(ids.dispenser("power_chamber"), Some(DispenserId(1)));
    assert_eq!(ids.profile("ceramic_powerball"), Some(ProfileId(2)));
    assert_eq!(ids.sound("sfx_bumper"), Some(SoundId(0)));
    assert_eq!(ids.scene("scene_powerball_mania"), Some(SceneId(0)));
    assert_eq!(ids.persist_key("games_played"), Some(PersistKey(2)));
    assert_eq!(ids.diverter("bumper_1"), None);
    assert_eq!(ids.entries(IdKind::Profile).len(), BUILTIN_PROFILE_NAMES.len());
    assert_eq!(ids.entries(IdKind::Switch).len(), 5 + 8);
    assert_eq!(ids.name(IdKind::Coil, 1), Some("left_flipper_coil"));

    assert_eq!(t.flippers.len(), 1);
    assert_eq!(t.flippers[0].coil, CoilId(1));
    assert_eq!(t.flippers[0].eos_switch, SwitchId(25));
    assert_eq!(t.flipper_stages, vec![FlipperStageHint::Lower]);
    assert_eq!(t.flippers[0].inertia_kg_m2, t.physics.flipper_inertia);

    let s = &t.switches[0];
    assert_eq!(s.position_m, Some([0.41, 0.72, 0.0]));
    assert_eq!(s.radius_m, Some(0.012));
    let m = &t.magnets[0];
    assert_eq!(m.d_min_m, 0.008);
    assert_eq!(m.range_m, 0.06);
    assert!((m.tau_s - 0.06).abs() < 1e-7);
    let w = &t.splines[0];
    assert_eq!(w.kind, SplineKind::Wireform);
    assert_eq!(w.gauge_m, Some(0.019));
    assert_eq!(w.points_m[0], [0.06, 0.9, 0.04]);
    assert!((w.roll_rad[1] - 8f32.to_radians()).abs() < 1e-6);
    assert!(!w.closed);
    assert_eq!(t.transitions[0].zone_max_m, [0.29, 0.68, 0.03]);
    assert_eq!(
        t.dispensers[0].contents,
        vec![ProfileId(0), ProfileId(0), ProfileId(0), ProfileId(0), ProfileId(2)]
    );
    assert_eq!(t.dispensers[0].exit_velocity_m_s, [0.0, -0.6, 0.0]);
    assert_eq!(t.mechs[0].coil, Some(CoilId(2)));
    assert_eq!(t.mechs[0].switch, Some(SwitchId(26)));
    assert_eq!(t.shots[0].sequence, vec![SwitchId(27), SwitchId(28)]);
    assert_eq!(t.shots[0].window_s, 2.5);
    assert_eq!(t.modes[0].start_scene, SceneId(0));
    assert!((t.pitch_rad - 6.5f32.to_radians()).abs() < 1e-6);
    let bb = t.backbox.as_ref().unwrap();
    assert_eq!(bb.camera_position_m, [0.0, -0.6, 0.3]);
    assert_eq!(bb.collision_layer, 4);
    assert!((bb.fov_rad - 35f32.to_radians()).abs() < 1e-6);
}
