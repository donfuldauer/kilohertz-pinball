;; S9 req 9 (ii): a player rules.wasm whose on_tick loops forever from tick 3000
;; (wasmtime-exact fuel budget kill -> tilt; the sim and page must continue).
(module
  (import "pinball" "tick" (func $tick (result i64)))
  (import "pinball" "set_score" (func $set_score (param i32 i64)))
  (memory (export "memory") 1)
  (func (export "on_init") (call $set_score (i32.const 1) (i64.const 777)))
  (func (export "on_tick")
    (if (i64.ge_u (call $tick) (i64.const 3000))
      (then (loop $l (br $l)))))
  (func (export "on_switch_edge") (param i32 i32)))
