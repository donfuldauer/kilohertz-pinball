(module (tag $t (param i32))
  (func (export "f") (result i32)
    try (result i32) i32.const 7 throw $t catch $t end))
