;; Spec parity ruleset (subphase 4.5), Wasm side: parity_rules.lua written by
;; hand against the fixed `pinball` import ABI. Names are resolved with id_of
;; in on_init (the Lua side reads the pinball.<kind> tables).
(module
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
  (data (i32.const 16) "trough_eject")   ;; 12
  (data (i32.const 32) "left_sling")     ;; 10
  (data (i32.const 48) "knocker")        ;; 7
  (data (i32.const 64) "hold_gate")      ;; 9
  (data (i32.const 80) "shoot_again")    ;; 11
  (data (i32.const 96) "jackpot")        ;; 7
  (data (i32.const 112) "center")        ;; 6
  (data (i32.const 128) "ramp_gate")     ;; 9
  (data (i32.const 144) "upper")         ;; 5
  (data (i32.const 160) "trough")        ;; 6
  (data (i32.const 176) "steel")         ;; 5
  (data (i32.const 192) "ceramic")       ;; 7
  (data (i32.const 208) "bell")          ;; 4
  (data (i32.const 224) "drain")         ;; 5
  (data (i32.const 240) "attract")       ;; 7
  (data (i32.const 256) "multiball")     ;; 9
  (data (i32.const 272) "high_score_1")  ;; 12
  (data (i32.const 288) "credits")       ;; 7
  (data (i32.const 304) "last")          ;; 4
  (data (i32.const 320) "spinner")       ;; 7
  (data (i32.const 336) "start")         ;; 5
  (data (i32.const 352) "trough_1")      ;; 8

  (global $c_eject (mut i32) (i32.const -1))
  (global $c_sling (mut i32) (i32.const -1))
  (global $c_knock (mut i32) (i32.const -1))
  (global $c_gate (mut i32) (i32.const -1))
  (global $l_again (mut i32) (i32.const -1))
  (global $l_jack (mut i32) (i32.const -1))
  (global $m_center (mut i32) (i32.const -1))
  (global $dv_gate (mut i32) (i32.const -1))
  (global $f_upper (mut i32) (i32.const -1))
  (global $d_trough (mut i32) (i32.const -1))
  (global $p_steel (mut i32) (i32.const -1))
  (global $p_ceramic (mut i32) (i32.const -1))
  (global $s_bell (mut i32) (i32.const -1))
  (global $s_drain (mut i32) (i32.const -1))
  (global $sc_attract (mut i32) (i32.const -1))
  (global $sc_multi (mut i32) (i32.const -1))
  (global $k_hs (mut i32) (i32.const -1))
  (global $k_credits (mut i32) (i32.const -1))
  (global $k_last (mut i32) (i32.const -1))
  (global $sw_start (mut i32) (i32.const -1))
  (global $sw_lsling (mut i32) (i32.const -1))
  (global $sw_trough (mut i32) (i32.const -1))
  (global $sw_spinner (mut i32) (i32.const -1))

  (global $n (mut i64) (i64.const 0))
  (global $score (mut i64) (i64.const 0))
  (global $gate (mut i32) (i32.const 0))
  (global $spins (mut i64) (i64.const 0))

  (func (export "on_init")
    ;; kinds: 0 sw, 1 coil, 2 lamp, 3 magnet, 4 diverter, 5 flipper, 6 dispenser,
    ;; 7 profile, 8 sound, 9 scene, 10 persist_key
    (global.set $c_eject (call $id_of (i32.const 1) (i32.const 16) (i32.const 12)))
    (global.set $c_sling (call $id_of (i32.const 1) (i32.const 32) (i32.const 10)))
    (global.set $c_knock (call $id_of (i32.const 1) (i32.const 48) (i32.const 7)))
    (global.set $c_gate (call $id_of (i32.const 1) (i32.const 64) (i32.const 9)))
    (global.set $l_again (call $id_of (i32.const 2) (i32.const 80) (i32.const 11)))
    (global.set $l_jack (call $id_of (i32.const 2) (i32.const 96) (i32.const 7)))
    (global.set $m_center (call $id_of (i32.const 3) (i32.const 112) (i32.const 6)))
    (global.set $dv_gate (call $id_of (i32.const 4) (i32.const 128) (i32.const 9)))
    (global.set $f_upper (call $id_of (i32.const 5) (i32.const 144) (i32.const 5)))
    (global.set $d_trough (call $id_of (i32.const 6) (i32.const 160) (i32.const 6)))
    (global.set $p_steel (call $id_of (i32.const 7) (i32.const 176) (i32.const 5)))
    (global.set $p_ceramic (call $id_of (i32.const 7) (i32.const 192) (i32.const 7)))
    (global.set $s_bell (call $id_of (i32.const 8) (i32.const 208) (i32.const 4)))
    (global.set $s_drain (call $id_of (i32.const 8) (i32.const 224) (i32.const 5)))
    (global.set $sc_attract (call $id_of (i32.const 9) (i32.const 240) (i32.const 7)))
    (global.set $sc_multi (call $id_of (i32.const 9) (i32.const 256) (i32.const 9)))
    (global.set $k_hs (call $id_of (i32.const 10) (i32.const 272) (i32.const 12)))
    (global.set $k_credits (call $id_of (i32.const 10) (i32.const 288) (i32.const 7)))
    (global.set $k_last (call $id_of (i32.const 10) (i32.const 304) (i32.const 4)))
    (global.set $sw_start (call $id_of (i32.const 0) (i32.const 336) (i32.const 5)))
    (global.set $sw_lsling (call $id_of (i32.const 0) (i32.const 32) (i32.const 10)))
    (global.set $sw_trough (call $id_of (i32.const 0) (i32.const 352) (i32.const 8)))
    (global.set $sw_spinner (call $id_of (i32.const 0) (i32.const 320) (i32.const 7)))

    (call $show_scene (global.get $sc_attract))
    (call $set_gi_tint (f32.const 1) (f32.const 0.5) (f32.const 0.25) (f32.const 1))
    (call $set_score (i32.const 1) (i64.const 0))
    (call $persist_set (global.get $k_credits)
      (i64.add (call $persist_get (global.get $k_credits)) (i64.const 1)))
    (call $set_lamp (global.get $l_again) (f32.const 1))
    (global.set $score (i64.mul (call $random (i64.const 1) (i64.const 6)) (i64.const 100)))
    (call $set_score (i32.const 2) (global.get $score)))

  (func (export "on_tick") (local $r i64) (local $q i64) (local $i i32)
    (global.set $n (i64.add (global.get $n) (i64.const 1)))

    (if (i64.eqz (i64.rem_u (global.get $n) (i64.const 7))) (then
      (local.set $r (call $random (i64.const 1) (i64.const 6)))
      (if (i64.eq (i64.rem_u (local.get $r) (i64.const 2)) (i64.const 1))
        (then (call $set_lamp (i32.wrap_i64 (local.get $r)) (f32.const 1)))
        (else (call $set_lamp (i32.wrap_i64 (local.get $r)) (f32.const 0.5))))))

    (if (i64.eqz (i64.rem_u (global.get $n) (i64.const 13))) (then
      (global.set $gate (i32.sub (i32.const 1) (global.get $gate)))
      (call $hold_coil (global.get $c_gate) (global.get $gate))))

    (if (i64.eqz (i64.rem_u (global.get $n) (i64.const 50))) (then
      (local.set $r (call $random (i64.const 0) (i64.const 999)))
      (global.set $score (i64.add (global.get $score) (local.get $r)))
      (call $set_score
        (i32.add (i32.const 1)
          (i32.wrap_i64 (i64.rem_u (i64.div_u (global.get $n) (i64.const 50)) (i64.const 4))))
        (global.get $score))))

    (if (i64.eqz (i64.rem_u (global.get $n) (i64.const 97))) (then
      (local.set $r (call $random (i64.const -5) (i64.const 5)))
      (if (i64.lt_s (local.get $r) (i64.const 0))
        (then (call $play_sound (global.get $s_drain) (f32.const 0.25)))
        (else (call $play_sound (global.get $s_bell) (f32.const 1))))
      (global.set $score (i64.add (global.get $score) (local.get $r)))))

    (if (i64.eqz (i64.rem_u (call $tick) (i64.const 250))) (then
      (call $persist_set (global.get $k_last)
        (i64.add
          (i64.add (call $persist_get (global.get $k_last))
                   (i64.extend_i32_u (call $balls_in_play)))
          (call $tick)))))

    (if (call $switch (global.get $sw_spinner))
      (then
        (global.set $spins (i64.add (global.get $spins) (i64.const 1)))
        (call $set_lamp (global.get $l_jack) (f32.const 1))
        (if (i64.eqz (i64.rem_u (global.get $spins) (i64.const 4))) (then
          (call $set_magnet (global.get $m_center) (f32.const 0.5)))))
      (else
        (if (i64.eqz (i64.rem_u (global.get $n) (i64.const 100))) (then
          (call $set_magnet (global.get $m_center) (f32.const 0))))))

    (if (i64.eqz (i64.rem_u (global.get $n) (i64.const 333))) (then
      (local.set $q (i64.div_u (global.get $n) (i64.const 333)))
      (if (i64.eqz (i64.rem_u (local.get $q) (i64.const 2)))
        (then (call $spawn_ball (global.get $d_trough) (global.get $p_steel)))
        (else (call $spawn_ball (global.get $d_trough) (global.get $p_ceramic))))
      (call $set_diverter (global.get $dv_gate)
        (i64.eq (i64.rem_u (local.get $q) (i64.const 2)) (i64.const 1)))
      (call $set_zipper (global.get $f_upper)
        (i64.eqz (i64.rem_u (local.get $q) (i64.const 3))))
      (call $show_scene (global.get $sc_multi))))

    (if (i64.eqz (i64.rem_u (global.get $n) (i64.const 211))) (then
      (local.set $r (call $random (i64.const -9223372036854775808) (i64.const 9223372036854775807)))
      ;; floored modulo 1000 from the truncating rem_s
      (global.set $score (i64.add (global.get $score)
        (i64.rem_s (i64.add (i64.rem_s (local.get $r) (i64.const 1000)) (i64.const 1000))
                   (i64.const 1000))))
      (call $persist_set (global.get $k_hs) (call $random (i64.const 0) (i64.const 1099511627776)))
      (call $set_score (i32.const 4) (global.get $score))))

    (if (i64.eqz (i64.rem_u (global.get $n) (i64.const 500))) (then
      (local.set $i (i32.const 0))
      (loop $burst
        (call $pulse_coil (global.get $c_knock) (local.get $i))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br_if $burst (i32.lt_u (local.get $i) (i32.const 40)))))))

  (func (export "on_switch_edge") (param $id i32) (param $closed i32) (local $r i64)
    (if (local.get $closed)
      (then
        (call $pulse_coil (i32.rem_u (local.get $id) (i32.const 8))
                          (i32.add (i32.const 10) (local.get $id)))
        (global.set $score (i64.add (global.get $score)
          (i64.mul (i64.extend_i32_u (local.get $id)) (i64.const 10))))
        (call $set_score (i32.const 1) (global.get $score))
        (if (i32.eq (local.get $id) (global.get $sw_lsling))
          (then
            (local.set $r (call $random (i64.const 1) (i64.const 3)))
            (call $pulse_coil (global.get $c_sling)
              (i32.add (i32.const 20) (i32.wrap_i64 (local.get $r))))
            (call $play_sound (global.get $s_bell) (f32.const 0.5)))
          (else
            (if (i32.eq (local.get $id) (global.get $sw_start)) (then
              (call $spawn_ball (global.get $d_trough) (global.get $p_steel))
              (call $pulse_coil (global.get $c_eject) (i32.const 30))
              (call $persist_set (global.get $k_credits)
                (i64.sub (call $persist_get (global.get $k_credits)) (i64.const 1))))))))
      (else
        (call $set_lamp (local.get $id) (f32.const 0))
        (if (i32.and (i32.eq (local.get $id) (global.get $sw_trough))
                     (i32.eqz (call $balls_in_play)))
          (then
            (call $show_scene (global.get $sc_attract))
            (if (i64.gt_s (global.get $score) (call $persist_get (global.get $k_hs))) (then
              (call $persist_set (global.get $k_hs) (global.get $score)))))))))
)
