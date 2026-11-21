//! Steps a [`Simulation`] through a [`Scenario`].

use crate::io::{Op, Scenario};
use arrayvec::ArrayVec;
use pin_core::input::InputEvent;
use pin_core::profiles::BUILTIN_PROFILE_NAMES;
use pin_core::sim::SimStep;
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::Simulation;
use pin_sim::replay::apply_game_op;

/// Most input events fed with one tick.
pub const MAX_INPUTS_PER_TICK: usize = 128;

/// A scenario cursor.
pub struct Driver {
    ops: Vec<(u64, Op)>,
    next: usize,
    inputs: ArrayVec<InputEvent, MAX_INPUTS_PER_TICK>,
    out: FrameStateSnapshot,
    /// Ticks to run.
    pub ticks: u64,
}

impl Driver {
    /// A driver for `scn`.
    #[must_use]
    pub fn new(scn: &Scenario) -> Self {
        Self {
            ops: scn.ops.clone(),
            next: 0,
            inputs: ArrayVec::new(),
            out: FrameStateSnapshot::default(),
            ticks: scn.ticks,
        }
    }

    /// Applies the bot operations and spawns recorded before the next tick
    /// and collects its input events; returns the tick number.
    ///
    /// # Errors
    /// A message when an operation fails or a tick has too many inputs.
    pub fn prepare(&mut self, sim: &mut Simulation) -> Result<u64, String> {
        let tick = sim.tick() + 1;
        self.inputs.clear();
        while self.next < self.ops.len() && self.ops[self.next].0 <= tick {
            let (t, op) = self.ops[self.next];
            self.next += 1;
            if t < tick {
                continue;
            }
            match op {
                Op::Game(g) => apply_game_op(sim, g).map_err(|e| e.to_string())?,
                Op::Input(e) => self
                    .inputs
                    .try_push(e)
                    .map_err(|_| format!("tick {tick}: more than {MAX_INPUTS_PER_TICK} inputs"))?,
                Op::Spawn {
                    profile,
                    layer,
                    pos,
                    vel,
                } => {
                    let name = BUILTIN_PROFILE_NAMES
                        .get(usize::from(profile))
                        .ok_or("bad profile")?;
                    // A full arena or a spawn inside geometry is part of
                    // the recorded scenario; ignore failures alike.
                    let _ = sim.spawn_ball_on_layer(name, layer, pos, vel);
                }
            }
        }
        Ok(tick)
    }

    /// Runs tick `tick` (from [`prepare`](Self::prepare)) with its inputs.
    pub fn step(&mut self, sim: &mut Simulation, tick: u64) {
        sim.step_1ms(tick, &self.inputs, &mut self.out);
    }

    /// [`prepare`](Self::prepare) then [`step`](Self::step).
    ///
    /// # Errors
    /// As [`prepare`](Self::prepare).
    pub fn advance(&mut self, sim: &mut Simulation) -> Result<u64, String> {
        let t = self.prepare(sim)?;
        self.step(sim, t);
        Ok(t)
    }
}
