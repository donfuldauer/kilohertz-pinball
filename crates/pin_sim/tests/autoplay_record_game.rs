//! gate-a2-cr1: sampled bot games are recorded during a soak
//! (`--record-games`) and replay without the bot (`--replay-game`) to the
//! soak's own state hash, score and tick count; recording does not change
//! the soak's output; the recording format refuses tampering, other tables
//! and random bytes without panicking.

#[path = "fixtures/autoplay/support.rs"]
mod support;

use pin_core::input::CabinetInput;
use pin_sim::alloc_guard::{CountingAlloc, count_allocs_split, is_installed};
use pin_sim::autoplay::{
    Bot, RecordSoakError, SkillLevel, SoakConfig, SoakReport, run_soak, run_soak_recording,
};
use pin_sim::cli::{
    CliError, DEFAULT_RECORD_DIR, RECORDED_GAMES_TSV, SOAK_JSON, SOAK_MARKDOWN, main_with_args,
    parse_record_spec, parse_soak_args, record_file_name, run_replay_game, run_soak_command,
};
use pin_sim::replay::{GameOp, GameRecording, ReplayError, replay_game};
use pin_sim::{SimConfig, Simulation};
use std::path::{Path, PathBuf};
use std::process::Command;
use support::*;

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// A scratch folder removed when dropped, also when the test panics.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        Self(scratch(name))
    }
}

impl std::ops::Deref for Scratch {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

fn cfg_with(games: u32, seed: u64, skill: SkillLevel, threads: usize, max: u64) -> SoakConfig {
    SoakConfig {
        skill,
        threads,
        ..cfg(games, seed, max)
    }
}

/// Replays every recording on `bundle` and checks it against the report.
fn assert_replays_match(bundle: &Path, report: &SoakReport, recs: &[(u32, GameRecording)]) {
    assert!(!recs.is_empty());
    for (g, rec) in recs {
        let sum = &report.game_list[*g as usize];
        assert_eq!(sum.game, *g);
        assert_eq!((rec.seed, rec.ticks), (sum.seed, sum.ticks));
        assert_eq!(rec.table_hash, report.table_hash);
        let r = replay_game(bundle, rec).expect("replays");
        assert_eq!(r.state_hash, sum.state_hash, "game {g}");
        assert_eq!(r.score, sum.score, "game {g}");
        assert_eq!(r.ticks, sum.ticks, "game {g}");
    }
}

// ---------------------------------------------------------------- T1

#[test]
fn t1_library_recordings_replay_to_the_soak_values_for_every_skill() {
    let bundle = fixture("clean");
    on_sim_thread(move || {
        for skill in [SkillLevel::Novice, SkillLevel::Standard, SkillLevel::Expert] {
            for seed in [1u64, 0xC0FFEE] {
                let mut by_threads = Vec::new();
                for threads in [1usize, 2] {
                    let c = cfg_with(3, seed, skill, threads, 6_000);
                    let r = run_soak_recording(&bundle, &c, &[0, 2]).expect("soak");
                    assert_eq!(
                        r.recordings.iter().map(|(g, _)| *g).collect::<Vec<_>>(),
                        vec![0, 2]
                    );
                    // The bot really played: it pressed buttons.
                    assert!(r.recordings.iter().all(|(_, rec)| {
                        rec.ops
                            .iter()
                            .any(|(_, op)| matches!(op, GameOp::SetSwitch { .. }))
                    }));
                    assert_replays_match(&bundle, &r.report, &r.recordings);
                    by_threads.push(r);
                }
                // Same recordings and report for 1 and 2 threads.
                assert_eq!(by_threads[0].recordings, by_threads[1].recordings);
                assert_eq!(
                    by_threads[0].report.to_json_deterministic(),
                    by_threads[1].report.to_json_deterministic()
                );
            }
        }
    });
}

#[test]
fn t1_recording_does_not_change_the_report() {
    let bundle = fixture("clean");
    on_sim_thread(move || {
        let c = cfg_with(4, 5, SkillLevel::Standard, 2, 5_000);
        let plain = run_soak(&bundle, &c).expect("soak");
        let rec = run_soak_recording(&bundle, &c, &[0, 1, 3]).expect("soak");
        assert_eq!(
            plain.to_json_deterministic(),
            rec.report.to_json_deterministic()
        );
        assert_eq!(
            plain.to_markdown_deterministic(),
            rec.report.to_markdown_deterministic()
        );
        let none = run_soak_recording(&bundle, &c, &[]).expect("soak");
        assert!(none.recordings.is_empty());
        assert_eq!(
            plain.to_json_deterministic(),
            none.report.to_json_deterministic()
        );
    });
}

#[test]
fn t1_selection_errors_run_nothing() {
    let c = cfg(3, 1, 1_000);
    let missing = PathBuf::from("/nonexistent/bundle");
    for bad in [&[3u32][..], &[1, 1], &[2, 1]] {
        assert!(matches!(
            run_soak_recording(&missing, &c, bad),
            Err(RecordSoakError::Selection(_))
        ));
    }
    // Non-default script limits cannot be replayed.
    let mut c2 = c;
    c2.sim.lua.instruction_budget = c2.sim.lua.instruction_budget.wrapping_add(1);
    assert!(matches!(
        run_soak_recording(&missing, &c2, &[0]),
        Err(RecordSoakError::Selection(_))
    ));
    // A valid selection gets to the (missing) bundle.
    assert!(matches!(
        run_soak_recording(&missing, &c, &[0]),
        Err(RecordSoakError::Sim(_))
    ));
}

/// Parses `recorded_games.tsv`.
fn read_tsv(dir: &Path) -> Vec<(u32, String, u64, String, String)> {
    let text = std::fs::read_to_string(dir.join(RECORDED_GAMES_TSV)).expect("tsv");
    assert!(text.ends_with('\n'));
    text.lines()
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 5, "{l:?}");
            assert_eq!(f[3].len(), 16);
            assert!(
                f[3].bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            );
            assert!(f[4] == "none" || f[4].parse::<i64>().is_ok());
            (
                f[0].parse().unwrap(),
                f[1].to_owned(),
                f[2].parse().unwrap(),
                f[3].to_owned(),
                f[4].to_owned(),
            )
        })
        .collect()
}

/// Every `pin_sim soak` argument list used by the CLI tests.
fn soak_args(bundle: &Path, out: &Path, extra: &[&str]) -> Vec<String> {
    let mut v = vec![
        "soak".to_owned(),
        bundle.display().to_string(),
        "--out".to_owned(),
        out.display().to_string(),
        "--no-tick-budget".to_owned(),
    ];
    v.extend(extra.iter().map(|s| (*s).to_owned()));
    v
}

#[test]
fn t1_cli_recordings_replay_to_the_tsv_and_report_values() {
    let bundle = fixture("clean");
    let root = Scratch::new("record_cli");
    on_sim_thread(move || {
        for (skill, seed) in [("novice", "3"), ("standard", "1"), ("expert", "0x2a")] {
            for threads in ["1", "2"] {
                let out = root.join(format!("{skill}_{threads}"));
                let a = soak_args(
                    &bundle,
                    &out,
                    &[
                        "--games",
                        "4",
                        "--seed",
                        seed,
                        "--skill",
                        skill,
                        "--threads",
                        threads,
                        "--max-ticks",
                        "5000",
                        "--record-games",
                        "spread:2",
                    ],
                );
                // Short games leave coverage incomplete: the soak fails, and
                // the recordings are still written.
                let res = main_with_args(&a);
                assert!(matches!(res, Err(CliError::Failed(_))), "{res:?}");
                let dir = out.join(DEFAULT_RECORD_DIR);
                let rows = read_tsv(&dir);
                assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), vec![0, 2]);
                let json: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(out.join(SOAK_JSON)).unwrap())
                        .unwrap();
                for (g, file, ticks, hash, score) in rows {
                    assert_eq!(file, record_file_name(g));
                    assert_eq!(file, format!("game_{g:06}.khzrec"));
                    let entry = &json["games"][g as usize];
                    assert_eq!(entry["game"], g);
                    assert_eq!(entry["ticks"], ticks);
                    assert_eq!(entry["state_hash"].as_str().unwrap(), hash);
                    match entry["score"].as_i64() {
                        Some(s) => assert_eq!(score, s.to_string()),
                        None => assert_eq!(score, "none"),
                    }
                    let path = dir.join(&file);
                    let text = main_with_args(&args(&[
                        bundle.to_str().unwrap(),
                        "--replay-game",
                        path.to_str().unwrap(),
                    ]))
                    .expect("replays");
                    assert_eq!(text, format!("state_hash={hash}\nscore={score}"));
                }
            }
            // Recordings and tsv are identical for 1 and 2 threads.
            let d1 = root.join(format!("{skill}_1")).join(DEFAULT_RECORD_DIR);
            let d2 = root.join(format!("{skill}_2")).join(DEFAULT_RECORD_DIR);
            for name in [
                RECORDED_GAMES_TSV.to_owned(),
                record_file_name(0),
                record_file_name(2),
            ] {
                assert_eq!(
                    std::fs::read(d1.join(&name)).unwrap(),
                    std::fs::read(d2.join(&name)).unwrap(),
                    "{name}"
                );
            }
        }
    });
}

#[test]
fn t1_replay_is_stable_over_ten_runs() {
    let bundle = fixture("clean");
    on_sim_thread(move || {
        let c = cfg_with(1, 9, SkillLevel::Expert, 1, 4_000);
        let r = run_soak_recording(&bundle, &c, &[0]).expect("soak");
        let rec = &r.recordings[0].1;
        let first = replay_game(&bundle, rec).unwrap();
        assert_eq!(first.state_hash, r.report.game_list[0].state_hash);
        for _ in 0..9 {
            assert_eq!(replay_game(&bundle, rec).unwrap(), first);
        }
    });
}

#[test]
fn recording_sink_keeps_the_tick_allocation_free() {
    assert!(is_installed());
    let bundle = fixture("clean");
    on_sim_thread(move || {
        let load = || Simulation::load(&bundle, SimConfig::default()).unwrap();
        let mut sim = load();
        let mut bot = Bot::new(11, SkillLevel::Expert);
        let mut rec = GameRecording::new(sim.config().seed, sim.table_hash());
        for _ in 0..12_000 {
            bot.tick_recorded(&mut sim, &mut rec);
            let ((), n, _exempt) = count_allocs_split(|| sim.advance(1));
            assert_eq!(n, 0, "tick {}: {n} allocation(s)", sim.tick());
        }
        rec.ticks = sim.tick();
        assert!(rec.ops.len() > 2, "the bot pressed buttons");
        // The same bot with the no-op sink plays the same game.
        let mut plain = load();
        let mut bot2 = Bot::new(11, SkillLevel::Expert);
        for _ in 0..12_000 {
            bot2.tick(&mut plain);
            plain.advance(1);
        }
        assert_eq!(plain.state_hash(), sim.state_hash());
        let mut again = load();
        let r = GameRecording::play(&mut again, &rec).unwrap();
        assert_eq!(r.state_hash, sim.state_hash());
        assert_eq!(r.score, sim.score(0));
    });
}

// ---------------------------------------------------------------- T2

/// Game 0 of a Standard soak on `clean`, with its recording.
fn recorded_game(seed: u64, max: u64) -> (u64, Option<i64>, GameRecording) {
    let bundle = fixture("clean");
    on_sim_thread(move || {
        let c = cfg_with(1, seed, SkillLevel::Standard, 1, max);
        let mut r = run_soak_recording(&bundle, &c, &[0]).expect("soak");
        let g = &r.report.game_list[0];
        (g.state_hash, g.score, r.recordings.remove(0).1)
    })
}

fn replay_hash(rec: GameRecording) -> u64 {
    let bundle = fixture("clean");
    on_sim_thread(move || replay_game(&bundle, &rec).expect("replays").state_hash)
}

fn is_flipper_press(op: &GameOp) -> bool {
    matches!(
        op,
        GameOp::SetSwitch {
            input: CabinetInput::LeftFlipper | CabinetInput::RightFlipper,
            closed: true
        }
    )
}

#[test]
fn t2a_operation_level_tampering_changes_the_hash() {
    let (hash, _, rec) = recorded_game(1, 8_000);
    // Not vacuous: the untouched recording, through its own bytes, matches.
    let back = GameRecording::from_bytes(&rec.to_bytes()).unwrap();
    assert_eq!(back, rec);
    assert_eq!(replay_hash(back), hash);

    // Drop the start press: the game never starts.
    let mut t = rec.clone();
    let i = t
        .ops
        .iter()
        .position(|(_, op)| {
            matches!(
                op,
                GameOp::SetSwitch {
                    input: CabinetInput::Start,
                    ..
                }
            )
        })
        .expect("start pressed");
    t.ops.remove(i);
    let t = GameRecording::from_bytes(&t.to_bytes()).expect("re-encoded validly");
    assert_ne!(replay_hash(t), hash);

    // Drop the first flipper press.
    let mut t = rec.clone();
    let i = t
        .ops
        .iter()
        .position(|(_, op)| is_flipper_press(op))
        .expect("a flipper was pressed");
    t.ops.remove(i);
    let t = GameRecording::from_bytes(&t.to_bytes()).expect("re-encoded validly");
    assert_ne!(replay_hash(t), hash);

    // Shift the first flipper press (and its release) 40 ms later.
    let mut t = rec.clone();
    let i = t
        .ops
        .iter()
        .position(|(_, op)| is_flipper_press(op))
        .unwrap();
    let pressed = t.ops[i].0;
    t.ops[i].0 = (pressed + 40).min(t.ticks);
    t.ops.sort_by_key(|(tick, _)| *tick);
    let t = GameRecording::from_bytes(&t.to_bytes()).expect("re-encoded validly");
    assert_ne!(replay_hash(t), hash);
}

#[test]
fn t2b_corrupt_files_are_errors_not_panics() {
    let (_, _, rec) = recorded_game(2, 2_000);
    let dir = Scratch::new("record_corrupt");
    let bundle = fixture("clean");
    let good = rec.to_bytes();
    let n = good.len();
    for at in [0usize, 9, 13, 21, 29, 37, 44, 52, n / 2, n - 9, n - 1] {
        let mut b = good.clone();
        b[at] ^= 0x41;
        let path = dir.join(format!("flip_{at}.khzrec"));
        std::fs::write(&path, &b).unwrap();
        let err = GameRecording::from_bytes(&b).unwrap_err();
        if at < 8 {
            assert_eq!(err, ReplayError::BadMagic);
        } else if (8..12).contains(&at) {
            assert!(matches!(err, ReplayError::UnsupportedVersion(_)), "{err:?}");
        } else if (36..44).contains(&at) {
            assert!(matches!(err, ReplayError::BadLength { .. }), "{err:?}");
        } else {
            assert_eq!(err, ReplayError::Checksum, "byte {at}");
        }
        let (b2, p2) = (bundle.clone(), path.clone());
        let e = on_sim_thread(move || run_replay_game(&b2, &p2)).unwrap_err();
        assert_eq!(e.exit_code(), 1);
        // Through the CLI too.
        let e = main_with_args(&args(&[
            bundle.to_str().unwrap(),
            "--replay-game",
            path.to_str().unwrap(),
        ]))
        .unwrap_err();
        assert_eq!(e.exit_code(), 1, "{e}");
    }
    // A missing file.
    let e = main_with_args(&args(&[
        bundle.to_str().unwrap(),
        "--replay-game",
        dir.join("missing.khzrec").to_str().unwrap(),
    ]))
    .unwrap_err();
    assert_eq!(e.exit_code(), 1);
    // A plain input recording is not a game recording.
    let plain = pin_sim::replay::Recording::new(1, 2).to_bytes();
    assert_eq!(
        GameRecording::from_bytes(&plain),
        Err(ReplayError::BadMagic)
    );
}

#[test]
fn t2c_another_table_is_refused_before_building() {
    let (_, _, rec) = recorded_game(3, 1_000);
    // A recording of `clean` against the `gap` bundle.
    let gap = fixture("gap");
    let r2 = rec.clone();
    let err = on_sim_thread(move || replay_game(&gap, &r2)).unwrap_err();
    assert!(
        matches!(err, ReplayError::TableMismatch { recorded, .. } if recorded == rec.table_hash)
    );
    // A "bundle" that could never build: the mismatch is reported, so the
    // hash check comes first.
    let junk = Scratch::new("record_junk");
    std::fs::write(junk.join("table.toml"), "this is not a table").unwrap();
    let (j2, r3) = (junk.clone(), rec.clone());
    let err = on_sim_thread(move || replay_game(&j2, &r3)).unwrap_err();
    assert!(matches!(err, ReplayError::TableMismatch { .. }), "{err:?}");
    // Through the binary: non-zero exit, message on stderr, nothing on stdout.
    let dir = Scratch::new("record_mismatch");
    let file = dir.join("g.khzrec");
    rec.save(&file).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_pin_sim"))
        .arg(fixture("gap"))
        .arg("--replay-game")
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(o.stdout.is_empty());
    assert!(String::from_utf8_lossy(&o.stderr).contains("recording is for table"));
}

#[test]
fn t2d_random_bytes_never_panic_the_parser() {
    // Fixed-seed xorshift: bounded, reproducible.
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let (_, _, rec) = recorded_game(4, 1_000);
    let good = rec.to_bytes();
    for round in 0..3_000 {
        let len = (next() % 600) as usize;
        let mut b: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        if round % 2 == 0 && b.len() >= 8 {
            b[..8].copy_from_slice(b"KHZGAME\0");
        }
        let _ = GameRecording::from_bytes(&b);
        // Mutations of a real file.
        let mut m = good.clone();
        for _ in 0..1 + next() % 4 {
            let i = (next() as usize) % m.len();
            m[i] = next() as u8;
        }
        let cut = (next() as usize) % (m.len() + 1);
        let _ = GameRecording::from_bytes(&m[..cut]);
        let _ = GameRecording::from_bytes(&m);
    }
}

// ---------------------------------------------------------------- T3

/// Runs the binary; returns (exit code, stdout, stderr).
fn run_bin(a: &[String]) -> (Option<i32>, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_pin_sim"))
        .args(a)
        .output()
        .unwrap();
    (
        o.status.code(),
        String::from_utf8(o.stdout).unwrap(),
        String::from_utf8(o.stderr).unwrap(),
    )
}

/// `text` without the wall-clock `speed:` line.
fn mask_speed(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with("speed:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The soak reports in `out`, with the wall-clock timing section masked.
fn reports(out: &Path) -> (serde_json::Value, String, String) {
    let json = std::fs::read_to_string(out.join(SOAK_JSON)).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let timing = v.as_object_mut().unwrap().remove("timing").expect("timing");
    // Everything before the timing key is byte-identical text.
    let prefix = json[..json.rfind("\"timing\"").unwrap()].to_owned();
    let md = std::fs::read_to_string(out.join(SOAK_MARKDOWN)).unwrap();
    let md = md[..md.find("\n## Timing").unwrap()].to_owned();
    let _ = timing;
    (v, prefix, md)
}

#[test]
fn t3_soak_outputs_are_identical_with_and_without_recording() {
    let bundle = fixture("clean");
    let root = Scratch::new("record_noninterference");
    let out = root.join("out");
    for extra in [
        &["--games", "3", "--seed", "7", "--max-ticks", "4000"][..],
        &[
            "--games",
            "3",
            "--seed",
            "7",
            "--max-ticks",
            "4000",
            "--gate",
            "a1",
        ],
        &["--games", "2", "--max-ticks", "300", "--skill", "expert"],
    ] {
        let plain_args = soak_args(&bundle, &out, extra);
        let (code_a, stdout_a, stderr_a) = run_bin(&plain_args);
        let rep_a = reports(&out);
        let mut rec_args = plain_args.clone();
        let rec_dir = root.join("rec");
        rec_args.extend(args(&[
            "--record-games",
            "0,1",
            "--record-dir",
            rec_dir.to_str().unwrap(),
        ]));
        let (code_b, stdout_b, stderr_b) = run_bin(&rec_args);
        let rep_b = reports(&out);
        assert_eq!(code_a, code_b, "{extra:?}");
        assert_eq!(mask_speed(&stdout_a), mask_speed(&stdout_b), "{extra:?}");
        assert_eq!(mask_speed(&stderr_a), mask_speed(&stderr_b), "{extra:?}");
        assert_eq!(rep_a, rep_b, "{extra:?}");
        assert!(rec_dir.join(RECORDED_GAMES_TSV).exists());
        assert!(rec_dir.join(record_file_name(1)).exists());
        let _ = std::fs::remove_dir_all(&rec_dir);
    }
}

// ---------------------------------------------------------------- T4

#[test]
fn t4_record_spec_parsing() {
    assert_eq!(parse_record_spec("0,49,99", 100), Ok(vec![0, 49, 99]));
    assert_eq!(parse_record_spec("5", 6), Ok(vec![5]));
    let spread = parse_record_spec("spread:20", 1000).unwrap();
    assert_eq!(spread, (0..20).map(|i| i * 50).collect::<Vec<u32>>());
    assert_eq!(parse_record_spec("spread:3", 20), Ok(vec![0, 6, 13]));
    assert_eq!(parse_record_spec("spread:4", 4), Ok(vec![0, 1, 2, 3]));
    assert_eq!(parse_record_spec("spread:1", 1), Ok(vec![0]));
    for (bad, games) in [
        ("100", 100),      // index >= games
        ("0,100", 100),    // index >= games
        ("spread:21", 20), // N > games
        ("spread:0", 20),
        ("spread:", 20),
        ("spread:x", 20),
        ("3,2", 10), // descending
        ("2,2", 10), // duplicate
        ("", 10),
        (",", 10),
        ("1,", 10),
        (" 1", 10),
        ("1;2", 10),
        ("-1", 10),
        ("+1", 10),
        ("0x1", 10),
        ("abc", 10),
        ("99999999999999999999999", 10),
        ("spread:99999999999999999999999", 10),
    ] {
        assert!(parse_record_spec(bad, games).is_err(), "{bad:?}");
    }
}

#[test]
fn t4_soak_arguments_with_recording() {
    let a = parse_soak_args(&args(&[
        "b",
        "--record-games",
        "spread:3",
        "--games",
        "20",
        "--out",
        "o",
    ]))
    .unwrap();
    assert_eq!(a.record_games, vec![0, 6, 13]);
    assert_eq!(a.record_dir, Some(Path::new("o").join("recordings")));
    let a = parse_soak_args(&args(&[
        "b",
        "--games",
        "200",
        "--record-games",
        "0,49,99",
        "--record-dir",
        "r",
        "--out",
        "o",
    ]))
    .unwrap();
    assert_eq!(a.record_games, vec![0, 49, 99]);
    assert_eq!(a.record_dir, Some(PathBuf::from("r")));
    let d = parse_soak_args(&args(&["b", "--out", "o"])).unwrap();
    assert!(d.record_games.is_empty() && d.record_dir.is_none());

    for bad in [
        &["b", "--out", "o", "--games", "5", "--record-games", "5"][..],
        &["b", "--out", "o", "--record-games", "100"], // default 100 games
        &[
            "b",
            "--out",
            "o",
            "--games",
            "5",
            "--record-games",
            "spread:6",
        ],
        &["b", "--out", "o", "--record-games", "3,1"],
        &["b", "--out", "o", "--record-games", "1,1"],
        &["b", "--out", "o", "--record-games", ""],
        &["b", "--out", "o", "--record-games", "all"],
        &["b", "--out", "o", "--record-games"],
        &["b", "--out", "o", "--record-dir", "r"],
        &["b", "--out", "o", "--record-games", "0", "--record-dir"],
    ] {
        let e = parse_soak_args(&args(bad)).unwrap_err();
        assert_eq!(e.exit_code(), 2, "{bad:?}");
        assert!(e.to_string().contains("pin_sim soak"), "{e}");
        let mut full = vec!["soak"];
        full.extend_from_slice(bad);
        assert_eq!(main_with_args(&args(&full)).unwrap_err().exit_code(), 2);
    }
}

#[test]
fn t4_record_dir_inside_the_bundle_is_refused() {
    // A copy of the fixture, so nothing is ever written into the real one.
    let root = Scratch::new("record_inside");
    let bundle = root.join("clean");
    std::fs::create_dir_all(&bundle).unwrap();
    for e in std::fs::read_dir(fixture("clean")).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), bundle.join(e.file_name())).unwrap();
        }
    }
    let out = root.join("out");
    for dir in [bundle.join("rec"), bundle.clone()] {
        let a = parse_soak_args(&args(&[
            bundle.to_str().unwrap(),
            "--games",
            "1",
            "--max-ticks",
            "10",
            "--record-games",
            "0",
            "--record-dir",
            dir.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ]))
        .unwrap();
        let e = run_soak_command(&a).unwrap_err();
        assert_eq!(e.exit_code(), 2, "{e}");
        assert!(e.to_string().contains("--record-dir"), "{e}");
        assert!(!out.exists(), "no soak ran");
        assert!(!bundle.join("rec").exists());
    }
}

#[test]
fn t4_replay_game_arguments() {
    for bad in [
        &["--replay-game", "f"][..],
        &["b", "--replay-game"],
        &["b", "--replay-game", "f", "--seed", "1"],
        &["b", "--replay-game", "f", "--ticks", "5"],
        &["b", "--replay-game", "f", "--replay-inputs", "g"],
        &["b", "c", "--replay-game", "f"],
        &["b", "--replay-game", "f", "--replay-game", "g"],
    ] {
        let e = main_with_args(&args(bad)).unwrap_err();
        assert_eq!(e.exit_code(), 2, "{bad:?}: {e}");
    }
    assert!(pin_sim::cli::USAGE.contains("--replay-game FILE"));
    assert!(pin_sim::cli::SOAK_USAGE.contains("--record-games SPEC"));
    assert!(pin_sim::cli::SOAK_USAGE.contains("--record-dir DIR"));
}

#[test]
fn t4_no_recording_means_no_recording_checks() {
    // The bundle does not exist, so a soak that gets past argument checks
    // fails with a run error (exit 1); the recording checks are usage errors
    // (exit 2). A tick limit above what a recording holds is fine for a
    // plain soak and refused only when games are recorded.
    let missing = "/nonexistent/bundle";
    let out = Scratch::new("record_nochecks");
    let out_s = out.to_str().unwrap();
    let plain = parse_soak_args(&args(&[
        missing,
        "--games",
        "1",
        "--max-ticks",
        "4294967297",
        "--out",
        out_s,
    ]))
    .unwrap();
    let e = run_soak_command(&plain).unwrap_err();
    assert_eq!(e.exit_code(), 1, "{e}");
    assert!(e.to_string().contains("soak:"), "{e}");
    let rec = parse_soak_args(&args(&[
        missing,
        "--games",
        "1",
        "--max-ticks",
        "4294967297",
        "--record-games",
        "0",
        "--out",
        out_s,
    ]))
    .unwrap();
    let e = run_soak_command(&rec).unwrap_err();
    assert_eq!(e.exit_code(), 2, "{e}");
    // Same for non-default script limits.
    let mut lua = plain;
    lua.cfg.sim.lua.instruction_budget = lua.cfg.sim.lua.instruction_budget.wrapping_add(1);
    lua.cfg.max_game_ticks = 10;
    assert_eq!(run_soak_command(&lua).unwrap_err().exit_code(), 1);
    lua.record_games = vec![0];
    assert_eq!(run_soak_command(&lua).unwrap_err().exit_code(), 2);
}

#[cfg(unix)]
#[test]
fn t4_recording_never_writes_through_a_symlink() {
    use std::os::unix::fs::symlink;
    let bundle = fixture("clean");
    let root = Scratch::new("record_symlink");
    for name in [record_file_name(0), RECORDED_GAMES_TSV.to_owned()] {
        let case = root.join(name.replace('.', "_"));
        let rec_dir = case.join("rec");
        std::fs::create_dir_all(&rec_dir).unwrap();
        let victim = case.join("victim.txt");
        std::fs::write(&victim, "precious").unwrap();
        symlink(&victim, rec_dir.join(&name)).unwrap();
        let a = soak_args(
            &bundle,
            &case.join("out"),
            &[
                "--games",
                "1",
                "--max-ticks",
                "300",
                "--record-games",
                "0",
                "--record-dir",
                rec_dir.to_str().unwrap(),
            ],
        );
        let res = main_with_args(&a);
        let Err(CliError::Failed(m)) = res else {
            panic!("{name}: {res:?}")
        };
        assert!(m.contains("symbolic link"), "{m}");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious");
    }
    // Reruns over regular files still replace them.
    let rec_dir = root.join("rerun");
    let a = soak_args(
        &bundle,
        &root.join("rerun_out"),
        &[
            "--games",
            "1",
            "--max-ticks",
            "300",
            "--record-games",
            "0",
            "--record-dir",
            rec_dir.to_str().unwrap(),
        ],
    );
    for _ in 0..2 {
        let res = main_with_args(&a);
        assert!(matches!(res, Err(CliError::Failed(m)) if !m.contains("symbolic")));
        assert!(rec_dir.join(record_file_name(0)).is_file());
        assert!(rec_dir.join(RECORDED_GAMES_TSV).is_file());
    }
}
