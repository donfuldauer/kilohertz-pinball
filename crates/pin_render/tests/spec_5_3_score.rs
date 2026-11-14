//! Independent spec tests for Subphase 5.3 — score formatting (GPU-free).
//!
//! Written from the brief (`docs/briefs/wave5/5.3.md`) and the published contract (DECISIONS
//! "5.3 — Displays public API") without reading the implementation. Covers: thousands
//! separators, negative and huge values, player rows, credit / ball status lines, and the
//! `HostCommandQueue::scores` hand-off.

use pin_core::script::HostCommandQueue;
use pin_render::displays::{
    MAX_PLAYERS, PlayerRow, ScoreLines, ScoreState, format_score, format_score_with, score_lines,
};
use proptest::prelude::*;

// ---------------------------------------------------------------------------------------
// format_score
// ---------------------------------------------------------------------------------------

#[test]
fn format_score_known_values() {
    let cases: &[(i64, &str)] = &[
        (0, "0"),
        (1, "1"),
        (9, "9"),
        (10, "10"),
        (99, "99"),
        (100, "100"),
        (999, "999"),
        (1_000, "1,000"),
        (1_001, "1,001"),
        (9_999, "9,999"),
        (10_000, "10,000"),
        (99_999, "99,999"),
        (100_000, "100,000"),
        (999_999, "999,999"),
        (1_000_000, "1,000,000"),
        (1_000_001, "1,000,001"),
        (1_234_567, "1,234,567"),
        (12_345_678, "12,345,678"),
        (123_456_789, "123,456,789"),
        (1_000_000_000, "1,000,000,000"),
        (-1, "-1"),
        (-12, "-12"),
        (-999, "-999"),
        (-1_000, "-1,000"),
        (-1_234_567, "-1,234,567"),
        (-100_000, "-100,000"),
        (i64::MAX, "9,223,372,036,854,775,807"),
        (i64::MIN, "-9,223,372,036,854,775,808"),
        (i64::MIN + 1, "-9,223,372,036,854,775,807"),
    ];
    for &(v, want) in cases {
        assert_eq!(format_score(v), want, "format_score({v})");
    }
}

#[test]
fn format_score_never_puts_separator_after_sign() {
    // -100 / -100,000 are the classic "-,100" off-by-one.
    for v in [-100i64, -999, -100_000, -999_999, -100_000_000] {
        let s = format_score(v);
        assert!(s.starts_with('-'), "{s}");
        assert!(
            s.as_bytes()[1].is_ascii_digit(),
            "separator right after the sign in {s:?}"
        );
    }
}

#[test]
fn format_score_with_custom_and_no_separator() {
    assert_eq!(format_score_with(1_234_567, None), "1234567");
    assert_eq!(format_score_with(-1_234_567, None), "-1234567");
    assert_eq!(format_score_with(0, None), "0");
    assert_eq!(format_score_with(i64::MIN, None), i64::MIN.to_string());
    assert_eq!(format_score_with(1_234_567, Some('.')), "1.234.567");
    assert_eq!(format_score_with(1_234_567, Some(' ')), "1 234 567");
    assert_eq!(format_score_with(-1_000, Some('\'')), "-1'000");
    assert_eq!(format_score_with(999, Some('.')), "999");
    assert_eq!(format_score_with(1_234_567, Some(',')), "1,234,567");
    // A multi-byte separator must not break grouping.
    assert_eq!(
        format_score_with(1_234_567, Some('\u{202f}')),
        "1\u{202f}234\u{202f}567"
    );
}

fn check_grouping(v: i64, s: &str, sep: char) {
    let body = s.strip_prefix('-').unwrap_or(s);
    assert_eq!(s.starts_with('-'), v < 0, "sign of {s:?} for {v}");
    let groups: Vec<&str> = body.split(sep).collect();
    assert!(!groups.is_empty());
    for (i, g) in groups.iter().enumerate() {
        assert!(
            !g.is_empty() && g.chars().all(|c| c.is_ascii_digit()),
            "group {g:?} in {s:?}"
        );
        if i == 0 {
            assert!((1..=3).contains(&g.len()), "first group of {s:?}");
        } else {
            assert_eq!(g.len(), 3, "inner group of {s:?}");
        }
    }
    // No leading zero unless the value is zero.
    if v != 0 {
        assert!(!groups[0].starts_with('0'), "leading zero in {s:?}");
    }
    let digits: String = s.chars().filter(|c| *c != sep).collect();
    assert_eq!(digits, v.to_string(), "digits of {s:?}");
}

fn any_score() -> impl Strategy<Value = i64> {
    prop_oneof![
        any::<i64>(),
        -2_000_000i64..2_000_000,
        // Around every power of ten (where the group count changes).
        (0u32..19, -2i64..=2, any::<bool>()).prop_map(|(e, d, neg)| {
            let p = 10i64.pow(e).saturating_add(d);
            if neg { -p } else { p }
        }),
        Just(i64::MIN),
        Just(i64::MAX),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn prop_format_score_groups_of_three_and_round_trips(v in any_score()) {
        let s = format_score(v);
        check_grouping(v, &s, ',');
        prop_assert_eq!(s.replace(',', "").parse::<i64>().unwrap(), v);
        prop_assert_eq!(&s, &format_score_with(v, Some(',')));
    }

    #[test]
    fn prop_format_score_with_none_is_plain_decimal(v in any_score()) {
        prop_assert_eq!(format_score_with(v, None), v.to_string());
    }

    #[test]
    fn prop_format_score_with_any_separator(v in any_score(), sep in prop_oneof![
        Just('.'), Just(' '), Just('_'), Just('\''), Just('\u{2009}'), Just('·')
    ]) {
        let s = format_score_with(v, Some(sep));
        check_grouping(v, &s, sep);
        // The separator count is fixed by the digit count.
        let n_digits = v.unsigned_abs().to_string().len();
        prop_assert_eq!(s.chars().filter(|c| *c == sep).count(), (n_digits - 1) / 3);
    }

    #[test]
    fn prop_format_score_length_is_monotone_in_magnitude(a in 0i64..i64::MAX, b in 0i64..i64::MAX) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        prop_assert!(format_score(lo).len() <= format_score(hi).len());
        // Negation adds exactly the sign.
        prop_assert_eq!(format_score(-lo).len(), format_score(lo).len() + usize::from(lo != 0));
    }
}

// ---------------------------------------------------------------------------------------
// ScoreState and the host command queue
// ---------------------------------------------------------------------------------------

#[test]
fn max_players_is_four() {
    assert_eq!(MAX_PLAYERS, 4);
}

#[test]
fn score_state_default_is_one_player_all_zero() {
    let s = ScoreState::default();
    assert_eq!(s.scores, [0; 4]);
    assert_eq!(s.players, 1);
    assert_eq!(s.current_player, 0);
    assert_eq!(s.ball, 0);
    assert_eq!(s.credits, 0);
}

#[test]
fn apply_commands_sets_scores_and_reports_change() {
    let mut s = ScoreState::default();
    let mut q = HostCommandQueue::default();
    assert!(!s.apply_commands(&q), "an empty queue changes nothing");
    assert_eq!(s, ScoreState::default());

    q.set_score(0, 1_500).unwrap();
    assert!(s.apply_commands(&q));
    assert_eq!(s.scores[0], 1_500);
    assert_eq!(s.players, 1);
    // The same command again: no change.
    assert!(
        !s.apply_commands(&q),
        "identical score must not be a change"
    );

    q.clear();
    q.set_score(0, 1_501).unwrap();
    assert!(s.apply_commands(&q));
    assert_eq!(s.scores[0], 1_501);
}

#[test]
fn apply_commands_in_order_last_wins_and_players_grow() {
    let mut s = ScoreState::default();
    let mut q = HostCommandQueue::default();
    q.set_score(1, 10).unwrap();
    q.set_score(1, 20).unwrap();
    q.set_score(2, -5).unwrap();
    assert!(s.apply_commands(&q));
    assert_eq!(s.scores, [0, 20, -5, 0]);
    assert_eq!(
        s.players, 3,
        "players grows to cover the highest player set"
    );

    // Setting a lower player later never shrinks the player count.
    q.clear();
    q.set_score(0, 7).unwrap();
    assert!(s.apply_commands(&q));
    assert_eq!(s.players, 3);
    assert_eq!(s.scores, [7, 20, -5, 0]);

    q.clear();
    q.set_score(3, i64::MAX).unwrap();
    assert!(s.apply_commands(&q));
    assert_eq!(s.players, 4);
    assert_eq!(s.scores[3], i64::MAX);
}

#[test]
fn apply_commands_ignores_players_out_of_range() {
    let mut s = ScoreState::default();
    let mut q = HostCommandQueue::default();
    q.set_score(4, 999).unwrap();
    q.set_score(200, 999).unwrap();
    q.set_score(255, 999).unwrap();
    assert!(
        !s.apply_commands(&q),
        "players >= 4 are ignored and are not a change"
    );
    assert_eq!(s, ScoreState::default());
}

proptest! {
    #[test]
    fn prop_apply_commands_matches_a_reference_model(
        cmds in proptest::collection::vec((0u8..8, any::<i64>()), 0..=4),
        start in proptest::array::uniform4(any::<i64>()),
        players in 1u8..=4,
    ) {
        let mut s = ScoreState { scores: start, players, ..ScoreState::default() };
        let before = s.clone();
        let mut q = HostCommandQueue::default();
        let mut want = start;
        let mut want_players = players;
        for &(p, v) in &cmds {
            q.set_score(p, v).unwrap();
            if (p as usize) < MAX_PLAYERS {
                want[p as usize] = v;
                want_players = want_players.max(p + 1);
            }
        }
        let changed = s.apply_commands(&q);
        prop_assert_eq!(s.scores, want);
        prop_assert_eq!(s.players, want_players);
        prop_assert_eq!(s.current_player, before.current_player);
        prop_assert_eq!(s.ball, before.ball);
        prop_assert_eq!(s.credits, before.credits);
        prop_assert_eq!(changed, s != before, "the return value reports whether anything changed");
        // Idempotent.
        prop_assert!(!s.apply_commands(&q));
    }
}

// ---------------------------------------------------------------------------------------
// score_lines: player rows and the status line
// ---------------------------------------------------------------------------------------

#[test]
fn score_lines_single_player_no_game() {
    let s = ScoreState {
        credits: 3,
        ..ScoreState::default()
    };
    let l: ScoreLines = score_lines(&s);
    assert_eq!(
        l.rows,
        vec![PlayerRow {
            player: 1,
            text: "0".to_string(),
            current: true
        }]
    );
    assert_eq!(l.status, "CREDITS 3");
}

#[test]
fn score_lines_single_player_in_game() {
    let s = ScoreState {
        scores: [12_345_670, 0, 0, 0],
        players: 1,
        current_player: 0,
        ball: 2,
        credits: 1,
    };
    let l = score_lines(&s);
    assert_eq!(l.rows.len(), 1);
    assert_eq!(l.rows[0].player, 1);
    assert_eq!(l.rows[0].text, "12,345,670");
    assert!(l.rows[0].current);
    assert_eq!(l.status, "BALL 2  CREDITS 1");
}

#[test]
fn score_lines_multi_player_in_game() {
    let s = ScoreState {
        scores: [1_000, 2_500_000, -30, 999],
        players: 4,
        current_player: 2,
        ball: 3,
        credits: 9,
    };
    let l = score_lines(&s);
    assert_eq!(l.rows.len(), 4);
    let texts: Vec<&str> = l.rows.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["1,000", "2,500,000", "-30", "999"]);
    let players: Vec<u8> = l.rows.iter().map(|r| r.player).collect();
    assert_eq!(
        players,
        [1, 2, 3, 4],
        "rows are 1-based and in player order"
    );
    let current: Vec<bool> = l.rows.iter().map(|r| r.current).collect();
    assert_eq!(current, [false, false, true, false]);
    assert_eq!(l.status, "PLAYER 3  BALL 3");
}

#[test]
fn score_lines_multi_player_no_game_shows_credits() {
    let s = ScoreState {
        scores: [5, 6, 0, 0],
        players: 2,
        current_player: 1,
        ball: 0,
        credits: 12,
    };
    let l = score_lines(&s);
    assert_eq!(l.rows.len(), 2);
    assert_eq!(
        l.status, "CREDITS 12",
        "ball == 0 means no game in progress"
    );
}

#[test]
fn score_lines_clamps_player_count() {
    for (players, want) in [(0u8, 1usize), (1, 1), (4, 4), (5, 4), (255, 4)] {
        let s = ScoreState {
            players,
            ..ScoreState::default()
        };
        assert_eq!(score_lines(&s).rows.len(), want, "players = {players}");
    }
}

proptest! {
    #[test]
    fn prop_score_lines_rows_and_status(
        scores in proptest::array::uniform4(any_score()),
        players in any::<u8>(),
        current_player in any::<u8>(),
        ball in any::<u8>(),
        credits in any::<u32>(),
    ) {
        let s = ScoreState { scores, players, current_player, ball, credits };
        let l = score_lines(&s);
        let n = players.clamp(1, 4) as usize;
        prop_assert_eq!(l.rows.len(), n);
        for (i, row) in l.rows.iter().enumerate() {
            prop_assert_eq!(row.player as usize, i + 1);
            prop_assert_eq!(&row.text, &format_score(scores[i]));
            if (current_player as usize) < n {
                prop_assert_eq!(row.current, i == current_player as usize);
            }
        }
        // At most one current row, whatever the (possibly out-of-range) current player.
        prop_assert!(l.rows.iter().filter(|r| r.current).count() <= 1);

        // Status line.
        prop_assert!(!l.status.is_empty());
        prop_assert!(l.status.chars().all(|c| c.is_ascii() && !c.is_ascii_control()),
            "status must be printable ASCII (the DMD fonts cover 32..=126): {:?}", &l.status);
        if ball == 0 {
            prop_assert_eq!(&l.status, &format!("CREDITS {credits}"));
        } else if n == 1 {
            prop_assert_eq!(&l.status, &format!("BALL {ball}  CREDITS {credits}"));
        } else if (current_player as usize) < n {
            prop_assert_eq!(&l.status, &format!("PLAYER {}  BALL {ball}", current_player + 1));
        } else {
            prop_assert!(l.status.starts_with("PLAYER "));
            prop_assert!(l.status.ends_with(&format!("  BALL {ball}")), "{:?}", &l.status);
        }
    }
}
