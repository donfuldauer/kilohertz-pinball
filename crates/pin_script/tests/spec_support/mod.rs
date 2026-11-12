//! Shared harness for the black-box spec tests of subphase 4.2.
//!
//! Everything here goes through the fixed public API only: `NameTables`,
//! `lua::{LuaConfig, LuaScriptBackend}` and the `pin_core` scripting contract.
#![allow(dead_code)]

use pin_core::{
    CoilId, DispenserId, DiverterId, FlipperId, HostCommandQueue, HostCtx, HostView, LampId,
    MagnetId, Pcg32, PersistKey, ProfileId, SceneId, SeedableRng, SoundId, SwitchEdge, SwitchId,
    TableScriptBackend,
};
use pin_script::NameTables;
use pin_script::lua::{LuaConfig, LuaScriptBackend};

/// The world a backend sees: view, seeded generator and command queue.
pub struct Host {
    pub view: HostView,
    pub rng: Pcg32,
    pub cmd: HostCommandQueue,
}

impl Host {
    pub fn new(seed: u64) -> Self {
        Self {
            view: HostView::default(),
            rng: Pcg32::seed_from_u64(seed),
            cmd: HostCommandQueue::default(),
        }
    }

    pub fn ctx(&mut self) -> HostCtx<'_> {
        HostCtx {
            view: &self.view,
            rng: &mut self.rng,
            cmd: &mut self.cmd,
        }
    }

    /// Calls `on_init` without clearing the queue first.
    pub fn init(&mut self, b: &mut dyn TableScriptBackend) {
        b.on_init(&mut self.ctx());
    }

    /// Clears the queue (as the sim does every tick), advances time, calls `on_tick`.
    pub fn tick(&mut self, b: &mut dyn TableScriptBackend) {
        self.cmd.clear();
        self.view.tick += 1;
        b.on_tick(&mut self.ctx());
    }

    /// Clears the queue, updates the switch bitset and delivers the edge.
    pub fn edge(&mut self, b: &mut dyn TableScriptBackend, switch: u16, closed: bool) {
        self.cmd.clear();
        self.view.set_switch(SwitchId(switch), closed);
        let edge = if closed {
            SwitchEdge::Closed
        } else {
            SwitchEdge::Opened
        };
        b.on_switch_edge(SwitchId(switch), edge, &mut self.ctx());
    }

    /// Delivers a closing edge for `switch` without touching the bitset: used
    /// by fixtures that dispatch test cases on the switch id.
    pub fn case(&mut self, b: &mut dyn TableScriptBackend, switch: u16) {
        self.cmd.clear();
        b.on_switch_edge(SwitchId(switch), SwitchEdge::Closed, &mut self.ctx());
    }
}

fn named<T>(items: &[(&str, u16)], make: impl Fn(u16) -> T) -> Vec<(String, T)> {
    items
        .iter()
        .map(|&(n, id)| (n.to_owned(), make(id)))
        .collect()
}

/// Name tables with distinctive, non-contiguous ids so mix-ups are visible.
pub fn names() -> NameTables {
    NameTables {
        switches: named(
            &[
                ("start", 0),
                ("left_sling", 7),
                ("right_sling", 8),
                ("trough_1", 20),
                ("spinner", 63),
                ("far", 255),
            ],
            SwitchId,
        ),
        coils: named(
            &[("trough_eject", 1), ("left_sling", 3), ("knocker", 31)],
            CoilId,
        ),
        lamps: named(&[("shoot_again", 2), ("jackpot", 200)], LampId),
        magnets: named(&[("center", 4)], MagnetId),
        diverters: named(&[("ramp_gate", 5)], DiverterId),
        flippers: named(&[("left", 0), ("right", 1), ("upper", 6)], FlipperId),
        dispensers: named(&[("trough", 9)], DispenserId),
        profiles: named(&[("steel", 0), ("ceramic", 11)], ProfileId),
        sounds: named(&[("bell", 12), ("drain", 13)], SoundId),
        scenes: named(&[("attract", 14), ("multiball", 15)], SceneId),
        persist: named(&[("high_score_1", 0), ("credits", 16), ("last", 63)], PersistKey),
    }
}

pub fn cfg(memory_limit_bytes: usize, instruction_budget: u32) -> LuaConfig {
    LuaConfig {
        memory_limit_bytes,
        instruction_budget,
    }
}

/// Builds a backend with the shared name tables and default limits; panics on error.
pub fn backend(source: &[u8]) -> LuaScriptBackend {
    backend_with(source, LuaConfig::default())
}

pub fn backend_with(source: &[u8], cfg: LuaConfig) -> LuaScriptBackend {
    match LuaScriptBackend::new(source, &names(), cfg) {
        Ok(b) => b,
        Err(e) => panic!("fixture failed to load: {e}"),
    }
}

/// Lamp intensities as a `lamp id -> value` lookup (last write wins).
pub fn lamp(cmd: &HostCommandQueue, id: u16) -> Option<f32> {
    cmd.lamps
        .iter()
        .rev()
        .find(|(l, _)| *l == LampId(id))
        .map(|&(_, v)| v)
}

/// Persist writes as a `key -> value` lookup (last write wins).
pub fn persisted(cmd: &HostCommandQueue, key: u16) -> Option<i64> {
    cmd.persist_writes
        .iter()
        .rev()
        .find(|(k, _)| *k == PersistKey(key))
        .map(|&(_, v)| v)
}

/// The score reported for 0-based `player` (last write wins).
pub fn score(cmd: &HostCommandQueue, player: u8) -> Option<i64> {
    cmd.scores
        .iter()
        .rev()
        .find(|(p, _)| *p == player)
        .map(|&(_, v)| v)
}
