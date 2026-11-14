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

/// The matrix combinations GitHub Actions runs, as `(key, value)` lists:
/// the Cartesian product of the plain axes, minus `exclude` entries, then each
/// `include` entry in order. An entry is merged into every product combination
/// whose original axis values it does not contradict (it may overwrite values
/// an earlier `include` entry added); an entry merged into none becomes a job
/// of its own. Include-created jobs are never merge targets. With no plain
/// axes, every `include` entry is its own job.
fn matrix_combinations(matrix: &Yaml) -> Vec<Vec<(&str, &Yaml)>> {
    let Some(map) = matrix.as_hash() else {
        return Vec::new();
    };
    let axes: Vec<(&str, Vec<&Yaml>)> = map
        .iter()
        .filter_map(|(k, v)| {
            let k = k.as_str().filter(|k| *k != "include" && *k != "exclude")?;
            Some((k, v.as_vec()?.iter().collect()))
        })
        .collect();
    let mut combos: Vec<Vec<(&str, &Yaml)>> = if axes.is_empty() {
        Vec::new()
    } else {
        vec![Vec::new()]
    };
    for (name, values) in &axes {
        combos = combos
            .iter()
            .flat_map(|c| {
                values.iter().map(move |v| {
                    let mut c = c.clone();
                    c.push((*name, *v));
                    c
                })
            })
            .collect();
    }
    let excludes: Vec<&Yaml> = matrix["exclude"].as_vec().into_iter().flatten().collect();
    combos.retain(|combo| {
        !excludes.iter().any(|ex| {
            ex.as_hash().is_some_and(|ex| {
                ex.iter().all(|(k, v)| {
                    combo
                        .iter()
                        .any(|(name, value)| k.as_str() == Some(*name) && *value == v)
                })
            })
        })
    });
    // Product combinations keep their original values; `added[i]` holds what
    // `include` entries merged into combination `i` (later entries win).
    let mut added: Vec<Vec<(&str, &Yaml)>> = vec![Vec::new(); combos.len()];
    let mut extra: Vec<Vec<(&str, &Yaml)>> = Vec::new();
    for inc in matrix["include"].as_vec().into_iter().flatten() {
        let Some(inc) = inc.as_hash() else { continue };
        let pairs: Vec<(&str, &Yaml)> = inc
            .iter()
            .filter_map(|(k, v)| Some((k.as_str()?, v)))
            .collect();
        let mut merged = false;
        for (combo, added) in combos.iter().zip(added.iter_mut()) {
            let clashes = pairs
                .iter()
                .any(|(k, v)| combo.iter().any(|(name, value)| name == k && *value != *v));
            if clashes {
                continue;
            }
            merged = true;
            for &(k, v) in &pairs {
                if combo.iter().any(|(name, _)| *name == k) {
                    continue;
                }
                match added.iter_mut().find(|(name, _)| *name == k) {
                    Some(slot) => slot.1 = v,
                    None => added.push((k, v)),
                }
            }
        }
        if !merged {
            extra.push(pairs);
        }
    }
    for (combo, added) in combos.iter_mut().zip(added) {
        combo.extend(added);
    }
    combos.extend(extra);
    combos
}

/// Values the matrix axis `axis` takes over the job's combinations
/// ([`matrix_combinations`]), lowercased and deduplicated in order.
fn matrix_axis_values(matrix: &Yaml, axis: &str) -> Vec<String> {
    let mut out = Vec::new();
    for combo in matrix_combinations(matrix) {
        for (name, value) in combo {
            if name == axis {
                collect_strings(value, &mut out);
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|v| seen.insert(v.clone()));
    out
}

/// Runner labels a job can execute on: literal `runs-on` labels, or — when
/// `runs-on` is an expression such as `${{ matrix.os }}` — the values of the
/// referenced matrix axis (after `exclude`, with `include`). Other matrix
/// axes do not select a runner and are ignored.
fn runner_labels(job: &Yaml) -> Vec<String> {
    let mut runs_on = Vec::new();
    collect_strings(&job["runs-on"], &mut runs_on);
    let mut labels: Vec<String> = Vec::new();
    for expr in runs_on.iter().filter(|s| s.contains("${{")) {
        for axis in expr.split("matrix.").skip(1) {
            let axis: String = axis
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if !axis.is_empty() {
                labels.extend(matrix_axis_values(&job["strategy"]["matrix"], &axis));
            }
        }
    }
    labels.extend(runs_on.into_iter().filter(|s| !s.contains("${{")));
    labels
}

fn job(yaml: &str) -> Yaml {
    YamlLoader::load_from_str(yaml)
        .expect("fixture parses")
        .remove(0)
}

#[test]
fn runner_labels_follow_the_referenced_axis() {
    let unrelated = job(
        "runs-on: ${{ matrix.os }}\nstrategy:\n  matrix:\n    os: [ubuntu-latest]\n    artifact: [windows, macos]\n",
    );
    assert_eq!(runner_labels(&unrelated), ["ubuntu-latest"]);
    let literal =
        job("runs-on: ubuntu-latest\nstrategy:\n  matrix:\n    os: [windows-latest, macos-14]\n");
    assert_eq!(runner_labels(&literal), ["ubuntu-latest"]);
    let excluded = job(concat!(
        "runs-on: ${{ matrix.os }}\nstrategy:\n  matrix:\n",
        "    os: [ubuntu-latest, windows-latest]\n    rust: [stable, beta]\n",
        "    exclude:\n      - os: windows-latest\n",
        "    include:\n      - os: macos-14\n        rust: stable\n",
    ));
    assert_eq!(runner_labels(&excluded), ["ubuntu-latest", "macos-14"]);
    let partly_excluded = job(concat!(
        "runs-on: ${{ matrix.os }}\nstrategy:\n  matrix:\n",
        "    os: [ubuntu-latest, windows-latest]\n    rust: [stable, beta]\n",
        "    exclude:\n      - os: windows-latest\n        rust: beta\n",
    ));
    assert!(runner_labels(&partly_excluded).contains(&"windows-latest".to_owned()));
    // `include` entries apply in order: each one that does not clash with an
    // original axis value is merged into every combination, and a later
    // entry overwrites `os` set by an earlier one, so every job is on macos-14.
    let overwritten = job(concat!(
        "runs-on: ${{ matrix.os }}\nstrategy:\n  matrix:\n    version: [1, 2]\n",
        "    include:\n      - os: windows-latest\n      - os: ubuntu-latest\n",
        "      - os: macos-14\n",
    ));
    assert_eq!(runner_labels(&overwritten), ["macos-14"]);
    // An entry that clashes with every combination's original values adds a
    // job; one that matches some combinations is merged only into those.
    let added = job(concat!(
        "runs-on: ${{ matrix.os }}\nstrategy:\n  matrix:\n    version: [1, 2]\n",
        "    include:\n      - os: ubuntu-latest\n",
        "      - os: windows-latest\n        version: 3\n",
        "      - os: macos-14\n        version: 2\n",
    ));
    assert_eq!(
        runner_labels(&added),
        ["ubuntu-latest", "macos-14", "windows-latest"]
    );
    // With no plain axes every `include` entry is its own job.
    let only_include = job(concat!(
        "runs-on: ${{ matrix.os }}\nstrategy:\n  matrix:\n",
        "    include:\n      - os: windows-latest\n      - os: macos-14\n",
    ));
    assert_eq!(runner_labels(&only_include), ["windows-latest", "macos-14"]);
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
