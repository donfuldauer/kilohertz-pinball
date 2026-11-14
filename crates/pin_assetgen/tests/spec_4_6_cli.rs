//! Spec tests, Subphase 4.6 — the CLI (brief deliverable 6):
//! `pin_assetgen --title <t> --palette <hex,...> --seed <n> --out <dir>`.

mod spec_4_6_support;

use std::process::Command;

use spec_4_6_support::*;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pin_assetgen"))
}

#[test]
fn cli_writes_the_same_kit_as_generate_kit() {
    let out = tmp("cli_out");
    let status = cli()
        .args([
            "--title",
            "Apex Circuit",
            "--palette",
            "0b1d3a,f2a900,e63946",
            "--seed",
            "1",
            "--out",
        ])
        .arg(&out)
        .output()
        .expect("run pin_assetgen");
    assert!(
        status.status.success(),
        "exit {:?}\nstderr: {}",
        status.status.code(),
        String::from_utf8_lossy(&status.stderr)
    );
    let (lib_dir, man) = generate(&apex_meta(), "cli_lib");
    let (a, b) = (read_tree(&out), read_tree(&lib_dir));
    assert!(a.contains_key("assets.toml"), "CLI wrote no assets.toml");
    assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>());
    for (name, bytes) in &a {
        assert!(
            bytes == &b[name],
            "{name}: CLI output differs from generate_kit"
        );
    }
    for e in &man.entries {
        assert!(out.join(&e.path).is_file(), "{}", e.path);
    }
    let _ = std::fs::remove_dir_all(out);
    let _ = std::fs::remove_dir_all(lib_dir);
}

#[test]
fn cli_seed_changes_the_output_and_large_seeds_parse() {
    let run = |seed: &str, name: &str| {
        let out = tmp(name);
        let o = cli()
            .args([
                "--title",
                "T",
                "--palette",
                "ff0000,00ff00",
                "--seed",
                seed,
                "--out",
            ])
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "seed {seed}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let t = read_tree(&out);
        let _ = std::fs::remove_dir_all(out);
        t
    };
    let a = run("1", "cli_s1");
    let b = run("2", "cli_s2");
    let max = run("18446744073709551615", "cli_smax");
    assert!(a != b, "--seed is ignored");
    assert!(a != max);
    let (lib, _) = generate(
        &meta("T", &[[255, 0, 0], [0, 255, 0]], u64::MAX),
        "cli_smax_lib",
    );
    assert!(
        max == read_tree(&lib),
        "--seed 18446744073709551615 != u64::MAX kit"
    );
    let _ = std::fs::remove_dir_all(lib);
}

#[test]
fn cli_rejects_bad_arguments_without_panicking() {
    let out = tmp("cli_bad");
    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["--title", "T", "--palette", "ff0000", "--seed", "1"], // no --out
        vec![
            "--title",
            "T",
            "--palette",
            "ff0000",
            "--seed",
            "not_a_number",
            "--out",
            "x",
        ],
        vec![
            "--title",
            "T",
            "--palette",
            "zzzzzz",
            "--seed",
            "1",
            "--out",
            "x",
        ],
        vec![
            "--title",
            "T",
            "--palette",
            "ff00",
            "--seed",
            "1",
            "--out",
            "x",
        ],
        vec![
            "--title",
            "T",
            "--palette",
            "ff0000",
            "--seed",
            "-1",
            "--out",
            "x",
        ],
        vec![
            "--title",
            "T",
            "--palette",
            "ff0000",
            "--seed",
            "18446744073709551616",
            "--out",
            "x",
        ],
        vec!["--bogus"],
        vec!["--title"],
    ];
    for args in cases {
        let o = cli().current_dir(&out).args(&args).output().unwrap();
        assert!(!o.status.success(), "{args:?} was accepted");
        assert_ne!(o.status.code(), Some(101), "{args:?} panicked");
        assert!(o.status.code().is_some(), "{args:?} died on a signal");
        assert!(!o.stderr.is_empty(), "{args:?}: no error message on stderr");
        assert!(
            !out.join("x").join("assets.toml").exists(),
            "{args:?} still wrote a kit"
        );
    }
    let _ = std::fs::remove_dir_all(out);
}
