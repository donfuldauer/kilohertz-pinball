//! Issue #205 (KP-099), SPEC §8 (CI): the `gpu-tests-cross` job must finish, including the
//! `Swatinem/rust-cache@v2` post-step save, inside the 45-minute job cap (DECISIONS.md
//! "#205 KP-099 — gpu-tests-cross build budget and robust cache save").
//!
//! Checked against `.github/workflows/ci.yml`: the cap stays 45 minutes, debug info is
//! line-tables-only, the build (`cargo test --no-run`) is a bounded step before the test run,
//! the cache key is the pinned compiler plus `Cargo.lock` and is saved on failure, and the test
//! step's timeout leaves a reserve for the cache save. The budget script is run in bash with a
//! fixed clock.

use std::path::PathBuf;
use std::process::Command;

use spec_ci::*;
use yaml_rust2::{Yaml, YamlLoader};

const JOB: &str = "gpu-tests-cross";
const GPU_TEST: &str = "cargo test -p pin_render --features gpu-tests -p pin_audio -p pin_app";
const BUILD_TIMEOUT_MIN: i64 = 32;
const MIN_TEST_MIN: i64 = 10;
// Step timeouts never exceed the job cap.
const _: () = assert!(BUILD_TIMEOUT_MIN <= 45);

fn key(k: &str) -> Yaml {
    Yaml::String(k.to_owned())
}

/// The `gpu-tests-cross` job node, parsed independently of the `spec_ci` helpers.
fn job_node() -> Yaml {
    let docs = YamlLoader::load_from_str(&workflow_text("ci.yml")).expect("ci.yml parses");
    docs[0]["jobs"][JOB].clone()
}

fn steps() -> Vec<Yaml> {
    job_node()["steps"]
        .as_vec()
        .expect("gpu-tests-cross has steps")
        .clone()
}

fn str_of<'a>(node: &'a Yaml, k: &str) -> Option<&'a str> {
    node.as_hash()?.get(&key(k))?.as_str()
}

/// Index of the step whose `run:` (whitespace-normalised) equals `cmd`.
fn run_step_index(steps: &[Yaml], cmd: &str) -> usize {
    steps
        .iter()
        .position(|s| str_of(s, "run").is_some_and(|r| normalize_script(r).trim() == cmd))
        .unwrap_or_else(|| panic!("{JOB} has no step running `{cmd}`"))
}

fn step_by_id<'a>(steps: &'a [Yaml], id: &str) -> (usize, &'a Yaml) {
    steps
        .iter()
        .enumerate()
        .find(|(_, s)| str_of(s, "id") == Some(id))
        .unwrap_or_else(|| panic!("{JOB} has no step with id `{id}`"))
}

#[test]
fn gpu_cross_cache_job_timeout_stays_45_minutes() {
    let j = Workflow::load("ci.yml").job(JOB).expect("job present");
    assert_eq!(
        j.timeout_minutes(),
        Some(45),
        "#205 keeps the 45-minute cap"
    );
}

#[test]
fn gpu_cross_cache_debug_info_is_line_tables_only() {
    let env = &job_node()["env"];
    for var in ["CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG"] {
        assert_eq!(
            str_of(env, var),
            Some("line-tables-only"),
            "{JOB} env {var}"
        );
    }
    // The existing adapter settings stay, and no opt-level override sneaks in.
    assert_eq!(str_of(env, "PIN_REQUIRE_GPU"), Some("1"));
    assert!(str_of(env, "WGPU_BACKEND").is_some());
    let hash = env.as_hash().expect("env is a map");
    assert!(
        !hash
            .keys()
            .filter_map(Yaml::as_str)
            .any(|k| k.contains("OPT_LEVEL")),
        "#205 must not change opt-level"
    );
}

#[test]
fn gpu_cross_cache_build_step_precedes_bounded_test_run() {
    let steps = steps();
    let build = run_step_index(&steps, &format!("{GPU_TEST} --no-run"));
    let test = run_step_index(&steps, GPU_TEST);
    assert!(
        build < test,
        "the --no-run build must come before the test run"
    );
    // Gate #163 round 13: the build timeout is computed by `build_budget`, not a literal.
    assert_eq!(
        str_of(&steps[build], "timeout-minutes"),
        Some("${{ fromJSON(steps.build_budget.outputs.minutes) }}"),
        "build step timeout comes from the build_budget step"
    );
    let (start, _) = step_by_id(&steps, "start");
    let (build_budget, _) = step_by_id(&steps, "build_budget");
    assert!(
        start < build_budget && build_budget < build,
        "build_budget is computed after `start` and before the build"
    );
    let (budget, _) = step_by_id(&steps, "budget");
    assert!(
        build < budget && budget < test,
        "budget is computed after the build"
    );
    assert_eq!(
        str_of(&steps[test], "timeout-minutes"),
        Some("${{ fromJSON(steps.budget.outputs.minutes) }}"),
        "the test run is bounded by the computed budget"
    );
    for i in [build_budget, build, budget, test] {
        assert_eq!(str_of(&steps[i], "if"), Some("runner.os != 'Linux'"));
    }
}

/// Gate #163 round 13: the build step's cap is min(BUILD_TIMEOUT_MIN, job cap - reserve -
/// elapsed), using the same reserve/elapsed formula as the test-step budget.
#[test]
fn gpu_cross_cache_build_budget_clamps_to_the_job_cap() {
    let steps = steps();
    let (_, bb) = step_by_id(&steps, "build_budget");
    let (_, tb) = step_by_id(&steps, "budget");
    let env = &bb["env"];
    assert_eq!(
        str_of(env, "MAX_BUILD_MIN"),
        Some(BUILD_TIMEOUT_MIN.to_string().as_str()),
        "the build never gets more than the old fixed cap"
    );
    let cap = Workflow::load("ci.yml").job(JOB).unwrap().timeout_minutes();
    assert_eq!(
        str_of(env, "JOB_TIMEOUT_MIN").and_then(|v| v.parse::<i64>().ok()),
        cap,
        "build_budget uses the job's own timeout"
    );
    assert_eq!(
        str_of(env, "JOB_START"),
        Some("${{ steps.start.outputs.epoch }}")
    );
    assert_eq!(
        str_of(env, "CACHE_HIT"),
        Some("${{ steps.rust-cache.outputs.cache-hit }}")
    );
    let script = str_of(bb, "run").expect("build_budget has a script");
    let test_script = str_of(tb, "run").expect("budget has a script");
    let lines: Vec<&str> = script.lines().map(str::trim).collect();
    let test_lines: Vec<&str> = test_script.lines().map(str::trim).collect();
    for formula in [
        r#"if [ "$CACHE_HIT" = "true" ]; then reserve=1; else reserve=8; fi"#,
        "elapsed=$(( ($(date +%s) - JOB_START + 59) / 60 + 1 ))",
        "minutes=$(( JOB_TIMEOUT_MIN - reserve - elapsed ))",
    ] {
        assert!(lines.contains(&formula), "build_budget lacks `{formula}`");
        assert!(test_lines.contains(&formula), "budget lacks `{formula}`");
    }
    let clamp = r#"if [ "$minutes" -gt "$MAX_BUILD_MIN" ]; then minutes=$MAX_BUILD_MIN; fi"#;
    let pos = |l: &str| lines.iter().position(|x| *x == l);
    let calc = pos("minutes=$(( JOB_TIMEOUT_MIN - reserve - elapsed ))").unwrap();
    assert!(
        pos(clamp).is_some_and(|c| c > calc),
        "the MAX_BUILD_MIN clamp follows the formula"
    );
    assert!(script.contains(r#"echo "minutes=$minutes" >> "$GITHUB_OUTPUT""#));
}

#[test]
fn gpu_cross_cache_rust_cache_saves_on_failure_with_a_pinned_key() {
    let steps = steps();
    let (cache, node) = step_by_id(&steps, "rust-cache");
    assert_eq!(str_of(node, "uses"), Some("Swatinem/rust-cache@v2"));
    let with = &node["with"];
    assert_eq!(with["cache-on-failure"].as_bool(), Some(true));
    assert_eq!(with["add-rust-environment-hash-key"].as_bool(), Some(false));
    assert_eq!(
        str_of(with, "key"),
        Some("rustc-${{ env.RUST_VERSION }}-${{ hashFiles('Cargo.lock') }}")
    );
    let (start, _) = step_by_id(&steps, "start");
    assert_eq!(start, 0, "the job start time is recorded by the first step");
    let build = run_step_index(&steps, &format!("{GPU_TEST} --no-run"));
    assert!(cache < build, "cache is restored before the build");
}

#[test]
fn gpu_cross_cache_budget_constants_match_the_job() {
    let steps = steps();
    let (_, budget) = step_by_id(&steps, "budget");
    let env = &budget["env"];
    let cap = Workflow::load("ci.yml").job(JOB).unwrap().timeout_minutes();
    assert_eq!(
        str_of(env, "JOB_TIMEOUT_MIN").and_then(|v| v.parse::<i64>().ok()),
        cap,
        "the budget uses the job's own timeout"
    );
    assert_eq!(
        str_of(env, "MIN_TEST_MIN").and_then(|v| v.parse::<i64>().ok()),
        Some(MIN_TEST_MIN),
        "the budget declares the minimum useful test time"
    );
    assert_eq!(
        str_of(env, "JOB_START"),
        Some("${{ steps.start.outputs.epoch }}")
    );
    assert_eq!(
        str_of(env, "CACHE_HIT"),
        Some("${{ steps.rust-cache.outputs.cache-hit }}")
    );
}

/// Outcome of the budget script: exit success, the `minutes=` lines written, and stdout.
struct Budget {
    success: bool,
    minutes: Vec<i64>,
    stdout: String,
}

/// Runs the budget step's script in bash with `date` fixed at `now`.
fn run_budget(start: u64, now: u64, cache_hit: &str) -> Budget {
    let steps = steps();
    let (_, budget) = step_by_id(&steps, "budget");
    let script = str_of(budget, "run")
        .expect("budget has a script")
        .to_owned();
    let timeout = str_of(&budget["env"], "JOB_TIMEOUT_MIN")
        .unwrap()
        .to_owned();
    let min_test = str_of(&budget["env"], "MIN_TEST_MIN")
        .expect("budget declares MIN_TEST_MIN")
        .to_owned();

    let dir: PathBuf = std::env::temp_dir().join(format!(
        "spec_ci_gpu_cross_cache_{}_{start}_{now}_{cache_hit}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let fake_date = dir.join("date");
    std::fs::write(&fake_date, format!("#!/bin/sh\necho {now}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_date, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = dir.join("output");
    std::fs::write(&output, "").unwrap();
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let res = Command::new("bash")
        .args(["--noprofile", "--norc", "-eo", "pipefail", "-c", &script])
        .env("PATH", path)
        .env("GITHUB_OUTPUT", &output)
        .env("JOB_START", start.to_string())
        .env("JOB_TIMEOUT_MIN", timeout)
        .env("MIN_TEST_MIN", min_test)
        .env("CACHE_HIT", cache_hit)
        .output()
        .expect("bash runs");
    let out = std::fs::read_to_string(&output).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    Budget {
        success: res.status.success(),
        minutes: out
            .lines()
            .filter_map(|l| l.strip_prefix("minutes="))
            .map(|v| v.trim().parse().expect("minutes is an integer"))
            .collect(),
        stdout: String::from_utf8_lossy(&res.stdout).into_owned(),
    }
}

/// Expects a successful run writing exactly one `minutes=` value.
fn minutes_of(start: u64, now: u64, cache_hit: &str) -> i64 {
    let b = run_budget(start, now, cache_hit);
    assert!(b.success, "budget script failed: {}", b.stdout);
    assert_eq!(b.minutes.len(), 1, "one minutes= line");
    b.minutes[0]
}

#[test]
fn gpu_cross_cache_budget_leaves_time_for_the_cache_save() {
    const T0: u64 = 1_000_000;
    // (a) Cold run (no exact cache hit), 25 min after the first step: elapsed rounds to
    // 25 + 1 min set-up allowance, reserve 8 -> 45 - 8 - 26 = 11.
    assert_eq!(minutes_of(T0, T0 + 25 * 60, "false"), 11);
    // A partial minute rounds up: 25 min 1 s counts as 26 (+1) -> 10, exactly MIN_TEST_MIN.
    assert_eq!(minutes_of(T0, T0 + 25 * 60 + 1, ""), 10);
    // Warm run (exact hit, save skipped): 12 min used, reserve 1 -> 45 - 1 - 13 = 31,
    // which covers the 15.5-minute test run.
    assert_eq!(minutes_of(T0, T0 + 12 * 60, "true"), 31);
    // (b) Too little time left: the step fails before the test step (non-zero exit), writes
    // no `minutes=` line and reports the reason. Previously the clamp wrote minutes=1 here
    // (assert_eq!(run_budget(T0, T0 + 44 * 60, "false"), 1)), eating into the 8-minute reserve.
    for (used, hit) in [(44u64, "false"), (30, "false"), (27, ""), (40, "true")] {
        let b = run_budget(T0, T0 + used * 60, hit);
        assert!(!b.success, "used {used} min, hit {hit:?} must fail");
        assert!(b.minutes.is_empty(), "no minutes= below the threshold");
        assert!(b.stdout.contains("::error::only "), "stdout: {}", b.stdout);
        assert!(b.stdout.contains("min left for the tests"));
    }
    // (c) The reserve is never reduced: every value written ends at least `reserve` minutes
    // before the job cap, and is at least MIN_TEST_MIN.
    for (hit, reserve) in [("false", 8i64), ("true", 1)] {
        for used in 0..=60u64 {
            let b = run_budget(T0, T0 + used * 60, hit);
            for m in &b.minutes {
                assert!(*m >= MIN_TEST_MIN);
                assert!(
                    (used as i64) + 1 + m + reserve <= 45,
                    "used {used} min, reserve {reserve}, test step gets {m} min"
                );
            }
            assert_eq!(b.success, !b.minutes.is_empty(), "used {used} min");
            assert_eq!(
                b.success,
                45 - reserve - (used as i64 + 1) >= MIN_TEST_MIN,
                "used {used} min, reserve {reserve}"
            );
        }
    }
}

#[test]
fn gpu_cross_cache_test_run_keeps_full_gpu_coverage() {
    let j = Workflow::load("ci.yml").job(JOB).unwrap();
    let script = j.script();
    let runs: Vec<&str> = cargo_lines(&script, "test")
        .into_iter()
        .filter(|l| !l.contains("--no-run"))
        .collect();
    assert_eq!(runs.len(), 1, "one test-run line: {runs:?}");
    assert_eq!(
        selected_packages(runs[0]),
        ["pin_render", "pin_audio", "pin_app"]
    );
    assert!(runs[0].contains("--features gpu-tests"));
}
