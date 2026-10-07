# briSH Architecture

briSH (brily SHell) is a memory-safe POSIX shell. This is the component
map; the original engineering plan lives in
[`bsh-technical-plan.md`](bsh-technical-plan.md) (kept as source of
truth; deviations are called out in each component below).

## Crates

```
crates/brish          binary: CLI, REPL (reedline), config, completions,
                      highlighters, history control
crates/brish-core     lexer, parser, AST, expansions, Env (zero deps)
crates/brish-builtin  execution engine + builtins (depends on core)
crates/brish-words    words: field splitting, globbing, pattern match,
                      arithmetic, test(1)
crates/brish-platform Unix syscalls: fork/exec/wait, signals, job
                      control, terminal (the only crate with `unsafe`)
crates/brish-plugin   extension API: traits + Registry + announce
crates/brish-theme    prompt catalog: themes + prompt segments
fuzz/                 libFuzzer targets (lexer/parser/expand)
```

Borrow rule: `brish-plugin` (API) and `brish-theme` (catalog) never
depend on the engine; the binary wires everything at startup.

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
| Reader/REPL | `brish::*` | reedline; indicators/colors configurable; history `HISTCONTROL` wrapper |
| Lexer | `brish-core::lexer` | hand-written, quote/heredoc state stack; `$'…'`, here-strings |
| Parser | `brish-core::parser` | hand-written recursive descent, `ModePolicy` |
| Expander | `brish-core::expand` | POSIX order; IFS-edge field splitting (see below) |
| Engine | `brish-builtin::exec` | `Engine` holds env + funcs + jobs + registry; non-local flow via `Stop` |
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

## Notable deviations from the plan

- Windows support removed (beta); unix-only (see CHANGELOG).
- Engine lives in `brish-builtin`, not `brish-core` (dependency cycle).
- `brish-plugin` is API-only; the theme catalog moved to `brish-theme`.
- Traps reset to default in forked subshells (POSIX) rather than the
  plan's "parent only" framing.

## Security

Trust model, file permissions, and named ceilings:
[`SECURITY.md`](SECURITY.md).