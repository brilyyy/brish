# WASM plugins

**Status: slice 1 shipped** — `PromptSegment`, behind `--features wasm`.
The remaining seams (completion, hooks, keymaps) are steps 3-5 below and
are still deferred; the reasoning for the ceilings is unchanged.

A store plugin with a `[wasm]` section runs a WebAssembly module in the
`wasmi` interpreter and uses its return value as prompt segment text:

```toml
[wasm]
path = "plugin.wasm"     # relative to the plugin directory
fuel = 1000000           # per render; the guest is cut off when spent
max_output = 4096        # cap on the returned string
```

Build the host with the feature:

```sh
cargo build --release --features wasm
```

A release binary built without it ignores `[wasm]` manifests entirely — the
interpreter is ~2.9 MiB and nothing else pulls it in.

### ABI

The module must export `memory`, `alloc` and `render`. No wit-bindgen, no
component model, no host imports:

```wat
(func (export "alloc") (param $ptr i32) (param $len i32) (result i32) …)
(func (export "render") (param $ptr i32) (param $len i32) (param $status i32)
      (param $cwd_ptr i32) (param $cwd_len i32) (result i64) …)
```

`alloc` reserves `len` bytes and returns the offset. The host writes the
decimal status code and the cwd there. `render` returns
`(out_ptr << 32) | out_len`; `out_len == 0` means no segment. Anything the
guest returns has control characters and ESC stripped, so a plugin cannot
inject escapes or newlines into the prompt line.

A working guest: `examples/wasm/segment-guest/` (build with
`--target wasm32-unknown-unknown`; the built module is checked in as
`examples/plugins/wasm-status/plugin.wasm`).

### What a guest cannot do

No imports are linkable, so: no filesystem, no network, no process spawn,
no clock, no env. A guest that *asks* for a host function fails to
instantiate. Fuel bounds its work (there is no process to kill), and
`max_output` bounds its output. A module that traps, runs out of fuel, or
returns garbage yields no segment and one warning — never a dead prompt.

## The original reasoning, and what changed

The four arguments for *not* building this were sound. What changed is
that we stopped arguing for "WASM in general" and built the one slice
that survives all four.

1. **Static Rust plugins are already memory-safe.** Unchanged, and it is
   why `[wasm]` is opt-in per plugin rather than a new default. The win
   is real only for third-party modules nobody compiles against our
   lints.

2. **Process isolation is already available.** Unchanged. Helpers still
   run as deadline-killed subprocesses and are still the right tool for
   anything that needs `git` or a shell-out. A WASM segment cannot
   compete with them and does not try to.

3. **The valuable plugins need process spawn, which WASI lacks.** This
   is the load-bearing one, and it is why the shipped seam is the *only*
   one that needs no spawn. `git status`, docker, kubectl and venv
   segments stay as helpers — a host `spawn` capability would hand back
   exactly the trust the sandbox removes.

4. **Dependency cost.** Partially answered by picking the wrong runtime
   on purpose. **wasmtime** is the heaviest crate in the ecosystem:
   +15-25 MiB, minutes of cold compile, and a large `unsafe` tree from
   Cranelift. **wasmi** is an interpreter: +2.9 MiB on a 3.7 MiB binary,
   no JIT pages (so no W^X surface), and a cold compile measured in
   seconds. For a function that formats a string it is the right trade.

The one genuine argument for WASM remains: making `plugin add <url>`
safe by default, rather than "review the source before you install it"
(see the trust table in [`SECURITY.md`](SECURITY.md)). That matters only
if the plugin index grows third-party plugins users are unwilling to
audit by hand.

## How it fit

No engine changes. `brish-plugin-wasm` depends on `brish-plugin-api` and
`wasmi` and implements the same trait — this is the reason the seam
traits live in their own crate below the engine.

### The capability split is forced by the seams

**Reachable** — data in, data out, all `&self`:

| Trait | Signature |
|---|---|
| `PromptSegment` | `render(&self, status, cwd) -> Option<String>` |
| `Theme` | `render(&self, status, cwd, segments) -> String` |
| `CompletionProvider` | `complete(&self, ctx) -> Vec<Completion>` |
| `KeymapProvider` | `bindings(&self) -> Vec<(String, String)>` |
| `PreExecHook` | `before(&self, ctx) -> HookAction` |
| `PostExecHook` | `after(&self, ctx, status)` |
| `ChdirHook` | `on_cd(&self, old, new)` |

**Not reachable** — they return `Box<dyn reedline::…>`, an in-process
object graph that cannot cross a WASM boundary: `EditModeFactory`,
`HighlighterFactory`, `HinterFactory`, `MenuFactory`,
`ValidatorFactory`, `HistoryFactory`.

So a WASM plugin could serve prompts, completion, keymaps and hooks —
and nothing that participates in line editing. This is a documented
ceiling, not a bug to be engineered around.

### Constraints, as implemented

**Every seam is `&self`, so the `Store` needs interior mutability.**
`PromptSegment::render(&self, …)` cannot take `&mut self`, so the
`Store` lives behind a `Mutex` — one lock per prompt render. The module
is instantiated once at load, never in the render path, and the exports
are resolved once there too (`check_abi`), so a render is a `set_fuel`, a
memory write and a call. `Registry` is immutable after startup, so this
holds for the process lifetime.

**Fuel is the deadline.** The helper protocol kills a subprocess
(250ms completion / 500ms segment / 1000ms hook); a WASM guest has no
process to kill. wasmi's answer is fuel metering:
`Config::consume_fuel(true)` plus `Store::set_fuel(n)` before each call,
so an infinite loop in a guest costs `n` operations and then returns an
error. Note this is a *work* bound, not a wall-clock one — the
interpreter runs at a predictable speed, so it is the analogue that
matters, but it is not the same guarantee a subprocess deadline gives.

### Capabilities would be default-deny

`plugin.toml` already declares `path`, `fuel` and `max_output`. A future
slice would add capabilities in the same style:

```toml
[wasm]
path = "plugin.wasm"
capabilities = ["read-cwd", "spawn:git"]
```

Nothing is granted unless listed, and today nothing *can* be granted. The
`spawn:` prefix is the sharp edge: it re-opens point 3, so it should
require an explicit opt-in distinct from read-only capabilities.

## Remaining steps

Sequence, each step independently useful:

1. ~~New crate `brish-plugin-wasm`, gated so the default build and
   binary are unchanged.~~ **Done** (`--features wasm`, +2.9 MiB).
2. ~~`PromptSegment` only, end to end.~~ **Done.**
3. `CompletionProvider`, exercising the `Mutex<Store>` constraint under
   the much hotter keystroke path.
4. Hooks, where a per-call fuel budget replaces a subprocess kill.
5. Capabilities, if the value is still there after 3–4.

Do not start with completion or hooks — they carry the concurrency and
interruption complexity, and the runtime plumbing is now proven on a
trivial seam.