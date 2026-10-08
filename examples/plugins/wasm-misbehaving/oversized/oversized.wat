(module
  ;; Returns 200 bytes of prompt text while the manifest caps output at
  ;; 32: the host rejects it, prints one warning, and renders nothing.
  ;;
  ;; Identical to the `oversized_output_is_rejected` test in
  ;; crates/brish-plugin-wasm — keep the two in sync.
  (memory (export "memory") 1)
  ;; "x" repeated 200 times, so the read stays inside the page
  (data (i32.const 2048) "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx")
  (global $bump (mut i32) (i32.const 1024))
  (func (export "alloc") (param $ptr i32) (param $len i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (local.get $p) (local.get $len)))
    (local.get $p))
  (func (export "render") (param i32 i32 i32 i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 2048) (i64.const 32)) (i64.const 200))))