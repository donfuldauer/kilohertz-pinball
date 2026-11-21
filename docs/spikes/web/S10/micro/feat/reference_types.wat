(module (table $t 2 externref) (func (export "f") (param externref) (result i32) (table.set $t (i32.const 1) (local.get 0)) (i32.add (table.size $t) (i32.const 5))))
