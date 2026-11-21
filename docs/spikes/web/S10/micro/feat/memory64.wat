(module (memory i64 1) (func (export "f") (result i32) (i64.store (i64.const 8) (i64.const 7)) (i32.wrap_i64 (i64.load (i64.const 8)))))
