//! Subphase 1.2 brief, checked against `.github/workflows/ci.yml`.

use spec_ci::*;

const CORE: [&str; 7] = [
    "pin_core",
    "pin_physics",
    "pin_input",
    "pin_script",
    "pin_assets",
    "pin_tablegen",
    "pin_sim",
];
const GPU: [&str; 3] = ["pin_render", "pin_audio", "pin_app"];
const REQUIRED_JOBS: [&str; 5] = ["core-tests", "gpu-tests", "clippy", "fmt", "replay-hash"];

fn ci() -> Workflow {
    Workflow::load("ci.yml")
}

fn job(id: &str) -> Job {
    ci().job(id)
        .unwrap_or_else(|| panic!("ci.yml has no job `{id}`"))
}

#[test]
fn parses_as_a_single_workflow_document() {
    ci();
}

#[test]
fn has_no_tabs_and_ends_with_newline() {
    let text = workflow_text("ci.yml");
    assert!(!text.contains('\t'), "tabs are invalid YAML indentation");
    assert!(text.ends_with('\n'));
}

#[test]
fn triggers_on_push_and_only_known_events() {
    let t = ci().triggers();
    assert!(t.iter().any(|e| e == "push"), "push trigger missing: {t:?}");
    for e in &t {
        assert!(
            [
                "push",
                "pull_request",
                "workflow_dispatch",
                "merge_group",
                "schedule"
            ]
            .contains(&e.as_str()),
            "unexpected trigger {e}"
        );
    }
}

#[test]
fn required_jobs_exist() {
    let ids: Vec<String> = ci().jobs().into_iter().map(|j| j.id).collect();
    for want in REQUIRED_JOBS {
        assert!(
            ids.iter().any(|i| i == want),
            "job `{want}` missing; have {ids:?}"
        );
    }
}

#[test]
fn build_and_test_jobs_cover_all_three_platforms() {
    for id in ["core-tests", "gpu-tests", "clippy"] {
        let j = job(id);
        for os in Os::ALL {
            assert!(
                j.covers(os),
                "{id} does not run on {os:?}; runners: {:?}",
                j.runner_labels()
            );
        }
    }
}

#[test]
fn macos_runner_is_apple_silicon() {
    for id in ["core-tests", "gpu-tests", "clippy"] {
        let labels = job(id).runner_labels();
        let mac: Vec<_> = labels
            .iter()
            .filter(|l| runner_os(l) == Some(Os::Macos))
            .collect();
        assert!(
            mac.iter().any(|l| is_arm64_macos_label(l)),
            "{id}: no aarch64-apple-darwin runner among {mac:?}"
        );
    }
}

#[test]
fn every_job_has_a_timeout_of_at_most_45_minutes() {
    for j in ci().jobs() {
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
fn every_job_has_a_runner_and_steps() {
    for j in ci().jobs() {
        assert!(
            !j.runner_labels().is_empty(),
            "job `{}` names no hosted runner",
            j.id
        );
        assert!(j.step_count() > 0, "job `{}` has no steps", j.id);
    }
}

#[test]
fn concurrency_cancels_superseded_runs_per_ref() {
    let wf = ci();
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
            c.is_per_event(),
            "concurrency group `{}` does not include github.event_name",
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
fn core_tests_runs_every_gpu_free_crate_and_no_gpu_crate() {
    let script = job("core-tests").script();
    let lines = cargo_lines(&script, "test");
    assert!(
        !lines.is_empty(),
        "core-tests never runs `cargo test`:\n{script}"
    );

    let mut covered: Vec<String> = Vec::new();
    for l in &lines {
        if l.contains("--manifest-path") {
            // A standalone package outside the root workspace (e.g. this spec crate).
            assert!(
                !GPU.iter().any(|g| l.contains(g)),
                "core-tests must stay GPU-free: {l}"
            );
            continue;
        }
        if l.split_whitespace()
            .any(|t| t == "--workspace" || t == "--all")
        {
            let ex = excluded_packages(l);
            for g in GPU {
                assert!(
                    ex.iter().any(|e| e == g),
                    "workspace test does not exclude {g}: {l}"
                );
            }
            for c in CORE {
                assert!(
                    !ex.iter().any(|e| e == c),
                    "core crate {c} is excluded: {l}"
                );
            }
            covered.extend(CORE.iter().map(|c| (*c).to_owned()));
        } else {
            let sel = selected_packages(l);
            assert!(
                !sel.is_empty(),
                "cargo test with neither --workspace nor -p: {l}"
            );
            for g in GPU {
                assert!(
                    !sel.iter().any(|s| s == g),
                    "core-tests must stay GPU-free but tests {g}: {l}"
                );
            }
            covered.extend(sel);
        }
    }
    for c in CORE {
        assert!(
            covered.iter().any(|x| x == c),
            "core-tests does not test {c}:\n{script}"
        );
    }
}

#[test]
fn gpu_tests_runs_render_audio_app() {
    let j = job("gpu-tests");
    let script = j.script();
    let lines = cargo_lines(&script, "test");
    assert!(
        !lines.is_empty(),
        "gpu-tests never runs `cargo test`:\n{script}"
    );
    let sel: Vec<String> = lines.iter().flat_map(|l| selected_packages(l)).collect();
    for g in GPU {
        assert!(
            sel.iter().any(|s| s == g),
            "gpu-tests does not test {g}:\n{script}"
        );
    }
}

#[test]
fn gpu_tests_installs_software_vulkan_and_xvfb_on_linux() {
    let script = job("gpu-tests").script();
    for pkg in ["mesa-vulkan-drivers", "xvfb"] {
        assert!(script.contains(pkg), "gpu-tests does not install {pkg}");
    }
}

#[test]
fn clippy_denies_warnings_across_the_workspace() {
    let script = job("clippy").script();
    let lines = cargo_lines(&script, "clippy");
    assert!(
        !lines.is_empty(),
        "clippy job never runs `cargo clippy`:\n{script}"
    );
    assert!(
        lines.iter().any(|l| {
            let (cargo_args, lint_args) = l.split_once(" -- ").unwrap_or((l, ""));
            cargo_args.contains("--workspace")
                && cargo_args.contains("--all-targets")
                && lint_args.contains("-D warnings")
        }),
        "want `cargo clippy --workspace --all-targets -- -D warnings`, got {lines:?}"
    );
}

#[test]
fn fmt_checks_whole_workspace_on_linux_only() {
    let j = job("fmt");
    assert!(j.covers(Os::Linux), "fmt must run on Linux");
    assert!(
        !j.covers(Os::Windows) && !j.covers(Os::Macos),
        "fmt is Linux only: {:?}",
        j.runner_labels()
    );
    let script = j.script();
    let lines = cargo_lines(&script, "fmt");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("--all") && l.contains("--check")),
        "want `cargo fmt --all --check`, got {lines:?}"
    );
}

#[test]
fn replay_hash_is_a_marked_placeholder_that_cannot_fail_on_cargo() {
    let j = job("replay-hash");
    let script = j.script();
    assert!(
        script.contains("echo"),
        "replay-hash should echo a placeholder message:\n{script}"
    );
    for sub in ["test", "run", "build", "bench"] {
        assert!(
            cargo_lines(&script, sub).is_empty(),
            "placeholder must not run cargo {sub} yet"
        );
    }
    // "Clearly marked": the file says, near the job, that Subphase 2.5 fills it in.
    let text = workflow_text("ci.yml");
    let at = text.find("replay-hash").expect("job id present in text");
    let tail = text[at..].to_ascii_lowercase();
    assert!(
        tail.contains("2.5"),
        "replay-hash is not marked as filled in by Subphase 2.5"
    );
    assert!(
        tail.contains("placeholder"),
        "replay-hash is not marked as a placeholder"
    );
}

#[test]
fn toolchain_is_the_exact_pin_from_versions_toml_with_cache() {
    let wf = ci();
    let pin = pinned_toolchain();
    let want = format!("dtolnay/rust-toolchain@{pin}");
    assert!(
        workflow_text("ci.yml").contains(&pin),
        "ci.yml never mentions toolchain {pin}"
    );
    for id in ["core-tests", "gpu-tests", "clippy"] {
        let uses = wf.job(id).unwrap().uses();
        assert!(
            uses.iter().any(|u| u.starts_with("actions/checkout@")),
            "{id}: no checkout"
        );
        assert!(uses.contains(&want), "{id}: want {want}, have {uses:?}");
        assert!(
            uses.iter().any(|u| u == "Swatinem/rust-cache@v2"),
            "{id}: {uses:?}"
        );
    }
    let fmt = wf.job("fmt").unwrap().uses();
    assert!(fmt.contains(&want), "fmt: want {want}, have {fmt:?}");
    // No job anywhere may float on another toolchain (stable, beta, other pins).
    for j in wf.jobs() {
        for u in j
            .uses()
            .iter()
            .filter(|u| u.starts_with("dtolnay/rust-toolchain@"))
        {
            assert_eq!(*u, want, "job `{}` uses a different toolchain", j.id);
        }
    }
}

#[test]
fn core_tests_installs_no_sdl_or_windowing_packages() {
    // GPU-free crates must build without SDL / display / audio system headers.
    let script = job("core-tests").script().to_ascii_lowercase();
    for pkg in [
        "libsdl",
        "libx11",
        "libxext",
        "libxrandr",
        "libxcursor",
        "libxi-",
        "libxkbcommon",
        "libwayland",
        "libasound",
        "libpulse",
        "libudev",
        "libdrm",
        "libgbm",
        "libegl",
        "libgl1",
        "mesa-vulkan-drivers",
        "xvfb",
    ] {
        assert!(
            !script.contains(pkg),
            "core-tests must stay display-free but installs {pkg}"
        );
    }
}

#[test]
fn clippy_also_lints_this_spec_package_with_warnings_denied() {
    let mut found = Vec::new();
    for j in ci().jobs() {
        for (dir, script) in j.run_steps_with_dir() {
            let in_dir = dir.trim_end_matches('/').ends_with("tests/spec_ci");
            let mut cd = false;
            for l in script.lines() {
                cd |= l.contains("cd ") && l.contains("tests/spec_ci");
                if !cargo_lines(l, "clippy").is_empty()
                    && (in_dir || cd || l.contains("tests/spec_ci/Cargo.toml"))
                {
                    found.push(l.to_owned());
                }
            }
        }
    }
    assert!(
        found.iter().any(|l| l
            .split_once(" -- ")
            .is_some_and(|(_, lint)| lint.contains("-D warnings"))),
        "ci.yml has no `cargo clippy ... -- -D warnings` for tests/spec_ci: {found:?}"
    );
}

#[test]
fn sdl_source_build_prerequisites_are_installed_on_linux() {
    // SDL builds from source: cmake + X11/Wayland/audio/udev headers.
    for id in ["gpu-tests", "clippy"] {
        let script = job(id).script();
        assert!(
            script.contains("apt-get") || script.contains("apt "),
            "{id}: no apt install step"
        );
        for pkg in [
            "cmake",
            "libx11-dev",
            "libwayland-dev",
            "libasound2-dev",
            "libudev-dev",
        ] {
            assert!(
                script.contains(pkg),
                "{id}: Linux dependency {pkg} not installed"
            );
        }
    }
}

#[test]
fn cargo_commands_in_ci_exist_in_the_brief() {
    // Guard against typos in subcommands: every `cargo X` is a known one.
    let known = [
        "test",
        "clippy",
        "fmt",
        "build",
        "check",
        "llvm-cov",
        "install",
        "fetch",
        "doc",
        "--version",
        "-V",
        "run",
        "metadata",
    ];
    for j in ci().jobs() {
        for line in j
            .script()
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
        {
            let toks: Vec<&str> = line.split_whitespace().collect();
            for (i, t) in toks.iter().enumerate() {
                if *t == "cargo"
                    && (i == 0
                        || ["run", "xvfb-run", "-a", "&&", ";", "then", "|"].contains(&toks[i - 1]))
                {
                    let mut sub = toks.get(i + 1).copied().unwrap_or("");
                    if sub.starts_with('+') {
                        sub = toks.get(i + 2).copied().unwrap_or("");
                    }
                    assert!(
                        known.contains(&sub),
                        "job `{}`: unknown `cargo {sub}` in: {line}",
                        j.id
                    );
                }
            }
        }
    }
}
