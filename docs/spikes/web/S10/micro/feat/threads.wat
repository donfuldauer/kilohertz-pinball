(module (memory 1 1 shared) (func (export "f") (result i32) (drop (i32.atomic.rmw.add (i32.const 0) (i32.const 7))) (i32.atomic.load (i32.const 0))))
