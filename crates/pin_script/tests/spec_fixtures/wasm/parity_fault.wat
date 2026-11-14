;; Spec fault-parity ruleset (subphase 4.5), Wasm side of parity_fault.lua.
(module
  (import "pinball" "pulse_coil" (func $pulse_coil (param i32 i32)))
  (import "pinball" "hold_coil" (func $hold_coil (param i32 i32)))
  (import "pinball" "set_lamp" (func $set_lamp (param i32 f32)))
  (import "pinball" "set_gi_tint" (func $set_gi_tint (param f32 f32 f32 f32)))
  (import "pinball" "set_score" (func $set_score (param i32 i64)))
  (import "pinball" "show_scene" (func $show_scene (param i32)))
  (memory (export "memory") 1)
  (global $n (mut i32) (i32.const 0))
  (func $hold_range (param $c i32) (param $end i32)
    (loop $l
      (call $hold_coil (local.get $c) (i32.const 1))
      (local.set $c (i32.add (local.get $c) (i32.const 1)))
      (br_if $l (i32.le_u (local.get $c) (local.get $end)))))
  (func (export "on_init"))
  (func (export "on_tick")
    (global.set $n (i32.add (global.get $n) (i32.const 1)))
    (if (i32.eq (global.get $n) (i32.const 1)) (then
      (call $hold_range (i32.const 100) (i32.const 131)) (return)))
    (if (i32.eq (global.get $n) (i32.const 2)) (then
      (call $hold_range (i32.const 200) (i32.const 207))
      (call $hold_coil (i32.const 100) (i32.const 0)) (return)))
    (if (i32.eq (global.get $n) (i32.const 3)) (then
      (call $pulse_coil (i32.const 1) (i32.const 5))
      (call $hold_coil (i32.const 300) (i32.const 1))
      (call $set_lamp (i32.const 1) (f32.const 1))
      (call $set_gi_tint (f32.const 0.5) (f32.const 0.5) (f32.const 0.5) (f32.const 0.5))
      (call $pulse_coil (i32.const 70000) (i32.const 1)) (return)))
    (call $show_scene (i32.const 1)))
  (func (export "on_switch_edge") (param $id i32) (param $closed i32)
    (call $set_score (i32.const 1) (i64.extend_i32_u (local.get $id))))
)
