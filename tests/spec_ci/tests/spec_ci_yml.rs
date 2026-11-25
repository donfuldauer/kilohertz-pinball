//! Subphase 1.2 brief, checked against `.github/workflows/ci.yml`.

use spec_ci::*;

const CORE: [&str; 7] = [
    "pin_core",
    "pin_physics",
    "pin_input",
    "pin_script",
    "pin_assets",
    "pin_tablegen",
    "pin_sim",
];
const GPU: [&str; 3] = ["pin_render", "pin_audio", "pin_app"];
const REQUIRED_JOBS: [&str; 5] = ["core-tests", "gpu-tests", "clippy", "fmt", "replay-hash"];

fn ci() -> Workflow {
    Workflow::load("ci.yml")
}

fn job(id: &str) -> Job {
    ci().job(id)
        .unwrap_or_else(|| panic!("ci.yml has no job `{id}`"))
}

/// Jobs that are split into a Linux job (`<id>`, PR and push) and a cross-OS job
/// (`<id>-cross`, Windows and macOS, never on pull requests) (D40).
const SPLIT_JOBS: [&str; 4] = ["core-tests", "gpu-tests", "clippy", "replay-hash"];

/// The Linux job and the cross-OS job of a split family.
fn family(id: &str) -> [Job; 2] {
    [job(id), job(&format!("{id}-cross"))]
}

/// Whether a job-level `if:` excludes pull requests.
fn skips_pull_requests(j: &Job) -> bool {
    j.condition()
        .is_some_and(|c| c.contains("github.event_name != 'pull_request'"))
}

#[test]
fn parses_as_a_single_workflow_document() {
    ci();
}

#[test]
fn has_no_tabs_and_ends_with_newline() {
    let text = workflow_text("ci.yml");
    assert!(!text.contains('\t'), "tabs are invalid YAML indentation");
    assert!(text.ends_with('\n'));
}

#[test]
fn triggers_on_push_and_only_known_events() {
    let t = ci().triggers();
    assert!(t.iter().any(|e| e == "push"), "push trigger missing: {t:?}");
    assert!(t.iter().any(|e| e == "pull_request"), "{t:?}");
    assert!(t.iter().any(|e| e == "workflow_dispatch"), "{t:?}");
    for e in &t {
        assert!(
            [
                "push",
                "pull_request",
                "workflow_dispatch",
                "merge_group",
                "schedule"
            ]
            .contains(&e.as_str()),
            "unexpected trigger {e}"
        );
    }
}

#[test]
fn push_runs_only_for_main() {
    // D40: the full CI runs on pushes to main; PR branches are covered by pull_request.
    assert_eq!(
        ci().trigger_branches("push"),
        Some(vec!["main".to_owned()]),
        "push must be limited to `branches: [main]`"
    );
}

#[test]
fn required_jobs_exist() {
    let ids: Vec<String> = ci().jobs().into_iter().map(|j| j.id).collect();
    for want in REQUIRED_JOBS {
        assert!(
            ids.iter().any(|i| i == want),
            "job `{want}` missing; have {ids:?}"
        );
    }
    for id in SPLIT_JOBS {
        let cross = format!("{id}-cross");
        assert!(ids.contains(&cross), "job `{cross}` missing; have {ids:?}");
    }
    for want in ["changes", "coverage", "dof-bridge", "gate-a1"] {
        assert!(ids.iter().any(|i| i == want), "job `{want}` missing");
    }
}

#[test]
fn build_and_test_jobs_cover_all_three_platforms() {
    for id in SPLIT_JOBS {
        let [lin, cross] = family(id);
        for os in Os::ALL {
            assert!(
                lin.covers(os) || cross.covers(os),
                "{id} does not run on {os:?} (Linux job {:?}, cross job {:?})",
                lin.runner_labels(),
                cross.runner_labels()
            );
        }
        // The split itself: Linux job is Linux only, the cross job is Windows + macOS only.
        assert!(lin.covers(Os::Linux), "{id} must run on Linux");
        assert!(
            !lin.covers(Os::Windows) && !lin.covers(Os::Macos),
            "{id}: the Linux job must not run on Windows or macOS"
        );
        assert!(
            cross.covers(Os::Windows) && cross.covers(Os::Macos) && !cross.covers(Os::Linux),
            "{id}-cross must run on Windows and macOS only"
        );
    }
}

#[test]
fn pull_requests_run_only_the_linux_set() {
    // D40: every Windows/macOS job and coverage is skipped on pull_request (job-level `if`,
    // so a skipped job reports success), and no Linux job of the split families is.
    for id in SPLIT_JOBS {
        let [lin, cross] = family(id);
        assert!(
            skips_pull_requests(&cross),
            "{id}-cross must skip pull requests: {:?}",
            cross.condition()
        );
        assert!(
            !skips_pull_requests(&lin),
            "{id} (Linux) must run on pull requests: {:?}",
            lin.condition()
        );
    }
    assert!(
        !skips_pull_requests(&job("fmt")),
        "fmt must run on pull requests"
    );
    assert!(
        skips_pull_requests(&job("coverage")),
        "coverage must skip pull requests"
    );
    // Any other job on a non-Linux runner (dof-bridge, path-gated) is exempt from the blanket rule.
    for j in ci().jobs() {
        if (j.covers(Os::Windows) || j.covers(Os::Macos)) && j.id != "dof-bridge" {
            assert!(
                skips_pull_requests(&j),
                "job `{}` runs on Windows/macOS but is not skipped on pull requests",
                j.id
            );
        }
    }
    // A guard that could run a job after an upstream failure would defeat the split.
    // The one exception is the `pr-checks` aggregator, which must run after any failure.
    for j in ci().jobs().into_iter().filter(|j| j.id != "pr-checks") {
        if let Some(c) = j.condition() {
            assert!(!c.contains("always()"), "job `{}`: {c}", j.id);
        }
    }
}

#[test]
fn path_gated_jobs_follow_the_changes_outputs() {
    let gpu = job("gpu-tests").condition().unwrap_or_default();
    assert!(
        gpu.contains("needs.changes.outputs.code == 'true'")
            && gpu.contains("needs.changes.outputs.gpu == 'true'"),
        "gpu-tests (lavapipe) must be gated on the gpu output: {gpu}"
    );
    let dof = job("dof-bridge").condition().unwrap_or_default();
    assert!(
        dof.contains("needs.changes.outputs.dof == 'true'"),
        "dof-bridge must be gated on the dof output: {dof}"
    );
    let text = job_text("changes");
    for path in [
        "crates/pin_render/",
        "crates/pin_app/",
        "*.wgsl",
        "tools/dof_bridge/",
    ] {
        assert!(text.contains(path), "changes job does not look at {path}");
    }
    // Fail closed: gpu and dof start true and are only cleared for a PR with a file list.
    assert!(
        text.contains("gpu=true\n          dof=true"),
        "gpu and dof must default to true (fail closed)"
    );
    assert!(
        text.contains("git diff -z --name-only --no-renames") && text.contains("read -r -d ''"),
        "changes step must read NUL-separated paths (git diff -z) so quoted paths cannot miss globs"
    );
    assert!(
        text.contains("docs/SPEC.md) code=true"),
        "docs-only skip must stay"
    );
}

#[test]
fn macos_runner_is_apple_silicon() {
    for id in [
        "core-tests-cross",
        "gpu-tests-cross",
        "clippy-cross",
        "replay-hash-cross",
    ] {
        let labels = job(id).runner_labels();
        let mac: Vec<_> = labels
            .iter()
            .filter(|l| runner_os(l) == Some(Os::Macos))
            .collect();
        assert!(
            mac.iter().any(|l| is_arm64_macos_label(l)),
            "{id}: no aarch64-apple-darwin runner among {mac:?}"
        );
    }
}

#[test]
fn every_job_has_a_timeout_of_at_most_45_minutes() {
    for j in ci().jobs() {
        let t = j
            .timeout_minutes()
            .unwrap_or_else(|| panic!("job `{}` lacks integer timeout-minutes", j.id));
        assert!(
            (1..=45).contains(&t),
            "job `{}` timeout-minutes = {t}",
            j.id
        );
    }
}

#[test]
fn every_job_has_a_runner_and_steps() {
    for j in ci().jobs() {
        assert!(
            !j.runner_labels().is_empty(),
            "job `{}` names no hosted runner",
            j.id
        );
        assert!(j.step_count() > 0, "job `{}` has no steps", j.id);
    }
}

#[test]
fn concurrency_cancels_superseded_runs_per_ref() {
    let wf = ci();
    let groups: Vec<Concurrency> = match wf.concurrency() {
        Some(c) => vec![c],
        None => wf
            .jobs()
            .iter()
            .map(|j| {
                j.concurrency()
                    .unwrap_or_else(|| panic!("no concurrency (workflow or job `{}`)", j.id))
            })
            .collect(),
    };
    for c in groups {
        assert!(
            c.is_per_ref(),
            "concurrency group `{}` is not keyed on the ref",
            c.group
        );
        assert!(
            c.is_per_event(),
            "concurrency group `{}` does not include github.event_name",
            c.group
        );
        assert!(
            c.cancel_in_progress,
            "cancel-in-progress not enabled for `{}`",
            c.group
        );
    }
}

#[test]
fn main_push_runs_are_never_cancelled() {
    // D40: the full matrix exists only on main push / workflow_dispatch runs, so only
    // pull_request runs may be cancelled by a newer run.
    let raw = ci()
        .concurrency_cancel_raw()
        .expect("ci.yml has no workflow-level concurrency.cancel-in-progress");
    assert_ne!(raw, "true", "cancel-in-progress is a literal true");
    assert_eq!(
        raw, "${{ github.event_name == 'pull_request' }}",
        "only pull_request runs may be cancelled"
    );
}

#[test]
fn core_tests_runs_every_gpu_free_crate_and_no_gpu_crate() {
    for j in family("core-tests") {
        core_tests_job_runs_every_gpu_free_crate(&j);
    }
}

fn core_tests_job_runs_every_gpu_free_crate(j: &Job) {
    let script = j.script();
    let lines = cargo_lines(&script, "test");
    assert!(
        !lines.is_empty(),
        "{} never runs `cargo test`:\n{script}",
        j.id
    );

    let mut covered: Vec<String> = Vec::new();
    for l in &lines {
        if l.contains("--manifest-path") {
            // A standalone package outside the root workspace (e.g. this spec crate).
            assert!(
                !GPU.iter().any(|g| l.contains(g)),
                "core-tests must stay GPU-free: {l}"
            );
            continue;
        }
        if l.split_whitespace()
            .any(|t| t == "--workspace" || t == "--all")
        {
            let ex = excluded_packages(l);
            for g in GPU {
                assert!(
                    ex.iter().any(|e| e == g),
                    "workspace test does not exclude {g}: {l}"
                );
            }
            for c in CORE {
                assert!(
                    !ex.iter().any(|e| e == c),
                    "core crate {c} is excluded: {l}"
                );
            }
            covered.extend(CORE.iter().map(|c| (*c).to_owned()));
        } else {
            let sel = selected_packages(l);
            assert!(
                !sel.is_empty(),
                "cargo test with neither --workspace nor -p: {l}"
            );
            for g in GPU {
                assert!(
                    !sel.iter().any(|s| s == g),
                    "core-tests must stay GPU-free but tests {g}: {l}"
                );
            }
            covered.extend(sel);
        }
    }
    for c in CORE {
        assert!(
            covered.iter().any(|x| x == c),
            "core-tests does not test {c}:\n{script}"
        );
    }
}

#[test]
fn core_tests_linux_also_runs_the_spec_ci_steps() {
    // The workflow conformance tests run in the PR set, so in the Linux core-tests job.
    let script = job("core-tests").script();
    assert!(
        cargo_lines(&script, "test")
            .iter()
            .any(|l| l.contains("--manifest-path tests/spec_ci/Cargo.toml")),
        "core-tests (Linux) must run the spec_ci tests:\n{script}"
    );
    assert!(
        cargo_lines(&script, "clippy")
            .iter()
            .any(|l| l.contains("tests/spec_ci/Cargo.toml") && l.contains("-D warnings")),
        "core-tests (Linux) must lint spec_ci:\n{script}"
    );
}

#[test]
fn gpu_tests_runs_render_audio_app() {
    for id in ["gpu-tests", "gpu-tests-cross"] {
        gpu_job_runs_render_audio_app(&job(id));
    }
}

#[test]
fn gpu_tests_cover_every_adapter_across_the_split() {
    let text = format!("{}{}", job_text("gpu-tests"), job_text("gpu-tests-cross"));
    for adapter in ["adapter: lavapipe", "adapter: WARP", "adapter: Metal"] {
        assert!(text.contains(adapter), "no gpu-tests leg for {adapter}");
    }
    assert!(job_text("gpu-tests").contains("adapter: lavapipe"));
    assert!(!job_text("gpu-tests").contains("adapter: WARP"));
    assert!(!job_text("gpu-tests").contains("adapter: Metal"));
}

fn gpu_job_runs_render_audio_app(j: &Job) {
    let script = j.script();
    let lines = cargo_lines(&script, "test");
    assert!(
        !lines.is_empty(),
        "{} never runs `cargo test`:\n{script}",
        j.id
    );
    let sel: Vec<String> = lines.iter().flat_map(|l| selected_packages(l)).collect();
    for g in GPU {
        assert!(
            sel.iter().any(|s| s == g),
            "gpu-tests does not test {g}:\n{script}"
        );
    }
}

#[test]
fn gpu_tests_installs_software_vulkan_and_xvfb_on_linux() {
    let script = job("gpu-tests").script();
    for pkg in ["mesa-vulkan-drivers", "xvfb"] {
        assert!(script.contains(pkg), "gpu-tests does not install {pkg}");
    }
}

#[test]
fn clippy_denies_warnings_across_the_workspace() {
    for id in ["clippy", "clippy-cross"] {
        clippy_job_denies_warnings(&job(id));
    }
}

fn clippy_job_denies_warnings(j: &Job) {
    let script = j.script();
    let lines = cargo_lines(&script, "clippy");
    assert!(
        !lines.is_empty(),
        "{} never runs `cargo clippy`:\n{script}",
        j.id
    );
    assert!(
        lines.iter().any(|l| {
            let (cargo_args, lint_args) = l.split_once(" -- ").unwrap_or((l, ""));
            cargo_args.contains("--workspace")
                && cargo_args.contains("--all-targets")
                && lint_args.contains("-D warnings")
        }),
        "want `cargo clippy --workspace --all-targets -- -D warnings`, got {lines:?}"
    );
}

#[test]
fn fmt_checks_whole_workspace_on_linux_only() {
    let j = job("fmt");
    assert!(j.covers(Os::Linux), "fmt must run on Linux");
    assert!(
        !j.covers(Os::Windows) && !j.covers(Os::Macos),
        "fmt is Linux only: {:?}",
        j.runner_labels()
    );
    let script = j.script();
    let lines = cargo_lines(&script, "fmt");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("--all") && l.contains("--check")),
        "want `cargo fmt --all --check`, got {lines:?}"
    );
}

/// A job's text (from its key to the next job key), for the job-level
/// settings the parsed `Job` does not expose (env, strategy).
fn job_text(id: &str) -> String {
    let text = workflow_text("ci.yml");
    let key = format!("\n  {id}:");
    let start = text.find(&key).expect("job key present in text") + 1;
    let mut end = start;
    for (i, line) in text[start..].split_inclusive('\n').enumerate() {
        let next_job = i > 0
            && line.starts_with("  ")
            && line[2..].starts_with(|c: char| !c.is_whitespace() && c != '#');
        if next_job {
            break;
        }
        end += line.len();
    }
    text[start..end].to_owned()
}

/// The committed x86_64-linux golden of the replay-hash fixture (D9, KP-069).
const GOLDEN: &str = "crates/pin_sim/tests/fixtures/replay_table.golden";
/// The one place the replay-hash legs share the record / replay / golden logic (KP-069).
const ASSERT_SCRIPT: &str = "scripts/ci/replay_hash_assert.sh";

fn assert_script_text() -> String {
    std::fs::read_to_string(repo_root().join(ASSERT_SCRIPT))
        .unwrap_or_else(|e| panic!("{ASSERT_SCRIPT}: {e}"))
}

/// Subphase 2.5 filled the 1.2 placeholder: the golden replay-hash check (D9). KP-069 (SPEC §0
/// row D9, §2.4; PROCESS R3): every leg also asserts the committed x86_64-linux golden.
#[test]
fn replay_hash_records_replays_and_asserts_the_golden_on_every_os() {
    let [lin, cross] = family("replay-hash");
    for os in Os::ALL {
        assert!(
            lin.covers(os) || cross.covers(os),
            "replay-hash must run on {os:?}"
        );
    }
    for (j, text_id) in [
        (lin, "replay-hash"),
        (cross, "replay-hash-cross"),
        (job("replay-hash-aarch64"), "replay-hash-aarch64"),
    ] {
        replay_hash_job_checks(&j, text_id);
    }
    // No job compares one leg's output with another's: each leg compares with the committed
    // golden. Only the pr-checks aggregator may list a replay-hash job (it reads results, not
    // hashes), no replay-hash job exports outputs, and no expression reads one.
    for j in ci().jobs().into_iter().filter(|j| j.id != "pr-checks") {
        assert!(
            !j.needs().iter().any(|n| n.starts_with("replay-hash")),
            "job `{}` consumes a replay-hash job via needs: {:?}",
            j.id,
            j.needs()
        );
    }
    for id in ["replay-hash", "replay-hash-cross", "replay-hash-aarch64"] {
        assert!(
            !job_text(id).contains("\n    outputs:"),
            "{id} must not export the hash as a job output"
        );
    }
    let all = workflow_text("ci.yml");
    assert!(
        !all.contains("needs.replay-hash"),
        "no expression may read a replay-hash job's outputs"
    );
}

fn replay_hash_job_checks(j: &Job, text_id: &str) {
    let script = j.script();
    let builds = cargo_lines(&script, "build");
    assert!(
        builds
            .iter()
            .any(|l| l.contains("--release") && selected_packages(l).iter().any(|p| p == "pin_sim")),
        "want `cargo build --release -p pin_sim`, got {builds:?}"
    );
    let text = job_text(text_id);
    // Record, 10 replays and the golden comparison live in the assert script; each leg runs it.
    assert!(
        script.contains(&format!("bash {ASSERT_SCRIPT} target/")),
        "{text_id} must run {ASSERT_SCRIPT} on its pin_sim:\n{script}"
    );
    assert!(
        text.contains("FIXTURE: crates/pin_sim/tests/fixtures/replay_table")
            && (text.contains("TICKS: \"10000\"") || text.contains("TICKS: \"10_000\"")),
        "{text_id} must record the fixture for 10 000 ticks:\n{text}"
    );
    // A matrix job runs every OS leg to the end: one failing OS must not cancel the others
    // (the old `fail-fast: false` assertion, restored under KP-069). Jobs without a
    // `strategy.matrix` (replay-hash-aarch64) have nothing to cancel and are not checked.
    if text.contains("\n    strategy:") && text.contains("\n      matrix:") {
        assert!(
            text.contains("fail-fast: false"),
            "one OS must not cancel the others: {text_id} must set `fail-fast: false`:\n{text}"
        );
    }
    // Seam digests: the test runs with --nocapture and its printed lines are checked.
    let seam = cargo_lines(&script, "test");
    assert!(
        seam.iter()
            .any(|l| selected_packages(l).iter().any(|p| p == "pin_physics")
                && l.contains("--test ball_seam_bitident")
                && l.contains("--release")
                && l.contains("--nocapture")),
        "{text_id} must run ball_seam_bitident:\n{script}"
    );
    assert!(
        script.contains("set -euo pipefail") && script.contains("| tee \"$log\""),
        "{text_id}: the seam output is captured with tee and pipe failures are not masked:\n{script}"
    );
    assert!(
        script.contains(&format!("bash {ASSERT_SCRIPT} --seam-digests \"$log\"")),
        "{text_id} must check the printed seam digests:\n{script}"
    );
    assert!(
        !text.contains("continue-on-error"),
        "{text_id} must gate (no continue-on-error)"
    );
    assert!(
        !text.to_ascii_lowercase().contains("report only"),
        "{text_id} gates the golden and the seam digests; no step may call itself report only"
    );
    assert!(
        text.contains("REPLAY_HASH_TARGET:"),
        "{text_id}: annotations must name the leg"
    );
}

#[test]
fn replay_hash_assert_script_records_replays_and_compares_with_the_golden() {
    let s = assert_script_text();
    assert!(s.starts_with("#!/usr/bin/env bash\n"), "bash shebang");
    assert!(s.contains("set -euo pipefail"));
    assert!(
        s.contains("--record-inputs") && s.contains("--ticks") && s.contains("--replay-inputs"),
        "the script must record and replay the fixture"
    );
    assert!(
        s.contains("TICKS:-10000") && s.contains("crates/pin_sim/tests/fixtures/replay_table}"),
        "defaults: the fixture at 10 000 ticks"
    );
    assert!(
        s.contains("for i in 1 2 3 4 5 6 7 8 9 10; do"),
        "replay 10 times"
    );
    assert!(
        s.contains("[ \"$replayed\" != \"$recorded\" ]"),
        "in-machine mismatch must be compared"
    );
    assert!(
        s.contains("[ \"$recorded\" != \"$want\" ]") && s.contains(GOLDEN),
        "the recording must be compared with the golden"
    );
    assert!(s.contains("exit $fail") && s.contains("::error"));
    assert!(s.contains("::notice title=replay-hash ${target}::"));
    // CRLF checkouts (Windows): \r is stripped from the golden and from program output.
    assert!(
        s.contains("want=\"$(tr -d '\\r' < \"$golden\")\""),
        "the golden must be read with \\r stripped"
    );
    assert!(
        s.contains("| tr -d '\\r' | grep '^state_hash='"),
        "pin_sim output must be read with \\r stripped"
    );
    // Seam check: exactly five lines, all `same`, any `differs` fails.
    assert!(s.contains("--seam-digests") && s.contains("'^\\[seam digest\\] '"));
    assert!(s.contains("grep -q 'differs'") && s.contains("-ne 5"));
    // Portable to bash 3.2 and Git Bash (comments may name what is avoided).
    let code: String = s
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    for banned in ["mapfile", "readarray", "declare -A", "local -A", "sed -i"] {
        assert!(!code.contains(banned), "{ASSERT_SCRIPT} uses {banned}");
    }
}

/// Runs the assert script with `args`; the exit code and stdout.
fn run_assert_script(args: &[&str]) -> (Option<i32>, String) {
    let out = std::process::Command::new("bash")
        .arg(repo_root().join(ASSERT_SCRIPT))
        .args(args)
        .current_dir(repo_root())
        .env("REPLAY_HASH_TARGET", "spec_ci")
        .output()
        .expect("bash is required");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn seam_fixture(lines: &[&str]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("spec_ci_seam_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let n = std::fs::read_dir(&dir).unwrap().count();
    let path = dir.join(format!("seam_{n}.log"));
    std::fs::write(&path, lines.join("\r\n") + "\r\n").unwrap();
    path
}

#[test]
fn seam_digest_check_fails_on_differs_or_missing_lines() {
    let same = [
        "running 1 test",
        "[seam digest] spec_scenario: 0xa8e8957e1b4cee40 (x86_64-linux HEAD: 0xa8e8957e1b4cee40, same)",
        "[seam digest] spec_scenario_hook: 0x9da6a4436316c72e (x86_64-linux HEAD: 0x9da6a4436316c72e, same)",
        "[seam digest] box: 0x7850af7058aace12 (x86_64-linux HEAD: 0x7850af7058aace12, same)",
        "[seam digest] belt: 0x710c25cd832c76c2 (x86_64-linux HEAD: 0x710c25cd832c76c2, same)",
        "[seam digest] slope: 0x1764f34e6e37436f (x86_64-linux HEAD: 0x1764f34e6e37436f, same)",
        "test no_bodies_no_hooks_is_bit_identical_to_head ... ok",
    ];
    let p = seam_fixture(&same);
    assert_eq!(
        run_assert_script(&["--seam-digests", p.to_str().unwrap()]).0,
        Some(0),
        "five `same` lines (CRLF) pass"
    );
    let mut differs = same;
    differs[4] =
        "[seam digest] belt: 0x0000000000000001 (x86_64-linux HEAD: 0x710c25cd832c76c2, differs)";
    let p = seam_fixture(&differs);
    let (code, out) = run_assert_script(&["--seam-digests", p.to_str().unwrap()]);
    assert_eq!(code, Some(1), "a `differs` line fails:\n{out}");
    assert!(out.contains("::error"), "{out}");
    let four: Vec<&str> = same
        .iter()
        .copied()
        .filter(|l| !l.contains("slope"))
        .collect();
    let p = seam_fixture(&four);
    assert_eq!(
        run_assert_script(&["--seam-digests", p.to_str().unwrap()]).0,
        Some(1),
        "four lines fail"
    );
    let mut six = same.to_vec();
    six.push("[seam digest] extra: 0x0 (x86_64-linux HEAD: 0x0, ?)");
    let p = seam_fixture(&six);
    assert_eq!(
        run_assert_script(&["--seam-digests", p.to_str().unwrap()]).0,
        Some(1),
        "a sixth, unknown line fails"
    );
    let p = seam_fixture(&["test result: ok. 1 passed"]);
    assert_eq!(
        run_assert_script(&["--seam-digests", p.to_str().unwrap()]).0,
        Some(1),
        "no lines fail"
    );
    let _ = std::fs::remove_dir_all(p.parent().unwrap());
}

#[test]
fn replay_hash_assert_script_rejects_a_malformed_golden_before_running() {
    let dir = std::env::temp_dir().join(format!("spec_ci_golden_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad.golden");
    std::fs::write(&bad, "state_hash=25097C21C2C3ADC1\n").unwrap();
    let (code, out) = run_assert_script(&["/nonexistent/pin_sim", "", bad.to_str().unwrap()]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("not lowercase hex"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replay_hash_golden_is_one_lowercase_hex_state_hash_line() {
    let raw = std::fs::read(repo_root().join(GOLDEN)).expect("golden file present");
    // A Windows checkout may turn LF into CRLF; the consumers strip \r.
    let text: String = String::from_utf8(raw)
        .expect("ASCII")
        .chars()
        .filter(|c| *c != '\r')
        .collect();
    let hex = text
        .strip_prefix("state_hash=")
        .and_then(|r| r.strip_suffix('\n'))
        .unwrap_or_else(|| panic!("golden is not `state_hash=<hex>\\n`: {text:?}"));
    assert_eq!(hex.len(), 16, "{text:?}");
    assert!(
        hex.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{text:?}"
    );
}

#[test]
fn replay_hash_aarch64_cross_builds_and_runs_under_qemu() {
    let j = job("replay-hash-aarch64");
    let text = job_text("replay-hash-aarch64");
    assert_eq!(j.runner_labels(), vec!["ubuntu-latest".to_owned()]);
    let t = j.timeout_minutes().expect("timeout-minutes");
    assert!((1..=45).contains(&t), "timeout-minutes = {t}");
    assert_eq!(j.needs(), vec!["changes".to_owned()]);
    let cond = j.condition().unwrap_or_default();
    assert!(
        cond.contains("needs.changes.outputs.aarch64 == 'true'")
            && cond.contains("|| github.event_name != 'pull_request'"),
        "runs on PRs that touch the simulation and on every non-PR event: {cond}"
    );
    assert!(!skips_pull_requests_only(&cond), "{cond}");
    let script = j.script();
    for pkg in [
        "qemu-user",
        "gcc-aarch64-linux-gnu",
        "g++-aarch64-linux-gnu",
        "libc6-dev-arm64-cross",
    ] {
        assert!(script.contains(pkg), "does not install {pkg}:\n{script}");
    }
    assert!(
        text.contains("targets: aarch64-unknown-linux-gnu"),
        "the aarch64 target must be added to the toolchain"
    );
    assert!(
        text.contains("key: aarch64"),
        "the rust-cache key must differ from the x86_64 job"
    );
    let builds = cargo_lines(&script, "build");
    assert!(
        builds.iter().any(|l| l.contains("--release")
            && l.contains("--target aarch64-unknown-linux-gnu")
            && selected_packages(l).iter().any(|p| p == "pin_sim")),
        "want `cargo build --release --target aarch64-unknown-linux-gnu -p pin_sim`: {builds:?}"
    );
    assert!(
        script.contains(&format!(
            "bash {ASSERT_SCRIPT} target/aarch64-unknown-linux-gnu/release/pin_sim \"qemu-aarch64 -L /usr/aarch64-linux-gnu\""
        )),
        "the golden is asserted on the aarch64 binary under qemu:\n{script}"
    );
    let tests = cargo_lines(&script, "test");
    assert!(
        tests
            .iter()
            .any(|l| l.contains("--target aarch64-unknown-linux-gnu")
                && l.contains("--test ball_seam_bitident")),
        "ball_seam_bitident must run for aarch64: {tests:?}"
    );
    for (k, v) in [
        (
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER:",
            "aarch64-linux-gnu-gcc",
        ),
        (
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER:",
            "qemu-aarch64",
        ),
        ("CC_aarch64_unknown_linux_gnu:", "aarch64-linux-gnu-gcc"),
        ("QEMU_LD_PREFIX:", "/usr/aarch64-linux-gnu"),
    ] {
        let line = text
            .lines()
            .find(|l| l.trim_start().starts_with(k))
            .unwrap_or_else(|| panic!("env {k} missing"));
        assert!(line.contains(v), "{line}");
    }
    assert!(!text.contains("continue-on-error"));
}

/// A condition that would skip every pull request (the aarch64 job must not).
fn skips_pull_requests_only(cond: &str) -> bool {
    cond.contains("github.event_name != 'pull_request'") && !cond.contains("||")
}

#[test]
fn changes_exposes_the_aarch64_filter_failing_closed() {
    let text = job_text("changes");
    assert!(
        text.contains("aarch64: ${{ steps.filter.outputs.aarch64 }}"),
        "changes must declare the aarch64 output"
    );
    assert!(
        text.contains("dof=true\n          aarch64=true\n"),
        "aarch64 must default to true (fail closed)"
    );
    assert!(text.contains("echo \"aarch64=$aarch64\" >> \"$GITHUB_OUTPUT\""));
    for path in [
        "crates/pin_core/*",
        "crates/pin_physics/*",
        "crates/pin_input/*",
        "crates/pin_sim/*",
        "crates/pin_script/*",
        "crates/pin_assets/*",
        "third_party/lua-src/*",
        "tables/*",
        "Cargo.toml",
        "Cargo.lock",
        "versions.toml",
        ".github/workflows/ci.yml",
        ASSERT_SCRIPT,
    ] {
        assert!(text.contains(path), "aarch64 filter misses {path}");
    }
}

#[test]
fn rosetta_and_cross_legs_assert_the_golden() {
    // ci.yml legs: x86_64 Linux, Windows x86_64 and macOS arm64 run the assert script.
    for id in ["replay-hash", "replay-hash-cross"] {
        assert!(
            job(id)
                .script()
                .contains(&format!("bash {ASSERT_SCRIPT} target/release/pin_sim")),
            "{id} must assert the golden"
        );
    }
    let cross = job_text("replay-hash-cross");
    assert!(
        cross.contains("target: x86_64-pc-windows-msvc")
            && cross.contains("target: aarch64-apple-darwin")
    );
    // release.yml: the x86_64-apple-darwin (Rosetta 2) verify leg asserts the golden.
    let release = Workflow::load("release.yml");
    let verify = release.job("verify").expect("release.yml verify job");
    let text = workflow_text("release.yml");
    let at = text
        .find(&format!(
            "bash {ASSERT_SCRIPT} target/x86_64-apple-darwin/release/pin_sim"
        ))
        .expect("release.yml runs the assert script on the x86_64-apple-darwin pin_sim");
    let step_start = text[..at].rfind("      - name:").expect("step");
    let step = &text[step_start..at];
    assert!(
        step.contains("if: matrix.target == 'x86_64-apple-darwin'"),
        "the Rosetta replay-hash step must be conditional on the x86_64-apple-darwin leg:\n{step}"
    );
    assert!(
        verify.script().contains(
            "cargo build --release -p pin_sim --bin pin_sim --target x86_64-apple-darwin"
        ),
        "the Rosetta step builds the x86_64 pin_sim"
    );
    assert!(
        text.contains("RUSTFLAGS: \"-D warnings\""),
        "-D warnings kept"
    );
}

#[test]
fn toolchain_is_the_exact_pin_from_versions_toml_with_cache() {
    let wf = ci();
    let pin = pinned_toolchain();
    let want = format!("dtolnay/rust-toolchain@{pin}");
    assert!(
        workflow_text("ci.yml").contains(&pin),
        "ci.yml never mentions toolchain {pin}"
    );
    for id in [
        "core-tests",
        "core-tests-cross",
        "gpu-tests",
        "gpu-tests-cross",
        "clippy",
        "clippy-cross",
    ] {
        let uses = wf.job(id).unwrap().uses();
        assert!(
            uses.iter().any(|u| u.starts_with("actions/checkout@")),
            "{id}: no checkout"
        );
        assert!(uses.contains(&want), "{id}: want {want}, have {uses:?}");
        assert!(
            uses.iter().any(|u| u == "Swatinem/rust-cache@v2"),
            "{id}: {uses:?}"
        );
    }
    let fmt = wf.job("fmt").unwrap().uses();
    assert!(fmt.contains(&want), "fmt: want {want}, have {fmt:?}");
    // No job anywhere may float on another toolchain (stable, beta, other pins).
    for j in wf.jobs() {
        for u in j
            .uses()
            .iter()
            .filter(|u| u.starts_with("dtolnay/rust-toolchain@"))
        {
            assert_eq!(*u, want, "job `{}` uses a different toolchain", j.id);
        }
    }
}

#[test]
fn core_tests_installs_no_sdl_or_windowing_packages() {
    // GPU-free crates must build without SDL / display / audio system headers.
    let script = family("core-tests")
        .iter()
        .map(Job::script)
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    for pkg in [
        "libsdl",
        "libx11",
        "libxext",
        "libxrandr",
        "libxcursor",
        "libxi-",
        "libxkbcommon",
        "libwayland",
        "libasound",
        "libpulse",
        "libudev",
        "libdrm",
        "libgbm",
        "libegl",
        "libgl1",
        "mesa-vulkan-drivers",
        "xvfb",
    ] {
        assert!(
            !script.contains(pkg),
            "core-tests must stay display-free but installs {pkg}"
        );
    }
}

#[test]
fn clippy_also_lints_this_spec_package_with_warnings_denied() {
    let mut found = Vec::new();
    for j in ci().jobs() {
        for (dir, script) in j.run_steps_with_dir() {
            let in_dir = dir.trim_end_matches('/').ends_with("tests/spec_ci");
            let mut cd = false;
            for l in script.lines() {
                cd |= l.contains("cd ") && l.contains("tests/spec_ci");
                if !cargo_lines(l, "clippy").is_empty()
                    && (in_dir || cd || l.contains("tests/spec_ci/Cargo.toml"))
                {
                    found.push(l.to_owned());
                }
            }
        }
    }
    assert!(
        found.iter().any(|l| l
            .split_once(" -- ")
            .is_some_and(|(_, lint)| lint.contains("-D warnings"))),
        "ci.yml has no `cargo clippy ... -- -D warnings` for tests/spec_ci: {found:?}"
    );
}

#[test]
fn sdl_source_build_prerequisites_are_installed_on_linux() {
    // SDL builds from source: cmake + X11/Wayland/audio/udev headers.
    for id in ["gpu-tests", "clippy"] {
        let script = job(id).script();
        assert!(
            script.contains("apt-get") || script.contains("apt "),
            "{id}: no apt install step"
        );
        for pkg in [
            "cmake",
            "libx11-dev",
            "libwayland-dev",
            "libasound2-dev",
            "libudev-dev",
        ] {
            assert!(
                script.contains(pkg),
                "{id}: Linux dependency {pkg} not installed"
            );
        }
    }
}

#[test]
fn cargo_commands_in_ci_exist_in_the_brief() {
    // Guard against typos in subcommands: every `cargo X` is a known one.
    let known = [
        "test",
        "clippy",
        "fmt",
        "build",
        "check",
        "llvm-cov",
        "install",
        "fetch",
        "doc",
        "--version",
        "-V",
        "run",
        "metadata",
    ];
    for j in ci().jobs() {
        for line in j
            .script()
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
        {
            let toks: Vec<&str> = line.split_whitespace().collect();
            for (i, t) in toks.iter().enumerate() {
                if *t == "cargo"
                    && (i == 0
                        || ["run", "xvfb-run", "-a", "&&", ";", "then", "|"].contains(&toks[i - 1]))
                {
                    let mut sub = toks.get(i + 1).copied().unwrap_or("");
                    if sub.starts_with('+') {
                        sub = toks.get(i + 2).copied().unwrap_or("");
                    }
                    assert!(
                        known.contains(&sub),
                        "job `{}`: unknown `cargo {sub}` in: {line}",
                        j.id
                    );
                }
            }
        }
    }
}

/// The Linux/PR-set job ids that `pr-checks` aggregates (D40).
const PR_SET: [&str; 8] = [
    "changes",
    "fmt",
    "clippy",
    "core-tests",
    "replay-hash",
    "replay-hash-aarch64",
    "gpu-tests",
    "dof-bridge",
];

#[test]
fn pr_checks_aggregates_exactly_the_pr_set_and_always_runs() {
    let j = job("pr-checks");
    assert_eq!(j.condition().as_deref(), Some("always()"));
    let mut needs = j.needs();
    needs.sort();
    let mut want: Vec<String> = PR_SET.iter().map(|s| (*s).to_owned()).collect();
    want.sort();
    assert_eq!(needs, want, "pr-checks needs");
    assert!(
        !needs.iter().any(|n| n.ends_with("-cross")),
        "pr-checks must not need a -cross job: {needs:?}"
    );
    // The check name must be exactly `pr-checks` (the required check in the ruleset).
    let text = job_text("pr-checks");
    assert!(
        !text.contains("\n    name:") || text.contains("\n    name: pr-checks\n"),
        "pr-checks check name must be `pr-checks`"
    );
    assert!(j.runner_labels().contains(&"ubuntu-latest".to_owned()));
}

#[test]
fn every_pull_request_job_is_wired_into_pr_checks() {
    let needs = job("pr-checks").needs();
    for j in ci().jobs() {
        if j.id == "pr-checks" || j.id.ends_with("-cross") {
            continue;
        }
        let runs_on_pr = !skips_pull_requests(&j)
            && !j
                .condition()
                .is_some_and(|c| c.contains("github.event_name == 'workflow_dispatch'"));
        if runs_on_pr {
            assert!(
                needs.contains(&j.id),
                "job `{}` can run on pull_request but is not in pr-checks needs {needs:?}",
                j.id
            );
        }
    }
}

fn pr_checks_script() -> String {
    let j = job("pr-checks");
    let scripts = j.raw_run_scripts();
    assert_eq!(scripts.len(), 1, "pr-checks has one run step");
    assert_eq!(
        j.step_env("jq", "NEEDS").as_deref(),
        Some("${{ toJSON(needs) }}"),
        "needs must reach the script as JSON through an env var"
    );
    assert!(
        !scripts[0].contains("${{"),
        "no expression interpolation inside the script"
    );
    scripts[0].clone()
}

#[test]
fn pr_checks_script_requires_changes_to_succeed() {
    let script = pr_checks_script();
    assert!(
        script.contains(".changes.result") && script.contains("\"success\""),
        "script must require changes == success:\n{script}"
    );
}

/// Run the extracted script with a NEEDS fixture; the exit code.
fn run_pr_checks(results: [(&str, &str); 8]) -> Option<i32> {
    let body: Vec<String> = results
        .iter()
        .map(|(k, v)| format!("\"{k}\": {{\"result\": \"{v}\", \"outputs\": {{}}}}"))
        .collect();
    let needs = format!("{{{}}}", body.join(","));
    std::process::Command::new("bash")
        .arg("-c")
        .arg(pr_checks_script())
        .env("NEEDS", needs)
        .output()
        .expect("bash and jq are required")
        .status
        .code()
}

fn fixture(over: &[(&str, &str)], default: &str) -> [(&'static str, &'static str); 8] {
    let mut out = [("", ""); 8];
    for (i, id) in PR_SET.iter().enumerate() {
        let r = over
            .iter()
            .find(|(k, _)| k == id)
            .map_or(default, |(_, v)| *v);
        out[i] = (id, Box::leak(r.to_owned().into_boxed_str()));
    }
    out
}

#[test]
fn pr_checks_script_exit_codes() {
    assert_eq!(run_pr_checks(fixture(&[], "success")), Some(0));
    assert_eq!(
        run_pr_checks(fixture(
            &[("gpu-tests", "skipped"), ("dof-bridge", "skipped")],
            "success"
        )),
        Some(0)
    );
    assert_ne!(
        run_pr_checks(fixture(&[("clippy", "failure")], "success")),
        Some(0)
    );
    assert_ne!(
        run_pr_checks(fixture(&[("core-tests", "cancelled")], "success")),
        Some(0)
    );
    // replay-hash-aarch64 (KP-069): skipped passes, failed or cancelled fails.
    assert_eq!(
        run_pr_checks(fixture(&[("replay-hash-aarch64", "skipped")], "success")),
        Some(0)
    );
    assert_ne!(
        run_pr_checks(fixture(&[("replay-hash-aarch64", "failure")], "success")),
        Some(0)
    );
    assert_ne!(
        run_pr_checks(fixture(&[("replay-hash-aarch64", "cancelled")], "success")),
        Some(0)
    );
    assert_ne!(
        run_pr_checks(fixture(&[("changes", "failure")], "skipped")),
        Some(0)
    );
    assert_ne!(
        run_pr_checks(fixture(&[("changes", "skipped")], "skipped")),
        Some(0)
    );
}
