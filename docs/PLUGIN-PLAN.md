# briSH Plugin / Extension Plan

Plan 6.6 of `bsh-technical-plan.md`, scoped for briSH. Static-first:
everything here is **compile-time**. No dynamic loading, no stable ABI,
no sandbox — those are Phase 7+ decisions, not this plan.

## Goals

- Let behaviour hang off six seams the shell already has: before/after a
  command, on directory change, in the prompt, in completion, in keymaps.
- Ship one real example plugin so the traits are proven by use, not by
  design review.
- Keep the zero-panic/zero-unwrap policy: plugin output is data, never
  something the shell `unwrap`s.

## Non-goals

| Out of scope | Why |
|---|---|
| `dlopen` / cdylib plugins | ABI instability, unsoundness surface, Windows loading quirks |
| WASM plugins | Plan marks Phase 7+ |
| Sandboxing / capability model | Nothing to sandbox yet |
| Runtime hot-reload of compiled-in plugins | Config may toggle them, not replace them |

## Crate: `brish-plugin`

Holds **traits only** (depends on `brish-core` for `Env`/AST types, never
on the engine). Registration lives in the binary.

```rust
/// What a hook tells the engine to do next.
pub enum HookAction {
    Continue,
    /// Skip the command; shell sets this exit status.
    Abort(i32),
}

/// Everything a hook may see. Read-only — hooks mutate the shell only
/// through documented channels (`HookAction`, `PromptCtx`).
pub struct CmdCtx<'a> {
    pub argv: &'a [String],      // post-expansion words
    pub status_before: i32,
    pub cwd: &'a Path,
}

pub trait PreExecHook: Send + Sync {
    fn before(&self, ctx: &CmdCtx<'_>) -> HookAction;
}
pub trait PostExecHook: Send + Sync {
    fn after(&self, ctx: &CmdCtx<'_>, status: i32);
}
pub trait ChdirHook: Send + Sync {
    fn on_cd(&self, old: &Path, new: &Path);
}
pub trait PromptSegment: Send + Sync {
    /// Text to append after the base prompt, `None` to skip this render.
    fn render(&self, status: i32, cwd: &Path) -> Option<String>;
}
pub trait CompletionProvider: Send + Sync {
    /// Suggestions for the word under the cursor.
    fn complete(&self, word: &str, is_command: bool) -> Vec<String>;
}
pub trait KeymapProvider: Send + Sync {
    /// (key description, event name) pairs merged into the default emacs map.
    fn bindings(&self) -> Vec<(String, String)>;
}

/// Everything a user build can register into.
#[derive(Default)]
pub struct Registry {
    pub pre_exec: Vec<Box<dyn PreExecHook>>,
    pub post_exec: Vec<Box<dyn PostExecHook>>,
    pub on_chdir: Vec<Box<dyn ChdirHook>>,
    pub prompt_segments: Vec<Box<dyn PromptSegment>>,
    pub completion_providers: Vec<Box<dyn CompletionProvider>>,
    pub keymaps: Vec<Box<dyn KeymapProvider>>,
}
impl Registry {
    /// Entry point downstream builds call from their own `main`.
    pub fn register(&mut self, plugin: impl Plugin) { /* push into the right vec */ }
}
```

## Registration model (static)

Three tiers, all recompile-to-change:

1. **In-tree plugins** behind cargo features (`brish = { features = ["git-prompt"] }`).
2. **Downstream custom binaries**: depend on `brish`-crates, write their
   own `fn main()` that builds a `Registry`, calls `register`, hands it to
   the engine. This is the real extension story today.
3. **Config gate**: `~/.config/brish/config.toml`
   `[plugins] enabled = ["git-prompt", ...]` — selects *which compiled-in*
   plugins run. Unknown names warn, never crash.

## Wiring points (where the shell calls out)

| Trait | Call site | Notes |
|---|---|---|
| `PreExecHook` / `PostExecHook` | `exec.rs::exec_words`, around dispatch | Covers builtins *and* externals; `Abort` short-circuits |
| `ChdirHook` | `brish-builtin` `cd` builtin, after successful chdir | |
| `PromptSegment` | `brish/src/prompt.rs`, appended to robbyrussell base | Ordered, joined with spaces |
| `CompletionProvider` | `brish/src/completion.rs`, after builtins/PATH/files | `is_command` mirrors the current position heuristic |
| `KeymapProvider` | `brish/src/main.rs`, merged into `default_emacs_keybindings()` | Stringly-typed events keep the crate engine-free |

`Registry` construction happens once in `main`; the engine receives
`&Registry` (or `Arc<Registry>` where the forked children need it —
children get a clone-free read: hooks run in the parent only, so forked
stages simply never see them v1).

## Phases

### P1 — hooks + one example plugin
- `brish-plugin` crate with the traits above.
- Wire `PreExecHook`/`PostExecHook`/`ChdirHook` in the engine.
- Example plugin: `announce-cd` (prints old → new dir via `ChdirHook`),
  enabled by feature + config gate.
- Tests: hook order is registration order; `Abort(7)` skips the command
  and sets `$? = 7`; disabled-in-config plugin never fires.

### P2 — prompt segments + completion providers
- Move the robbyrussell git segment in as the flagship `PromptSegment`
  (**deferred pending desire** — single `git status --porcelain=v1
  --branch` call, no cache; add a 1s cache if prompt latency bites).
- Promote the lite completer's builtin/PATH/`$VAR`/file logic into
  `CompletionProvider`s; `BrishCompleter` becomes a chain over providers.
- Tests: segment ordering; provider suggestions merge + dedupe.

### P3 — keymaps + config polish
- `KeymapProvider` merge with conflict warning (last registered wins).
- `config.toml` loader for `[plugins]` (unknown key → warning, bad TOML →
  error message, never panic).
- Tests: conflict detection; bad config degrades to defaults.

## Acceptance

Each phase lands with its tests green under the workspace gate
(`cargo test/clippy/fmt` + Windows `cargo check`). P1 is the smallest
useful slice; P2/P3 wait for a real consumer of each trait.

## State

Implemented 2026-10-04 (P1+P2+P3), one commit per stage:

| Stage | Commit | Delivers |
|---|---|---|
| S1 | `e1d7c7d` | `brish-plugin` crate: traits, `Registry`, `HookAction`, `Completion` |
| S2 | `faaf248` | engine hook wiring (pre/post/chdir, parent-only, `theme`/`plugin` builtins) |
| S3 | `ccfbd4f` | builtin catalog: `default-themes`, `git-prompt`, `announce-cd` |
| S4 | `bd30881` | `config.toml` loader, `~/.config/brish/*` paths, `--theme`/`BRISH_THEME` |
| S5 | `1f8516a` | themes wired into the prompt, `PS1` still wins |
| S6 | `661cffb` | completion provider chain (`default-completion` plugin) |
| S7 | `b2d0310` | keymap provider merge with conflict warnings |
| S8 | `cf47398` | state/deviations, `docs/PLUGINS.md` authoring guide |

Trait signatures drifted from the draft above as the seams were
exercised (see deviations); **`docs/PLUGINS.md` is the authoritative
API reference**.

## Deviations

- **No cargo features.** Tier 1 (feature flags) replaced by
  config-only gating: every catalog plugin is compiled in, `config.toml`
  selects which run. One mechanism instead of two.
- **`Theme` seam added** (not in the draft trait list): whole-prompt
  rendering, with `Theme::render` receiving the registered segments —
  themes decide whether to show them (`minimal`/`plain` ignore them).
- **`Completion` is structured** (value/description/keep_typing) and the
  router takes `CompletionCtx` (command/`$var`/file position), not bare
  `(word, is_command)`.
- **`Registry::install`/`record`** replace the draft `register`;
  `installed()` powers the `plugin` builtin.
- **Downstream custom binaries (tier 2)** not yet exercised — no
  in-repo consumer besides `brish` itself.
