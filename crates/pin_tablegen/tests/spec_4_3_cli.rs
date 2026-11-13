//! Subphase 4.3 spec tests (Independent Tester, black box): the `pin_tablegen`
//! binary (deliverable 1; criteria 1, 3, 5 and 6 through the CLI).
//!
//! `pin_tablegen <layout.toml> --table <table.toml> --out <bundle_dir>`

#[path = "fixtures/spec_4_3/support.rs"]
mod support;

use pin_core::schema::table::TableManifest;
use proptest::prelude::*;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use support::*;

fn run(layout: &Path, table: &Path, out: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pin_tablegen"))
        .arg(layout)
        .arg("--table")
        .arg(table)
        .arg("--out")
        .arg(out)
        .output()
        .expect("the pin_tablegen binary runs")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A clean failure: non-zero exit code, no panic, no signal.
fn assert_clean_failure(o: &Output, what: &str) {
    let err = stderr(o);
    assert!(!o.status.success(), "{what}: the CLI must fail");
    assert!(
        o.status.code().is_some_and(|c| c != 101),
        "{what}: exit status {:?} (panic or signal)\n{err}",
        o.status
    );
    assert!(
        !err.contains("panicked"),
        "{what}: the CLI panicked:\n{err}"
    );
    assert!(!err.trim().is_empty(), "{what}: no error message");
}

fn assert_no_outputs(dir: &Path, what: &str) {
    for f in ["playfield.glb", "table.toml"] {
        assert!(
            !dir.join(f).exists(),
            "{what}: {f} was written although the CLI failed"
        );
    }
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn a3_path() -> PathBuf {
    fixture_dir().join("a3_layout.toml")
}

#[test]
fn cli_generates_a_bundle_that_validates() {
    let out = temp_dir("cli-a3");
    copy_base_extras(&out);
    let o = run(&a3_path(), &base_table_path(), &out);
    assert!(o.status.success(), "{}", stderr(&o));
    let loaded = load_dir(&out).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        pin_assets::validate_gltf_bindings(&loaded.scene, &loaded.table.ids).is_empty()
    );
    // Same result as the library on the same inputs.
    let generated = gen_ok(&a3_layout(), &base_table());
    assert!(std::fs::read(out.join("playfield.glb")).unwrap() == generated.glb);
    assert_eq!(
        std::fs::read_to_string(out.join("table.toml")).unwrap(),
        generated.table_toml
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn cli_creates_a_missing_output_directory_or_fails_cleanly() {
    let root = temp_dir("cli-mkdir");
    let out = root.join("new_bundle");
    let o = run(&a3_path(), &base_table_path(), &out);
    if o.status.success() {
        assert!(out.join("playfield.glb").is_file());
        assert!(out.join("table.toml").is_file());
    } else {
        assert_clean_failure(&o, "missing --out directory");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Criterion 3 through the CLI: ten runs, byte-identical outputs.
#[test]
fn cli_is_deterministic_over_ten_runs() {
    let geo = fixture_dir().join("geo_layout.toml");
    let mut first: Option<(Vec<u8>, Vec<u8>)> = None;
    for run_index in 0..10 {
        let out = temp_dir("cli-det");
        let o = run(&geo, &base_table_path(), &out);
        assert!(o.status.success(), "{}", stderr(&o));
        let pair = (
            std::fs::read(out.join("playfield.glb")).unwrap(),
            std::fs::read(out.join("table.toml")).unwrap(),
        );
        match &first {
            None => first = Some(pair),
            Some(f) => {
                assert!(f.0 == pair.0, "glb differs on run {run_index}");
                assert!(f.1 == pair.1, "table.toml differs on run {run_index}");
            }
        }
        let _ = std::fs::remove_dir_all(&out);
    }
}

/// Deliverable 1: a re-run on the generated table.toml replaces the
/// generated entries and never duplicates them.
#[test]
fn cli_rerun_on_its_own_output_does_not_duplicate() {
    let out = temp_dir("cli-rerun");
    copy_base_extras(&out);
    let geo = fixture_dir().join("geo_layout.toml");
    let o = run(&geo, &base_table_path(), &out);
    assert!(o.status.success(), "{}", stderr(&o));
    let first_toml = std::fs::read_to_string(out.join("table.toml")).unwrap();
    let first_glb = std::fs::read(out.join("playfield.glb")).unwrap();
    let first = TableManifest::from_toml_str(&first_toml).unwrap();

    // In place: the previous output is the base of the next run.
    for _ in 0..2 {
        let o = run(&geo, &out.join("table.toml"), &out);
        assert!(o.status.success(), "{}", stderr(&o));
        let again =
            TableManifest::from_toml_str(&std::fs::read_to_string(out.join("table.toml")).unwrap())
                .unwrap();
        assert_eq!(again, first);
        let ids: BTreeSet<&str> = again.splines.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), again.splines.len(), "duplicate spline ids");
        assert!(std::fs::read(out.join("playfield.glb")).unwrap() == first_glb);
        load_dir(&out).unwrap_or_else(|e| panic!("{e}"));
    }

    // A different layout on the same table replaces what was generated.
    let o = run(&a3_path(), &out.join("table.toml"), &out);
    assert!(o.status.success(), "{}", stderr(&o));
    let direct = gen_ok(&a3_layout(), &base_table());
    let replaced =
        TableManifest::from_toml_str(&std::fs::read_to_string(out.join("table.toml")).unwrap())
            .unwrap();
    assert_eq!(replaced, direct.table);
    let _ = std::fs::remove_dir_all(&out);
}

/// Criterion 5 through the CLI: the message names the element and nothing
/// is emitted.
#[test]
fn cli_names_the_offending_element_and_emits_nothing() {
    let a3 = std::fs::read_to_string(a3_path()).unwrap();
    let cases: [(&str, String, &str); 4] = [
        (
            "unknown lamp",
            a3.replace("lamp = \"overdrive_ready\"", "lamp = \"no_such_lamp\""),
            "inserts[0]",
        ),
        (
            "missing spline",
            a3.replace("spline = \"return_wireform\"", "spline = \"no_such_spline\""),
            "ramps[0]",
        ),
        (
            "self-intersecting closed wall",
            format!(
                "{a3}\n[[walls]]\npoints = [[100.0, 100.0], [200.0, 200.0], [200.0, 100.0], \
                 [100.0, 200.0]]\nclosed = true\nmaterial = \"steel\"\n"
            ),
            "walls[1]",
        ),
        (
            "second unknown lamp",
            format!(
                "{a3}\n[[inserts]]\nlamp = \"lamp_a\"\nposition = [100.0, 500.0]\nshape = \
                 \"rect\"\nsize = 12.0\n\n[[inserts]]\nlamp = \"lamp_zz\"\nposition = [150.0, \
                 500.0]\nshape = \"arrow\"\nsize = 12.0\n"
            ),
            "inserts[2]",
        ),
    ];
    for (what, text, element) in cases {
        assert_ne!(text, a3, "{what}: the fixture was not modified");
        let work = temp_dir("cli-invalid");
        let out = work.join("bundle");
        std::fs::create_dir_all(&out).unwrap();
        let layout = write(&work, "layout.toml", text.as_bytes());
        let o = run(&layout, &base_table_path(), &out);
        assert_clean_failure(&o, what);
        assert!(
            stderr(&o).contains(element),
            "{what}: the message must name {element}:\n{}",
            stderr(&o)
        );
        assert_no_outputs(&out, what);
        let _ = std::fs::remove_dir_all(&work);
    }
}

/// A failed run leaves an existing bundle untouched (it never replaces a
/// valid bundle by half of an invalid one).
#[test]
fn cli_failure_leaves_previous_outputs_untouched() {
    let work = temp_dir("cli-keep");
    let out = work.join("bundle");
    std::fs::create_dir_all(&out).unwrap();
    let o = run(&a3_path(), &base_table_path(), &out);
    assert!(o.status.success(), "{}", stderr(&o));
    let before = (
        std::fs::read(out.join("playfield.glb")).unwrap(),
        std::fs::read(out.join("table.toml")).unwrap(),
    );
    let bad = std::fs::read_to_string(a3_path())
        .unwrap()
        .replace("lamp = \"overdrive_ready\"", "lamp = \"no_such_lamp\"");
    let layout = write(&work, "layout.toml", bad.as_bytes());
    let o = run(&layout, &base_table_path(), &out);
    assert_clean_failure(&o, "unknown lamp over an existing bundle");
    assert!(std::fs::read(out.join("playfield.glb")).unwrap() == before.0);
    assert!(std::fs::read(out.join("table.toml")).unwrap() == before.1);
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn cli_reports_unreadable_or_malformed_inputs() {
    let work = temp_dir("cli-inputs");
    let out = work.join("bundle");
    std::fs::create_dir_all(&out).unwrap();

    let o = run(&work.join("missing.toml"), &base_table_path(), &out);
    assert_clean_failure(&o, "missing layout file");
    assert_no_outputs(&out, "missing layout file");

    let o = run(&a3_path(), &work.join("missing_table.toml"), &out);
    assert_clean_failure(&o, "missing table file");
    assert_no_outputs(&out, "missing table file");

    let bad_table = write(&work, "bad_table.toml", b"schema_version = 1\n[meta\n");
    let o = run(&a3_path(), &bad_table, &out);
    assert_clean_failure(&o, "malformed table.toml");
    assert_no_outputs(&out, "malformed table.toml");

    let v2 = std::fs::read_to_string(a3_path())
        .unwrap()
        .replace("schema_version = 1", "schema_version = 2");
    let layout = write(&work, "v2.toml", v2.as_bytes());
    let o = run(&layout, &base_table_path(), &out);
    assert_clean_failure(&o, "schema_version 2");
    assert_no_outputs(&out, "schema_version 2");

    let unknown_key = format!("{}\nbogus = 1\n", std::fs::read_to_string(a3_path()).unwrap());
    let layout = write(&work, "unknown.toml", unknown_key.as_bytes());
    let o = run(&layout, &base_table_path(), &out);
    assert_clean_failure(&o, "unknown key");
    assert_no_outputs(&out, "unknown key");

    // Argument errors.
    let exe = env!("CARGO_BIN_EXE_pin_tablegen");
    let o = Command::new(exe).output().unwrap();
    assert_clean_failure(&o, "no arguments");
    let o = Command::new(exe).arg(a3_path()).output().unwrap();
    assert_clean_failure(&o, "no --table / --out");
    let o = Command::new(exe)
        .arg(a3_path())
        .arg("--table")
        .arg(base_table_path())
        .output()
        .unwrap();
    assert_clean_failure(&o, "no --out");
    let _ = std::fs::remove_dir_all(&work);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, failure_persistence: None, ..ProptestConfig::default() })]

    /// Criterion 6: no panics on random layout.toml bytes, through the CLI.
    #[test]
    fn cli_survives_random_layout_bytes(bytes in prop::collection::vec(any::<u8>(), 0..1024)) {
        let work = temp_dir("cli-fuzz");
        let out = work.join("bundle");
        std::fs::create_dir_all(&out).unwrap();
        let layout = write(&work, "layout.toml", &bytes);
        let o = run(&layout, &base_table_path(), &out);
        let err = stderr(&o);
        prop_assert!(o.status.code().is_some_and(|c| c != 101), "status {:?}\n{}", o.status, err);
        prop_assert!(!err.contains("panicked"), "{}", err);
        if !o.status.success() {
            prop_assert!(!out.join("playfield.glb").exists() && !out.join("table.toml").exists());
        }
        let _ = std::fs::remove_dir_all(&work);
    }

    /// Criterion 6 with inputs that reach the generator: the example layout
    /// with a few characters or numbers replaced.
    #[test]
    fn cli_survives_mutated_example_layouts(
        edits in prop::collection::vec((any::<usize>(), 0usize..10), 1..=3),
    ) {
        const TOKENS: [&str; 10] =
            ["nan", "inf", "-1.0", "0.0", "1e30", "99999.0", "\"\"", "[", "true", "1e-9"];
        let mut text = std::fs::read_to_string(a3_path()).unwrap();
        for (pos, token) in edits {
            // Replace the numeric literal nearest after `pos`.
            let b = text.as_bytes();
            let start = (pos % b.len()..b.len()).find(|&i| b[i].is_ascii_digit());
            let Some(start) = start else { continue };
            let end = (start..b.len())
                .find(|&i| !(b[i].is_ascii_digit() || b[i] == b'.'))
                .unwrap_or(b.len());
            text = format!("{}{}{}", &text[..start], TOKENS[token], &text[end..]);
        }
        let work = temp_dir("cli-mut");
        let out = work.join("bundle");
        copy_base_extras(&out);
        let layout = write(&work, "layout.toml", text.as_bytes());
        let o = run(&layout, &base_table_path(), &out);
        let err = stderr(&o);
        prop_assert!(o.status.code().is_some_and(|c| c != 101), "status {:?}\n{}", o.status, err);
        prop_assert!(!err.contains("panicked"), "{}", err);
        if o.status.success() {
            // Whatever the CLI emits is a valid bundle.
            load_dir(&out).map_err(TestCaseError::fail)?;
        } else {
            prop_assert!(!out.join("playfield.glb").exists() && !out.join("table.toml").exists());
        }
        let _ = std::fs::remove_dir_all(&work);
    }
}
