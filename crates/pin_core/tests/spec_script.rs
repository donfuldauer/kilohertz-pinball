//! Black-box spec tests for `pin_core::script` (PLAN.md §3.4).

use pin_core::Pcg32;
use pin_core::ids::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId, SwitchId,
};
use pin_core::script::{
    HostCommandQueue, HostCtx, HostView, ScriptError, SwitchEdge, TableScriptBackend,
};

fn empty_view(tick: u64) -> HostView {
    // Exactly the four fields of §3.4.
    HostView {
        tick,
        switches: [0u64; 4],
        balls_in_play: 0u8,
        persist: [0i64; 64],
    }
}

// ---------------------------------------------------------------- SwitchEdge

#[test]
fn switch_edge_variants_and_derives() {
    fn assert_traits<T: Copy + Clone + std::fmt::Debug + PartialEq + Eq>() {}
    assert_traits::<SwitchEdge>();

    let closed = SwitchEdge::Closed;
    let copy = closed; // Copy
    assert_eq!(closed, copy);
    assert_ne!(SwitchEdge::Closed, SwitchEdge::Opened);
    assert_eq!(format!("{:?}", SwitchEdge::Closed), "Closed");
    assert_eq!(format!("{:?}", SwitchEdge::Opened), "Opened");

    // Exactly two variants.
    let as_bool = |e: SwitchEdge| match e {
        SwitchEdge::Closed => true,
        SwitchEdge::Opened => false,
    };
    assert!(as_bool(SwitchEdge::Closed));
    assert!(!as_bool(SwitchEdge::Opened));
}

// ---------------------------------------------------------------- HostView

#[test]
fn host_view_field_shapes() {
    let mut view = empty_view(42);
    let tick: u64 = view.tick;
    assert_eq!(tick, 42);

    // 256-switch bitset in four words.
    let switches: &mut [u64; 4] = &mut view.switches;
    switches[3] |= 1 << 63;
    assert_eq!(view.switches.len() * 64, 256);
    assert_eq!(view.switches[3] >> 63, 1);

    let balls: u8 = view.balls_in_play;
    assert_eq!(balls, 0);

    let persist: &mut [i64; 64] = &mut view.persist;
    persist[63] = i64::MIN;
    assert_eq!(view.persist.len(), 64);
    assert_eq!(view.persist[63], i64::MIN);
}

// ---------------------------------------------------------------- HostCommandQueue

#[test]
fn default_queue_is_empty() {
    let q = HostCommandQueue::default();
    assert!(q.coil_pulses.is_empty());
    assert!(q.coil_holds.is_empty());
    assert!(q.magnets.is_empty());
    assert!(q.zippers.is_empty());
    assert!(q.diverters.is_empty());
    assert!(q.ball_spawns.is_empty());
    assert!(q.lamps.is_empty());
    assert!(q.gi_tint.is_none());
    assert!(q.scores.is_empty());
    assert!(q.display_scenes.is_empty());
    assert!(q.sounds.is_empty());
    assert!(q.persist_writes.is_empty());
}

#[test]
fn queue_capacities_match_the_contract() {
    let q = HostCommandQueue::default();
    assert_eq!(q.coil_pulses.capacity(), 32);
    assert_eq!(q.coil_holds.capacity(), 32);
    assert_eq!(q.magnets.capacity(), 8);
    assert_eq!(q.zippers.capacity(), 8);
    assert_eq!(q.diverters.capacity(), 16);
    assert_eq!(q.ball_spawns.capacity(), 8);
    assert_eq!(q.lamps.capacity(), 256);
    assert_eq!(q.scores.capacity(), 4);
    assert_eq!(q.display_scenes.capacity(), 8);
    assert_eq!(q.sounds.capacity(), 16);
    assert_eq!(q.persist_writes.capacity(), 8);
}

#[test]
fn queue_element_types_match_the_contract() {
    let mut q = HostCommandQueue::default();
    q.coil_pulses.push((CoilId(1), 50u16));
    q.coil_holds.push((CoilId(2), true));
    q.magnets.push((MagnetId(3), 0.5f32));
    q.zippers.push((FlipperId(4), true));
    q.diverters.push((DiverterId(5), false));
    q.ball_spawns.push((DispenserId(6), ProfileId(7)));
    q.lamps.push((LampId(8), 1.0f32));
    q.gi_tint = Some([1.0f32, 0.5, 0.25, 1.0]);
    q.scores.push((0u8, 1_000_000i64));
    q.display_scenes.push(SceneId(9));
    q.sounds.push((SoundId(10), 0.8f32));
    q.persist_writes.push((PersistKey(11), -1i64));

    assert_eq!(q.coil_pulses[0], (CoilId(1), 50));
    assert_eq!(q.coil_holds[0], (CoilId(2), true));
    assert_eq!(q.magnets[0], (MagnetId(3), 0.5));
    assert_eq!(q.zippers[0], (FlipperId(4), true));
    assert_eq!(q.diverters[0], (DiverterId(5), false));
    assert_eq!(q.ball_spawns[0], (DispenserId(6), ProfileId(7)));
    assert_eq!(q.lamps[0], (LampId(8), 1.0));
    assert_eq!(q.gi_tint, Some([1.0, 0.5, 0.25, 1.0]));
    assert_eq!(q.scores[0], (0, 1_000_000));
    assert_eq!(q.display_scenes[0], SceneId(9));
    assert_eq!(q.sounds[0], (SoundId(10), 0.8));
    assert_eq!(q.persist_writes[0], (PersistKey(11), -1));
}

#[test]
fn queues_fill_to_capacity_and_then_refuse() {
    let mut q = HostCommandQueue::default();
    for i in 0..32u16 {
        assert!(q.coil_pulses.try_push((CoilId(i), 10)).is_ok());
    }
    assert!(q.coil_pulses.is_full());
    assert!(q.coil_pulses.try_push((CoilId(99), 10)).is_err());
    assert_eq!(q.coil_pulses.len(), 32);

    for i in 0..256u16 {
        assert!(q.lamps.try_push((LampId(i), 1.0)).is_ok());
    }
    assert!(q.lamps.try_push((LampId(0), 0.0)).is_err());

    for i in 0..8u16 {
        assert!(q.magnets.try_push((MagnetId(i), 1.0)).is_ok());
    }
    assert!(q.magnets.try_push((MagnetId(8), 1.0)).is_err());

    for i in 0..16u16 {
        assert!(q.sounds.try_push((SoundId(i), 1.0)).is_ok());
    }
    assert!(q.sounds.try_push((SoundId(16), 1.0)).is_err());
}

// ---------------------------------------------------------------- backend trait

/// A minimal rules script: counts callbacks, mirrors switch edges onto a lamp
/// and pulses a coil on every tenth tick.
#[derive(Default)]
struct DummyBackend {
    inits: u32,
    ticks: u32,
    edges: Vec<(SwitchId, SwitchEdge, u64)>,
    reloads: Vec<Vec<u8>>,
}

impl TableScriptBackend for DummyBackend {
    fn on_init(&mut self, ctx: &mut HostCtx) {
        self.inits += 1;
        ctx.cmd.display_scenes.push(SceneId(0));
        ctx.cmd.gi_tint = Some([1.0, 1.0, 1.0, 1.0]);
    }

    fn on_tick(&mut self, ctx: &mut HostCtx) {
        self.ticks += 1;
        if ctx.view.tick.is_multiple_of(10) {
            let _ = ctx.cmd.coil_pulses.try_push((CoilId(1), 25));
        }
    }

    fn on_switch_edge(&mut self, switch: SwitchId, edge: SwitchEdge, ctx: &mut HostCtx) {
        self.edges.push((switch, edge, ctx.view.tick));
        let level = match edge {
            SwitchEdge::Closed => 1.0,
            SwitchEdge::Opened => 0.0,
        };
        let _ = ctx.cmd.lamps.try_push((LampId(switch.0), level));
        // The sim-owned RNG is reachable from a callback.
        let _rng: &mut Pcg32 = ctx.rng;
    }

    fn apply_reload(&mut self, source: &[u8]) -> Result<(), ScriptError> {
        self.reloads.push(source.to_vec());
        Ok(())
    }
}

#[test]
fn dummy_backend_can_be_driven_through_host_ctx() {
    let mut backend = DummyBackend::default();
    let mut rng = Pcg32::new(0x853c_49e6_748f_ea9b, 0xda3e_39cb_94b9_5bdb);
    let mut cmd = HostCommandQueue::default();

    let view = empty_view(0);
    {
        let mut ctx = HostCtx {
            view: &view,
            rng: &mut rng,
            cmd: &mut cmd,
        };
        backend.on_init(&mut ctx);
    }
    assert_eq!(backend.inits, 1);
    assert_eq!(cmd.display_scenes.len(), 1);
    assert_eq!(cmd.gi_tint, Some([1.0, 1.0, 1.0, 1.0]));

    for tick in 0..100u64 {
        let view = empty_view(tick);
        cmd = HostCommandQueue::default(); // the host clears the queue each tick
        let mut ctx = HostCtx {
            view: &view,
            rng: &mut rng,
            cmd: &mut cmd,
        };
        if tick == 33 {
            backend.on_switch_edge(SwitchId(24), SwitchEdge::Closed, &mut ctx);
        }
        if tick == 35 {
            backend.on_switch_edge(SwitchId(24), SwitchEdge::Opened, &mut ctx);
        }
        backend.on_tick(&mut ctx);

        assert_eq!(cmd.coil_pulses.len(), usize::from(tick.is_multiple_of(10)));
        match tick {
            33 => assert_eq!(cmd.lamps.as_slice(), &[(LampId(24), 1.0)]),
            35 => assert_eq!(cmd.lamps.as_slice(), &[(LampId(24), 0.0)]),
            _ => assert!(cmd.lamps.is_empty()),
        }
    }
    assert_eq!(backend.ticks, 100);
    assert_eq!(
        backend.edges,
        vec![
            (SwitchId(24), SwitchEdge::Closed, 33),
            (SwitchId(24), SwitchEdge::Opened, 35),
        ]
    );

    assert!(backend.apply_reload(b"-- new rules").is_ok());
    assert_eq!(backend.reloads, vec![b"-- new rules".to_vec()]);
}

#[test]
fn backend_is_object_safe_and_send() {
    fn assert_send<T: Send + ?Sized>() {}
    assert_send::<dyn TableScriptBackend>();
    assert_send::<Box<dyn TableScriptBackend>>();

    let mut boxed: Box<dyn TableScriptBackend> = Box::new(DummyBackend::default());
    let view = empty_view(10);
    let mut rng = Pcg32::new(1, 1);
    let mut cmd = HostCommandQueue::default();
    let mut ctx = HostCtx {
        view: &view,
        rng: &mut rng,
        cmd: &mut cmd,
    };
    boxed.on_init(&mut ctx);
    boxed.on_tick(&mut ctx);
    boxed.on_switch_edge(SwitchId(1), SwitchEdge::Closed, &mut ctx);
    assert!(boxed.apply_reload(&[]).is_ok());
    assert_eq!(cmd.coil_pulses.as_slice(), &[(CoilId(1), 25)]);
    assert_eq!(cmd.lamps.as_slice(), &[(LampId(1), 1.0)]);

    // A boxed backend can be handed to another thread (the sim thread).
    let handle = std::thread::spawn(move || {
        let mut boxed = boxed;
        boxed.apply_reload(b"x").is_ok()
    });
    assert!(handle.join().expect("thread"));
}

#[test]
fn seeded_pcg32_is_reproducible() {
    // Same seed and stream give equal generator state: replays depend on it.
    let a = Pcg32::new(42, 54);
    let b = Pcg32::new(42, 54);
    let c = Pcg32::new(43, 54);
    assert_eq!(a, b);
    assert_ne!(a, c);
}
