//! Score formatting and the score-view layout (GPU-free).

use super::scene::{Align, VAlign};
use pin_core::script::HostCommandQueue;

/// Players a score display shows.
pub const MAX_PLAYERS: usize = 4;

/// `score` in decimal with `,` thousands separators: `0`, `999`, `1,000`,
/// `-1,234,567`. Every `i64` works, including `i64::MIN`.
pub fn format_score(score: i64) -> String {
    format_score_with(score, Some(','))
}

/// `score` in decimal, digits grouped by three with `separator` (`None`: no
/// grouping). A negative score starts with `-`.
pub fn format_score_with(score: i64, separator: Option<char>) -> String {
    let digits = score.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if score < 0 {
        out.push('-');
    }
    let n = digits.len();
    for (i, d) in digits.chars().enumerate() {
        if i > 0
            && (n - i) % 3 == 0
            && let Some(sep) = separator
        {
            out.push(sep);
        }
        out.push(d);
    }
    out
}

/// What the score view shows: player scores, the current player, ball and
/// credits. Filled from the rules' `scores` commands
/// ([`apply_commands`](Self::apply_commands)); ball and credits come from
/// the game layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreState {
    /// Score per player (index = 0-based player).
    pub scores: [i64; MAX_PLAYERS],
    /// Players in the game, `1..=4` (other values are clamped when shown).
    pub players: u8,
    /// Current player, 0-based.
    pub current_player: u8,
    /// Ball in play, 1-based; 0 = no game in progress.
    pub ball: u8,
    /// Credits.
    pub credits: u32,
}

impl Default for ScoreState {
    fn default() -> Self {
        ScoreState {
            scores: [0; MAX_PLAYERS],
            players: 1,
            current_player: 0,
            ball: 0,
            credits: 0,
        }
    }
}

impl ScoreState {
    /// Applies `q.scores` in order (`(player, score)`, player 0-based; players
    /// ≥ 4 are ignored; `players` grows to cover the highest player set).
    /// Returns whether anything changed.
    pub fn apply_commands(&mut self, q: &HostCommandQueue) -> bool {
        let before = self.clone();
        for &(player, score) in &q.scores {
            let p = usize::from(player);
            if p >= MAX_PLAYERS {
                continue;
            }
            self.scores[p] = score;
            self.players = self.players.max(player + 1);
        }
        *self != before
    }

    /// `players` clamped to `1..=4`.
    pub fn player_count(&self) -> usize {
        usize::from(self.players).clamp(1, MAX_PLAYERS)
    }

    /// The current player's score (player clamped to the player count).
    pub fn current_score(&self) -> i64 {
        self.scores[usize::from(self.current_player).min(self.player_count() - 1)]
    }
}

/// One player's line of the score view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerRow {
    /// Player number, 1-based.
    pub player: u8,
    /// [`format_score`] of the player's score.
    pub text: String,
    /// Whether this is the player up.
    pub current: bool,
}

/// The text content of the score view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreLines {
    /// One row per player (1–4).
    pub rows: Vec<PlayerRow>,
    /// Ball / credit / player line.
    pub status: String,
}

/// The score view's text: one row per player (`players` clamped to 1..=4)
/// and a status line: `ball == 0` → `"CREDITS <n>"`; one player →
/// `"BALL <b>  CREDITS <n>"`; several → `"PLAYER <p>  BALL <b>"`.
pub fn score_lines(state: &ScoreState) -> ScoreLines {
    let n = state.player_count();
    let current = usize::from(state.current_player).min(n - 1);
    let rows = (0..n)
        .map(|i| PlayerRow {
            player: i as u8 + 1,
            text: format_score(state.scores[i]),
            current: i == current,
        })
        .collect();
    let status = if state.ball == 0 {
        format!("CREDITS {}", state.credits)
    } else if n == 1 {
        format!("BALL {}  CREDITS {}", state.ball, state.credits)
    } else {
        format!("PLAYER {}  BALL {}", current + 1, state.ball)
    };
    ScoreLines { rows, status }
}

/// One text item of the score view, placed in display fractions.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreText {
    /// Text.
    pub text: String,
    /// Anchor x, fraction of the display width.
    pub x: f32,
    /// Anchor y, fraction of the display height.
    pub y: f32,
    /// Glyph cell height, fraction of the display height.
    pub size: f32,
    /// Horizontal alignment at the anchor.
    pub align: Align,
    /// Vertical alignment at the anchor.
    pub valign: VAlign,
    /// Brightness multiplier (1 for the player up, 0.5 for the others).
    pub emphasis: f32,
}

/// Brightness of players who are not up in a multi-player view.
pub const OTHER_PLAYER_EMPHASIS: f32 = 0.5;

/// Layout of the score view (shared by DMD and LCD), in fractions of the
/// display.
///
/// One player: the score large in the upper part (size 0.55, centred at
/// y = 0.38) and the status line at the bottom (size 0.25). Several: a 2 × 2
/// grid (players 1 / 2 on the top row, 3 / 4 below; left column
/// left-aligned, right column right-aligned; size 0.28), the player up at
/// full brightness and the others at [`OTHER_PLAYER_EMPHASIS`], and the
/// status line at the bottom.
pub fn score_view_layout(state: &ScoreState) -> Vec<ScoreText> {
    let lines = score_lines(state);
    let mut out = Vec::with_capacity(lines.rows.len() + 1);
    if lines.rows.len() == 1 {
        out.push(ScoreText {
            text: lines.rows[0].text.clone(),
            x: 0.5,
            y: 0.38,
            size: 0.55,
            align: Align::Center,
            valign: VAlign::Middle,
            emphasis: 1.0,
        });
    } else {
        for (i, row) in lines.rows.iter().enumerate() {
            let right = i % 2 == 1;
            out.push(ScoreText {
                text: row.text.clone(),
                x: if right { 0.98 } else { 0.02 },
                y: if i < 2 { 0.04 } else { 0.38 },
                size: 0.28,
                align: if right { Align::Right } else { Align::Left },
                valign: VAlign::Top,
                emphasis: if row.current {
                    1.0
                } else {
                    OTHER_PLAYER_EMPHASIS
                },
            });
        }
    }
    out.push(ScoreText {
        text: lines.status,
        x: 0.5,
        y: 0.98,
        size: 0.25,
        align: Align::Center,
        valign: VAlign::Bottom,
        emphasis: 1.0,
    });
    out
}

/// Replaces the placeholders of a scene text: `{score}` (player up),
/// `{score1}`…`{score4}`, `{player}` (1-based), `{ball}`, `{credits}`.
/// Unknown `{…}` sequences stay as written.
pub fn expand_text(text: &str, state: &ScoreState) -> String {
    if !text.contains('{') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find('}') else {
            out.push_str(tail);
            return out;
        };
        let key = &tail[1..end];
        let value = match key {
            "score" => Some(format_score(state.current_score())),
            "score1" | "score2" | "score3" | "score4" => {
                let i = usize::from(key.as_bytes()[5] - b'1');
                Some(format_score(state.scores[i]))
            }
            "player" => Some(
                (usize::from(state.current_player).min(state.player_count() - 1) + 1).to_string(),
            ),
            "ball" => Some(state.ball.to_string()),
            "credits" => Some(state.credits.to_string()),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}
