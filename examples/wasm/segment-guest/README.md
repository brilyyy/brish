# WASM segment guest — example

The source for `examples/plugins/wasm-status/plugin.wasm`: a prompt
segment that shows the last exit status on failure and the cwd's
basename otherwise.

A guest gets **no host imports**. Not "no imports by convention" — the
host's `Linker` defines nothing, so a module that imports anything fails
to instantiate. That means this guest cannot read git, stat a file, or
shell out; it can only format the two things briSH hands it (the status
code and the cwd) and return a string.

For anything that needs real access, use a
[helper plugin](../../plugins/words/) instead: helpers are subprocesses
and can do anything the user can. See
[`docs/PLUGIN-WASM.md`](../../../docs/PLUGIN-WASM.md).

## Build

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/brish_wasm_example_guest.wasm \
   ../../plugins/wasm-status/plugin.wasm
```

Then install the plugin and run a shell built with the feature:

```sh
brish plugin add ../../plugins/wasm-status
brish --version   # from a build with --features wasm
```

A default build (no `--features wasm`) ignores `[wasm]` manifests.

## The ABI

Three exports, no wit-bindgen:

| export | signature |
|---|---|
| `memory` | linear memory, exported |
| `alloc` | `(ptr: i32, len: i32) -> i32` — reserve `len` bytes, return the offset |
| `render` | `(ptr, len, status, cwd_ptr, cwd_len) -> i64` — returns `(out_ptr << 32) \| out_len` |

`out_len == 0` means "no segment this render". The host writes the
decimal status code at `ptr/len` and the cwd at `cwd_ptr/cwd_len`; both
come from `alloc`, so the guest's allocator has to be real (this one is
a bump allocator, capped at 1 MiB — a session renders the prompt many
times and the host never frees).

`#[unsafe(no_mangle)] extern "C"` is the whole interface. `panic = "abort"`
and `opt-level = "s"` in the profile keep the module small (14 KiB); a
guest does not need a panic handler because there is no host to unwind
into.