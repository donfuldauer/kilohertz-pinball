//! Helpers for the Subphase 0.1 conformance tests.
//!
//! Everything here works from files on disk only; nothing under `spikes/`
//! other than this crate is read.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

static EXACT_PIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^=(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)",
        r"(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?",
        r"(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$",
    ))
    .expect("exact-pin regex compiles")
});

/// Returns `true` when `s` is an exact Cargo version pin: a leading `=`
/// followed by `MAJOR.MINOR.PATCH`, optionally with a semver pre-release
/// (`-rc.1`) and/or build-metadata (`+spec-1.1.0`) suffix.
///
/// No surrounding whitespace, no wildcards, no partial versions, no ranges.
pub fn is_exact_pin(s: &str) -> bool {
    EXACT_PIN.is_match(s)
}

/// Splits an exact pin into its numeric `(major, minor, patch)` triple.
/// Returns `None` if `s` is not an exact pin or a component overflows `u64`.
pub fn pin_triple(s: &str) -> Option<(u64, u64, u64)> {
    let caps = EXACT_PIN.captures(s)?;
    let n = |i: usize| caps.get(i)?.as_str().parse::<u64>().ok();
    Some((n(1)?, n(2)?, n(3)?))
}

/// Repository root: two levels above this crate (`spikes/spec_0_1/../..`).
///
/// `SPEC_0_1_ROOT` overrides it so the checks themselves can be exercised
/// against a fixture tree; CI and normal runs leave it unset.
pub fn repo_root() -> PathBuf {
    match std::env::var_os("SPEC_0_1_ROOT") {
        Some(root) if !root.is_empty() => PathBuf::from(root),
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."),
    }
}

/// Reads a file relative to the repository root, panicking with a message
/// that names the missing deliverable.
pub fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => panic!(
            "Subphase 0.1 deliverable `{rel}` could not be read at {}: {err}",
            path.display()
        ),
    }
}

/// One `###` subsection of a markdown section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subsection {
    pub heading: String,
    pub body: String,
}

fn heading_level(line: &str) -> Option<(usize, &str)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    if rest.is_empty() {
        return Some((hashes, ""));
    }
    rest.starts_with([' ', '\t'])
        .then(|| (hashes, rest.trim().trim_end_matches('#').trim_end()))
}

/// Extracts the body of the first level-2 (`## `) section whose heading
/// satisfies `matches`, up to the next level-1/2 heading. Fenced code blocks
/// are not treated as headings.
pub fn level2_section(markdown: &str, matches: impl Fn(&str) -> bool) -> Option<String> {
    let mut out: Option<String> = None;
    let mut in_fence = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        let heading = if in_fence { None } else { heading_level(line) };
        match (&mut out, heading) {
            (None, Some((2, title))) if matches(title) => out = Some(String::new()),
            (Some(_), Some((level, _))) if level <= 2 => break,
            (Some(body), _) => {
                body.push_str(line);
                body.push('\n');
            }
            (None, _) => {}
        }
    }
    out
}

/// Splits a level-2 section body into its level-3 (`### `) subsections.
/// Deeper headings stay inside the body of their parent subsection.
pub fn level3_subsections(section: &str) -> Vec<Subsection> {
    let mut subs: Vec<Subsection> = Vec::new();
    let mut in_fence = false;
    for line in section.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        match heading_level(line) {
            Some((3, title)) if !in_fence => subs.push(Subsection {
                heading: title.to_owned(),
                body: String::new(),
            }),
            _ => {
                if let Some(last) = subs.last_mut() {
                    last.body.push_str(line);
                    last.body.push('\n');
                }
            }
        }
    }
    subs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_pin_examples() {
        for ok in [
            "=1.2.3",
            "=0.0.0",
            "=27.0.1",
            "=1.0.0-rc.1",
            "=0.3.0-alpha",
            "=1.2.1+spec-1.1.0",
        ] {
            assert!(is_exact_pin(ok), "{ok:?} should be accepted");
        }
        for bad in [
            "",
            "=",
            "1.2.3",
            "^1.2.3",
            "~1.2.3",
            ">=1.2.3",
            "=1.2",
            "=1",
            "=1.2.*",
            "= 1.2.3",
            " =1.2.3",
            "=1.2.3 ",
            "=1.2.3\n",
            "==1.2.3",
            "=1.2.3.4",
            "=01.2.3",
            "=1.2.3-",
            "=v1.2.3",
            "=1.2.3, <2",
            "=١.٢.٣",
        ] {
            assert!(!is_exact_pin(bad), "{bad:?} should be rejected");
        }
        assert_eq!(pin_triple("=27.0.1-rc.1"), Some((27, 0, 1)));
        assert_eq!(pin_triple("27.0.1"), None);
    }

    #[test]
    fn markdown_sectioning() {
        let md = "# T\n## A\nx\n## 0.1 — Dep\nintro\n### One\na\n```\n## not a heading\n```\n#### deep\nb\n### Two ##\nc\n## Z\nq\n";
        let sec = level2_section(md, |h| h.starts_with("0.1")).expect("section");
        assert!(!sec.contains('q') && sec.contains("not a heading"));
        let subs = level3_subsections(&sec);
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].heading, "One");
        assert!(subs[0].body.contains("deep") && subs[0].body.contains('b'));
        assert_eq!(subs[1].heading, "Two");
        assert!(level2_section(md, |h| h == "missing").is_none());
    }
}
