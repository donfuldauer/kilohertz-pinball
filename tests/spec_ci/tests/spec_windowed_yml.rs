//! Structure of `.github/workflows/windowed.yml` (KP-002 #82, gate review
//! finding M9 of #163): triggers, concurrency, permissions, toolchain pin, the
//! windowed steps and the artifact upload, SPEC §7.5 (REQ-LAT-001 and
//! REQ-LAT-002 in their CI form). DEVELOPMENT.md: "`tests/spec_ci` asserts
//! the workflow structure; a workflow change updates it in the same PR."

use spec_ci::*;
use yaml_rust2::{Yaml, YamlLoader};

fn windowed() -> Workflow {
    Workflow::load("windowed.yml")
}

fn job() -> Job {
    let jobs = windowed().jobs();
    assert_eq!(jobs.len(), 1, "expected the single `windowed` job");
    let j = jobs.into_iter().next().unwrap();
    assert_eq!(j.id, "windowed");
    j
}

fn root_yaml() -> Yaml {
    YamlLoader::load_from_str(&workflow_text("windowed.yml"))
        .expect("windowed.yml parses")
        .remove(0)
}

fn key<'a>(node: &'a Yaml, k: &str) -> &'a Yaml {
    &node[k]
}

/// The steps of the `windowed` job as YAML nodes.
fn steps() -> Vec<Yaml> {
    let root = root_yaml();
    key(key(key(&root, "jobs"), "windowed"), "steps")
        .as_vec()
        .expect("steps is a list")
        .clone()
}

#[test]
fn triggers_are_exactly_pull_request_push_and_workflow_dispatch() {
    let mut t = windowed().triggers();
    t.sort();
    assert_eq!(t, ["pull_request", "push", "workflow_dispatch"]);
}

#[test]
fn pull_request_paths_cover_every_crate_the_lockfile_the_tables_the_scripts_and_the_workflow() {
    let paths = windowed()
        .trigger_paths("pull_request")
        .expect("pull_request has a paths filter");
    assert_eq!(
        paths,
        [
            "crates/**",
            "Cargo.toml",
            "Cargo.lock",
            "tables/**",
            "scripts/windowed/**",
            ".github/workflows/windowed.yml",
        ]
    );
    // pin_app is built from the other crates, so a change deep in one of them
    // (here the simulation loader) must match a listed glob.
    let matches = |file: &str| {
        paths.iter().any(|g| match g.strip_suffix("/**") {
            Some(prefix) => file.starts_with(&format!("{prefix}/")),
            None => file == g,
        })
    };
    assert!(matches("crates/pin_sim/src/load/mod.rs"));
    assert!(matches("Cargo.lock"));
    assert!(matches("scripts/windowed/run.sh"));
    assert!(!matches("docs/SPEC.md"));
}

#[test]
fn push_runs_on_main_without_a_path_filter() {
    let wf = windowed();
    assert_eq!(wf.trigger_branches("push"), Some(vec!["main".to_owned()]));
    assert_eq!(wf.trigger_paths("push"), None, "push to main is unfiltered");
}

#[test]
fn concurrency_is_per_ref_per_event_and_cancels_only_pull_requests() {
    let wf = windowed();
    let c = wf.concurrency().expect("workflow-level concurrency");
    assert!(c.is_per_ref(), "group `{}` is not per ref", c.group);
    assert!(c.is_per_event(), "group `{}` is not per event", c.group);
    assert_eq!(
        wf.concurrency_cancel_raw().as_deref(),
        Some("${{ github.event_name == 'pull_request' }}")
    );
}

#[test]
fn concurrency_group_cannot_collide_with_ci_yml() {
    let w = windowed().concurrency().expect("windowed concurrency");
    if let Some(ci) = Workflow::load("ci.yml").concurrency() {
        assert!(
            ci.group != w.group || w.group.contains("github.workflow"),
            "ci.yml and windowed.yml share concurrency group `{}`",
            w.group
        );
    }
    assert!(
        w.group.contains("github.workflow"),
        "group `{}` is not keyed on the workflow name",
        w.group
    );
    // `github.workflow` expands to the workflow's `name:` (its file name when
    // there is none), not to a unique id: equal names would share one group at
    // the same ref and event.
    let effective_name = |file: &str| {
        YamlLoader::load_from_str(&workflow_text(file))
            .unwrap_or_else(|e| panic!("{file} parses: {e}"))
            .remove(0)["name"]
            .as_str()
            .map_or_else(|| file.to_owned(), str::to_owned)
    };
    assert_ne!(
        effective_name("windowed.yml"),
        effective_name("ci.yml"),
        "windowed.yml and ci.yml expand `github.workflow` to the same name, so they share a concurrency group"
    );
}

#[test]
fn permissions_are_contents_read_and_nothing_broader() {
    let root = root_yaml();
    let perms = key(&root, "permissions")
        .as_hash()
        .expect("permissions map");
    assert_eq!(perms.len(), 1, "permissions: {perms:?}");
    assert_eq!(
        key(key(&root, "permissions"), "contents").as_str(),
        Some("read")
    );
    let job_node = key(key(&root, "jobs"), "windowed");
    assert!(
        key(job_node, "permissions").is_badvalue(),
        "the job must not widen permissions"
    );
}

#[test]
fn job_runs_on_linux_with_a_timeout_of_at_most_45_minutes() {
    let j = job();
    assert!(j.covers(Os::Linux), "windowed job does not run on Linux");
    let t = j.timeout_minutes().expect("integer timeout-minutes");
    assert!((1..=45).contains(&t), "timeout-minutes = {t}");
}

#[test]
fn toolchain_is_the_exact_pin_from_versions_toml() {
    let want = format!("dtolnay/rust-toolchain@{}", pinned_toolchain());
    let uses: Vec<String> = job()
        .uses()
        .into_iter()
        .filter(|u| u.starts_with("dtolnay/rust-toolchain@"))
        .collect();
    assert_eq!(uses, [want]);
    let root = root_yaml();
    assert_eq!(
        key(key(&root, "env"), "RUST_VERSION").as_str(),
        Some(pinned_toolchain().as_str())
    );
}

#[test]
fn lavapipe_threads_are_capped_at_two() {
    let root = root_yaml();
    assert_eq!(key(key(&root, "env"), "LP_NUM_THREADS").as_str(), Some("2"));
}

#[test]
fn installs_xvfb_and_xdotool() {
    let apt: Vec<String> = job()
        .run_scripts()
        .into_iter()
        .filter(|s| s.contains("apt-get install"))
        .collect();
    assert_eq!(apt.len(), 1, "one apt-get install step: {apt:?}");
    for pkg in ["xvfb", "xdotool", "mesa-vulkan-drivers"] {
        assert!(
            apt[0].split_whitespace().any(|w| w == pkg),
            "apt install lacks {pkg}: {}",
            apt[0]
        );
    }
}

#[test]
fn runs_the_driver_script_and_the_windowed_verdict_tests() {
    let scripts = job().run_scripts();
    assert!(
        scripts
            .iter()
            .any(|s| s.contains("scripts/windowed/run.sh")),
        "no step runs scripts/windowed/run.sh"
    );
    assert!(
        scripts.iter().any(|s| {
            cargo_lines(s, "test").iter().any(|l| {
                l.contains("--release -p pin_app --test windowed") && !l.contains("--no-run")
            })
        }),
        "no step runs `cargo test --release -p pin_app --test windowed`"
    );
}

#[test]
fn run_step_forces_the_fallback_adapter() {
    assert_eq!(
        job()
            .step_env("scripts/windowed/run.sh", "PIN_FORCE_FALLBACK_ADAPTER")
            .as_deref(),
        Some("1")
    );
}

#[test]
fn artifact_windowed_latency_is_uploaded_on_every_outcome() {
    let ups: Vec<Yaml> = steps()
        .into_iter()
        .filter(|s| {
            s["uses"]
                .as_str()
                .is_some_and(|u| u.starts_with("actions/upload-artifact@"))
        })
        .collect();
    assert_eq!(ups.len(), 1, "one upload-artifact step");
    assert_eq!(ups[0]["if"].as_str(), Some("always()"));
    assert_eq!(ups[0]["with"]["name"].as_str(), Some("windowed-latency"));
}

#[test]
fn pr_checks_does_not_need_the_windowed_job() {
    let ci = Workflow::load("ci.yml");
    let pr = ci.job("pr-checks").expect("ci.yml has pr-checks");
    assert!(
        !pr.needs().iter().any(|n| n == "windowed"),
        "pr-checks needs windowed: {:?}",
        pr.needs()
    );
    assert!(
        ci.job("windowed").is_none(),
        "ci.yml must not define windowed"
    );
}
