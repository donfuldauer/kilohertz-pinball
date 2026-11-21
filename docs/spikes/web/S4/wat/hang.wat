(module (memory (export "memory") 1)
  (func (export "on_init")) (func (export "on_tick") (loop $l (br $l)))
  (func (export "on_switch_edge") (param i32 i32)))
