//! PR #21 review #1 (r4576949217): the table is validated and simulated
//! from one immutable bundle snapshot. A bundle replaced on disk after
//! validation must not reach the simulation.

mod support;

use pin_app::orchestrator::{load_checked, load_validated, validate_table};
use pin_sim::load::BundleSnapshot;
use pin_sim::{SimConfig, Simulation};
use support::{build_greybox_bundle, greybox_src, on_big_stack, scratch};

/// Overwrites the grey-box bundle at `dir` with a variant whose orbit lane
/// is narrower than a ball (fails the 2.8 clearance check) and whose rules
/// differ.
fn replace_with_failing_bundle(dir: &std::path::Path) {
    let src = greybox_src();
    let layout = std::fs::read_to_string(src.join("layout.toml"))
        .unwrap()
        .replace("width = 40.0", "width = 24.0");
    let base = std::fs::read_to_string(src.join("table.toml")).unwrap();
    let layout = pin_tablegen::layout::parse_layout(&layout).unwrap();
    let base = pin_core::schema::table::TableManifest::from_toml_str(&base).unwrap();
    let generated = pin_tablegen::generate(&layout, &base).unwrap();
    pin_tablegen::write_bundle(&generated, dir).unwrap();
    let mut rules = std::fs::read_to_string(dir.join("rules.lua")).unwrap();
    rules.push_str("\n-- replaced after validation\n");
    std::fs::write(dir.join("rules.lua"), rules).unwrap();
}

#[test]
fn a_bundle_replaced_after_validation_is_not_simulated() {
    let dir = scratch("wiring_snapshot_swap");
    build_greybox_bundle(&dir);
    let pristine = scratch("wiring_snapshot_pristine");
    build_greybox_bundle(&pristine);
    let (d, p) = (dir.clone(), pristine.clone());
    on_big_stack(move || {
        let checked = validate_table(&d).unwrap_or_else(|e| panic!("{e}"));
        let validated_hash = checked.snapshot.bundle_hash();
        // Swap the bundle on disk between validation and build.
        replace_with_failing_bundle(&d);
        assert!(
            validate_table(&d).is_err_and(|e| e.contains("clearance")),
            "the replacement must fail validation on its own"
        );
        let on_disk = BundleSnapshot::capture(&d).unwrap().bundle_hash();
        assert_ne!(on_disk, validated_hash, "the bundle really changed");

        let mut sim = load_checked(&checked, SimConfig::default()).unwrap();
        assert_eq!(
            sim.table_hash(),
            validated_hash,
            "simulated bytes = validated bytes"
        );
        // Same behaviour as the untouched bundle, tick for tick.
        let mut reference = Simulation::load(&p, SimConfig::default()).unwrap();
        assert_eq!(reference.table_hash(), validated_hash);
        for _ in 0..5 {
            sim.advance(200);
            reference.advance(200);
            assert_eq!(sim.state_hash(), reference.state_hash());
        }
    });
}

#[test]
fn load_validated_equals_validate_then_load_checked() {
    let dir = scratch("wiring_snapshot_same");
    build_greybox_bundle(&dir);
    on_big_stack(move || {
        let a = load_validated(&dir, SimConfig::default()).unwrap();
        let checked = validate_table(&dir).unwrap();
        let b = load_checked(&checked, SimConfig::default()).unwrap();
        let c = Simulation::load(&dir, SimConfig::default()).unwrap();
        assert_eq!(a.table_hash(), checked.snapshot.bundle_hash());
        assert_eq!(a.table_hash(), c.table_hash(), "snapshot hash = path hash");
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(a.state_hash(), c.state_hash());
        assert!(checked.snapshot.file_count() > 5);
    });
}

#[test]
fn a_snapshot_of_a_zip_bundle_matches_the_folder() {
    let dir = scratch("wiring_snapshot_zip");
    build_greybox_bundle(&dir);
    let folder = BundleSnapshot::capture(&dir).unwrap();
    // Re-archive the folder snapshot's files as a stored zip and read it back.
    let zip_dir = scratch("wiring_snapshot_zipfile");
    let zip_path = zip_dir.join("greybox.zip");
    let mut names = folder.vfs().list();
    names.sort();
    write_stored_zip(&zip_path, &names, |n| folder.read(n).unwrap());
    let zipped = BundleSnapshot::capture(&zip_path).unwrap();
    assert_eq!(zipped.bundle_hash(), folder.bundle_hash());
    assert_eq!(zipped.file_count(), folder.file_count());
}

/// Writes `names` as an uncompressed ("stored") zip archive (test-only; the
/// zip crate is not a pin_app dependency).
fn write_stored_zip(path: &std::path::Path, names: &[String], read: impl Fn(&str) -> Vec<u8>) {
    fn crc32(b: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &x in b {
            c ^= u32::from(x);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    let mut out = Vec::new();
    let mut cd = Vec::new();
    for n in names {
        let data = read(n);
        let off = out.len() as u32;
        let crc = crc32(&data);
        let len = data.len() as u32;
        let hdr = |sig: u32, central: bool, buf: &mut Vec<u8>| {
            buf.extend_from_slice(&sig.to_le_bytes());
            if central {
                buf.extend_from_slice(&20u16.to_le_bytes());
            }
            for v in [20u16, 0, 0, 0, 33] {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            for v in [crc, len, len] {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            buf.extend_from_slice(&(n.len() as u16).to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes());
            if central {
                for v in [0u16, 0, 0] {
                    buf.extend_from_slice(&v.to_le_bytes());
                }
                buf.extend_from_slice(&0u32.to_le_bytes());
                buf.extend_from_slice(&off.to_le_bytes());
            }
            buf.extend_from_slice(n.as_bytes());
        };
        hdr(0x0403_4B50, false, &mut out);
        out.extend_from_slice(&data);
        hdr(0x0201_4B50, true, &mut cd);
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&cd);
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    for v in [0u16, 0, names.len() as u16, names.len() as u16] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    std::fs::write(path, out).unwrap();
}
