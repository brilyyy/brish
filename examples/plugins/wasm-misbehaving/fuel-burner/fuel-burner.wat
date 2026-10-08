(module
  ;; An infinite loop. The host sets a fuel budget before every render,
  ;; so this costs `fuel` operations and then returns an error: the
  ;; segment is skipped and one warning is printed.
  ;;
  ;; Identical to the `out_of_fuel_yields_no_segment` test in
  ;; crates/brish-plugin-wasm — keep the two in sync.
  (memory (export "memory") 1)
  (func (export "alloc") (param $ptr i32) (param $len i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (local.get $p) (local.get $len)))
    (local.get $p))
  (global $bump (mut i32) (i32.const 1024))
  (func (export "render") (param i32 i32 i32 i32 i32) (result i64)
    (loop $spin (br $spin))
    (i64.const 0)))