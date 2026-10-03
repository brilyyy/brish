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
brish --theme plain -c 'hi'  # pick a prompt theme for this run
echo 'echo from stdin' | brish
```

Interactive default theme is **robbyrussell**: `➜` arrow (green on
success, red on failure) + cyan cwd basename + git segment. `theme`
lists/switches themes, `plugin` lists installed plugins. `PS1`/`PS2`
set in the environment override the theme as literal strings;
`NO_COLOR` disables ANSI. Tab runs the completion provider chain
(commands, `$VARS`, files — plugins can add more).

Files (all optional, `~/.config/brish/`):

| Path | Purpose |
|---|---|
| `config.toml` | `[theme] name`, `[plugins] enabled/disabled`, `[store] index` |
| `.brishrc` | default rcfile (override with `--rcfile`) |
| `.brish_history` | interactive history, 1000 entries |
| `plugins/` | store plugins, one dir per plugin (`plugin.toml`) |
| `index/` | cached store index (git clone) |

Environment knobs:

| Variable | Effect |
|---|---|
| `PS1`, `PS2` | prompt / continuation prompt override (interactive) |
| `BRISH_THEME` | theme name for this run (overrides config) |
| `BRISH_DEBUG` | space-separated tags `lexer parser expand exec jobs` → stderr traces |
| `NO_COLOR` | disable prompt colors |

## Plugin store

```sh
brish -c 'plugin search example'    # query the index
brish -c 'plugin add starter'       # index name, git URL, or local dir
brish -c 'plugin list'              # installed + catalog, on/off
brish -c 'plugin update starter'    # re-fetch, verify pinned commit, swap
brish -c 'plugin rm --purge starter'  # disable + delete files
```

A plugin is a directory with a `plugin.toml` manifest (theme, keymaps,
completion wordlists, prompt segment, hooks, helper binary). `add`
enables it in `config.toml` — restart the shell to activate. Index
installs are commit-pinned and fail closed on mismatch; helpers run
deadline-killed but **unsandboxed** (same trust as `PATH` tools).
Examples live in `examples/plugins/`, authoring guide in
[`docs/PLUGINS.md`](docs/PLUGINS.md).

## What works

- POSIX core: expansions (`$var`, `${…}` defaults, `$((…))`, `$(…)`,
  backticks), quoting, field splitting, globbing, redirections
  (`>`, `>>`, `<`, `2>`, `>&N`, `>|`, heredocs, here-strings), pipelines,
  `&&`/`||`, `;`, `&`, `if/while/until/for/case`, functions, subshells,
  groups, `$?`, `$!`, `$@`/`$*`, positional parameters. Bash-style
  extras: `((…))` arithmetic command (exit status 0 iff non-zero),
  `$'…'` ANSI-C quoting.
- Builtins: `cd pwd echo printf test [ true false : exit return eval .
  source unset export readonly shift set type command break continue
  alias wait jobs kill fg bg` (+ PATH externals).
- Execution model: builtins run in-process (fd save/restore), pipelines
  /subshells/background/command-substitutions fork — a crashing command
  cannot take the shell with it.
- Job control: background jobs get their own process group;
  `wait`/`jobs`/`kill %n` with POSIX statuses (`128+signal`); in
  interactive tty shells also `fg`/`bg`, `%n` job specs, Ctrl-Z
  suspend, and terminal handoff (`tcsetpgrp`).
- Plugin system (`brish-plugin` crate): pre/post/chdir hooks (parent
  process only), themes, prompt segments, completion providers, keymap
  providers; catalog gated by `config.toml`, never crashes on bad
  config.

## POSIX conformance

`crates/brish/tests/posix.rs` runs a curated subset against expected
output/status **and** cross-checks every case against `dash` when it is
on `PATH` — so the expectations themselves are validated by an actual
POSIX reference shell. Property tests
(`crates/brish-core/tests/proptest.rs`) guarantee lexer/parser/expander
never panic on arbitrary input.

## Extending

Write a plugin: [`docs/PLUGINS.md`](docs/PLUGINS.md) — traits, config
gate, worked examples (hooks, themes, completion, keymaps). The design
record is [`docs/PLUGIN-PLAN.md`](docs/PLUGIN-PLAN.md); the engineering
roadmap is [`docs/bsh-technical-plan.md`](docs/bsh-technical-plan.md)
(original project name; still the source of truth for phases).

## Deliberately not yet implemented

Brace expansion, globstar, `~user`, `echo -e` escapes, runtime plugin
toggling (config is read at startup), history builtin, right prompt,
quote-aware completion.
