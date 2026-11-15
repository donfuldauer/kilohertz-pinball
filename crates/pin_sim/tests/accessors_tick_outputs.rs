//! Subphase 6.0 (PR #21 review #12): the additive, output-only accessors
//! `Simulation::display_scenes`, `sound_commands` and `score_reports`.
//!
//! * They report exactly the last applied command queue's entries (after
//!   load: `on_init`'s), replaced every tick.
//! * Reading them never changes the simulation: `state_hash` per tick is
//!   identical with and without reading them, and a scripted run's final
//!   hash is stable over 10 runs (D9).
//! * Stepping with them in use does not allocate (D5).

mod sim_support;

use pin_core::input::InputEvent;
use pin_core::sim::SimStep;
use pin_core::snapshot::FrameStateSnapshot;
use pin_core::{SceneId, SoundId};
use pin_sim::alloc_guard::{CountingAlloc, count_allocs_split};
use pin_sim::{SimConfig, Simulation};
use sim_support::{ScratchDir, on_sim_thread, scratch_dir};
use std::path::{Path, PathBuf};

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn clean_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/autoplay/clean")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

/// A 10 ms 48 kHz mono 16-bit PCM WAV.
fn beep_wav() -> Vec<u8> {
    let n: u32 = 480;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + n * 2).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&48_000u32.to_le_bytes());
    w.extend_from_slice(&96_000u32.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(n * 2).to_le_bytes());
    for i in 0..n {
        let v: i16 = if (i / 24) % 2 == 0 { 12_000 } else { -12_000 };
        w.extend_from_slice(&v.to_le_bytes());
    }
    w
}

/// The 2.9 "clean" fixture plus a sound `beep` that the rules play (gain
/// 0.5) when a game starts, next to the warmup scene.
fn bundle() -> ScratchDir {
    let dir = scratch_dir("accessors");
    copy_dir(&clean_fixture(), &dir);
    let mut toml = std::fs::read_to_string(dir.join("table.toml")).unwrap();
    toml.push_str("\n[[sounds]]\nid = \"beep\"\nfile = \"beep.wav\"\nbus = \"sfx\"\n");
    std::fs::write(dir.join("table.toml"), toml).unwrap();
    std::fs::write(dir.join("beep.wav"), beep_wav()).unwrap();
    let rules = std::fs::read_to_string(dir.join("rules.lua")).unwrap();
    let hook = "start_mode(pinball.persist_key[\"mode:warmup\"], pinball.scene.warmup)";
    assert!(rules.contains(hook));
    let rules = rules.replace(
        hook,
        &format!("{hook}\n      pinball.play_sound(pinball.sound.beep, 0.5)"),
    );
    std::fs::write(dir.join("rules.lua"), rules).unwrap();
    dir
}

fn load(dir: &Path) -> Simulation {
    Simulation::load(dir, SimConfig::default()).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn accessors_report_the_last_ticks_commands() {
    let dir = bundle();
    let path = dir.to_path_buf();
    on_sim_thread(move || {
        let mut s = load(&path);
        let warmup = s.ids().scene("warmup").expect("scene id");
        let beep = s.ids().sound("beep").expect("sound id");
        // After load: on_init's set_score(1, 0), no scene, no sound.
        assert_eq!(s.score_reports(), &[(0u8, 0i64)]);
        assert!(s.display_scenes().is_empty());
        assert!(s.sound_commands().is_empty());
        s.advance(1);
        assert!(s.score_reports().is_empty(), "replaced each tick");
        // Start: the edge reaches the script in the next tick.
        s.set_switch("start", true).unwrap();
        s.advance(1);
        assert_eq!(s.display_scenes(), &[warmup]);
        assert_eq!(s.sound_commands(), &[(beep, 0.5f32)]);
        assert_eq!(s.score_reports(), &[(0u8, 0i64)]);
        s.advance(1);
        assert!(s.display_scenes().is_empty());
        assert!(s.sound_commands().is_empty());
        assert!(s.score_reports().is_empty());
        assert!(!s.script_faulted(), "{:?}", s.script_error());
        let _: (&[SceneId], &[(SoundId, f32)]) = (s.display_scenes(), s.sound_commands());
    });
    drop(dir);
}

/// A scripted run: start, bumper and rollover hits, inputs; the per-tick
/// state hashes, optionally reading the accessors every tick.
fn scripted_hashes(path: &Path, read: bool) -> (Vec<u64>, usize) {
    let mut s = load(path);
    let mut out = FrameStateSnapshot::default();
    let mut hashes = Vec::new();
    let mut seen = 0usize;
    for t in 1..=3000u64 {
        match t {
            10 => s.set_switch("start", true).unwrap(),
            20 => s.release_switch("start").unwrap(),
            500 | 900 | 1300 | 1700 => s.set_switch("bumper_ring", true).unwrap(),
            520 | 920 | 1320 | 1720 => s.release_switch("bumper_ring").unwrap(),
            2000 => s.set_switch("top_rollover", true).unwrap(),
            2010 => s.release_switch("top_rollover").unwrap(),
            _ => {}
        }
        let inputs: [InputEvent; 0] = [];
        s.step_1ms(t, &inputs, &mut out);
        if read {
            seen += s.display_scenes().len() + s.sound_commands().len() + s.score_reports().len();
        }
        hashes.push(s.state_hash());
    }
    (hashes, seen)
}

#[test]
fn accessors_do_not_change_state_hashes_and_replays_are_stable() {
    let dir = bundle();
    let path = dir.to_path_buf();
    on_sim_thread(move || {
        let (base, _) = scripted_hashes(&path, false);
        let (read, seen) = scripted_hashes(&path, true);
        assert!(
            seen >= 6,
            "the run must emit scenes / sounds / scores ({seen})"
        );
        assert_eq!(base, read, "reading the accessors changed the state hash");
        for run in 0..10 {
            let (h, _) = scripted_hashes(&path, run % 2 == 0);
            assert_eq!(h.last(), base.last(), "run {run}");
        }
    });
    drop(dir);
}

#[test]
fn stepping_with_the_accessors_in_use_does_not_allocate() {
    let dir = bundle();
    let path = dir.to_path_buf();
    on_sim_thread(move || {
        let mut s = load(&path);
        let mut out = FrameStateSnapshot::default();
        let mut sink = [0u64; 3];
        for t in 1..=2000u64 {
            if t == 10 {
                s.set_switch("start", true).unwrap();
            }
            if t == 600 {
                s.set_switch("bumper_ring", true).unwrap();
            }
            let ((), n, _exempt_lua) = count_allocs_split(|| {
                s.step_1ms(t, &[], &mut out);
                for &id in s.display_scenes() {
                    sink[0] += u64::from(id.0);
                }
                for &(id, _) in s.sound_commands() {
                    sink[1] += u64::from(id.0);
                }
                for &(p, v) in s.score_reports() {
                    sink[2] = sink[2].wrapping_add(u64::from(p) ^ v as u64);
                }
            });
            assert_eq!(n, 0, "tick {t}: {n} allocation(s)");
        }
        assert!(sink[0] + sink[1] + sink[2] > 0 || s.score(0).is_some());
    });
    drop(dir);
}
