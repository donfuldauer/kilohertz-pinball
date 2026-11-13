//! The per-table music loop and the fixed callout motifs.
//!
//! The music loop is seeded by [`meta_seed`](crate::meta_seed) (title,
//! palette and seed): tempo, key, mode, chord progression and the
//! arpeggio/rest pattern all come from the kit's PCG; the palette's first
//! colour also biases the mode (dark → minor). It is 4 bars of 4/4 with bass,
//! arpeggio, kick, snare and hats, rendered with its release tails folded back
//! onto the start so the loop is seamless.

use super::dsp::{Filter, Synth, normalize};
use crate::TableMeta;
use crate::rng::Pcg32;

const STREAM_MUSIC: u64 = 0x6d75_7369;

/// Bars in the music loop.
pub const MUSIC_BARS: u32 = 4;

const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];
/// Chord progressions as scale degrees (0-based).
const PROGRESSIONS: [[usize; 4]; 6] = [
    [0, 5, 3, 4],
    [0, 3, 4, 3],
    [0, 4, 5, 3],
    [0, 6, 5, 4],
    [5, 3, 0, 4],
    [0, 3, 0, 4],
];

const BASS: [f64; 4] = [1.0, 0.5, 0.25, 0.12];
const TRIANGLE: [f64; 5] = [1.0, 0.0, 1.0 / 9.0, 0.0, 1.0 / 25.0];

fn chord_note(scale: &[i32; 7], root: i32, degree: usize, step: usize) -> i32 {
    // Stacked thirds within the scale.
    let d = degree + 2 * step;
    root + scale[d % 7] + 12 * (d / 7) as i32
}

/// Renders the table's music loop at `sr` (mono, peak-normalised).
pub(crate) fn render(meta: &TableMeta, sr: u32) -> Vec<f32> {
    let mut rng = Pcg32::new(crate::meta_seed(meta), STREAM_MUSIC);
    let bpm = 100 + rng.below(41);
    let spb = libm::round(f64::from(sr) * 60.0 / f64::from(bpm)) as usize;
    let beat = spb as f64 / f64::from(sr);
    let dark = crate::art::luma(crate::art::palette_color(meta, 0)) < 128;
    let minor = if rng.below(4) == 0 { !dark } else { dark };
    let scale = if minor { &MINOR } else { &MAJOR };
    let root = 45 + rng.below(12) as i32;
    let prog = PROGRESSIONS[rng.below(PROGRESSIONS.len() as u32) as usize];
    let beats = (MUSIC_BARS * 4) as usize;
    let loop_len = spb * beats;
    // One extra second for tails, folded back at the end.
    let mut s = Synth::new(sr, (loop_len as f64 / f64::from(sr)) + 1.0);

    // Arpeggio pattern: 16 sixteenths per bar, each a chord step or a rest.
    let pattern: Vec<Option<usize>> = (0..16)
        .map(|i| {
            if i % 4 == 0 || rng.below(4) != 0 {
                Some(rng.below(4) as usize)
            } else {
                None
            }
        })
        .collect();
    let hat_open = rng.below(2) == 1;
    let mut drums = Pcg32::new(crate::meta_seed(meta), STREAM_MUSIC + 1);

    for bar in 0..MUSIC_BARS as usize {
        let degree = prog[bar % 4];
        let bar_t = (bar * 4) as f64 * beat;
        // Bass: eighth notes, root and octave.
        for e in 0..8 {
            let note = chord_note(scale, root, degree, 0) + if e % 2 == 1 { 12 } else { 0 };
            let f = super::dsp::midi_hz(f64::from(note));
            let t = bar_t + e as f64 * beat / 2.0;
            s.tone(t, beat / 2.0, f, f, 1.0, 0.004, 0.18, 0.55, &BASS);
        }
        // Arpeggio two octaves up.
        for (i, step) in pattern.iter().enumerate() {
            let Some(step) = *step else { continue };
            let note = chord_note(scale, root + 24, degree, step);
            let f = super::dsp::midi_hz(f64::from(note));
            let t = bar_t + i as f64 * beat / 4.0;
            s.tone(t, beat * 0.6, f, f, 1.0, 0.003, 0.09, 0.3, &TRIANGLE);
        }
        // Drums.
        for b in 0..4 {
            let t = bar_t + b as f64 * beat;
            if b % 2 == 0 {
                s.tone(t, 0.25, 130.0, 45.0, 0.03, 0.0005, 0.09, 1.0, &[1.0]);
            } else {
                s.noise(&mut drums, t, 0.2, Filter::Band(1800.0, 0.8), 0.0, 0.05, 0.9);
                s.tone(t, 0.12, 190.0, 170.0, 0.05, 0.0005, 0.04, 0.35, &[1.0]);
            }
            for h in 0..2 {
                let th = t + h as f64 * beat / 2.0;
                let tau = if hat_open && h == 1 { 0.06 } else { 0.015 };
                s.noise(&mut drums, th, 0.12, Filter::High(7000.0), 0.0, tau, 0.35);
            }
        }
    }
    // Fold the tails onto the start.
    let mut out = s.out;
    let tail = out.split_off(loop_len.min(out.len()));
    for (i, v) in tail.into_iter().enumerate() {
        out[i % loop_len.max(1)] += v;
    }
    normalize(out)
}

/// One motif: notes as `(midi note, length in sixteenths)`; `None` is a rest.
struct Motif {
    id: &'static str,
    voice: &'static [f64],
    tau: f64,
    notes: &'static [(Option<u8>, u8)],
}

const BELL: [f64; 4] = [1.0, 0.0, 0.3, 0.12];
const BRASS: [f64; 6] = [1.0, 0.5, 0.33, 0.25, 0.2, 0.16];
const SQUARE: [f64; 7] = [1.0, 0.0, 0.33, 0.0, 0.2, 0.0, 0.14];

/// Length of a sixteenth in the motifs, seconds.
pub const MOTIF_STEP: f64 = 0.075;

static MOTIFS: [Motif; 10] = [
    Motif {
        id: "callout_extra_ball",
        voice: &BELL,
        tau: 0.25,
        notes: &[(Some(72), 1), (Some(76), 1), (Some(79), 1), (Some(84), 5)],
    },
    Motif {
        id: "callout_jackpot",
        voice: &BRASS,
        tau: 0.3,
        notes: &[
            (Some(67), 1),
            (Some(72), 1),
            (Some(76), 1),
            (Some(79), 2),
            (Some(76), 1),
            (Some(79), 6),
        ],
    },
    Motif {
        id: "callout_multiball",
        voice: &SQUARE,
        tau: 0.12,
        notes: &[
            (Some(72), 1),
            (Some(74), 1),
            (Some(76), 1),
            (Some(79), 1),
            (Some(81), 1),
            (Some(84), 1),
            (Some(81), 1),
            (Some(84), 4),
        ],
    },
    Motif {
        id: "callout_tilt",
        voice: &SQUARE,
        tau: 0.6,
        notes: &[(Some(52), 3), (None, 1), (Some(46), 8)],
    },
    Motif {
        id: "callout_game_over",
        voice: &TRIANGLE,
        tau: 0.4,
        notes: &[(Some(67), 3), (Some(64), 3), (Some(60), 3), (Some(55), 8)],
    },
    Motif {
        id: "callout_ball_saved",
        voice: &BELL,
        tau: 0.15,
        notes: &[(Some(72), 1), (Some(79), 1), (None, 1), (Some(72), 1), (Some(79), 4)],
    },
    Motif {
        id: "callout_replay",
        voice: &SQUARE,
        tau: 0.2,
        notes: &[(Some(72), 2), (Some(72), 2), (Some(84), 6)],
    },
    Motif {
        id: "callout_high_score",
        voice: &BRASS,
        tau: 0.35,
        notes: &[
            (Some(72), 1),
            (Some(76), 1),
            (Some(79), 1),
            (Some(84), 1),
            (Some(88), 1),
            (Some(91), 8),
        ],
    },
    Motif {
        id: "callout_mode_start",
        voice: &BRASS,
        tau: 0.2,
        notes: &[(Some(62), 2), (Some(69), 2), (Some(74), 6)],
    },
    Motif {
        id: "callout_mode_complete",
        voice: &BELL,
        tau: 0.3,
        notes: &[(Some(69), 1), (Some(73), 1), (Some(76), 1), (Some(81), 2), (Some(76), 1), (Some(81), 6)],
    },
];

/// Ids of the tone motifs that stand in for spoken callouts.
pub const MOTIF_IDS: &[&str] = &[
    "callout_extra_ball",
    "callout_jackpot",
    "callout_multiball",
    "callout_tilt",
    "callout_game_over",
    "callout_ball_saved",
    "callout_replay",
    "callout_high_score",
    "callout_mode_start",
    "callout_mode_complete",
];

/// Renders motif `id` at `sr`, or `None` for an unknown id.
pub(crate) fn render_motif(id: &str, sr: u32) -> Option<Vec<f32>> {
    let m = MOTIFS.iter().find(|m| m.id == id)?;
    let steps: u32 = m.notes.iter().map(|&(_, l)| u32::from(l)).sum();
    let mut s = Synth::new(sr, f64::from(steps) * MOTIF_STEP + 0.05);
    let mut t = 0.0;
    for &(note, len) in m.notes {
        let dur = f64::from(len) * MOTIF_STEP;
        if let Some(n) = note {
            let f = super::dsp::midi_hz(f64::from(n));
            s.tone(t, dur + 0.04, f, f, 1.0, 0.004, m.tau, 0.6, m.voice);
        }
        t += dur;
    }
    Some(s.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motif_table_matches_ids() {
        let ids: Vec<&str> = MOTIFS.iter().map(|m| m.id).collect();
        assert_eq!(ids, MOTIF_IDS);
        let seqs: std::collections::BTreeSet<Vec<(Option<u8>, u8)>> =
            MOTIFS.iter().map(|m| m.notes.to_vec()).collect();
        assert_eq!(seqs.len(), MOTIFS.len());
    }

    #[test]
    fn chord_notes_stack_thirds() {
        assert_eq!(chord_note(&MAJOR, 60, 0, 0), 60);
        assert_eq!(chord_note(&MAJOR, 60, 0, 1), 64);
        assert_eq!(chord_note(&MAJOR, 60, 0, 2), 67);
        assert_eq!(chord_note(&MAJOR, 60, 6, 1), 60 + 12 + 2);
    }
}
