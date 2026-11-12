;; Spec-test driver guest for subphase 4.5 (hand-written against the fixed ABI).
;;
;; on_switch_edge(op, closed) performs ONE operation selected by `op`, taking its
;; arguments from persist slots 0..=3 of the host view (i32 = low 32 bits,
;; f32 = bit pattern in the low 32 bits, i64 = the slot). Value-returning
;; operations report through set_score(1, value).
;; When `closed` is 0 the callback first queues a fixed set of marker commands,
;; so tests can observe that a faulting callback is rolled back.
;; on_init shows scene 1; on_tick shows scene 7.
;;
;; driver.wasm is this file assembled to binary (see README.md).
(module
  (type $void (func))
  (import "pinball" "pulse_coil" (func $pulse_coil (param i32 i32)))
  (import "pinball" "hold_coil" (func $hold_coil (param i32 i32)))
  (import "pinball" "set_magnet" (func $set_magnet (param i32 f32)))
  (import "pinball" "set_zipper" (func $set_zipper (param i32 i32)))
  (import "pinball" "set_diverter" (func $set_diverter (param i32 i32)))
  (import "pinball" "spawn_ball" (func $spawn_ball (param i32 i32)))
  (import "pinball" "set_lamp" (func $set_lamp (param i32 f32)))
  (import "pinball" "set_gi_tint" (func $set_gi_tint (param f32 f32 f32 f32)))
  (import "pinball" "set_score" (func $set_score (param i32 i64)))
  (import "pinball" "show_scene" (func $show_scene (param i32)))
  (import "pinball" "play_sound" (func $play_sound (param i32 f32)))
  (import "pinball" "persist_set" (func $persist_set (param i32 i64)))
  (import "pinball" "persist_get" (func $persist_get (param i32) (result i64)))
  (import "pinball" "switch" (func $switch (param i32) (result i32)))
  (import "pinball" "tick" (func $tick (result i64)))
  (import "pinball" "balls_in_play" (func $balls_in_play (result i32)))
  (import "pinball" "random" (func $random (param i64 i64) (result i64)))
  (import "pinball" "id_of" (func $id_of (param i32 i32 i32) (result i32)))

  (memory (export "memory") 1)
  (table 1 funcref)

  ;; Names for id_of tests (offset, length):
  (data (i32.const 1024) "left_sling")    ;; 10
  (data (i32.const 1040) "trough_eject")  ;; 12
  (data (i32.const 1056) "jackpot")       ;; 7
  (data (i32.const 1072) "center")        ;; 6
  (data (i32.const 1088) "ramp_gate")     ;; 9
  (data (i32.const 1104) "upper")         ;; 5
  (data (i32.const 1120) "trough")        ;; 6
  (data (i32.const 1136) "ceramic")       ;; 7
  (data (i32.const 1152) "drain")         ;; 5
  (data (i32.const 1168) "multiball")     ;; 9
  (data (i32.const 1184) "credits")       ;; 7
  (data (i32.const 1200) "nope")          ;; 4
  (data (i32.const 1216) "far")           ;; 3
  (data (i32.const 1232) "\ff\fe\80")     ;; 3, not UTF-8
  ;; the last 5 bytes of the first page
  (data (i32.const 65531) "start")

  ;; argument accessors
  (func $a (param $k i32) (result i32) (i32.wrap_i64 (call $persist_get (local.get $k))))
  (func $f (param $k i32) (result f32) (f32.reinterpret_i32 (call $a (local.get $k))))
  (func $l (param $k i32) (result i64) (call $persist_get (local.get $k)))

  (func $rec (param $x i32) (result i32)
    (i32.add (call $rec (i32.add (local.get $x) (i32.const 1))) (i32.const 1)))

  (func (export "on_init") (call $show_scene (i32.const 1)))
  (func (export "on_tick") (call $show_scene (i32.const 7)))

  (func (export "on_switch_edge") (param $op i32) (param $closed i32)
    (local $i i32) (local $end i32)
    (if (i32.eqz (local.get $closed)) (then
      (call $pulse_coil (i32.const 9) (i32.const 9))
      (call $hold_coil (i32.const 8) (i32.const 1))
      (call $set_lamp (i32.const 9) (f32.const 1))
      (call $set_gi_tint (f32.const 0.5) (f32.const 0.5) (f32.const 0.5) (f32.const 0.5))
      (call $set_score (i32.const 4) (i64.const 99))
      (call $persist_set (i32.const 9) (i64.const 9))))

    (if (i32.eq (local.get $op) (i32.const 0)) (then
      (call $pulse_coil (call $a (i32.const 0)) (call $a (i32.const 1))) (return)))
    (if (i32.eq (local.get $op) (i32.const 1)) (then
      (call $hold_coil (call $a (i32.const 0)) (call $a (i32.const 1))) (return)))
    (if (i32.eq (local.get $op) (i32.const 2)) (then
      (call $set_magnet (call $a (i32.const 0)) (call $f (i32.const 1))) (return)))
    (if (i32.eq (local.get $op) (i32.const 3)) (then
      (call $set_zipper (call $a (i32.const 0)) (call $a (i32.const 1))) (return)))
    (if (i32.eq (local.get $op) (i32.const 4)) (then
      (call $set_diverter (call $a (i32.const 0)) (call $a (i32.const 1))) (return)))
    (if (i32.eq (local.get $op) (i32.const 5)) (then
      (call $spawn_ball (call $a (i32.const 0)) (call $a (i32.const 1))) (return)))
    (if (i32.eq (local.get $op) (i32.const 6)) (then
      (call $set_lamp (call $a (i32.const 0)) (call $f (i32.const 1))) (return)))
    (if (i32.eq (local.get $op) (i32.const 7)) (then
      (call $set_gi_tint (call $f (i32.const 0)) (call $f (i32.const 1))
                         (call $f (i32.const 2)) (call $f (i32.const 3))) (return)))
    (if (i32.eq (local.get $op) (i32.const 8)) (then
      (call $set_score (call $a (i32.const 0)) (call $l (i32.const 1))) (return)))
    (if (i32.eq (local.get $op) (i32.const 9)) (then
      (call $show_scene (call $a (i32.const 0))) (return)))
    (if (i32.eq (local.get $op) (i32.const 10)) (then
      (call $play_sound (call $a (i32.const 0)) (call $f (i32.const 1))) (return)))
    (if (i32.eq (local.get $op) (i32.const 11)) (then
      (call $persist_set (call $a (i32.const 0)) (call $l (i32.const 1))) (return)))
    (if (i32.eq (local.get $op) (i32.const 12)) (then
      (call $set_score (i32.const 1) (call $persist_get (call $a (i32.const 0)))) (return)))
    (if (i32.eq (local.get $op) (i32.const 13)) (then
      (call $set_score (i32.const 1) (i64.extend_i32_s (call $switch (call $a (i32.const 0))))) (return)))
    (if (i32.eq (local.get $op) (i32.const 14)) (then
      (call $set_score (i32.const 1) (call $tick)) (return)))
    (if (i32.eq (local.get $op) (i32.const 15)) (then
      (call $set_score (i32.const 1) (i64.extend_i32_s (call $balls_in_play))) (return)))
    (if (i32.eq (local.get $op) (i32.const 16)) (then
      (call $set_score (i32.const 1) (call $random (call $l (i32.const 0)) (call $l (i32.const 1)))) (return)))
    (if (i32.eq (local.get $op) (i32.const 17)) (then
      (call $set_score (i32.const 1) (i64.extend_i32_s
        (call $id_of (call $a (i32.const 0)) (call $a (i32.const 1)) (call $a (i32.const 2))))) (return)))

    ;; 100: unreachable
    (if (i32.eq (local.get $op) (i32.const 100)) (then (unreachable)))
    ;; 101: spin forever
    (if (i32.eq (local.get $op) (i32.const 101)) (then (loop $spin (br $spin))))
    ;; 102: integer division by zero
    (if (i32.eq (local.get $op) (i32.const 102)) (then
      (call $set_score (i32.const 1)
        (i64.extend_i32_s (i32.div_s (i32.const 1) (call $a (i32.const 3))))) (return)))
    ;; 103: out-of-bounds load at address a0
    (if (i32.eq (local.get $op) (i32.const 103)) (then
      (call $set_score (i32.const 1) (i64.load (call $a (i32.const 0)))) (return)))
    ;; 104: unbounded recursion
    (if (i32.eq (local.get $op) (i32.const 104)) (then
      (call $set_score (i32.const 1) (i64.extend_i32_s (call $rec (i32.const 0)))) (return)))
    ;; 105: memory.grow(a0) -> result
    (if (i32.eq (local.get $op) (i32.const 105)) (then
      (call $set_score (i32.const 1) (i64.extend_i32_s (memory.grow (call $a (i32.const 0))))) (return)))
    ;; 106: memory.size -> result
    (if (i32.eq (local.get $op) (i32.const 106)) (then
      (call $set_score (i32.const 1) (i64.extend_i32_s (memory.size))) (return)))
    ;; 107: bits of (f0 + f1) -> result
    (if (i32.eq (local.get $op) (i32.const 107)) (then
      (call $set_score (i32.const 1) (i64.extend_i32_u (i32.reinterpret_f32
        (f32.add (call $f (i32.const 0)) (call $f (i32.const 1)))))) (return)))
    ;; 108: call_indirect through a null table entry
    (if (i32.eq (local.get $op) (i32.const 108)) (then
      (call_indirect (type $void) (i32.const 0)) (return)))
    ;; 109: count a0 down to zero, then report 1
    (if (i32.eq (local.get $op) (i32.const 109)) (then
      (local.set $i (call $a (i32.const 0)))
      (block $done
        (loop $count
          (br_if $done (i32.eqz (local.get $i)))
          (local.set $i (i32.sub (local.get $i) (i32.const 1)))
          (br $count)))
      (call $set_score (i32.const 1) (i64.const 1)) (return)))
    ;; 111: 40 pulses of coil 2 (the queue holds 32)
    (if (i32.eq (local.get $op) (i32.const 111)) (then
      (local.set $i (i32.const 0))
      (loop $p
        (call $pulse_coil (i32.const 2) (local.get $i))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br_if $p (i32.lt_u (local.get $i) (i32.const 40))))
      (return)))
    ;; 112: spin forever calling the host
    (if (i32.eq (local.get $op) (i32.const 112)) (then
      (loop $h
        (call $pulse_coil (i32.const 2) (i32.const 1))
        (call $set_lamp (i32.const 2) (f32.const 1))
        (drop (call $random (i64.const 1) (i64.const 6)))
        (br $h))))
    ;; 113: hold_coil(c, 1) for c in a0 .. a0 + a1
    (if (i32.eq (local.get $op) (i32.const 113)) (then
      (local.set $i (call $a (i32.const 0)))
      (local.set $end (i32.add (local.get $i) (call $a (i32.const 1))))
      (block $done
        (loop $hold
          (br_if $done (i32.ge_u (local.get $i) (local.get $end)))
          (call $hold_coil (local.get $i) (i32.const 1))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $hold)))
      (return)))
    ;; 114: out-of-bounds store at address a0
    (if (i32.eq (local.get $op) (i32.const 114)) (then
      (i64.store (call $a (i32.const 0)) (i64.const -1)) (return)))
  )
)
