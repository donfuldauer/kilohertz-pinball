(module (memory 1) (func (export "f") (result i32) (memory.fill (i32.const 0) (i32.const 7) (i32.const 16)) (memory.copy (i32.const 32) (i32.const 0) (i32.const 16)) (i32.load8_u (i32.const 40))))
