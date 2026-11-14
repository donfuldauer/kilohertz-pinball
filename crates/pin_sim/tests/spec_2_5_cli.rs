//! Subphase 2.5 spec tests (Independent Tester): the `pin_sim` binary.
//!
//!   pin_sim <bundle> --ticks N [--seed S] [--record-inputs FILE]  -> `state_hash=<16 hex>`
//!   pin_sim <bundle> --replay-inputs FILE                         -> `state_hash=<16 hex>`
//!
//! Also: determinism across processes (D9: same binary + same machine).

#[path = "fixtures/spec_2_5/support.rs"]
mod support;

use pin_sim::replay::{Recording, replay};
use std::ffi::OsStr;
use std::process::{Command, Output};
use support::*;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pin_sim"))
}

fn run(args: &[&OsStr]) -> Output {
    cli().args(args).output().expect("run pin_sim")
}

/// Runs the CLI, requires success and returns the printed hash.
fn cli_hash(args: &[&OsStr]) -> u64 {
    let out = run(args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "pin_sim {args:?} failed ({}): stdout={stdout} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    parse_state_hash(&stdout)
        .unwrap_or_else(|| panic!("no `state_hash=<16 hex>` line in pin_sim output: {stdout:?}"))
}

/// Requires a clean failure: non-zero exit, no hash printed, no panic.
fn cli_fails(args: &[&OsStr], what: &str) {
    let out = run(args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{what}: pin_sim {args:?} succeeded: {stdout}");
    assert!(
        parse_state_hash(&stdout).is_none(),
        "{what}: a failed run must not print a state hash: {stdout}"
    );
    assert!(
        !stderr.contains("panicked"),
        "{what}: pin_sim panicked instead of reporting an error: {stderr}"
    );
    assert!(!stderr.trim().is_empty(), "{what}: no error message on stderr");
}

fn s(x: &str) -> &OsStr {
    OsStr::new(x)
}

#[test]
fn ticks_prints_the_state_hash_of_an_inputless_run() {
    let b = bundle();
    let got = cli_hash(&[b.as_os_str(), s("--ticks"), s("2000")]);
    let want = on_sim_thread(|| {
        let mut sim = load();
        sim.advance(2000);
        sim.state_hash()
    });
    assert_eq!(got, want, "CLI {got:016x} vs library {want:016x}");

    // The line format is exactly `state_hash=` + 16 lower/upper hex digits.
    let out = run(&[b.as_os_str(), s("--ticks"), s("3")]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("state_hash="))
        .expect("state_hash line");
    assert_eq!(line.len(), "state_hash=".len() + 16, "{line:?}");
}

#[test]
fn seed_flag_sets_the_rng_seed() {
    let b = bundle();
    let default = cli_hash(&[b.as_os_str(), s("--ticks"), s("300")]);
    let seven = cli_hash(&[b.as_os_str(), s("--ticks"), s("300"), s("--seed"), s("7")]);
    let eight = cli_hash(&[b.as_os_str(), s("--ticks"), s("300"), s("--seed"), s("8")]);
    assert_ne!(seven, eight, "the RNG state is part of the hash");
    assert_ne!(seven, default);
    let want = on_sim_thread(|| {
        let mut sim = load_at(&bundle(), 7);
        sim.advance(300);
        sim.state_hash()
    });
    assert_eq!(seven, want);
    // The default seed is 0x4B485A = 4933722.
    let explicit = cli_hash(&[b.as_os_str(), s("--ticks"), s("300"), s("--seed"), s("4933722")]);
    assert_eq!(explicit, default);
    // Flag order does not matter.
    let swapped = cli_hash(&[b.as_os_str(), s("--seed"), s("7"), s("--ticks"), s("300")]);
    assert_eq!(swapped, seven);
}

#[test]
fn record_then_replay_round_trip_prints_the_same_hash() {
    let b = bundle();
    let dir = temp_dir("cli_rt");
    let file = dir.join("run.khzrec");
    let recorded = cli_hash(&[
        b.as_os_str(),
        s("--ticks"),
        s("1500"),
        s("--seed"),
        s("99"),
        s("--record-inputs"),
        file.as_os_str(),
    ]);
    // Recording does not change the run.
    let plain = cli_hash(&[b.as_os_str(), s("--ticks"), s("1500"), s("--seed"), s("99")]);
    assert_eq!(recorded, plain);

    // The file is a Recording with the run's seed, length and table hash.
    let bytes = std::fs::read(&file).expect("--record-inputs wrote the file");
    let rec = Recording::from_bytes(&bytes).expect("the CLI writes a parseable recording");
    assert_eq!(rec.seed, 99);
    assert_eq!(rec.ticks, 1500);
    let (table_hash, lib_replay) = {
        let rec2 = Recording::from_bytes(&bytes).unwrap();
        on_sim_thread(move || (load().table_hash(), replay(&bundle(), &rec2).unwrap()))
    };
    assert_eq!(rec.table_hash, table_hash);
    assert_eq!(lib_replay, recorded);

    for _ in 0..3 {
        let replayed = cli_hash(&[b.as_os_str(), s("--replay-inputs"), file.as_os_str()]);
        assert_eq!(replayed, recorded, "replay {replayed:016x} vs record {recorded:016x}");
    }
}

/// Determinism across 10 processes: a 10,000-tick recording with balls,
/// flippers and scoring replays to one hash in 10 separate processes, and
/// that hash is the in-process one.
#[test]
fn ten_processes_replay_a_10000_tick_recording_to_one_hash() {
    const TICKS: u64 = 10_000;
    let dir = temp_dir("cli_10");
    let file = dir.join("inputs.khzrec");
    let (bytes, want) = on_sim_thread(|| {
        let events = input_script(TICKS, 4);
        let live = drive(&bundle(), 2024, TICKS, &events);
        assert!(live.score(P_SCORE).unwrap() > 0);
        let rec = recording(live.table_hash(), 2024, TICKS, &events);
        (rec.to_bytes(), live.state_hash())
    });
    std::fs::write(&file, bytes).unwrap();
    let b = bundle();
    let children: Vec<_> = (0..10)
        .map(|_| {
            cli()
                .arg(&b)
                .arg("--replay-inputs")
                .arg(&file)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn pin_sim")
        })
        .collect();
    let mut hashes = Vec::new();
    for c in children {
        let out = c.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "replay failed: {stdout} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        hashes.push(parse_state_hash(&stdout).expect("state_hash line"));
    }
    assert!(
        hashes.iter().all(|h| *h == want),
        "in-process {want:016x}, processes {hashes:016x?}"
    );
}

/// Ten separate recording processes agree as well (no inputs, fixed seed).
#[test]
fn ten_processes_agree_on_an_inputless_run() {
    let b = bundle();
    let first = cli_hash(&[b.as_os_str(), s("--ticks"), s("4000"), s("--seed"), s("5")]);
    for _ in 0..9 {
        assert_eq!(
            cli_hash(&[b.as_os_str(), s("--ticks"), s("4000"), s("--seed"), s("5")]),
            first
        );
    }
}

#[test]
fn zip_bundles_work_on_the_command_line() {
    let dir = temp_dir("cli_zip");
    let zip = dir.join("table.zip");
    zip_dir(&bundle(), &zip);
    let file = dir.join("zip.khzrec");
    let a = cli_hash(&[
        zip.as_os_str(),
        s("--ticks"),
        s("500"),
        s("--record-inputs"),
        file.as_os_str(),
    ]);
    let b = cli_hash(&[zip.as_os_str(), s("--replay-inputs"), file.as_os_str()]);
    assert_eq!(a, b);
}

#[test]
fn replay_refuses_another_table_and_bad_files() {
    let b = bundle();
    let dir = temp_dir("cli_bad");
    let file = dir.join("good.khzrec");
    cli_hash(&[
        b.as_os_str(),
        s("--ticks"),
        s("200"),
        s("--record-inputs"),
        file.as_os_str(),
    ]);

    // Another table (one byte more in the script).
    let edited = bundle_copy("cli_edited");
    let mut lua = std::fs::read_to_string(edited.join("rules.lua")).unwrap();
    lua.push_str("\n-- edited\n");
    std::fs::write(edited.join("rules.lua"), lua).unwrap();
    cli_fails(
        &[edited.as_os_str(), s("--replay-inputs"), file.as_os_str()],
        "table hash mismatch",
    );

    // Corrupt, truncated, empty and missing replay files.
    let good = std::fs::read(&file).unwrap();
    let mut bad_magic = good.clone();
    bad_magic[0] ^= 0xFF;
    let cases: [(&str, Vec<u8>); 4] = [
        ("bad magic", bad_magic),
        ("truncated", good[..good.len() / 2].to_vec()),
        ("empty", Vec::new()),
        ("garbage", vec![0xA5; 300]),
    ];
    for (what, bytes) in cases {
        let f = dir.join(format!("{}.khzrec", what.replace(' ', "_")));
        std::fs::write(&f, bytes).unwrap();
        cli_fails(&[b.as_os_str(), s("--replay-inputs"), f.as_os_str()], what);
    }
    let missing = dir.join("missing.khzrec");
    cli_fails(
        &[b.as_os_str(), s("--replay-inputs"), missing.as_os_str()],
        "missing replay file",
    );
}

#[test]
fn bad_invocations_fail_cleanly() {
    let b = bundle();
    let dir = temp_dir("cli_args");
    let missing_bundle = dir.join("no_such_bundle");
    cli_fails(&[missing_bundle.as_os_str(), s("--ticks"), s("10")], "missing bundle");
    cli_fails(&[b.as_os_str(), s("--ticks"), s("ten")], "non-numeric --ticks");
    cli_fails(&[b.as_os_str(), s("--ticks")], "--ticks without a value");
    cli_fails(&[b.as_os_str(), s("--ticks"), s("10"), s("--seed"), s("x")], "bad --seed");
    cli_fails(&[b.as_os_str(), s("--no-such-flag")], "unknown flag");
    cli_fails(&[], "no arguments");
    // An unwritable record path is an error, not a silent success.
    let unwritable = dir.join("no_such_dir").join("x.khzrec");
    cli_fails(
        &[b.as_os_str(), s("--ticks"), s("10"), s("--record-inputs"), unwritable.as_os_str()],
        "unwritable --record-inputs path",
    );
}
