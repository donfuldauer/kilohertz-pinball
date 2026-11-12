//! Subphase 1.2 hygiene items for `.github/workflows/spikes.yml`
//! (folded-in review findings from Subphase 0.1).

use spec_ci::*;

const MANIFEST: &str = "spikes/spec_0_1/Cargo.toml";

fn spikes() -> Workflow {
    Workflow::load("spikes.yml")
}

/// Lines running `cargo <sub>` against the spec_0_1 package, on Linux jobs.
fn spec_0_1_lines(sub: &str) -> Vec<String> {
    let mut out = Vec::new();
    for j in spikes().jobs().iter().filter(|j| j.covers(Os::Linux)) {
        for (dir, script) in j.run_steps_with_dir() {
            let in_dir = dir.trim_end_matches('/').ends_with("spikes/spec_0_1");
            let mut cd = false;
            for l in script.lines() {
                cd |= l.contains("cd ") && l.contains("spikes/spec_0_1");
                if !cargo_lines(l, sub).is_empty() && (in_dir || cd || l.contains(MANIFEST)) {
                    out.push(l.to_owned());
                }
            }
        }
    }
    out
}

#[test]
fn parses_and_still_triggers_on_push() {
    let t = spikes().triggers();
    assert!(t.iter().any(|e| e == "push"), "push trigger missing: {t:?}");
}

#[test]
fn runs_spec_0_1_tests_on_linux() {
    let lines = spec_0_1_lines("test");
    assert!(
        !lines.is_empty(),
        "spikes.yml never runs `cargo test --manifest-path {MANIFEST}` on Linux"
    );
}

#[test]
fn checks_spec_0_1_formatting() {
    let lines = spec_0_1_lines("fmt");
    assert!(
        lines.iter().any(|l| l.contains("--check")),
        "no `cargo fmt ... --check` for spec_0_1: {lines:?}"
    );
}

#[test]
fn lints_spec_0_1_with_warnings_denied() {
    let lines = spec_0_1_lines("clippy");
    assert!(
        lines.iter().any(|l| l
            .split_once(" -- ")
            .is_some_and(|(_, lint)| lint.contains("-D warnings"))),
        "no `cargo clippy ... -- -D warnings` for spec_0_1: {lines:?}"
    );
}

#[test]
fn every_job_has_a_timeout_of_at_most_45_minutes() {
    for j in spikes().jobs() {
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
fn concurrency_cancels_superseded_runs_per_ref() {
    let wf = spikes();
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
            c.cancel_in_progress,
            "cancel-in-progress not enabled for `{}`",
            c.group
        );
    }
}

#[test]
fn concurrency_groups_of_the_two_workflows_cannot_collide() {
    // Same ref, two workflows: a shared literal group would make them cancel each other.
    let (a, b) = (
        Workflow::load("ci.yml").concurrency(),
        spikes().concurrency(),
    );
    if let (Some(a), Some(b)) = (a, b) {
        assert!(
            a.group != b.group || a.group.contains("github.workflow"),
            "ci.yml and spikes.yml share concurrency group `{}`",
            a.group
        );
    }
}
