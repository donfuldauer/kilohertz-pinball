(module (func (export "f") (result i32) (i32.trunc_f32_s (f32x4.extract_lane 0 (f32x4.relaxed_madd (v128.const f32x4 2 0 0 0) (v128.const f32x4 3 0 0 0) (v128.const f32x4 1 0 0 0))))))
