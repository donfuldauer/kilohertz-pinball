//! Subphase 6.2 spec test (Independent Tester, brief C7): the real "Sky
//! Deck" simulation stepped through `pin_app::orchestrator::SimLoop` makes
//! no heap allocation per tick once warmed up (PLAN §2.3 rule 4, D5),
//! including the ticks of a Sky Bridge `action` press, of the arming
//! (Newton ball and drop bank struck by balls) and of a Night Overdrive
//! start (UV GI, Glo-Ball served and lifted to the deck). The guard is
//! `pin_sim::alloc_guard`; allocations of the Lua VM's own arena are exempt
//! by that guard's rules. State injection (spawning and removing the
//! striker balls) happens between ticks, outside the counted region.

#[path = "fixtures/spec_6_2/common.rs"]
mod common;

use common::{GLO_ID, STEEL, at, bundle, dir, mech, newton_aim, persist_index, scale};
use pin_app::orchestrator::{SimLoop, load_validated};
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::sim::TICK_NS;
use pin_core::snapshot::FrameStateSnapshot;
use pin_core::spsc;
use pin_core::triple_buffer::TripleBuffer;
use pin_sim::alloc_guard::{CountingAlloc, assert_no_alloc, count_allocs, is_installed};
use pin_sim::{BallHandle, SimConfig, Simulation};

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// The loop plus the worst per-tick allocation count seen.
struct Rig {
    sim_loop: SimLoop<Simulation>,
    input: spsc::SpscProducer<InputEvent>,
    worst: u64,
    measured: u64,
}

impl Rig {
    /// Queues a button event for tick `tick` (timestamp inside the tick).
    fn push(&mut self, tick: u64, i: CabinetInput, p: bool) {
        self.input
            .push(InputEvent::button(tick * TICK_NS - TICK_NS / 2, i, p))
            .expect("input ring has room");
    }

    /// Queues the lower-flipper presses of the next tick (left and right
    /// alternately every 250 ticks, held 80) so flips keep flowing.
    fn flippers(&mut self) {
        let next = self.sim_loop.tick() + 1;
        let i = if (next / 250).is_multiple_of(2) {
            CabinetInput::LeftFlipper
        } else {
            CabinetInput::RightFlipper
        };
        if next >= 600 && next.is_multiple_of(250) {
            self.push(next, i, true);
        } else if next >= 600 && next % 250 == 80 {
            self.push(next, i, false);
        }
    }

    /// One uncounted warm-up tick.
    fn warm_tick(&mut self) {
        self.flippers();
        self.sim_loop.run_ticks(1);
    }

    /// One counted tick.
    fn tick(&mut self) {
        self.flippers();
        let ((), n) = count_allocs(|| self.sim_loop.run_ticks(1));
        self.worst = self.worst.max(n);
        self.measured += 1;
        assert_eq!(n, 0, "tick {} allocated {n} times", self.sim_loop.tick());
    }

    fn sim(&self) -> &Simulation {
        self.sim_loop.sim()
    }

    fn persist(&self, key: &str) -> i64 {
        self.sim().persist(persist_index(key))
    }

    /// Counted ticks until switch `sw` counts a hit (<= `max`).
    fn until_hit(&mut self, sw: &str, max: u64) -> bool {
        let before = self.sim().switch_hits(sw).expect("switch");
        for _ in 0..max {
            self.tick();
            if self.sim().switch_hits(sw).expect("switch") > before {
                return true;
            }
        }
        false
    }

    /// Spawns a striker (uncounted), runs counted ticks until `sw` is hit,
    /// then removes the striker (uncounted).
    fn strike(&mut self, layer: u8, pos: [f32; 3], vel: [f32; 3], sw: &str) {
        let h: BallHandle = self
            .sim_loop
            .sim_mut()
            .spawn_ball_on_layer(STEEL, layer, pos, vel)
            .expect("spawn striker");
        assert!(self.until_hit(sw, 400), "striker closed {sw}");
        for _ in 0..5 {
            self.tick();
        }
        let _ = self.sim_loop.sim_mut().remove_ball(h);
    }
}

fn run() {
    assert!(is_installed(), "CountingAlloc is the global allocator");
    let sim = load_validated(bundle(), SimConfig::default()).expect("bundle loads validated");
    let (input, consumer) = spsc::channel::<InputEvent>(4096);
    let (writer, mut reader) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut rig = Rig {
        sim_loop: SimLoop::new(sim, consumer, writer),
        input,
        worst: 0,
        measured: 0,
    };
    // Warm-up: start a game, flippers.
    rig.push(50, CabinetInput::Start, true);
    rig.push(120, CabinetInput::Start, false);
    let warm = 4_000u64;
    for _ in 0..warm {
        rig.warm_tick();
    }
    assert_eq!(rig.persist("game_state"), 1, "a game is running");
    assert!(rig.sim().balls_in_play() > 0, "the served ball");

    // 1. Sky Bridge action press (counted).
    let t = rig.sim_loop.tick();
    rig.push(t + 100, CabinetInput::Action, true);
    rig.push(t + 160, CabinetInput::Action, false);
    let mut opened = false;
    for _ in 0..2_000 {
        rig.tick();
        opened |= rig.persist("bridge_open") == 1;
    }
    assert!(opened, "the action press opened the bridge");
    for _ in 0..7_000 {
        rig.tick();
    }

    // 2. Arming by contact (counted ticks; spawns between ticks).
    let (d, c0) = newton_aim(rig.sim());
    let nl = mech("newton").layer.unwrap_or(0);
    rig.strike(
        nl,
        [c0[0] - d[0] * 0.045, c0[1] - d[1] * 0.045, c0[2]],
        scale(d, 1.5),
        "newton_sw",
    );
    for id in ["drop_1", "drop_2", "drop_3"] {
        let def = mech(id);
        let u = dir(def.angle_deg.unwrap_or(90.0));
        let p = def.position.expect("drop position");
        let sw = def.switch.clone().expect("drop switch");
        rig.strike(def.layer.unwrap_or(0), at(p, u, 0.04), scale(u, -1.2), &sw);
    }
    for _ in 0..20 {
        rig.tick();
    }
    assert_eq!(rig.persist("overdrive_ready"), 1, "armed");

    // 3. Night Overdrive start by the action button (counted), Glo-Ball on
    // the deck.
    let mode0 = rig.persist("mode:night_overdrive");
    let t = rig.sim_loop.tick();
    rig.push(t + 10, CabinetInput::Action, true);
    rig.push(t + 70, CabinetInput::Action, false);
    let mut glo_on_deck = false;
    for _ in 0..6_000 {
        rig.tick();
        let s = rig.sim();
        glo_on_deck |= s.ball_handles().iter().any(|&h| {
            s.ball(h)
                .is_some_and(|b| b.profile.0 == GLO_ID && b.layer == 1)
        });
    }
    assert_eq!(
        rig.persist("mode:night_overdrive"),
        mode0 + 1,
        "the mode started"
    );
    assert!(glo_on_deck, "the Glo-Ball reached the deck");

    // 4. One guarded block.
    assert_no_alloc("SimLoop::run_ticks with Sky Deck", || {
        rig.sim_loop.run_ticks(500);
    });
    let snap = reader.read();
    assert_eq!(
        snap.tick,
        rig.sim_loop.tick(),
        "the loop published every tick"
    );
    println!(
        "Sky Deck: {} counted ticks after a {warm}-tick warm-up, worst {} allocations per tick",
        rig.measured, rig.worst
    );
    assert_eq!(rig.worst, 0);
    assert!(
        !rig.sim().script_faulted(),
        "{:?}",
        rig.sim().script_error()
    );
    assert_eq!(rig.sim().script_queue_overflows(), 0);
}

#[test]
fn sky_deck_ticks_do_not_allocate_across_bridge_and_overdrive() {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(run)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}
