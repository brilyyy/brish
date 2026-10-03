# briSH

**briSH** (brily SHell) — a memory-safe, crash-resistant POSIX shell
written in Rust. Unix-first (Linux/macOS), Windows compiles and runs
basic `-c` scripts.

Zero-panic policy: `clippy::unwrap_used`/`expect_used` are denied across
the workspace (tests exempt). Only `brish-platform` contains `unsafe`,
each block SAFETY-commented. The shell restores POSIX `SIGPIPE`
behavior and never dies because a command did.

## Build & test

```sh
cargo build --workspace          # debug binary at target/debug/brish
cargo test --workspace           # 200+ tests, includes a dash-checked POSIX subset
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo check --workspace --target x86_64-pc-windows-msvc   # Windows gate
```

## Usage

```sh
brish -c 'echo hi'           # run a command string
brish script.sh arg1 arg2    # run a script ($0 = path, $1 = arg1)
brish                        # interactive (reedline: editing, history, completion)
echo 'echo from stdin' | brish
```

Interactive default prompt is **robbyrussell-style**: `➜` arrow (green
on success, red on failure) + cyan cwd basename. `PS1`/`PS2` set in the
environment override it as literal strings; `NO_COLOR` disables ANSI.
Tab completes command names (builtins + `$PATH`), `$VARS`, and files.

Environment knobs:

| Variable | Effect |
|---|---|
| `PS1`, `PS2` | prompt / continuation prompt override (interactive) |
| `BRISH_DEBUG` | space-separated tags `lexer parser expand exec jobs` → stderr traces |
| `NO_COLOR` | disable prompt colors |
| `~/.brish_history` | interactive history, 1000 entries |

## What works

- POSIX core: expansions (`$var`, `${…}` defaults, `$((…))`, `$(…)`,
  backticks), quoting, field splitting, globbing, redirections
  (`>`, `>>`, `<`, `2>`, `>&N`, `>|`, heredocs), pipelines, `&&`/`||`,
  `;`, `&`, `if/while/until/for/case`, functions, subshells, groups,
  `$?`, `$!`, `$@`/`$*`, positional parameters.
- Builtins: `cd pwd echo printf test [ true false : exit return eval .
  source unset export readonly shift set type command break continue
  alias wait jobs kill` (+ PATH externals).
- Execution model: builtins run in-process (fd save/restore), pipelines
  /subshells/background/command-substitutions fork — a crashing command
  cannot take the shell with it.
- Background jobs get their own process group; `wait`/`jobs`/`kill %n`
  with POSIX statuses (`128+signal`).

## POSIX conformance

`crates/brish/tests/posix.rs` runs a curated subset against expected
output/status **and** cross-checks every case against `dash` when it is
on `PATH` — so the expectations themselves are validated by an actual
POSIX reference shell. Property tests
(`crates/brish-core/tests/proptest.rs`) guarantee lexer/parser/expander
never panic on arbitrary input.

## Extending

The static plugin/extension design (hooks, prompt segments, completion
providers, keymaps) is specified in [`docs/PLUGIN-PLAN.md`](docs/PLUGIN-PLAN.md).
The engineering roadmap is [`docs/bsh-technical-plan.md`](docs/bsh-technical-plan.md)
(original project name; still the source of truth for phases).

## Deliberately not yet implemented

Brace expansion, here-strings, `$'...'`, globstar, `~user`, `echo -e`
escapes, `((…))` arithmetic command, full job control (`fg`/`bg`,
Ctrl-Z/`tcsetpgrp`), plugins (plan only), config files, git prompt
segment (planned as a `PromptSegment` example).
