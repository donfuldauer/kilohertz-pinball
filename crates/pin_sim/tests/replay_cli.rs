//! Subphase 2.5 deliverable 3 (gate step 5/6): the `pin_sim` binary records
//! with `--record-inputs` and replays with `--replay-inputs` to the same
//! `state_hash=` line; argument errors exit with code 2.

mod sim_support;

use pin_sim::cli::{CliError, Command, hash_line, main_with_args, parse_args, run};
use sim_support::*;
use std::process::Command as Process;

fn bin() -> Process {
    Process::new(env!("CARGO_BIN_EXE_pin_sim"))
}

fn hash_of(out: &std::process::Output) -> String {
    assert!(
        out.status.success(),
        "pin_sim failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    let line = stdout.trim();
    assert!(line.starts_with("state_hash="), "{line}");
    let hex = &line["state_hash=".len()..];
    assert_eq!(hex.len(), 16, "{line}");
    assert!(
        hex.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    line.to_owned()
}

#[test]
fn record_then_replay_prints_the_same_hash() {
    let dir = scratch_dir("cli");
    let rec = dir.join("run.khzrec");
    let bundle = fixture();
    let recorded = hash_of(
        &bin()
            .arg(&bundle)
            .args(["--ticks", "3000", "--seed", "0x2A", "--record-inputs"])
            .arg(&rec)
            .output()
            .unwrap(),
    );
    for _ in 0..2 {
        let replayed = hash_of(
            &bin()
                .arg(&bundle)
                .arg("--replay-inputs")
                .arg(&rec)
                .output()
                .unwrap(),
        );
        assert_eq!(replayed, recorded);
    }
    // The same run without recording prints the same hash.
    let plain = hash_of(
        &bin()
            .arg(&bundle)
            .args(["--ticks", "3000", "--seed", "42"])
            .output()
            .unwrap(),
    );
    assert_eq!(plain, recorded);
    let idle = hash_of(
        &bin()
            .arg(&bundle)
            .args(["--ticks", "3000", "--seed", "42", "--idle"])
            .output()
            .unwrap(),
    );
    assert_ne!(idle, recorded);
}

#[test]
fn bad_arguments_exit_with_usage() {
    for args in [
        vec![],
        vec!["--ticks", "5"],
        vec!["b", "--ticks"],
        vec!["b", "--ticks", "x"],
        vec!["b", "--frobnicate"],
        vec!["b", "c", "--ticks", "1"],
        vec!["b"],
        vec!["b", "--ticks", "1", "--replay-inputs", "f"],
        vec!["b", "--replay-inputs", "f", "--seed", "1"],
    ] {
        let out = bin().args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage"));
    }
    let out = bin()
        .arg(fixture())
        .args(["--replay-inputs", "/no/such/recording"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn library_entry_points() {
    let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
    let cmd = parse_args(&args(&["t", "--ticks", "0x10", "--idle"])).unwrap();
    assert_eq!(
        cmd,
        Command::Run {
            bundle: "t".into(),
            ticks: 16,
            seed: pin_sim::DEFAULT_SEED,
            record: None,
            idle: true
        }
    );
    assert_eq!(hash_line(0xab), "state_hash=00000000000000ab");
    let e = CliError::Usage("x".into());
    assert_eq!(e.exit_code(), 2);
    assert!(e.to_string().contains("usage"));
    assert_eq!(CliError::Failed("y".into()).exit_code(), 1);
    let fixture = fixture().to_string_lossy().into_owned();
    let line = main_with_args(&args(&[&fixture, "--ticks", "50"])).unwrap();
    let again = on_sim_thread(move || {
        run(&parse_args(&args(&[&fixture, "--ticks", "50"])).unwrap()).unwrap()
    });
    assert_eq!(line, hash_line(again));
    let missing = main_with_args(&["/no/such/bundle".to_owned(), "--ticks".into(), "1".into()]);
    assert!(matches!(missing, Err(CliError::Failed(_))));
}

#[test]
fn recording_inside_the_folder_bundle_is_refused() {
    // A recording written into the bundle would change its table hash, so
    // it could never replay; the run refuses it before loading.
    let bundle = fixture_variant("rec_inside", &[]);
    for rec in [
        bundle.join("run.khzrec"),
        bundle.join(".").join("run.khzrec"),
    ] {
        let err = run(&Command::Run {
            bundle: bundle.to_path_buf(),
            ticks: 10,
            seed: 1,
            record: Some(rec.clone()),
            idle: false,
        })
        .unwrap_err();
        assert!(matches!(err, CliError::Usage(_)), "{err:?}");
        assert_eq!(err.exit_code(), 2);
        assert!(!rec.exists());
    }
    let out = bin()
        .arg(&*bundle)
        .args(["--ticks", "10", "--record-inputs"])
        .arg(bundle.join("run.khzrec"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // Next to the bundle is fine, and replays.
    let outside = scratch_dir("rec_outside");
    let rec = outside.join("run.khzrec");
    let recorded = hash_of(
        &bin()
            .arg(&*bundle)
            .args(["--ticks", "200", "--record-inputs"])
            .arg(&rec)
            .output()
            .unwrap(),
    );
    let replayed = hash_of(
        &bin()
            .arg(&*bundle)
            .arg("--replay-inputs")
            .arg(&rec)
            .output()
            .unwrap(),
    );
    assert_eq!(replayed, recorded);
}

#[cfg(unix)]
#[test]
fn recording_through_a_symlink_into_the_bundle_is_refused() {
    // An existing output path that is a symlink to a bundle file (or a
    // dangling one pointing into the bundle) would make `save` write into
    // the bundle; it is judged by where it resolves, not by its parent.
    use std::os::unix::fs::symlink;
    let bundle = fixture_variant("rec_symlink", &[]);
    let victim = std::fs::read_dir(&*bundle)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_file())
        .expect("the bundle has a file");
    let before = std::fs::read(&victim).unwrap();
    let outside = scratch_dir("rec_symlink_out");
    let to_file = outside.join("to_file.khzrec");
    symlink(&victim, &to_file).unwrap();
    let dangling = outside.join("dangling.khzrec");
    symlink(bundle.join("new.khzrec"), &dangling).unwrap();
    let to_dir = outside.join("bundle_link");
    symlink(&*bundle, &to_dir).unwrap();
    for rec in [to_file, dangling, to_dir.join("run.khzrec")] {
        let err = run(&Command::Run {
            bundle: bundle.to_path_buf(),
            ticks: 10,
            seed: 1,
            record: Some(rec.clone()),
            idle: false,
        })
        .unwrap_err();
        assert!(matches!(err, CliError::Usage(_)), "{rec:?}: {err:?}");
    }
    assert_eq!(std::fs::read(&victim).unwrap(), before);
    assert!(!bundle.join("new.khzrec").exists());
    assert!(!bundle.join("run.khzrec").exists());
}
