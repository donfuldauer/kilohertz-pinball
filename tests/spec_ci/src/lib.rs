//! Helpers for inspecting GitHub Actions workflow files as data.
//!
//! Nothing here knows what a particular workflow *should* contain; the spec
//! tests under `tests/` hold the expectations from the Subphase 1.2 brief.

use std::path::PathBuf;

use yaml_rust2::{Yaml, YamlLoader};

/// Repository root (this package lives at `<root>/tests/spec_ci`).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Raw text of `.github/workflows/<name>`.
pub fn workflow_text(name: &str) -> String {
    let path = repo_root().join(".github/workflows").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Parse the `rust = "x.y.z"` entry of the `[toolchain]` table from
/// `versions.toml` text. `None` when the table or a full x.y.z value is absent.
pub fn toolchain_from_versions_toml(text: &str) -> Option<String> {
    let mut in_table = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_table = line == "[toolchain]";
            continue;
        }
        if !in_table {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "rust" {
            continue;
        }
        let v = value.trim().strip_prefix('"')?.strip_suffix('"')?;
        let parts: Vec<&str> = v.split('.').collect();
        let numeric = parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        return (parts.len() == 3 && numeric).then(|| v.to_owned());
    }
    None
}

/// The exact Rust toolchain recorded in `<root>/versions.toml`.
pub fn pinned_toolchain() -> String {
    let path = repo_root().join("versions.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    toolchain_from_versions_toml(&text).expect("versions.toml has no [toolchain] rust = \"x.y.z\"")
}

/// One parsed workflow document.
#[derive(Debug, Clone)]
pub struct Workflow {
    root: Yaml,
}

/// One job of a workflow.
#[derive(Debug, Clone)]
pub struct Job {
    /// The key under `jobs:`.
    pub id: String,
    node: Yaml,
}

/// The three operating-system families the matrix must cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Macos,
}

impl Os {
    pub const ALL: [Os; 3] = [Os::Windows, Os::Linux, Os::Macos];
}

/// Classify a GitHub-hosted runner label (`ubuntu-24.04`, `windows-latest`...).
pub fn runner_os(label: &str) -> Option<Os> {
    let l = label.trim().to_ascii_lowercase();
    if l.starts_with("windows-") {
        Some(Os::Windows)
    } else if l.starts_with("ubuntu-") {
        Some(Os::Linux)
    } else if l.starts_with("macos-") {
        Some(Os::Macos)
    } else {
        None
    }
}

/// True when `label` is a GitHub-hosted macOS runner on Apple Silicon
/// (aarch64-apple-darwin): `macos-latest`, `macos-14` and later, including
/// `-xlarge` / `-arm64` variants. `macos-13`, `-large` and `-intel` images are
/// x86_64 and do not count.
pub fn is_arm64_macos_label(label: &str) -> bool {
    let l = label.trim().to_ascii_lowercase();
    let Some(rest) = l.strip_prefix("macos-") else {
        return false;
    };
    let mut parts = rest.split('-');
    let version = parts.next().unwrap_or("");
    let suffixes: Vec<&str> = parts.collect();
    if suffixes.iter().any(|s| *s == "large" || *s == "intel") {
        return false;
    }
    if suffixes.iter().any(|s| *s != "xlarge" && *s != "arm64") {
        return false;
    }
    if version == "latest" {
        return true;
    }
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    version.parse::<u32>().is_ok_and(|v| v >= 14)
}

fn collect_strings(node: &Yaml, out: &mut Vec<String>) {
    match node {
        Yaml::String(s) => out.push(s.clone()),
        Yaml::Array(a) => a.iter().for_each(|n| collect_strings(n, out)),
        Yaml::Hash(h) => h.values().for_each(|n| collect_strings(n, out)),
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
    let axes: Vec<(&str, &Vec<Yaml>)> = map
        .iter()
        .filter_map(|(k, v)| {
            let k = k.as_str().filter(|k| *k != "include" && *k != "exclude")?;
            Some((k, v.as_vec()?))
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
                    c.push((*name, v));
                    c
                })
            })
            .collect();
    }
    let entries = |key: &str| -> Vec<&Yaml> {
        get(matrix, key)
            .and_then(Yaml::as_vec)
            .into_iter()
            .flatten()
            .collect()
    };
    combos.retain(|combo| {
        !entries("exclude").iter().any(|ex| {
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
    for inc in entries("include") {
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

/// Strings the matrix axis `axis` takes over the job's combinations
/// ([`matrix_combinations`]).
fn matrix_axis_values(matrix: &Yaml, axis: &str) -> Vec<String> {
    let mut out = Vec::new();
    for combo in matrix_combinations(matrix) {
        for (name, value) in combo {
            if name == axis {
                collect_strings(value, &mut out);
            }
        }
    }
    out
}

fn get<'a>(node: &'a Yaml, key: &str) -> Option<&'a Yaml> {
    let h = node.as_hash()?;
    h.get(&Yaml::String(key.to_owned()))
}

impl Workflow {
    /// Parse workflow text; `Err` carries a human-readable reason.
    pub fn parse(text: &str) -> Result<Workflow, String> {
        let mut docs = YamlLoader::load_from_str(text).map_err(|e| format!("YAML error: {e}"))?;
        if docs.len() != 1 {
            return Err(format!(
                "expected exactly 1 YAML document, found {}",
                docs.len()
            ));
        }
        let root = docs.remove(0);
        if root.as_hash().is_none() {
            return Err("top level is not a mapping".into());
        }
        match get(&root, "jobs").and_then(Yaml::as_hash) {
            Some(j) if !j.is_empty() => {}
            _ => return Err("`jobs` is missing, empty or not a mapping".into()),
        }
        Ok(Workflow { root })
    }

    /// Load and parse `.github/workflows/<name>`, panicking with context.
    pub fn load(name: &str) -> Workflow {
        Workflow::parse(&workflow_text(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// Event names under `on:` (scalar, list and mapping forms). YAML 1.1
    /// parsers read the bare key `on` as boolean true, so that is accepted.
    pub fn triggers(&self) -> Vec<String> {
        let node = get(&self.root, "on").or_else(|| self.root.as_hash()?.get(&Yaml::Boolean(true)));
        match node {
            Some(Yaml::String(s)) => vec![s.clone()],
            Some(Yaml::Array(a)) => a
                .iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect(),
            Some(Yaml::Hash(h)) => h
                .keys()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn jobs(&self) -> Vec<Job> {
        let jobs = get(&self.root, "jobs")
            .and_then(Yaml::as_hash)
            .expect("checked in parse");
        jobs.iter()
            .map(|(k, v)| Job {
                id: k.as_str().unwrap_or_default().to_owned(),
                node: v.clone(),
            })
            .collect()
    }

    pub fn job(&self, id: &str) -> Option<Job> {
        self.jobs().into_iter().find(|j| j.id == id)
    }

    /// Workflow-level concurrency settings, if any.
    pub fn concurrency(&self) -> Option<Concurrency> {
        Concurrency::from_node(get(&self.root, "concurrency")?)
    }
}

/// A `concurrency:` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Concurrency {
    pub group: String,
    /// True for literal `true` or any `${{ }}` expression (conditional cancel).
    pub cancel_in_progress: bool,
}

impl Concurrency {
    fn from_node(node: &Yaml) -> Option<Concurrency> {
        match node {
            Yaml::String(s) => Some(Concurrency {
                group: s.clone(),
                cancel_in_progress: false,
            }),
            Yaml::Hash(_) => {
                let group = get(node, "group")?.as_str()?.to_owned();
                let cancel_in_progress = match get(node, "cancel-in-progress") {
                    Some(Yaml::Boolean(b)) => *b,
                    Some(Yaml::String(s)) => s.contains("${{"),
                    _ => false,
                };
                Some(Concurrency {
                    group,
                    cancel_in_progress,
                })
            }
            _ => None,
        }
    }

    /// The group distinguishes triggering events, so e.g. a manual
    /// `workflow_dispatch` run is not cancelled by a later push to the same ref.
    pub fn is_per_event(&self) -> bool {
        self.group.contains("github.event_name")
    }

    /// The group distinguishes refs, so runs for different branches coexist.
    pub fn is_per_ref(&self) -> bool {
        [
            "github.ref",
            "github.head_ref",
            "github.event.pull_request.number",
        ]
        .iter()
        .any(|k| self.group.contains(k))
    }
}

impl Job {
    /// `timeout-minutes`, when it is a literal integer.
    pub fn timeout_minutes(&self) -> Option<i64> {
        get(&self.node, "timeout-minutes")?.as_i64()
    }

    pub fn concurrency(&self) -> Option<Concurrency> {
        Concurrency::from_node(get(&self.node, "concurrency")?)
    }

    /// Every runner label the job can land on: literal `runs-on` labels plus,
    /// for each `${{ matrix.<axis> }}` in `runs-on`, the values that axis
    /// takes (after `exclude`, with `include`). Matrix axes that `runs-on`
    /// does not reference do not select a runner and are ignored.
    pub fn runner_labels(&self) -> Vec<String> {
        let mut runs_on = Vec::new();
        if let Some(r) = get(&self.node, "runs-on") {
            collect_strings(r, &mut runs_on);
        }
        let matrix = get(&self.node, "strategy").and_then(|s| get(s, "matrix"));
        let mut all = Vec::new();
        for label in runs_on {
            if !label.contains("${{") {
                all.push(label);
                continue;
            }
            let Some(m) = matrix else { continue };
            for axis in label.split("matrix.").skip(1) {
                let axis: String = axis
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                if !axis.is_empty() {
                    all.extend(matrix_axis_values(m, &axis));
                }
            }
        }
        all.retain(|s| runner_os(s).is_some());
        all.sort();
        all.dedup();
        all
    }

    pub fn covers(&self, os: Os) -> bool {
        self.runner_labels()
            .iter()
            .any(|l| runner_os(l) == Some(os))
    }

    fn steps(&self) -> &[Yaml] {
        get(&self.node, "steps")
            .and_then(Yaml::as_vec)
            .map_or(&[], Vec::as_slice)
    }

    /// `run:` scripts of all steps, with shell line continuations joined and
    /// whitespace runs collapsed so commands can be matched as substrings.
    pub fn run_scripts(&self) -> Vec<String> {
        self.steps()
            .iter()
            .filter_map(|s| get(s, "run")?.as_str())
            .map(normalize_script)
            .collect()
    }

    /// All `run:` scripts joined by newlines.
    pub fn script(&self) -> String {
        self.run_scripts().join("\n")
    }

    /// `(working-directory, script)` per run step; job `defaults.run` applies.
    pub fn run_steps_with_dir(&self) -> Vec<(String, String)> {
        let default_dir = get(&self.node, "defaults")
            .and_then(|d| get(d, "run"))
            .and_then(|r| get(r, "working-directory"))
            .and_then(Yaml::as_str)
            .unwrap_or("");
        self.steps()
            .iter()
            .filter_map(|s| {
                let run = get(s, "run")?.as_str()?;
                let dir = get(s, "working-directory")
                    .and_then(Yaml::as_str)
                    .unwrap_or(default_dir);
                Some((dir.to_owned(), normalize_script(run)))
            })
            .collect()
    }

    /// `uses:` references of all steps.
    pub fn uses(&self) -> Vec<String> {
        self.steps()
            .iter()
            .filter_map(|s| get(s, "uses")?.as_str().map(str::to_owned))
            .collect()
    }

    /// Number of steps of any kind.
    pub fn step_count(&self) -> usize {
        self.steps().len()
    }
}

/// Join `\`-newline (sh) and backtick-newline (pwsh) continuations, then
/// collapse runs of spaces/tabs; line structure is otherwise kept.
pub fn normalize_script(script: &str) -> String {
    let joined = script
        .replace("\\\r\n", " ")
        .replace("\\\n", " ")
        .replace("`\n", " ");
    joined
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Cargo invocations in `script` that run `cargo <sub>` (e.g. `sub = "test"`),
/// also matching `cargo +toolchain <sub>` and wrappers such as
/// `xvfb-run cargo test`. Each match is one command of its line: a line is
/// split at the shell separators `&&`, `||`, `;` and `|`, so the flags of
/// another command on the same line are not part of the match.
pub fn cargo_lines<'a>(script: &'a str, sub: &str) -> Vec<&'a str> {
    script
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .flat_map(|l| l.split(['&', '|', ';']))
        .map(str::trim)
        .filter(|cmd| {
            let toks: Vec<&str> = cmd.split_whitespace().collect();
            toks.iter().enumerate().any(|(i, t)| {
                *t == "cargo"
                    && match toks.get(i + 1) {
                        Some(n) if *n == sub => true,
                        Some(n) if n.starts_with('+') => toks.get(i + 2) == Some(&sub),
                        _ => false,
                    }
            })
        })
        .collect()
}

/// Packages selected with `-p X`, `-pX`, `--package X` or `--package=X`.
pub fn selected_packages(line: &str) -> Vec<String> {
    flag_values(line, "-p", "--package")
}

/// Packages removed with `--exclude X` / `--exclude=X`.
pub fn excluded_packages(line: &str) -> Vec<String> {
    flag_values(line, "\0", "--exclude")
}

fn flag_values(line: &str, short: &str, long: &str) -> Vec<String> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t == "--" {
            break;
        }
        if t == short || t == long {
            if let Some(v) = toks.get(i + 1) {
                out.push((*v).to_owned());
                i += 1;
            }
        } else if let Some(v) = t.strip_prefix(long).and_then(|r| r.strip_prefix('=')) {
            out.push(v.to_owned());
        } else if let Some(v) = t
            .strip_prefix(short)
            .filter(|v| !v.is_empty() && !t.starts_with("--"))
        {
            out.push(v.to_owned());
        }
        i += 1;
    }
    out
}
