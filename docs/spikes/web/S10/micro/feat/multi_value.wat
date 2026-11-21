(module (func $p (result i32 i32) (i32.const 3) (i32.const 4)) (func (export "f") (result i32) (call $p) (i32.add)))
