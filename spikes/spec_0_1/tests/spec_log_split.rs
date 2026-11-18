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

/// Checks every `## ` section of a decision file carries the three labels.
fn check_decision_text(text: &str) -> Result<(), String> {
    let heading = Regex::new(r"^## (\S.*?) — (\S.*)$").unwrap();
    let mut sections: Vec<Vec<&str>> = Vec::new();
    for l in text.lines() {
        if l.starts_with("## ") || l == "##" {
            if !heading.is_match(l) {
                return Err(format!("heading `{l}` must be `## <id> — <topic>`"));
            }
            sections.push(Vec::new());
        }
        if let Some(cur) = sections.last_mut() {
            cur.push(l);
        }
    }
    if sections.is_empty() {
        return Err("needs a `## ` heading".into());
    }
    for sec in &sections {
        let body = sec.join("\n");
        for label in ["Decision", "Rationale", "Alternatives"] {
            let re = Regex::new(&format!(r"\*\*{label}(:\*\*|\*\*)")).unwrap();
            if !re.is_match(&body) {
                return Err(format!("section `{}` missing **{label}**", sec[0]));
            }
        }
    }
    Ok(())
}

/// Checks a queue file has at least one line and that checkbox lines are well formed.
fn check_queue_text(text: &str) -> Result<(), String> {
    let boxed = Regex::new(r"^- \[( |x)\] \[[^\]]+\] \S").unwrap();
    let legacy = Regex::new(r"^- \[[^\]]+\] \S").unwrap();
    let mut any = false;
    for l in text.lines() {
        if l.starts_with("- [ ]") || l.starts_with("- [x]") {
            if !boxed.is_match(l) {
                return Err(format!("malformed checkbox line: `{l}`"));
            }
            any = true;
        } else if l.starts_with("- ~~") || legacy.is_match(l) {
            any = true;
        }
    }
    if any {
        Ok(())
    } else {
        Err("needs at least one queue line".into())
    }
}

#[test]
fn decision_files_have_heading_and_labels() {
    for n in decision_files() {
        let text = read_repo_file(&format!("docs/decisions/{n}"));
        if let Err(e) = check_decision_text(&text) {
            panic!("docs/decisions/{n}: {e}");
        }
    }
}

const SECTION: &str =
    "## a-1 — first topic\n**Decision:** x\n**Rationale:** y\n**Alternatives:** z\n";

#[test]
fn decision_check_rejects_second_section_without_rationale() {
    let text = format!("{SECTION}\n## b-2 — second\n**Decision:** x\n**Alternatives:** z\n");
    let e = check_decision_text(&text).unwrap_err();
    assert!(e.contains("## b-2") && e.contains("Rationale"), "{e}");
}

#[test]
fn decision_check_rejects_malformed_headings() {
    let body = "**Decision:** x\n**Rationale:** y\n**Alternatives:** z\n";
    for bad in ["## ", "##", "## a", "## a — ", "## — topic", "## a - topic"] {
        let text = format!("{bad}\n{body}");
        assert!(check_decision_text(&text).is_err(), "accepted `{bad}`");
    }
}

#[test]
fn decision_check_accepts_multi_section_and_title() {
    let text = format!(
        "# Title\n\n{SECTION}\n## b-2 — second\n**Decision** x\n**Rationale** y\n**Alternatives** z\n"
    );
    assert_eq!(check_decision_text(&text), Ok(()));
    assert!(check_decision_text("no heading").is_err());
}

#[test]
fn queue_check_rejects_bare_checkbox() {
    assert!(check_queue_text("- [ ]\n").is_err());
    assert!(check_queue_text("- [ ] [id]\n").is_err());
    assert!(check_queue_text("- [x] no id here\n").is_err());
}

#[test]
fn queue_check_accepts_checkbox_and_legacy_lines() {
    assert_eq!(check_queue_text("- [ ] [a-1] open\n"), Ok(()));
    assert_eq!(check_queue_text("- [x] [a-1] done\n"), Ok(()));
    assert_eq!(check_queue_text("- [a-1] legacy text\n"), Ok(()));
    assert_eq!(check_queue_text("- ~~[a-1] struck~~ Resolved: x\n"), Ok(()));
}

#[test]
fn queue_check_accepts_frozen_item_update_and_resolution_forms() {
    let upd = "- [ ] [a-1] update: new info (frozen HUMAN_QUEUE.md item: \"quote\")\n";
    let res = "- [x] [a-1] resolved: done (frozen HUMAN_QUEUE.md item: \"quote\")\n";
    assert_eq!(check_queue_text(upd), Ok(()));
    assert_eq!(check_queue_text(res), Ok(()));
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
        if let Err(e) = check_queue_text(&text) {
            panic!("docs/queue/{n}: {e}");
        }
    }
    assert_unique("docs/queue", &names, |n| n.to_lowercase(), "unit id");
}

#[test]
fn d41_decision_file_exists() {
    let text = read_repo_file("docs/decisions/2026-11-18-w7pre-log-split.md");
    assert!(text.contains("D41"));
}
