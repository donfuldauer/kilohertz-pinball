//! #103 R4: publishing the dynamic mesh slots (REQ-VIS-002) allocates
//! nothing. The all-kinds table of `mech_motion_impl.rs` (one mech of
//! every kind) is stepped under the counting allocator while its mechs
//! move; only the Lua VM's exempt arena may allocate.

mod sim_support;

use pin_core::sim::SimStep;
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::alloc_guard::{CountingAlloc, count_allocs_split, is_installed};
use pin_sim::{SimConfig, Simulation};
use sim_support::*;

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

const EXTRA: &str = r#"
[[coils]]
id = "gate_coil"
number = 4
kind = "kicker"

[[coils]]
id = "drop_reset"
number = 5
kind = "drop_reset"

[[coils]]
id = "sling_coil"
number = 6
kind = "sling"

[[coils]]
id = "plunger_coil"
number = 8
kind = "autoplunger"

[[switches]]
id = "newton_sw"
number = 21
kind = "target"
layer = 0

[[mechs]]
id = "posts[0]"
kind = "gate"
coil = "gate_coil"
position = [140.0, 760.0, 0.0]

[[mechs]]
id = "posts[1]"
kind = "diverter"
position = [374.0, 760.0, 0.0]

[[mechs]]
id = "newton"
kind = "newton_ball"
switch = "newton_sw"
position = [257.0, 300.0, 0.0]

[[mechs]]
id = "drop"
kind = "drop_target"
coil = "drop_reset"
position = [420.0, 450.0, 0.0]

[[mechs]]
id = "spinner"
kind = "spinner"
position = [300.0, 600.0, 0.0]

[[mechs]]
id = "sling"
kind = "sling"
coil = "sling_coil"
position = [80.0, 250.0, 0.0]
angle_deg = 20.0

[[mechs]]
id = "kickback"
kind = "kickback"
coil = "sling_coil"
position = [30.0, 200.0, 0.0]

[[mechs]]
id = "plunger"
kind = "plunger"
coil = "plunger_coil"
position = [490.0, 60.0, 0.0]

[[mechs]]
id = "vuk"
kind = "vuk"
position = [450.0, 850.0, 0.0]
"#;

/// Integer-only rules that cycle every coil and the diverter on a fixed
/// schedule (no tables or strings created after load).
const RULES: &str = r#"
local t = 0
function on_tick()
  t = t + 1
  local p = t % 600
  if p == 1 then
    pinball.hold_coil(pinball.coil.gate_coil, true)
    pinball.set_diverter(pinball.diverter["posts[1]"], true)
    pinball.pulse_coil(pinball.coil.sling_coil, 30)
    pinball.hold_coil(pinball.coil.plunger_coil, true)
  elseif p == 300 then
    pinball.hold_coil(pinball.coil.gate_coil, false)
    pinball.set_diverter(pinball.diverter["posts[1]"], false)
    pinball.hold_coil(pinball.coil.plunger_coil, false)
    pinball.pulse_coil(pinball.coil.drop_reset, 200)
  end
end
"#;

/// REQ-VIS-002, R4: no allocation in `step_1ms` (outside the Lua arena)
/// over 6 000 ticks of the all-kinds table with balls striking the drop
/// target, the spinner and the Newton ball and every coil cycling; the
/// slots are published (count 10) and some of them move.
#[test]
fn publishing_mech_slots_does_not_allocate() {
    assert!(is_installed());
    let table = fixture_table_toml() + EXTRA;
    let b = fixture_variant(
        "kp023_alloc",
        &[
            ("table.toml", Some(table.as_bytes())),
            ("rules.lua", Some(RULES.as_bytes())),
        ],
    );
    on_sim_thread(move || {
        let mut sim = Simulation::load(&b, SimConfig::default()).unwrap();
        let mut out = FrameStateSnapshot::default();
        let mut moved = [false; 10];
        let rest = *sim.snapshot();
        for k in 0..6_000u64 {
            if k % 1000 == 5 {
                let _ = sim.spawn_ball("standard_steel", [0.300, 0.50, 0.0135], [0.0, 3.0, 0.0]);
                let _ = sim.spawn_ball("standard_steel", [0.420, 0.52, 0.0135], [0.0, -1.5, 0.0]);
                let _ = sim.spawn_ball("standard_steel", [0.257, 0.24, 0.0135], [0.0, 1.5, 0.0]);
            }
            let t = sim.tick() + 1;
            let ((), n, _exempt) = count_allocs_split(|| sim.step_1ms(t, &[], &mut out));
            assert_eq!(n, 0, "tick {t}: {n} allocation(s) in step_1ms");
            assert_eq!(out.dynamic_mesh_count, 10);
            assert_eq!(
                out.ball_count as usize,
                (sim.balls_in_play() + 1).min(16),
                "tick {t}: free balls plus the captive Newton ball"
            );
            for (i, m) in out.dynamic_meshes().iter().enumerate() {
                moved[i] |= *m != rest.dynamic_meshes()[i];
            }
        }
        assert!(!sim.script_faulted(), "{:?}", sim.script_error());
        // Bumper (0), VUK (9) never move; every other kind did.
        assert_eq!(
            moved,
            [false, true, true, true, true, true, true, true, true, false]
        );
    });
}
