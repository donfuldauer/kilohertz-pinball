//! Contract (D41): decision and queue entries are one file per unit under
//! `docs/decisions/` and `docs/queue/`.

use regex::Regex;
use spec_0_1::{read_repo_file, repo_root};
use std::collections::BTreeMap;
use std::path::Path;

/// Sorted names of the `*.md` files in `dir` other than `README.md`.
fn md_files(rel_dir: &str) -> Vec<String> {
    let dir = repo_root().join(rel_dir);
    assert!(dir.is_dir(), "{rel_dir} must exist");
    assert!(
        dir.join("README.md").is_file(),
        "{rel_dir}/README.md must exist"
    );
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {rel_dir}: {e}"))
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|n| Path::new(n).extension().is_some_and(|x| x == "md") && n != "README.md")
        .collect();
    names.sort();
    names
}

/// Fails naming the colliding files when two share a case-insensitive key.
fn assert_unique(dir: &str, names: &[String], key: impl Fn(&str) -> String, what: &str) {
    let mut seen: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for n in names {
        seen.entry(key(n)).or_default().push(n);
    }
    for (k, files) in &seen {
        assert!(files.len() == 1, "{dir}: {what} `{k}` collides: {files:?}");
    }
}

fn decision_files() -> Vec<String> {
    let re = Regex::new(r"^\d{4}-\d{2}-\d{2}-[a-z0-9][a-z0-9._-]*\.md$").unwrap();
    let names = md_files("docs/decisions");
    for n in &names {
        assert!(
            re.is_match(n),
            "docs/decisions/{n} must be <YYYY-MM-DD>-<unit-id>.md"
        );
    }
    names
}

#[test]
fn decision_files_have_dated_names() {
    decision_files();
}

#[test]
fn decision_files_have_heading_and_labels() {
    for n in decision_files() {
        let text = read_repo_file(&format!("docs/decisions/{n}"));
        assert!(
            text.lines().any(|l| l.starts_with("## ")),
            "docs/decisions/{n}: needs a `## ` heading"
        );
        for label in ["Decision", "Rationale", "Alternatives"] {
            let re = Regex::new(&format!(r"\*\*{label}(:\*\*|\*\*)")).unwrap();
            assert!(
                re.is_match(&text),
                "docs/decisions/{n}: missing **{label}**"
            );
        }
    }
}

#[test]
fn decision_names_and_unit_ids_are_unique() {
    let names = decision_files();
    assert_unique("docs/decisions", &names, |n| n.to_lowercase(), "name");
    let date = Regex::new(r"^\d{4}-\d{2}-\d{2}-").unwrap();
    assert_unique(
        "docs/decisions",
        &names,
        |n| date.replace(n, "").to_lowercase(),
        "unit id",
    );
}

#[test]
fn queue_files_are_named_by_unit_and_hold_items() {
    let re = Regex::new(r"^[a-z0-9][a-z0-9._-]*\.md$").unwrap();
    let names = md_files("docs/queue");
    for n in &names {
        assert!(re.is_match(n), "docs/queue/{n} must be <unit-id>.md");
        let text = read_repo_file(&format!("docs/queue/{n}"));
        assert!(
            text.lines().any(|l| {
                l.starts_with("- [ ]") || l.starts_with("- [x]") || l.starts_with("- ~~")
            }),
            "docs/queue/{n}: needs at least one queue line"
        );
    }
    assert_unique("docs/queue", &names, |n| n.to_lowercase(), "unit id");
}

#[test]
fn d41_decision_file_exists() {
    let text = read_repo_file("docs/decisions/2026-11-18-w7pre-log-split.md");
    assert!(text.contains("D41"));
}
