# WASM plugins — deferred, and how they'd fit

**Status: not implemented, deliberately.** This document records the
reasoning so the decision is not re-litigated from scratch, and so
whoever picks it up does not have to rediscover the constraints.

## Why not

1. **Static Rust plugins are already memory-safe.** The API crate
   (`brish-plugin-api`) is `#![forbid(unsafe_code)]`, and the
   workspace denies `clippy::unwrap_used` / `expect_used` outside tests.
   A static plugin cannot corrupt the shell's memory. WASM's
   memory-isolation win only applies to *third-party* code, which by
   definition is not compiled against our lints.

2. **Process isolation is already available.** The zero-compile helper
   protocol (see [`PLUGINS.md`](PLUGINS.md)) runs each seam as a
   subprocess with a deadline. A segfaulting or hanging helper kills
   the subprocess, not the shell. WASM would give a *finer* capability
   boundary — filesystem and process access only if explicitly granted —
   but a helper is already confined to whatever PATH tools it can exec.

3. **The valuable plugins need process spawn, which WASI lacks.** The
   flagship prompt segment is `git status`
   (`crates/brish-theme/src/git.rs`); docker, kubectl and venv segments
   all shell out. WASI preview1 has no process spawn, so serving these
   from WASM requires a host `spawn` capability — which hands back
   precisely the trust the sandbox was meant to remove.

4. **Dependency cost.** wasmtime is one of the heaviest crates in the
   ecosystem: large binary, long cold compiles, and a sizeable `unsafe`
   tree — against a project whose current runtime deps are `nix` and
   `reedline`.

The one genuine argument for WASM: making `plugin add <url>` safe by
default, rather than "review the source before you install it" (see the
trust table in [`SECURITY.md`](SECURITY.md)). That matters only if the
plugin index grows third-party plugins users are unwilling to audit by
hand.

## How it would fit

The crate layering already accommodates this with no engine changes:
a `brish-plugin-wasm` crate would depend on `brish-plugin-api` and
`wasmtime`, and implement the same traits. This is the reason the seam
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

### Two implementation constraints

**Every seam is `&self`, so the wasmtime `Store` needs interior
mutability.** `CompletionProvider::complete(&self, …)` cannot take
`&mut self`, so the `Store` lives behind a `Mutex` — which keeps the
impl `Sync` at the cost of one lock per keystroke-path call. Instantiate
once per session (never in the render path) and hold the
`Mutex<Store>` for the process lifetime; `Registry` is immutable after
startup, so that works.

**Deadlines need epoch interruption.** The helper protocol already has
deadlines (250ms completion / 500ms segment / 1000ms hook). wasmtime's
`Config::epoch_interruption` plus `Store::set_epoch_deadline` is the
direct analogue — there is no subprocess to kill, so the guest must be
interrupted from the host side.

### Capabilities would be default-deny

Declared in `plugin.toml`, mirroring the existing `[completion]` /
`match_description` style that `store.rs` already parses:

```toml
[wasm]
path = "plugin.wasm"
capabilities = ["read-cwd", "spawn:git"]
```

No capability not listed is available. Note the `spawn:` prefix is the
sharp edge: it re-opens point 3 above, so it should require an explicit
opt-in distinct from read-only capabilities.

## If you implement it

Sequence, each step independently useful:

1. New crate `brish-plugin-wasm` behind a `brish-plugin-wasm` manifest
   section, gated so the default build and binary are unchanged.
2. `PromptSegment` only, end to end. Simplest seam: no cap, pure
   function, string out.
3. `CompletionProvider`, exercising the `Mutex<Store>` constraint.
4. Hooks, exercising deadline interruption.
5. Capabilities, if the value is still there after 1–4.

Do not start with completion or hooks — they carry the concurrency and
interruption complexity, and you want the runtime plumbing proven on a
trivial seam first.