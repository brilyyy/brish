<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/banner-dark.svg">
  <img alt="briSH — a memory-safe POSIX shell" src="assets/banner.svg">
</picture>

![license](https://img.shields.io/badge/license-MIT-1e66f5?style=flat-square)
![rust](https://img.shields.io/badge/rust-2024%20edition-179299?style=flat-square)
![tests](https://img.shields.io/badge/tests-350%2B-179299?style=flat-square)
![posix](https://img.shields.io/badge/POSIX-cross-checked%20against%20dash-1e66f5?style=flat-square)
![platform](https://img.shields.io/badge/platform-linux%20%2B%20macOS-1e66f5?style=flat-square)

**briSH** (brily SHell) is a memory-safe POSIX shell written in Rust.
It runs your scripts unchanged, then gets out of your way.

## Why briSH

**A bad command can't take your session with it.** Every external
command runs in its own `fork`/`exec`, so a segfaulting binary dies
alone instead of killing the shell that spawned it. Builtins run
in-process with proper fd save/restore, and `SIGPIPE` is restored to
POSIX behavior, so `yes | head -2` behaves instead of warning:

```console
$ brish -c 'echo one; /bin/sh -c "kill -SEGV \$\$"; echo two; exit 7'
one
two
$ echo $?
7
```

**The shell itself has no memory-safety bugs to have.** `unwrap` and
`expect` are denied across the whole workspace by clippy, and `unsafe`
is confined to a single crate (`brish-platform`) where every block
carries a SAFETY comment. Fuzzing targets and property tests feed
arbitrary input to the lexer, parser and expander — they don't panic.

**Polish that works before you configure anything.** Syntax highlighting
and muted completion descriptions are on by default. `cd` uses
[zoxide](https://github.com/ajeetdsouza/zoxide) when it's installed,
`ls` uses [eza](https://github.com/eza-community/eza), and four prompt
themes ship in the box. Parse errors carry a source span, so an
interactive typo shows the offending line, a caret, and `(line L, col C)`
instead of a bare message.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/brilyyy/brish/main/install.sh | sh
```

Prebuilt for linux x86_64/aarch64 and macOS aarch64/x86_64, sha256
verified, installs to `/usr/local/bin`. Set `PREFIX` to override:

```sh
curl -fsSL https://raw.githubusercontent.com/brilyyy/brish/main/install.sh | PREFIX=$HOME/.local/bin sh
```

## Sixty seconds

```console
$ brish                        # interactive: editing, history, completion
$ brish -c 'echo hi'           # run a command string
$ brish script.sh arg1 arg2    # run a script ($0 = path, $1 = arg1)
$ brish --theme briiish-plain -c 'echo hi'   # pick a prompt theme for this run
$ echo 'echo from stdin' | brish
```

Nothing to migrate. briSH is POSIX, so your existing scripts, `.sh`
files, Makefiles and CI steps are the scripts it already runs.

## Run your existing scripts

`crates/brish/tests/posix.rs` holds ~150 POSIX cases, and each one is
cross-checked against `dash` — the POSIX reference shell — whenever
it's on `PATH`. A wrong expectation cannot pass, so the conformance
suite is validated by a real POSIX shell rather than by hand-written
trust.

Coverage includes parameter expansion, arithmetic, quoting,
redirections, heredocs, globbing, subshells, functions, `test`/`[`,
pipelines, `getopts`, `trap EXIT`, and `printf` format cycling.
Measured line coverage and interactive-latency numbers live in
[`docs/POSIX.md`](docs/POSIX.md).

## Extend it without compiling

A plugin is a directory with a `plugin.toml` manifest plus any
executable you like. Themes, prompt segments, completion providers,
keymaps, edit modes and hooks are all plugins — the shell has no
built-in special cases to work around.

```sh
brish -c 'plugin add starter'    # index name, git URL, or a local dir
brish -c 'plugin list'
```

No compiler, no crate to publish, no ABI to track. See
[`docs/PLUGINS.md`](docs/PLUGINS.md).

## Documentation

| Document | What's in it |
|---|---|
| [`docs/POSIX.md`](docs/POSIX.md) | Conformance corpus, documented deviations, coverage + latency numbers |
| [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) | Every `config.toml` key, env var, and file |
| [`docs/PLUGINS.md`](docs/PLUGINS.md) | Plugin authoring and the plugin store |
| [`docs/SECURITY.md`](docs/SECURITY.md) | Trust boundaries and hardening ceilings |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Crate map and the dependency rule |
| [`docs/PLUGIN-WASM.md`](docs/PLUGIN-WASM.md) | Why WASM plugins are deferred |
| [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) | Dev gates every change must pass |
| [`qa/report.md`](qa/report.md) | Differential QA against `dash`/`bash` on real Debian scripts |

## Build from source

```sh
cargo build --workspace          # debug binary at target/debug/brish
cargo test --workspace           # 350+ tests, dash-checked POSIX subset
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Or install the binary directly from the repo:

```sh
cargo install --git https://github.com/brilyyy/brish brish
```

## Not yet implemented

Deliberately absent right now, so you don't discover them mid-migration:
process substitution, `trap ERR`, restricted mode (`-r`), arrays, and
mid-command signal delivery (`sleep 100` isn't cut short). Runtime
plugin toggling needs a restart or `relconf`. Full detail, including
known deviations from `dash`, in
[`docs/POSIX.md`](docs/POSIX.md).

## License

MIT — see [`LICENSE`](LICENSE).