//! Contract: `docs/DECISIONS.md` has a "## 0.1 — Dependency spike results"
//! section with one answered subsection per spike question, and every
//! negative answer names a fallback.

use regex::Regex;
use spec_0_1::{Subsection, level2_section, level3_subsections, read_repo_file};

const FILE: &str = "docs/DECISIONS.md";

/// (label, pattern identifying the subsection that answers the question).
const QUESTIONS: [(&str, &str); 8] = [
    ("Q1 sdl3 Event nanosecond timestamp", r"(?i)timestamp"),
    (
        "Q2 SDL virtual joysticks headless",
        r"(?i)virtual[ _-]?joystick",
    ),
    (
        "Q3 wgpu EXPERIMENTAL_RAY_QUERY per adapter",
        r"(?i)ray[ _-]?quer|ray[ _-]?trac|\bray\b",
    ),
    (
        "Q4 present modes per adapter",
        r"(?i)present(ation)?[ _-]?modes?",
    ),
    (
        "Q5 offscreen rendering on lavapipe/WARP/Metal",
        r"(?i)off[ -]?screen",
    ),
    ("Q6a mlua memory limit + instruction hook", r"(?i)mlua"),
    ("Q6b wasmtime fuel metering", r"(?i)wasmtime"),
    (
        "Q6c parry3d cast_shapes_nonlinear capsule vs ball",
        r"(?i)parry",
    ),
];

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("test regex compiles")
}

fn section() -> String {
    let text = read_repo_file(FILE);
    let heading = re(r"(?i)^0\.1\b.*dependency spike results");
    level2_section(&text, |h| heading.is_match(h)).unwrap_or_else(|| {
        let level2: Vec<&str> = text.lines().filter(|l| l.starts_with("## ")).collect();
        panic!(
            "{FILE}: no `## 0.1 — Dependency spike results` section found; level-2 headings present: {level2:#?}"
        )
    })
}

fn subsections() -> Vec<Subsection> {
    let subs = level3_subsections(&section());
    assert!(
        !subs.is_empty(),
        "{FILE}: the 0.1 section has no `###` subsections; expected one per spike question"
    );
    subs
}

/// The subsection answering a question: a heading match wins, otherwise the
/// first subsection whose body mentions the keyword (question 6 may be one
/// combined "smoke tests" subsection).
fn find<'a>(subs: &'a [Subsection], label: &str, pattern: &str) -> &'a Subsection {
    let pattern = re(pattern);
    subs.iter()
        .find(|s| pattern.is_match(&s.heading))
        .or_else(|| subs.iter().find(|s| pattern.is_match(&s.body)))
        .unwrap_or_else(|| {
            panic!(
                "{FILE}: 0.1 section has no subsection for {label} (looked for /{pattern}/); subsections present: {:#?}",
                subs.iter().map(|s| &s.heading).collect::<Vec<_>>()
            )
        })
}

fn is_negative(body: &str) -> bool {
    re(concat!(
        r"(?i)\banswer\b[\s*_`:—–-]*(no|not|none|negative|unsupported|unavailable|fail(s|ed|ure)?)\b",
        r"|\bnegative\b|\bunsupported\b|\bunavailable\b",
        r"|\b(not|isn't|is not|aren't|are not) (supported|available|exposed|possible|present|usable)\b",
        r"|\b(does not|doesn't|did not|didn't|do not|don't|cannot|can't) (work|exist|expose|support|run|compile|build)\b",
        r"|\bfail(s|ed)\b",
    ))
    .is_match(body)
}

#[test]
fn section_0_1_exists() {
    let body = section();
    assert!(
        !body.trim().is_empty(),
        "{FILE}: the `## 0.1 — Dependency spike results` section is empty"
    );
}

#[test]
fn section_0_1_appears_exactly_once() {
    let text = read_repo_file(FILE);
    let heading = re(r"(?im)^##[ \t]+0\.1\b.*dependency spike results");
    let count = heading.find_iter(&text).count();
    assert_eq!(
        count, 1,
        "{FILE}: expected exactly one 0.1 spike-results section, found {count}"
    );
}

#[test]
fn has_at_least_one_subsection_per_question() {
    let subs = subsections();
    assert!(
        subs.len() >= 6,
        "{FILE}: 0.1 section must have one subsection per question (6), found {}: {:#?}",
        subs.len(),
        subs.iter().map(|s| &s.heading).collect::<Vec<_>>()
    );
}

#[test]
fn every_question_has_a_subsection() {
    let subs = subsections();
    for (label, pattern) in QUESTIONS {
        find(&subs, label, pattern);
    }
}

#[test]
fn questions_one_to_five_have_distinct_subsections() {
    let subs = subsections();
    let found: Vec<&Subsection> = QUESTIONS[..5]
        .iter()
        .map(|(label, pattern)| find(&subs, label, pattern))
        .collect();
    for (i, a) in found.iter().enumerate() {
        for (j, b) in found.iter().enumerate().skip(i + 1) {
            assert!(
                !std::ptr::eq(*a, *b),
                "{FILE}: {} and {} resolve to the same subsection {:?}; each question needs its own",
                QUESTIONS[i].0,
                QUESTIONS[j].0,
                a.heading
            );
        }
    }
}

#[test]
fn every_subsection_states_an_answer() {
    let subs = subsections();
    let marker = re(concat!(
        r"(?i)\b(answer|result|verdict|outcome|conclusion|finding)s?\b",
        r"|\b(yes|no)\b|\b(un)?supported\b|\b(un)?available\b|\bworks?\b|\bpass(es|ed)?\b|\bfail(s|ed)?\b",
    ));
    let placeholder = re(r"(?i)\b(TODO|TBD|FIXME|XXX)\b|\?\?\?");
    for (label, pattern) in QUESTIONS {
        let sub = find(&subs, label, pattern);
        let substance = sub.body.chars().filter(|c| c.is_alphanumeric()).count();
        assert!(
            substance >= 20,
            "{FILE}: subsection {:?} ({label}) has no real content",
            sub.heading
        );
        assert!(
            marker.is_match(&sub.body),
            "{FILE}: subsection {:?} ({label}) does not state an answer (expected e.g. `Answer: yes/no ...`)",
            sub.heading
        );
        assert!(
            !placeholder.is_match(&sub.body),
            "{FILE}: subsection {:?} ({label}) still contains a placeholder (TODO/TBD/???)",
            sub.heading
        );
    }
}

#[test]
fn negative_answers_name_a_fallback() {
    let subs = subsections();
    let fallback = re(r"(?i)fall[ -]?back");
    for (label, pattern) in QUESTIONS {
        let sub = find(&subs, label, pattern);
        if is_negative(&sub.body) {
            assert!(
                fallback.is_match(&sub.body),
                "{FILE}: subsection {:?} ({label}) reports a negative or partially negative result \
                 but does not name a fallback (the word \"fallback\" must appear in that subsection)",
                sub.heading
            );
        }
    }
}

#[test]
fn negative_detector_self_check() {
    for text in [
        "**Answer:** No. The field is absent.",
        "Answer: not available on lavapipe",
        "Ray query is unsupported on WARP.",
        "The adapter does not support Mailbox.",
        "Creation failed under the dummy driver.",
    ] {
        assert!(
            is_negative(text),
            "should be detected as negative: {text:?}"
        );
    }
    for text in [
        "**Answer:** Yes. `Event::timestamp` is in nanoseconds.",
        "Answer: yes — works on all three adapters, no caveats.",
        "Fuel metering traps deterministically.",
    ] {
        assert!(
            !is_negative(text),
            "should not be detected as negative: {text:?}"
        );
    }
}
