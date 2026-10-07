# briSH Architecture

briSH (brily SHell) is a memory-safe POSIX shell. This is the component
map.

## Crates

```
crates/brish-core     lexer, parser, AST, expansions, Env (zero deps)
crates/brish-words    words: field splitting, globbing, pattern match,
                      arithmetic, test(1)
crates/brish-platform Unix syscalls: fork/exec/wait, signals, job
                      control, terminal (the only crate with `unsafe`)
crates/brish-plugin-api  extension API: traits + Registry (leaf — a
                      third-party Rust plugin compiles against this
                      alone)
crates/brish-builtin  POSIX builtins + the plugin store
                      (store/store_cmd) + config paths
crates/brish-engine   execution engine: AST eval, pipelines, redirs,
                      job control, plugin hook dispatch
crates/brish-plugin   every bundled plugin: config, completion packs,
                      highlight, hinter, edit modes, prompt, keymaps,
                      announce builtin
crates/brish-theme    prompt catalog: themes + prompt segments
crates/brish          binary: CLI, REPL (reedline), registry wiring
fuzz/                 libFuzzer targets (lexer/parser/expand)
```

Dependency rule, bottom-up: `brish-core` (zero deps) ← `brish-words` /
`brish-platform` / `brish-plugin-api` ← `brish-builtin` ← `brish-engine`
← `brish-plugin` ← `brish` (binary). `brish-plugin-api` sits *below*
the engine on purpose: the engine dispatches plugins through the
Registry, so the trait layer cannot depend on the engine, and plugins
that need engine types live above it in `brish-plugin`. Nothing in
`brish-plugin-api` ever grows an implementation.

## Data flow

```
  stdin / script ─→ reedline (interactive)  ─┐
                └→ parser → lexer          ─┼→ ExecContext
                   AST ─→ expander ─→ engine ─→ spawn / builtins / jobs
                                           └→ stdout / stderr / $?
```

Interactive only: syntax highlighting and autosuggest are **read-only
previews** over the same lexer/expander — they never execute anything.

## Key subsystems

| Subsystem | Where | Notes |
|---|---|---|
| Reader/REPL | `brish` (binary) | reedline; indicators/colors configurable; history `HISTCONTROL` wrapper |
| Lexer | `brish-core::lexer` | hand-written, quote/heredoc state stack; `$'…'`, here-strings |
| Parser | `brish-core::parser` | hand-written recursive descent, `ModePolicy` |
| Expander | `brish-core::expand` | POSIX order; IFS-edge field splitting (see below) |
| Engine | `brish-engine` | `Engine` holds env + funcs + jobs + registry; non-local flow via `Stop` |
| Builtins | `brish-builtin::run` | in-process with fd save/restore |
| Platform | `brish-platform` | nix-based; trap handlers set flags only |
| Extensions | `brish-plugin` / `brish-theme` | static registry; store plugins (commit-pinned) |

## Execution model

- Builtins run in-process (fd redirection via `FdScope`); externals,
  pipelines, subshells, background jobs and command substitutions fork.
  A crashing command cannot kill the shell.
- Background/pipeline/subshell children reset trappable signal
  dispositions at fork (POSIX subshell rule).
- Signal traps use a **flag-only handler**; the engine drains at command
  boundaries, before each prompt, and — via EINTR-aware waits — while
  blocked in `waitpid` (mid-command delivery). Because a
  process-directed signal lands on *any* unblocked thread, EINTR alone
  is not enough in a threaded process: `wait_pid`/`wait_untraced` also
  poll `WNOHANG` (2ms) and re-check the pending-trap word while another
  thread is alive. Single-threaded shells keep the blocking wait.
- Zero-panic: `clippy::unwrap_used`/`expect_used` denied outside tests;
  only `brish-platform` contains `unsafe` (SAFETY-commented).

## Design decisions worth knowing

- Windows support removed (beta); unix-only (see CHANGELOG).
- The engine lives in `brish-engine`, not `brish-core` — `brish-core`
  must stay dependency-free.
- The seam traits live in `brish-plugin-api` *below* the engine so that
  plugins needing engine types can live in `brish-plugin` above it
  without a cycle. `brish-theme` is a separate catalog crate so theme
  authors need not pull the behavioral plugins.
- The store (`store`/`store_cmd`/`helper`) lives in `brish-builtin`
  rather than `brish-plugin`, because the engine calls
  `store_cmd::dispatch` and the prompt calls `store::expand_template`.
- Traps reset to default in forked subshells (POSIX) rather than the
  plan's "parent only" framing.
- Static plugins only; WASM is deferred — see
  [`PLUGIN-WASM.md`](PLUGIN-WASM.md).

## Security

Trust model, file permissions, and named ceilings:
[`SECURITY.md`](SECURITY.md).