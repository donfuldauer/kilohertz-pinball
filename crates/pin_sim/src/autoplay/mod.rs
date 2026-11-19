//! Autoplay bot and table soak harness (Subphase 2.9).
//!
//! Stands in for human playtesting: [`Bot`] is a seeded heuristic player
//! that plays any table through the public [`Simulation`](crate::Simulation)
//! API, and [`run_soak`] plays N seeded games per table headless, faster
//! than real time and in parallel, then reports mechanical soundness and
//! coverage ([`SoakReport`], as JSON and Markdown). It proves a table is
//! mechanically sound and fully reachable; it does not judge feel.
//!
//! * [`bot`]: [`Bot`], [`SkillLevel`], [`SkillParams`], [`BotConfig`].
//! * [`table`]: [`TableInfo`], what the bot and monitor read from the
//!   bundle.
//! * [`monitor`]: [`GameMonitor`], the per-game health checks.
//! * [`soak`]: [`SoakConfig`], [`run_soak`], [`run_soak_recording`] (the
//!   soak with selected games' bot input recorded for
//!   [`replay_game`](crate::replay::replay_game)).
//! * [`report`]: [`Finding`], [`FindingKind`], [`SoakReport`] and its JSON /
//!   Markdown forms (and the failure policy).
//!
//! The bot and the harness live outside the tick path: they call the
//! simulation's public API between ticks, so the tick, its allocation guard
//! and the replay hashes are unchanged. `pin_sim soak` ([`crate::cli`]) is
//! the command line.
//!
//! **Mode starts** are observed through a `[persist]` key named
//! `mode:<id>` that the script increments when the mode starts
//! ([`table::MODE_PERSIST_PREFIX`]); the simulation does not expose the
//! `display_scenes` commands (DECISIONS.md "2.9 — mode-start signal").

pub mod bot;
pub mod monitor;
pub mod report;
pub mod soak;
pub mod table;

pub use bot::{
    Bot, BotConfig, BotStats, FlipperModel, NudgeConfig, ReactionDraw, SkillLevel, SkillParams,
};
pub use monitor::{GameMonitor, GameOutcome};
pub use report::{
    CheckStatus, CounterTotals, Distribution, Finding, FindingKind, GATE_A1_GAMES, GameEnd,
    GameSummary, GateA1Verdict, GateCheck, ModeCount, ShotCount, SoakReport, SwitchCount, Timing,
};
pub use soak::{
    RecordSoakError, RecordedSoak, SoakConfig, TickHistogram, bot_seed, check_record_selection,
    game_seed, play_soak_game, run_soak, run_soak_recording,
};
pub use table::TableInfo;
