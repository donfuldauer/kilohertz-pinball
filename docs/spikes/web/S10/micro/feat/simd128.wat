(module (func (export "f") (result i32) (i32x4.extract_lane 2 (i32x4.add (v128.const i32x4 1 2 3 4) (v128.const i32x4 0 0 4 0)))))
