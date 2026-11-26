//! SPEC §4 Subphase 3.3 and 4.4 (config), D25, DECISIONS "Human ruling -
//! decoded-audio budget (512 MiB, user setting)": the configured budget, not
//! the 512 MiB constant, limits a table load. Bundles are copies of the
//! grey-box bundle whose sound files are replaced by generated 16-bit mono
//! 48 kHz WAVs (decoded size = frames x 8 bytes at the 48 kHz mixer rate).

mod support;

use pin_app::orchestrator::{load_app_table_with_budget, table_samples, validate_table};
use pin_audio::mixer::{BYTES_PER_FRAME, DecodeBudget, MixerConfig};
use proptest::prelude::*;
use std::path::{Path, PathBuf};
use support::{greybox_bundle, on_big_stack, scratch};

const MIB: u64 = 1 << 20;
const RATE: u32 = 48_000;

fn wav(frames: usize) -> Vec<u8> {
    let data = (frames * 2) as u32;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for k in 0..frames {
        b.extend_from_slice(&(((k * 37) % 20_000) as i16 - 10_000).to_le_bytes());
    }
    b
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

/// `(id, file)` of every sound, in manifest order.
fn ids(lens: &[usize]) -> Vec<(String, String)> {
    (0..lens.len())
        .map(|i| match i {
            0 => ("sfx_bumper".to_string(), "audio/sfx_bumper.wav".to_string()),
            1 => ("sfx_sling".to_string(), "audio/sfx_sling.wav".to_string()),
            n => (format!("sfx_t{n}"), format!("audio/sfx_t{n}.wav")),
        })
        .collect()
}

/// A copy of the grey-box bundle whose sounds are the given lengths (frames
/// at 48 kHz): the first two replace the grey box's two, the rest are
/// appended to the manifest. At least two lengths.
fn bundle(tag: &str, lens: &[usize]) -> (PathBuf, Vec<(String, String)>) {
    assert!(lens.len() >= 2);
    let dir = scratch(tag);
    copy_dir(greybox_bundle(), &dir);
    let sounds = ids(lens);
    let mut toml = std::fs::read_to_string(dir.join("table.toml")).unwrap();
    for (i, (id, file)) in sounds.iter().enumerate() {
        if i >= 2 {
            toml.push_str(&format!(
                "\n[[sounds]]\nid = \"{id}\"\nfile = \"{file}\"\nbus = \"sfx\"\n"
            ));
        }
        std::fs::write(dir.join(file), wav(lens[i])).unwrap();
    }
    std::fs::write(dir.join("table.toml"), toml).unwrap();
    (dir, sounds)
}

/// First-fit model: indices of the sounds that fit, in manifest order.
fn model(lens: &[usize], budget: u64) -> Vec<usize> {
    let mut used = 0;
    let mut fit = vec![];
    for (i, &l) in lens.iter().enumerate() {
        let b = l as u64 * BYTES_PER_FRAME;
        if used + b <= budget {
            used += b;
            fit.push(i);
        }
    }
    fit
}

/// Loads `dir` through `table_samples` and checks the result against the
/// first-fit model.
fn check(dir: PathBuf, sounds: Vec<(String, String)>, lens: Vec<usize>, budget: u64) {
    on_big_stack(move || {
        let checked = validate_table(&dir).unwrap();
        let (samples, errors) = table_samples(&checked, MixerConfig::default().sample_rate, budget);
        let fit = model(&lens, budget);
        assert_eq!(samples.len() + errors.len(), lens.len(), "{errors:?}");
        assert_eq!(samples.len(), fit.len(), "{errors:?}");
        let mut total = 0;
        for (got, &i) in samples.iter().zip(&fit) {
            let id = checked.table.ids.sound(&sounds[i].0).unwrap();
            assert_eq!(got.0, id, "loaded sounds in manifest order");
            assert_eq!(got.1.frames(), lens[i]);
            total += got.1.frames() as u64 * BYTES_PER_FRAME;
        }
        assert!(total <= budget);
        let failed: Vec<usize> = (0..lens.len()).filter(|i| !fit.contains(i)).collect();
        assert_eq!(errors.len(), failed.len(), "{errors:?}");
        for (e, &i) in errors.iter().zip(&failed) {
            assert!(e.contains(&sounds[i].0), "{e}");
            assert!(e.contains(&sounds[i].1), "{e}");
            assert!(e.contains("audio.decoded_budget_mib"), "{e}");
            for &j in &fit {
                assert!(!e.contains(&format!("{} (", sounds[j].0)), "{e}");
            }
        }
    });
}

/// SPEC §4 3.3, D25: a 1 MiB budget loads the sounds that fit first-fit and
/// reports one error per failed sound.
#[test]
fn one_mib_budget_loads_what_fits_and_reports_the_rest() {
    // 131072 frames fit in 1 MiB.
    for (tag, lens) in [
        ("fit_mixed", vec![100_000usize, 100_000, 20_000, 200_000]),
        ("fit_single_large", vec![144_000, 1_000]), // ~3 s alone > 1 MiB
        ("fit_pair", vec![80_000, 80_000]),         // each fits, not together
        ("fit_all", vec![10_000, 10_000, 10_000]),
    ] {
        let (dir, sounds) = bundle(tag, &lens);
        check(dir, sounds, lens, MIB);
    }
}

/// SPEC §4 4.4, D25: with the 512 MiB default every sound loads.
#[test]
fn default_budget_loads_every_sound() {
    let lens = vec![100_000usize, 100_000, 20_000, 200_000];
    let (dir, sounds) = bundle("default", &lens);
    on_big_stack(move || {
        let checked = validate_table(&dir).unwrap();
        let (samples, errors) = table_samples(
            &checked,
            MixerConfig::default().sample_rate,
            DecodeBudget::DEFAULT_BYTES,
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(samples.len(), sounds.len());
    });
}

/// SPEC §4 3.3, D25: a budget exactly equal to the decoded bytes loads
/// everything; one frame (8 bytes) less fails only the last sound.
#[test]
fn budget_boundary_is_exact() {
    let lens = vec![30_000usize, 20_000, 10_000];
    let total: u64 = lens.iter().map(|&l| l as u64 * BYTES_PER_FRAME).sum();
    let (dir, sounds) = bundle("boundary", &lens);
    check(dir.clone(), sounds.clone(), lens.clone(), total);
    check(
        dir.clone(),
        sounds.clone(),
        lens.clone(),
        total - BYTES_PER_FRAME,
    );
    on_big_stack(move || {
        let checked = validate_table(&dir).unwrap();
        let (s, e) = table_samples(&checked, RATE, total - BYTES_PER_FRAME);
        assert_eq!((s.len(), e.len()), (2, 1));
        assert!(e[0].contains(&sounds[2].0), "{e:?}");
    });
}

/// SPEC §4 3.3 and 4.4, D25: for random budgets and sound lengths, loaded
/// plus failed equals the manifest count and the loaded bytes never exceed
/// the budget (`check` also compares with the first-fit model).
#[test]
fn sweep_random_budgets_and_lengths() {
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 8,
        ..ProptestConfig::default()
    });
    let strat = (
        prop::collection::vec(1usize..40_000, 2..=5),
        0u64..1_200_000,
    );
    runner
        .run(&strat, |(lens, budget)| {
            let (dir, sounds) = bundle("sweep", &lens);
            check(dir, sounds, lens, budget);
            Ok(())
        })
        .unwrap();
}

/// SPEC §4 4.4, D25: `load_app_table_with_budget` applies the budget.
#[test]
fn load_app_table_with_budget_smoke() {
    let lens = vec![100_000usize, 100_000, 20_000, 200_000];
    let (dir, sounds) = bundle("smoke", &lens);
    on_big_stack(move || {
        let app = load_app_table_with_budget(&dir, MixerConfig::default(), MIB).unwrap();
        assert_eq!(app.samples.len(), model(&lens, MIB).len());
        let app =
            load_app_table_with_budget(&dir, MixerConfig::default(), DecodeBudget::DEFAULT_BYTES)
                .unwrap();
        assert_eq!(app.samples.len(), sounds.len());
    });
}
