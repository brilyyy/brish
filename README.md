<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/banner-dark.svg">
  <img alt="briSH — a memory-safe POSIX shell" src="assets/banner.svg">
</picture>

**briSH** (brily SHell) — a memory-safe, crash-resistant POSIX shell
written in Rust. Unix-only (Linux/macOS).

Zero-panic policy: `clippy::unwrap_used`/`expect_used` are denied across
the workspace (tests exempt). Only `brish-platform` contains `unsafe`,
each block SAFETY-commented. The shell restores POSIX `SIGPIPE`
behavior and never dies because a command did.

## Build & test

```sh
cargo build --workspace          # debug binary at target/debug/brish
cargo test --workspace           # 350+ tests, includes a dash-checked POSIX subset
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## Install

Prebuilt releases (linux x86_64/aarch64, macOS aarch64/x86_64; sha256
verified, installs to `/usr/local/bin` — override with `PREFIX`):

```sh
curl -fsSL https://raw.githubusercontent.com/brilyyy/brish/main/install.sh | sh
VERSION=v0.1.0 ./install.sh           # pin a tag (from a clone)
PREFIX=$HOME/.local/bin ./install.sh   # no sudo
```

From source (works today, no release needed):

```sh
cargo install --path crates/brish                                    # from a clone
cargo install --git https://github.com/brilyyy/brish brish             # straight from git
cargo binstall --git https://github.com/brilyyy/brish brish            # uses release assets when tagged
```

## Usage

```sh
brish -c 'echo hi'           # run a command string
brish script.sh arg1 arg2    # run a script ($0 = path, $1 = arg1)
brish                        # interactive (reedline: editing, history, completion)
brish --theme briiish-plain -c 'hi'  # pick a prompt theme for this run
echo 'echo from stdin' | brish
```

Interactive default theme is **briiish-minimal** (bare cwd). Also
builtin: `briiish-plain` (`$ `), `briiish-nerd-font` (folder glyph),
`briiish-emoji` (✅/❌). Set with `theme NAME` or `[theme] name`. `theme`
lists/switches themes, `plugin` lists installed plugins. `PS1`/`PS2`
set in the environment override the theme as literal strings;
`NO_COLOR` disables ANSI. Tab runs the completion provider chain
(commands, `$VARS`, files — plugins can add more).

Files (all optional, `~/.config/brish/`):

| Path | Purpose |
|---|---|
| `config.toml` | see below |
| `.brishrc` | default rcfile (`--rcfile` overrides; fallback `~/.brishrc`) |
| `.brish_history` | interactive history, 1000 entries |
| `plugins/` | store plugins, one dir per plugin (`plugin.toml`) |
| `index/` | cached store index (git clone) |

`config.toml` sections:

```toml
[theme]
name = "briiish-minimal"             # briiish-{minimal,plain,nerd-font,emoji}
prompt = "{arrow} {cwd} {segments}\n❯ "   # optional template (\n = multiline)
prompt_right = "{segments}"          # optional right-hand prompt
prompt_transient = "> "             # optional short prompt after a command

[prompt]
indicator = "> "          # emacs prompt indicator ("" = off)
vi_normal = ": "
vi_visual = "+ "
multiline = "::: "
completion_description = "darkgray"  # or #rrggbb / off

[plugins]
enabled = ["brish-vi"]     # exact list; or disabled = [...]

[cd]
zoxide = "auto"           # cd falls back to `zoxide query` when present

[ls]
backend = "auto"          # auto | builtin | eza
icons = true

[output]
table = "auto"            # aligned plugin/theme/jobs lists

[highlight]
dynamic = true            # missing commands red, existing paths underlined

[errors]
style = "auto"            # auto (fancy on a tty) | fancy | short | plain

[hooks]
command_not_found = "command-not-found"   # optional, interactive

[completion]
algorithm = "prefix"      # prefix | substring | fuzzy
match_description = false

[store]
index = "https://github.com/you/index"
```

`relconf -e` opens `config.toml` in `$VISUAL`/`$EDITOR` first, then
reloads it. `relconf` reloads `config.toml` and rebuilds the plugin registry
(theme/segments/hooks live immediately; reedline-owned
highlighter/menus/edit-mode need a restart).

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
  backticks, brace expansion `{a,b}` / `{1..3}`), quoting, field
  splitting, globbing (`set -o globstar` for `**`), redirections
  (`>`, `>>`, `<`, `2>`, `>&N`, `>|`, heredocs, here-strings), pipelines,
  `&&`/`||`, `;`, `&`, `if/while/until/for/case`, functions, subshells,
  groups, `$?`, `$!`, `$@`/`$*`, positional parameters. Bash-style
  extras: `((…))` arithmetic command (exit status 0 iff non-zero),
  `$'…'` ANSI-C quoting, `~user` tilde, `echo -e`/`-E`/`-n` escapes,
  `set -o pipefail`.
- Builtins: `cd pwd echo printf test [ true false : exit return eval .
  source unset export readonly shift set type command break continue
  alias unalias trap umask times history ls relconf wait jobs kill
  fg bg exec` (+ PATH externals).
- `cd` uses zoxide for frecent jumps when installed (`[cd] zoxide`);
  `ls` uses eza when installed (`[ls] backend`), builtin fallback
  with icons otherwise. Syntax highlighting and muted completion
  descriptions are on by default (zero config).
- `alias`/`unalias` expand interactively only (POSIX batch safe);
  `trap` covers `EXIT|INT|TERM|HUP|QUIT` with flag-based delivery at
  command/prompt boundaries; `printf` cycles the format over args
  with `%d i o u x X s c b f` + width/precision; `umask` takes octal
  or symbolic (`go-w`).
- Execution model: builtins run in-process (fd save/restore), pipelines
  /subshells/background/command-substitutions fork — a crashing command
  cannot take the shell with it.
- Job control: background jobs get their own process group;
  `wait`/`jobs`/`kill %n` with POSIX statuses (`128+signal`); in
  interactive tty shells also `fg`/`bg`, `%n` job specs, Ctrl-Z
  suspend, and terminal handoff (`tcsetpgrp`).
- `exec` (bare: permanent redirections; with a command: replaces the
shell). Extension crates: `brish-plugin-api` (traits + `Registry`),
  `brish-plugin` (every bundled plugin), `brish-theme` (themes +
  prompt segments). Catalogs gated by
  `config.toml`, never crash on bad config.

- `command not found` (interactive only) prints a `did you mean '…'?`
  line when a builtin, alias or `$PATH` command is within edit distance
  2, then runs `[hooks] command_not_found` with the name as `$1`.
- Right prompt (`[theme] prompt_right`) and a transient prompt
  (`[theme] prompt_transient`) — both optional, off by default.
- `[completion] algorithm` (`prefix`/`substring`/`fuzzy`), `sort`,
  `match_description`.
- `HISTCONTROL=ignorespace` keeps space-prefixed lines out of history.
- Errors: lexer/parser errors carry a source span, so an interactive
  failure shows the offending line, a caret and `(line L, col C)`;
  batch keeps the one-line `brish: parse error: …` text. `[errors]
  style` (`fancy`/`short`/`plain`) overrides.

## POSIX conformance

`crates/brish/tests/posix.rs` runs a curated subset against expected
output/status **and** cross-checks every case against `dash` when it is
on `PATH` — so the expectations themselves are validated by an actual
POSIX reference shell. Property tests
(`crates/brish-core/tests/proptest.rs`) guarantee lexer/parser/expander
never panic on arbitrary input.

## Extending

[`docs/PLUGINS.md`](docs/PLUGINS.md) — traits, config gate, worked
examples (hooks, themes, completion, keymaps), the zero-compile helper
protocol, and the plugin store. No compilation required: drop a
`plugin.toml` plus any executable in `~/.config/brish/plugins/<name>/`
and `plugin add` it. Trust boundaries and file-permission policy:
[`docs/SECURITY.md`](docs/SECURITY.md). More:
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) (crate map),
[`docs/PLUGIN-WASM.md`](docs/PLUGIN-WASM.md) (why WASM is deferred),
[`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) (config reference),
[`docs/POSIX.md`](docs/POSIX.md) (conformance + stability numbers),
[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) (dev gates).

Differential QA against `dash`/`bash` on real Debian scripts:
[`qa/report.md`](qa/report.md) (harness in `qa/`).

Crate split: a Rust plugin compiles against `brish-plugin-api` alone
(traits + `Registry`, no engine dependency); the bundled plugins live
in `brish-plugin`, and prompt themes/segments in `brish-theme`.

## Deliberately not yet implemented

Runtime plugin toggling (config is read at startup; `relconf` bridges
the gap), right prompt, quote-aware completion, process substitution,
`trap ERR`, restricted mode (`-r`), mid-command signal traps
(`sleep 100` is not cut short), `ls`/`zoxide` depth (`-R`, `zi` picker). Prompt segments are computed
synchronously (a slow `git` call delays the prompt), and
`prompt_right`/`prompt_transient` have no clock or `%`-style escapes —
templates only.
