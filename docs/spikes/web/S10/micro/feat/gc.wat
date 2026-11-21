(module (type $s (struct (field i32))) (func (export "f") (result i32) (struct.get $s 0 (struct.new $s (i32.const 7)))))
