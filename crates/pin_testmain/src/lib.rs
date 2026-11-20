//! Libtest-compatible argument handling for `harness = false` test targets
//! (dev-only; w8h-h).
//!
//! `cargo test` passes everything after `--` (filters, `--list`, `--exact`,
//! `--skip`, `--nocapture`, ...) to every test executable, including the
//! `harness = false` ones whose plain `main` would otherwise ignore them and
//! open SDL windows on an unrelated filtered run. Each such target names its
//! single test after its `[[test]] name`, renames its body to `run` and calls
//! [`libtest_main`] from `main`:
//!
//! ```no_run
//! fn run() {
//!     // SDL / GPU work, on the process main thread.
//! }
//!
//! fn main() {
//!     pin_testmain::libtest_main("gpu_window", run)
//! }
//! ```
//!
//! [`parse_args`] and [`plan`] are pure; [`libtest_main`] only formats libtest's
//! output and dispatches on the [`Plan`]. The helper spawns no thread and
//! touches no SDL or GPU state, and it calls `body` on the calling thread.

use std::io::Write as _;
use std::time::Instant;

/// The libtest options that matter to a target with a single test, parsed
/// by [`parse_args`]. Every other libtest option is accepted and ignored.
#[derive(Debug, Default, PartialEq, Clone)]
pub struct Options {
    /// `--list`: list the selected tests instead of running them.
    pub list: bool,
    /// `--exact`: filters and skips match the whole test name, not a substring.
    pub exact: bool,
    /// `--ignored`: run only `#[ignore]`d tests (this target has none).
    pub ignored: bool,
    /// `--include-ignored`: also run `#[ignore]`d tests (no effect here).
    pub include_ignored: bool,
    /// `--help` / `-h`: print a usage line instead of running.
    pub help: bool,
    /// Positional filters; the test is selected if any of them matches
    /// (all tests are selected when this is empty).
    pub filters: Vec<String>,
    /// `--skip` values; the test is deselected if any of them matches.
    pub skips: Vec<String>,
}

/// What [`libtest_main`] does for its single test, as decided by [`plan`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Plan {
    /// The test is selected and not listing: run the body.
    Run,
    /// `--list`: print the test (if selected) and exit without running it.
    List,
    /// The test is filtered out: report `running 0 tests` and exit.
    SkipAll,
}

/// libtest options whose value is consumed and ignored (`--opt value` or `--opt=value`).
const VALUE_OPTIONS: [&str; 6] = [
    "--test-threads",
    "--format",
    "--color",
    "--logfile",
    "--report-time",
    "--shuffle-seed",
];

/// Parses libtest arguments. `args` excludes `argv[0]`.
///
/// Pure: no I/O, never panics, never exits. Unknown `-x` / `--x` flags are
/// accepted and ignored; any argument that does not start with `-` (and is
/// not consumed as an option's value) is a positional filter.
pub fn parse_args<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> Options {
    let mut opts = Options::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let arg = arg.as_ref();
        if !arg.starts_with('-') {
            opts.filters.push(arg.to_owned());
            continue;
        }
        let (name, inline_value) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v)),
            _ => (arg, None),
        };
        match name {
            "--" => {}
            "--list" => opts.list = true,
            "--exact" => opts.exact = true,
            "--ignored" => opts.ignored = true,
            "--include-ignored" => opts.include_ignored = true,
            "--help" | "-h" => opts.help = true,
            "--skip" => {
                let value = match inline_value {
                    Some(v) => Some(v.to_owned()),
                    None => iter.next().map(|v| v.as_ref().to_owned()),
                };
                if let Some(v) = value {
                    opts.skips.push(v);
                }
            }
            "-Z" => {
                // `-Z <flag>`: consume the flag. (`-Zflag` falls to the catch-all.)
                let _ = iter.next();
            }
            // `--opt value`: consume the value (`--opt=value` falls to the catch-all).
            n if VALUE_OPTIONS.contains(&n) && inline_value.is_none() => {
                let _ = iter.next();
            }
            // --opt=value of a value option, --nocapture, --no-capture, --show-output, --quiet, -q, --test,
            // --bench, --force-run-in-process, --ensure-time, --shuffle, -Zflag
            // and any unknown flag: accepted and ignored.
            _ => {}
        }
    }
    opts
}

/// Whether a filter or skip pattern matches `test_name` under `exact`.
fn pattern_matches(exact: bool, pattern: &str, test_name: &str) -> bool {
    if exact {
        test_name == pattern
    } else {
        test_name.contains(pattern)
    }
}

/// Whether libtest's filter/skip logic selects the (never ignored) test `test_name`.
fn selected(opts: &Options, test_name: &str) -> bool {
    if opts.ignored {
        // Only `#[ignore]`d tests would run, and this test is not ignored.
        return false;
    }
    let filtered_in = opts.filters.is_empty()
        || opts
            .filters
            .iter()
            .any(|f| pattern_matches(opts.exact, f, test_name));
    let skipped = opts
        .skips
        .iter()
        .any(|s| pattern_matches(opts.exact, s, test_name));
    filtered_in && !skipped
}

/// Decides what to do with the single test `test_name` under `opts`. Pure.
///
/// [`Plan::List`] whenever `opts.list` is set (whether or not the test is
/// selected), [`Plan::Run`] when the test is selected, otherwise
/// [`Plan::SkipAll`].
pub fn plan(opts: &Options, test_name: &str) -> Plan {
    if opts.list {
        Plan::List
    } else if selected(opts, test_name) {
        Plan::Run
    } else {
        Plan::SkipAll
    }
}

/// Flushes stdout and exits the process with status 0.
fn exit_ok() -> ! {
    let _ = std::io::stdout().flush();
    std::process::exit(0)
}

/// The `main` of a `harness = false` target with one test named `test_name`.
///
/// Reads `std::env::args().skip(1)`, prints libtest-style output to stdout
/// and either runs `body` once on the calling thread (then returns) or exits
/// the process with status 0 without running it (`--help`, `--list`, or the
/// test filtered out). A panic in `body` propagates unchanged. Call it before
/// any SDL or GPU initialisation; it spawns no thread.
pub fn libtest_main(test_name: &str, body: impl FnOnce()) {
    let opts = parse_args(std::env::args().skip(1));
    if opts.help {
        println!(
            "{test_name}: libtest-compatible harness=false test; accepts --list, --exact, --skip and filters"
        );
        exit_ok();
    }
    match plan(&opts, test_name) {
        Plan::List => {
            if selected(&opts, test_name) {
                println!("{test_name}: test");
                print!("\n1 test, 0 benchmarks\n");
            } else {
                print!("\n0 tests, 0 benchmarks\n");
            }
            exit_ok();
        }
        Plan::SkipAll => {
            print!(
                "\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s\n"
            );
            exit_ok();
        }
        Plan::Run => {
            print!("\nrunning 1 test\n");
            let _ = std::io::stdout().flush();
            let start = Instant::now();
            body();
            let elapsed = start.elapsed().as_secs_f64();
            print!(
                "test {test_name} ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in {elapsed:.2}s\n"
            );
            let _ = std::io::stdout().flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAME: &str = "orchestrator_window";

    fn p(args: &[&str]) -> Options {
        parse_args(args.iter().copied())
    }

    #[test]
    fn empty_args_run() {
        let o = p(&[]);
        assert_eq!(o, Options::default());
        assert!(selected(&o, NAME));
        assert_eq!(plan(&o, NAME), Plan::Run);
    }

    #[test]
    fn list_lists() {
        let o = p(&["--list"]);
        assert!(o.list);
        assert!(selected(&o, NAME));
        assert_eq!(plan(&o, NAME), Plan::List);
    }

    #[test]
    fn list_format_terse() {
        let o = p(&["--list", "--format", "terse"]);
        assert!(o.list);
        assert!(o.filters.is_empty(), "terse must not become a filter");
        assert!(selected(&o, NAME));
        assert_eq!(plan(&o, NAME), Plan::List);
    }

    #[test]
    fn list_ignored_lists_nothing() {
        let o = p(&["--list", "--ignored"]);
        assert!(o.list && o.ignored);
        assert_eq!(plan(&o, NAME), Plan::List);
        assert!(!selected(&o, NAME));
    }

    #[test]
    fn list_with_non_matching_filter_lists_nothing() {
        let o = p(&["--list", "some_unrelated_name"]);
        assert_eq!(plan(&o, NAME), Plan::List);
        assert!(!selected(&o, NAME));
    }

    #[test]
    fn substring_filter() {
        let o = p(&["orchestrator"]);
        assert_eq!(o.filters, vec!["orchestrator".to_owned()]);
        assert_eq!(plan(&o, NAME), Plan::Run);
        let o = p(&["window"]);
        assert_eq!(plan(&o, NAME), Plan::Run);
        let o = p(&["some_unrelated_name"]);
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
    }

    #[test]
    fn exact_filter() {
        let o = p(&["--exact", "orchestrator_window"]);
        assert!(o.exact);
        assert_eq!(plan(&o, NAME), Plan::Run);
        let o = p(&["--exact", "orchestrator"]);
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
        // Flag order does not matter.
        let o = p(&["orchestrator", "--exact"]);
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
    }

    #[test]
    fn skip_forms() {
        let o = p(&["--skip", "orchestrator"]);
        assert_eq!(o.skips, vec!["orchestrator".to_owned()]);
        assert!(o.filters.is_empty());
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
        let o = p(&["--skip=orchestrator"]);
        assert_eq!(o.skips, vec!["orchestrator".to_owned()]);
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
        let o = p(&["--skip", "unrelated"]);
        assert_eq!(plan(&o, NAME), Plan::Run);
    }

    #[test]
    fn exact_skip() {
        let o = p(&["--exact", "--skip", "orchestrator"]);
        assert_eq!(plan(&o, NAME), Plan::Run);
        let o = p(&["--exact", "--skip", NAME]);
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
    }

    #[test]
    fn skip_beats_filter() {
        let o = p(&["orchestrator", "--skip", "window"]);
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
    }

    #[test]
    fn value_options_consume_their_values() {
        for args in [
            &["--test-threads", "1"][..],
            &["--test-threads=1"],
            &["--color", "always"],
            &["--format=terse"],
            &["--format", "json"],
            &["--logfile", "out.txt"],
            &["--report-time", "plain"],
            &["--shuffle-seed", "42"],
            &["--shuffle-seed=42"],
            &["-Z", "unstable-options"],
            &["-Zunstable-options"],
        ] {
            let o = p(args);
            assert_eq!(o, Options::default(), "{args:?}");
            assert_eq!(plan(&o, NAME), Plan::Run, "{args:?}");
        }
        // The value is consumed even when it looks like a filter for this test.
        let o = p(&["--color", NAME, "unrelated"]);
        assert_eq!(o.filters, vec!["unrelated".to_owned()]);
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
    }

    #[test]
    fn ignored_flags_are_accepted() {
        for flag in [
            "--nocapture",
            "--no-capture",
            "-q",
            "--quiet",
            "--show-output",
            "--test",
            "--bench",
            "--force-run-in-process",
            "--ensure-time",
            "--shuffle",
            "--frobnicate",
            "--frobnicate=3",
            "-y",
            "-",
        ] {
            let o = p(&[flag]);
            assert_eq!(o, Options::default(), "{flag}");
            assert_eq!(plan(&o, NAME), Plan::Run, "{flag}");
        }
        // An unknown flag does not swallow the next argument.
        let o = p(&["--frobnicate", "unrelated"]);
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
    }

    #[test]
    fn lone_double_dash_is_ignored() {
        let o = p(&["--"]);
        assert_eq!(o, Options::default());
        assert_eq!(plan(&o, NAME), Plan::Run);
        let o = p(&["--", "unrelated"]);
        assert_eq!(o.filters, vec!["unrelated".to_owned()]);
    }

    #[test]
    fn empty_filter_matches_everything() {
        let o = p(&[""]);
        assert_eq!(o.filters, vec![String::new()]);
        assert_eq!(plan(&o, NAME), Plan::Run);
        assert_eq!(plan(&o, ""), Plan::Run);
    }

    #[test]
    fn ignored_skips_all() {
        let o = p(&["--ignored"]);
        assert!(o.ignored);
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
    }

    #[test]
    fn include_ignored_runs() {
        let o = p(&["--include-ignored"]);
        assert!(o.include_ignored);
        assert_eq!(plan(&o, NAME), Plan::Run);
    }

    #[test]
    fn help_flags() {
        assert!(p(&["--help"]).help);
        assert!(p(&["-h"]).help);
        assert!(!p(&[]).help);
    }

    #[test]
    fn multiple_filters_are_ored() {
        let o = p(&["unrelated", "orchestrator", "other"]);
        assert_eq!(o.filters.len(), 3);
        assert_eq!(plan(&o, NAME), Plan::Run);
        let o = p(&["unrelated", "other"]);
        assert_eq!(plan(&o, NAME), Plan::SkipAll);
    }

    #[test]
    fn missing_values_at_end_do_not_panic() {
        for flag in ["--skip", "--test-threads", "--format", "-Z"] {
            let o = p(&[flag]);
            assert!(o.filters.is_empty() && o.skips.is_empty(), "{flag}");
            assert_eq!(plan(&o, NAME), Plan::Run, "{flag}");
        }
    }

    /// Deterministic xorshift64 (fixed seed) for the robustness table.
    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    #[test]
    fn random_arguments_never_panic_and_plan_is_total() {
        const ALPHABET: [&str; 14] = [
            "-",
            "=",
            " ",
            "",
            "a",
            "Z",
            "skip",
            "list",
            "exact",
            "o",
            "\u{e9}",
            "test-threads",
            "_",
            "1",
        ];
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
        let mut table: Vec<String> = Vec::new();
        for _ in 0..256 {
            let len = (rng.next() % 8) as usize;
            let mut s = String::new();
            for _ in 0..len {
                s.push_str(ALPHABET[(rng.next() % ALPHABET.len() as u64) as usize]);
            }
            table.push(s);
        }
        assert!(table.len() >= 200);
        assert!(table.iter().any(String::is_empty));
        for start in 0..table.len() {
            let n = (rng.next() % 6) as usize;
            let end = (start + n).min(table.len());
            let args = &table[start..end];
            let o = parse_args(args);
            for name in [NAME, "", "a", "-"] {
                let pl = plan(&o, name);
                assert!(matches!(pl, Plan::Run | Plan::List | Plan::SkipAll));
                assert_eq!(pl == Plan::List, o.list);
            }
        }
    }
}
