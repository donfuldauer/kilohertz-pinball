(module (func $g (param i32) (result i32) (local.get 0)) (func (export "f") (result i32) (return_call $g (i32.const 7))))
