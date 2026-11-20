//! Contract (DECISIONS.md "Process — One decision log and a P8-only queue (D41
//! superseded)"): `docs/DECISIONS.md` is the single live decision log and
//! `docs/HUMAN_QUEUE.md` holds only P8 items (PROCESS.md P8). The per-unit
//! folders of D41, `docs/decisions/` and `docs/queue/`, were merged into those
//! two files and no longer exist.

use proptest::prelude::*;
use regex::Regex;
use spec_0_1::{read_repo_file, repo_root};
use std::collections::BTreeSet;
use std::sync::LazyLock;

const LOG: &str = "docs/DECISIONS.md";
const QUEUE: &str = "docs/HUMAN_QUEUE.md";

/// Last section of the log as it was frozen on 2026-11-18 (D41). The sections
/// before it keep their original layout; every section after it follows the
/// current format.
const LAST_FROZEN: &str = "w7pre-ci-main-no-cancel — main push runs are not cancelled (2026-11-18)";

/// Heading prefix of the markers that introduce each merged per-unit decision
/// file (they hold the file's title and preamble, not a decision).
const IMPORTED: &str = "Imported — docs/decisions/";

/// The kinds of `[P8 <kind>]` queue items (PROCESS.md P8).
const P8_KINDS: [&str; 6] = ["spec", "hardware", "assets", "release", "legal", "setting"];

static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ {0,3}(`{3,}|~{3,})(.*)$").unwrap());
static ENTRY_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\S.*?) — (\S.*)$").unwrap());
static IMPORTED_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}-[a-z0-9][a-z0-9._-]*\.md$").unwrap());
static LIST_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*- (.*)$").unwrap());
static P8_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[P8 ([a-z]+)\] \[[^\]\s][^\]]*\] \S").unwrap());

/// Lines outside fenced code blocks. A block closes only on a fence of the
/// opening character, at least as long as the opening run, with nothing but
/// whitespace after it (CommonMark).
fn unfenced(text: &str) -> Vec<&str> {
    // Opening fence character and run length while inside a block.
    let mut open: Option<(char, usize)> = None;
    let mut out = Vec::new();
    for l in text.lines() {
        let fence = FENCE.captures(l).map(|c| {
            let run = c.get(1).map_or("", |m| m.as_str());
            let rest = c.get(2).map_or("", |m| m.as_str());
            (run.chars().next().unwrap_or('`'), run.len(), rest)
        });
        match (open, fence) {
            (None, Some((ch, len, _))) => {
                open = Some((ch, len));
                continue;
            }
            (Some((och, olen)), Some((ch, len, rest)))
                if ch == och && len >= olen && rest.trim().is_empty() =>
            {
                open = None;
                continue;
            }
            _ => {}
        }
        if open.is_none() {
            out.push(l);
        }
    }
    out
}

/// `## ` sections as (heading text, body lines), fenced code ignored. The text
/// before the first section is returned separately.
fn sections(text: &str) -> (Vec<&str>, Vec<(String, Vec<&str>)>) {
    let mut preamble = Vec::new();
    let mut secs: Vec<(String, Vec<&str>)> = Vec::new();
    for l in unfenced(text) {
        if l.starts_with("## ") || l == "##" {
            secs.push((l.get(3..).unwrap_or("").to_owned(), Vec::new()));
        } else if let Some((_, body)) = secs.last_mut() {
            body.push(l);
        } else {
            preamble.push(l);
        }
    }
    (preamble, secs)
}

/// A decision entry: `## <work item> — <title>` with the three labels.
fn check_entry(heading: &str, body: &[&str]) -> Result<(), String> {
    if !ENTRY_HEADING.is_match(heading) {
        return Err(format!(
            "heading `## {heading}` must be `## <work item> — <title>`"
        ));
    }
    let body = body.join("\n");
    for label in ["Decision", "Rationale", "Alternatives"] {
        let re = Regex::new(&format!(r"\*\*{label}(\*\*|[:.]\*\*)")).unwrap();
        if !re.is_match(&body) {
            return Err(format!("section `## {heading}` missing **{label}**"));
        }
    }
    Ok(())
}

/// The whole log: a `# Decisions` title, no frozen banner, unique `## `
/// headings, and every section after the frozen part either a well-formed
/// entry or an import marker (markers in file-name order).
fn check_log(text: &str) -> Result<(), String> {
    let (preamble, secs) = sections(text);
    if preamble.first() != Some(&"# Decisions") {
        return Err("must start with `# Decisions`".into());
    }
    if let Some(l) = preamble
        .iter()
        .find(|l| l.to_lowercase().contains("frozen"))
    {
        return Err(format!("frozen banner or note in the header: `{l}`"));
    }
    let mut seen = BTreeSet::new();
    for (h, _) in &secs {
        if h.trim().is_empty() {
            return Err("empty `## ` heading".into());
        }
        if !seen.insert(h.as_str()) {
            return Err(format!("duplicate heading `## {h}`"));
        }
    }
    let Some(start) = secs.iter().position(|(h, _)| h == LAST_FROZEN) else {
        return Err(format!("frozen-part boundary `## {LAST_FROZEN}` not found"));
    };
    let mut last_import: Option<&str> = None;
    for (h, body) in &secs[start + 1..] {
        if let Some(name) = h.strip_prefix(IMPORTED) {
            if !IMPORTED_NAME.is_match(name) {
                return Err(format!(
                    "import marker `## {h}` must name a <YYYY-MM-DD>-<unit-id>.md file"
                ));
            }
            if last_import.is_some_and(|prev| prev >= name) {
                return Err(format!("import marker `## {h}` out of file-name order"));
            }
            last_import = Some(name);
        } else {
            check_entry(h, body)?;
        }
    }
    Ok(())
}

/// One queue list line: struck through with a disposition after the closing
/// `~~`, or an open `[P8 <kind>] [<id>] text` item.
fn check_queue_line(line: &str) -> Result<(), String> {
    let Some(caps) = LIST_LINE.captures(line) else {
        return Ok(());
    };
    let body = caps.get(1).map_or("", |m| m.as_str());
    if let Some(rest) = body.strip_prefix("~~") {
        return match rest.find("~~") {
            None => Err(format!("unclosed strike-through: `{line}`")),
            Some(i) if rest[i + 2..].trim().is_empty() => {
                Err(format!("struck line without a disposition: `{line}`"))
            }
            Some(_) => Ok(()),
        };
    }
    match P8_LINE.captures(body) {
        Some(c) if P8_KINDS.contains(&c.get(1).map_or("", |m| m.as_str())) => Ok(()),
        _ => Err(format!(
            "open line is not a `[P8 <kind>] [<id>] text` item (kinds {P8_KINDS:?}): `{line}`"
        )),
    }
}

/// The whole queue: a `# Human Queue` title and only P8 items open. Returns the
/// number of open items.
fn check_queue(text: &str) -> Result<usize, String> {
    let lines = unfenced(text);
    if lines.first() != Some(&"# Human Queue") {
        return Err("must start with `# Human Queue`".into());
    }
    let mut open = 0;
    for l in lines {
        check_queue_line(l)?;
        if let Some(c) = LIST_LINE.captures(l)
            && !c.get(1).map_or("", |m| m.as_str()).starts_with("~~")
        {
            open += 1;
        }
    }
    Ok(open)
}

#[test]
fn per_unit_folders_are_gone() {
    for dir in ["docs/decisions", "docs/queue"] {
        assert!(
            !repo_root().join(dir).exists(),
            "{dir} must not exist: its content belongs in {LOG} / {QUEUE}"
        );
    }
}

#[test]
fn decision_log_is_single_live_and_well_formed() {
    if let Err(e) = check_log(&read_repo_file(LOG)) {
        panic!("{LOG}: {e}");
    }
}

#[test]
fn queue_holds_only_p8_items() {
    if let Err(e) = check_queue(&read_repo_file(QUEUE)) {
        panic!("{QUEUE}: {e}");
    }
}

#[test]
fn d41_entry_is_kept_and_superseded() {
    let text = read_repo_file(LOG);
    let (_, secs) = sections(&text);
    let pos = |h: &str| secs.iter().position(|(s, _)| s == h);
    let d41 = pos("w7pre-log-split — one file per decision and queue entry (user ruling D41)")
        .expect("the D41 entry of the merged per-unit files is missing");
    let sup = pos("Process — One decision log and a P8-only queue (D41 superseded)")
        .expect("the entry superseding D41 is missing");
    assert!(
        d41 < sup,
        "the superseding entry must come after the D41 entry"
    );
}

const ENTRY: &str =
    "## a-1 — first topic\n**Decision:** x\n**Rationale:** y\n**Alternatives:** z\n";

fn log_with(after_frozen: &str) -> String {
    format!(
        "# Decisions\n\nHeader.\n\n## old — legacy entry without labels\ntext\n\n## {LAST_FROZEN}\ntext\n\n{after_frozen}"
    )
}

#[test]
fn log_check_accepts_entries_and_import_markers() {
    let text = log_with(&format!(
        "## {IMPORTED}2026-11-18-a.md\n\n**Title**\n\n{ENTRY}\n## {IMPORTED}2026-11-19-b.md\n\n## b-2 — second\n**Decision.** x\n**Rationale.** y\n**Alternatives.** z\n"
    ));
    assert_eq!(check_log(&text), Ok(()));
}

#[test]
fn log_check_rejects_a_frozen_banner() {
    let text = log_with(ENTRY).replace(
        "Header.",
        "> Frozen history as of 2026-11-18 (D41). New entries go elsewhere.",
    );
    let e = check_log(&text).unwrap_err();
    assert!(e.contains("frozen"), "{e}");
}

#[test]
fn log_check_rejects_duplicate_headings_but_not_fenced_ones() {
    let dup = log_with(&format!("{ENTRY}\n{ENTRY}"));
    let e = check_log(&dup).unwrap_err();
    assert!(e.contains("duplicate") && e.contains("a-1"), "{e}");
    let fenced = log_with(&format!("{ENTRY}\n```\n## a-1 — first topic\n```\n"));
    assert_eq!(check_log(&fenced), Ok(()));
}

#[test]
fn unfenced_closes_a_block_only_on_a_matching_fence() {
    // A four-backtick block holding a three-backtick example: the inner fence
    // must not end the block (CommonMark closing-fence rule).
    let text = "before\n````md\n```\n## inner — heading\n```\nstill inside\n````\nafter\n\
                ~~~\n```\n## tilde inner\n~~~\nlast\n";
    assert_eq!(unfenced(text), vec!["before", "after", "last"]);
    // A longer closing fence closes; a closing fence with an info string does not.
    let text = "```\n```x\n## still inside\n`````\nout\n";
    assert_eq!(unfenced(text), vec!["out"]);
    // The documentation check sees no duplicate from the nested example.
    let nested = log_with(&format!(
        "{ENTRY}\n````\n```\n## a-1 — first topic\n```\n````\n"
    ));
    assert_eq!(check_log(&nested), Ok(()));
}

#[test]
fn log_check_requires_the_format_only_after_the_frozen_part() {
    let missing = log_with("## b-2 — second\n**Decision:** x\n**Alternatives:** z\n");
    let e = check_log(&missing).unwrap_err();
    assert!(e.contains("## b-2") && e.contains("Rationale"), "{e}");
    let no_boundary = log_with(ENTRY).replace(&format!("## {LAST_FROZEN}"), "## other — x");
    assert!(check_log(&no_boundary).unwrap_err().contains("boundary"));
}

#[test]
fn log_check_rejects_malformed_headings_and_markers() {
    let body = "**Decision:** x\n**Rationale:** y\n**Alternatives:** z\n";
    for bad in ["## a", "## a — ", "## — topic", "## a - topic"] {
        assert!(
            check_log(&log_with(&format!("{bad}\n{body}"))).is_err(),
            "accepted `{bad}`"
        );
    }
    assert!(
        check_log(&log_with("##\n")).is_err(),
        "accepted an empty heading"
    );
    let unordered = log_with(&format!(
        "## {IMPORTED}2026-11-19-b.md\n\n## {IMPORTED}2026-11-18-a.md\n"
    ));
    assert!(check_log(&unordered).unwrap_err().contains("order"));
    let bad_name = log_with(&format!("## {IMPORTED}notes.md\n"));
    assert!(check_log(&bad_name).is_err());
}

#[test]
fn entry_check_accepts_each_label_style() {
    for sep in [":**", ".**", "**"] {
        let body = format!("**Decision{sep} x\n**Rationale{sep} y\n**Alternatives{sep} z");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(check_entry("a — b", &lines), Ok(()), "style {sep}");
    }
}

#[test]
fn queue_check_accepts_p8_and_struck_lines() {
    let text = "# Human Queue\n\nIntro text.\n\n- [P8 hardware] [3.2] Check the plunger.\n\
                - ~~[3.2] old text~~ Kept open as the P8 line below.\n  - ~~sub item~~ Resolved: x\n\
                - ~~[ ] [a-1] open then~~ Decided: DECISIONS.md \"a-1 — topic\"\n\
                - ~~[x] [a-2] done~~ Resolved: y\n- [P8 setting] [#12] Make it required.\n";
    assert_eq!(check_queue(text), Ok(2));
}

#[test]
fn queue_check_rejects_open_lines_that_are_not_p8() {
    for bad in [
        "- [3.2] an old-style open line",
        "- [ ] [a-1] an open checkbox line",
        "- [x] [a-1] a checked line that is not struck",
        "  - an indented open sub-item",
        "- no id at all",
        "- [P8 wish] [a-1] an unknown kind",
        "- [P8 hardware] no id",
        "- [P8 hardware] [] empty id",
        "- ~~struck without a disposition~~",
        "- ~~struck but never closed",
    ] {
        let text = format!("# Human Queue\n\n{bad}\n");
        assert!(check_queue(&text).is_err(), "accepted `{bad}`");
    }
    assert!(check_queue("# Queue\n").is_err(), "accepted a wrong title");
}

proptest! {
    #[test]
    fn checks_never_panic_on_arbitrary_text(s in any::<String>()) {
        let _ = check_log(&s);
        let _ = check_queue(&s);
        let _ = check_queue_line(&s);
    }

    #[test]
    fn checks_never_panic_on_markdown_shaped_noise(
        lines in proptest::collection::vec(r"(## |- |  - |- ~~|```|# )?[^\n]{0,40}", 0..30)
    ) {
        let text = lines.join("\n");
        let _ = check_log(&format!("# Decisions\n{text}"));
        let _ = check_queue(&format!("# Human Queue\n{text}"));
    }

    #[test]
    fn well_formed_p8_lines_are_accepted(
        kind in proptest::sample::select(P8_KINDS.to_vec()),
        id in r"[A-Za-z0-9#.-][A-Za-z0-9 #.-]{0,20}",
        text in r"[A-Za-z0-9][^\n]{0,60}",
        indent in r" {0,4}",
    ) {
        let line = format!("{indent}- [P8 {kind}] [{id}] {text}");
        prop_assert_eq!(check_queue_line(&line), Ok(()));
    }

    #[test]
    fn struck_lines_need_a_disposition(
        old in r"[^~\n]{1,60}",
        disposition in r"[A-Za-z][^\n]{0,40}",
    ) {
        let with = format!("- ~~{old}~~ {disposition}");
        prop_assert_eq!(check_queue_line(&with), Ok(()));
        let without = format!("- ~~{old}~~");
        prop_assert!(check_queue_line(&without).is_err());
    }
}
