(module (global $g (export "g") (mut i32) (i32.const 7)) (func (export "f") (result i32) (global.get $g)))
