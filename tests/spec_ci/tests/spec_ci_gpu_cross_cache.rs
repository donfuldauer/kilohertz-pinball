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
    assert_eq!(
        steps[build]["timeout-minutes"].as_i64(),
        Some(BUILD_TIMEOUT_MIN),
        "build step timeout"
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
    for i in [build, budget, test] {
        assert_eq!(str_of(&steps[i], "if"), Some("runner.os != 'Linux'"));
    }
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
        str_of(env, "JOB_START"),
        Some("${{ steps.start.outputs.epoch }}")
    );
    assert_eq!(
        str_of(env, "CACHE_HIT"),
        Some("${{ steps.rust-cache.outputs.cache-hit }}")
    );
}

/// Runs the budget step's script in bash with `date` fixed at `now` and returns the
/// `minutes=` value it writes to `$GITHUB_OUTPUT`.
fn run_budget(start: u64, now: u64, cache_hit: &str) -> i64 {
    let steps = steps();
    let (_, budget) = step_by_id(&steps, "budget");
    let script = str_of(budget, "run")
        .expect("budget has a script")
        .to_owned();
    let timeout = str_of(&budget["env"], "JOB_TIMEOUT_MIN")
        .unwrap()
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
    let status = Command::new("bash")
        .args(["--noprofile", "--norc", "-eo", "pipefail", "-c", &script])
        .env("PATH", path)
        .env("GITHUB_OUTPUT", &output)
        .env("JOB_START", start.to_string())
        .env("JOB_TIMEOUT_MIN", timeout)
        .env("CACHE_HIT", cache_hit)
        .status()
        .expect("bash runs");
    assert!(status.success(), "budget script failed");
    let out = std::fs::read_to_string(&output).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    out.lines()
        .find_map(|l| l.strip_prefix("minutes="))
        .expect("budget writes minutes=")
        .trim()
        .parse()
        .expect("minutes is an integer")
}

#[test]
fn gpu_cross_cache_budget_leaves_time_for_the_cache_save() {
    const T0: u64 = 1_000_000;
    // Cold run (no exact cache hit), build finished 30 min after the first step:
    // elapsed rounds to 30 + 1 min set-up allowance, reserve 8 -> 45 - 8 - 31 = 6.
    assert_eq!(run_budget(T0, T0 + 30 * 60, "false"), 6);
    // A partial minute rounds up: 30 min 1 s counts as 31 (+1) -> 5.
    assert_eq!(run_budget(T0, T0 + 30 * 60 + 1, ""), 5);
    // Warm run (exact hit, save skipped): 12 min used, reserve 1 -> 45 - 1 - 13 = 31,
    // which covers the 15.5-minute test run.
    assert_eq!(run_budget(T0, T0 + 12 * 60, "true"), 31);
    // Out of time: the step still gets the 1-minute minimum GitHub accepts.
    assert_eq!(run_budget(T0, T0 + 44 * 60, "false"), 1);
    // Every cold-run budget that can still fit ends at least 8 minutes before the job cap.
    for used in 0..=35u64 {
        let minutes = run_budget(T0, T0 + used * 60, "false");
        assert!(minutes >= 1);
        assert!(
            (used as i64) + 1 + minutes <= 45 - 8,
            "used {used} min, test step gets {minutes} min"
        );
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
