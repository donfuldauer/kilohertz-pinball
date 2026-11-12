//! The helper validator itself is checked against inline fixtures, so that a
//! green run on the real workflows means something.

use proptest::prelude::*;
use spec_ci::*;

const FIXTURE: &str = r#"
name: fx
on:
  push:
  pull_request:
concurrency:
  group: ci-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true
jobs:
  a:
    runs-on: ${{ matrix.os }}
    timeout-minutes: 45
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest]
        include:
          - os: macos-14
            target: aarch64-apple-darwin
    steps:
      - uses: actions/checkout@v4
      - run: |
          cargo test --workspace \
            --exclude pin_render   --exclude=pin_audio
          # cargo clippy is only a comment here
      - run: xvfb-run -a cargo +stable test -p pin_app -ppin_sim --package=pin_core -- -p nope
        working-directory: sub/dir
  b:
    runs-on: ubuntu-24.04
    steps:
      - run: echo hi
"#;

#[test]
fn fixture_is_read_correctly() {
    let wf = Workflow::parse(FIXTURE).unwrap();
    let mut t = wf.triggers();
    t.sort();
    assert_eq!(t, ["pull_request", "push"]);
    let c = wf.concurrency().unwrap();
    assert!(c.cancel_in_progress && c.is_per_ref());

    let a = wf.job("a").unwrap();
    assert_eq!(a.timeout_minutes(), Some(45));
    assert_eq!(
        a.runner_labels(),
        ["macos-14", "ubuntu-latest", "windows-latest"]
    );
    assert!(Os::ALL.iter().all(|os| a.covers(*os)));
    assert_eq!(a.uses(), ["actions/checkout@v4"]);
    assert_eq!(a.step_count(), 3);

    let script = a.script();
    let tests = cargo_lines(&script, "test");
    assert_eq!(tests.len(), 2, "{script}");
    assert_eq!(excluded_packages(tests[0]), ["pin_render", "pin_audio"]);
    assert_eq!(
        selected_packages(tests[1]),
        ["pin_app", "pin_sim", "pin_core"]
    );
    assert!(
        cargo_lines(&script, "clippy").is_empty(),
        "comments are not commands"
    );
    assert_eq!(a.run_steps_with_dir()[1].0, "sub/dir");

    let b = wf.job("b").unwrap();
    assert_eq!(b.timeout_minutes(), None);
    assert!(b.covers(Os::Linux) && !b.covers(Os::Windows) && !b.covers(Os::Macos));
    assert!(wf.job("c").is_none());
}

#[test]
fn malformed_workflows_are_rejected() {
    for bad in [
        "",
        "jobs: {}",
        "- a\n- b",
        "on: push",
        "jobs: [a]",
        "a: [1\n",
        "a: 1\n---\nb: 2",
    ] {
        assert!(Workflow::parse(bad).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn concurrency_forms() {
    let wf = Workflow::parse("concurrency: fixed\njobs:\n  a:\n    runs-on: x\n").unwrap();
    let c = wf.concurrency().unwrap();
    assert!(!c.cancel_in_progress && !c.is_per_ref());
    let wf = Workflow::parse(
        "concurrency:\n  group: g-${{ github.head_ref || github.run_id }}\n  cancel-in-progress: ${{ github.ref != 'refs/heads/main' }}\njobs:\n  a:\n    runs-on: x\n",
    )
    .unwrap();
    let c = wf.concurrency().unwrap();
    assert!(c.cancel_in_progress && c.is_per_ref());
    let wf =
        Workflow::parse("concurrency:\n  group: g\n  cancel-in-progress: false\njobs:\n  a: {}\n")
            .unwrap();
    assert!(!wf.concurrency().unwrap().cancel_in_progress);
}

#[test]
fn arm64_macos_labels() {
    for ok in [
        "macos-latest",
        "macos-14",
        "macos-15",
        "macos-26",
        "macos-14-xlarge",
        "macos-15-arm64",
        "macos-latest-xlarge",
    ] {
        assert!(is_arm64_macos_label(ok), "{ok}");
    }
    for no in [
        "macos-13",
        "macos-12",
        "macos-14-large",
        "macos-latest-large",
        "macos-15-intel",
        "macos-13-xlarge-x",
        "macos-",
        "macos",
        "ubuntu-latest",
        "windows-latest",
        "macos-1x",
        "macos-14.1",
        "",
    ] {
        assert!(!is_arm64_macos_label(no), "{no}");
    }
}

proptest! {
    #[test]
    fn numeric_macos_labels_follow_the_14_threshold(v in 0u32..200) {
        prop_assert_eq!(is_arm64_macos_label(&format!("macos-{v}")), v >= 14);
        let large = format!("macos-{v}-large");
        prop_assert!(!is_arm64_macos_label(&large));
        prop_assert_eq!(runner_os(&format!("macos-{v}")), Some(Os::Macos));
    }

    #[test]
    fn classifiers_never_panic_and_agree(s in "\\PC{0,40}") {
        if is_arm64_macos_label(&s) {
            prop_assert_eq!(runner_os(&s), Some(Os::Macos));
        }
        let _ = selected_packages(&s);
        let _ = excluded_packages(&s);
        let _ = cargo_lines(&s, "test");
    }

    #[test]
    fn parser_survives_random_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let text = String::from_utf8_lossy(&bytes);
        if let Ok(wf) = Workflow::parse(&text) {
            for j in wf.jobs() {
                let _ = (j.runner_labels(), j.script(), j.timeout_minutes(), j.uses());
            }
            let _ = (wf.triggers(), wf.concurrency());
        }
    }

    #[test]
    fn package_flags_round_trip(pkgs in proptest::collection::vec("[a-z][a-z0-9_]{0,12}", 0..6), form in 0u8..3) {
        let args: Vec<String> = pkgs.iter().map(|p| match form {
            0 => format!("-p {p}"),
            1 => format!("--package={p}"),
            _ => format!("--package {p}"),
        }).collect();
        let line = format!("cargo test {} -- --nocapture", args.join(" "));
        prop_assert_eq!(selected_packages(&line), pkgs.clone());
        let ex: Vec<String> = pkgs.iter().map(|p| format!("--exclude {p}")).collect();
        prop_assert_eq!(excluded_packages(&format!("cargo test --workspace {}", ex.join(" "))), pkgs);
    }

    #[test]
    fn normalize_collapses_blank_runs(s in "[ -~\\n\\t]{0,120}") {
        let out = normalize_script(&s);
        prop_assert!(!out.contains('\t') && !out.contains("  "));
        prop_assert!(out.lines().all(|l| l == l.trim()));
    }
}
