//! Contract: `.github/workflows/spikes.yml` is valid YAML and runs the spikes
//! on Windows, Linux and macOS runners.

use spec_0_1::read_repo_file;
use yaml_rust2::{Yaml, YamlLoader};

const FILE: &str = ".github/workflows/spikes.yml";

fn load() -> Yaml {
    let text = read_repo_file(FILE);
    let mut docs = match YamlLoader::load_from_str(&text) {
        Ok(docs) => docs,
        Err(err) => panic!("{FILE} is not valid YAML: {err}"),
    };
    assert_eq!(
        docs.len(),
        1,
        "{FILE}: expected exactly one YAML document, found {}",
        docs.len()
    );
    docs.remove(0)
}

fn jobs(doc: &Yaml) -> Vec<(String, &Yaml)> {
    let Some(map) = doc["jobs"].as_hash() else {
        panic!("{FILE}: top-level `jobs` mapping is missing or not a mapping");
    };
    assert!(!map.is_empty(), "{FILE}: `jobs` is empty");
    map.iter()
        .map(|(k, v)| (k.as_str().unwrap_or("<non-string job id>").to_owned(), v))
        .collect()
}

fn collect_strings(node: &Yaml, out: &mut Vec<String>) {
    match node {
        Yaml::String(s) => out.push(s.to_lowercase()),
        Yaml::Array(items) => items.iter().for_each(|i| collect_strings(i, out)),
        Yaml::Hash(map) => map.values().for_each(|v| collect_strings(v, out)),
        _ => {}
    }
}

/// Runner labels a job can execute on: literal `runs-on` labels, or — when
/// `runs-on` is an expression such as `${{ matrix.os }}` — every string in
/// the job's `strategy.matrix` (including `include` entries).
fn runner_labels(job: &Yaml) -> Vec<String> {
    let mut runs_on = Vec::new();
    collect_strings(&job["runs-on"], &mut runs_on);
    let mut labels: Vec<String> = Vec::new();
    if runs_on.iter().any(|s| s.contains("matrix.")) {
        collect_strings(&job["strategy"]["matrix"], &mut labels);
    }
    labels.extend(runs_on.into_iter().filter(|s| !s.contains("${{")));
    labels
}

#[test]
fn workflow_parses_as_yaml_mapping() {
    let doc = load();
    assert!(
        doc.as_hash().is_some(),
        "{FILE}: top level must be a YAML mapping"
    );
}

#[test]
fn workflow_has_a_trigger() {
    let doc = load();
    // YAML 1.1 parsers read a bare `on` key as boolean true; accept both spellings.
    let has_trigger = doc.as_hash().is_some_and(|map| {
        map.keys()
            .any(|k| k.as_str() == Some("on") || k.as_bool() == Some(true))
    });
    assert!(has_trigger, "{FILE}: workflow has no `on:` trigger");
}

#[test]
fn every_job_declares_runs_on_and_steps() {
    let doc = load();
    for (id, job) in jobs(&doc) {
        if !job["uses"].is_badvalue() {
            continue; // reusable-workflow call: no runs-on/steps of its own
        }
        assert!(
            !job["runs-on"].is_badvalue(),
            "{FILE}: job `{id}` has no `runs-on`"
        );
        let steps = job["steps"].as_vec().map_or(0, Vec::len);
        assert!(steps > 0, "{FILE}: job `{id}` has no steps");
    }
}

#[test]
fn runners_cover_windows_linux_and_macos() {
    let doc = load();
    let labels: Vec<String> = jobs(&doc)
        .iter()
        .flat_map(|(_, job)| runner_labels(job))
        .collect();
    let has = |needles: &[&str]| labels.iter().any(|l| needles.iter().any(|n| l.contains(n)));
    let mut missing = Vec::new();
    if !has(&["windows"]) {
        missing.push("windows");
    }
    if !has(&["ubuntu", "linux"]) {
        missing.push("ubuntu/linux");
    }
    if !has(&["macos"]) {
        missing.push("macos");
    }
    assert!(
        missing.is_empty(),
        "{FILE}: no job runs on {missing:?}; runner labels found across jobs/matrices: {labels:?}"
    );
}

#[test]
fn some_job_runs_cargo() {
    let doc = load();
    let mut strings = Vec::new();
    for (_, job) in jobs(&doc) {
        for step in job["steps"].as_vec().into_iter().flatten() {
            collect_strings(&step["run"], &mut strings);
        }
    }
    assert!(
        strings.iter().any(|s| s.contains("cargo ")),
        "{FILE}: no step runs cargo, so the spikes are never executed in CI"
    );
}
