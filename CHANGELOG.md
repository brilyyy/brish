# Changelog

## 0.2.0 — unreleased

Everything shipped so far, folded from the per-batch `## 0.1.0` sections
this file used to grow one commit at a time (plus a `## 1.0.0 — stable`
header that never shipped). Batches are in commit order; the entries
themselves are unchanged.

### Hardening close-out, CI, and login shell

#### Added
- WASM prompt segments, behind `--features wasm` (off by default, so the
  default build and the released binary are unchanged). A store plugin
  with a `[wasm]` section runs a WebAssembly module in the `wasmi`
  interpreter and uses its return value as segment text. The guest gets
  **no** host imports — no filesystem, no network, no spawn, no clock —
  so the boundary is genuinely the interpreter; fuel bounds its work per
  render and `max_output` bounds its output, which is stripped of control
  characters and ESC. Hand-written ABI (`memory`/`alloc`/`render`), no
  wit-bindgen and no component model. +2.9 MiB on a 3.7 MiB binary.
  wasmi over wasmtime deliberately: an interpreter, not a JIT, so there
  is no W^X surface and no Cranelift `unsafe` tree. Example guest and a
  built module in `examples/`, plus `examples/plugins/wasm-misbehaving/`
  — a fuel burner and an oversized-output guest — so the limits are
  demonstrable rather than claimed. Documented in `docs/PLUGIN-WASM.md`,
  which is no longer a "deferred" note.
- Login-shell support: `brish -l` (accepted, sets `l` in `$-`) and a
  startup chain — `/etc/profile`, then the first existing of
  `~/.brish_profile`, `~/.bash_profile`, `~/.bash_login`, `~/.profile`.
  Native name first, bash names as fallback; `.bashrc`/`.brishrc` are
  not implied on the login path (bash-exact), so chain them from
  `~/.brish_profile`. `install.sh` no longer skips the `/etc/shells` +
  `chsh` step silently when stdin is not a tty (`curl … | sh`); it
  prints the two commands instead.

#### Changed
- Crate layering (NOTES.md 1). `brish-plugin` → `brish-plugin-api` (the
  trait + `Registry` layer, engine-free), and `brish-engine` split out
  of `brish-builtin`. A new `brish-plugin` crate now holds every
  bundled plugin — `config`, `completion`, `highlight`, `hinter`,
  `edit_mode`, `packs`, `prompt`, `keymap`, `hist` and the announce
  builtin — so plugins needing engine types no longer live in the
  binary. The split direction is forced: the engine dispatches plugins
  through the Registry, so the traits must sit below it. The store
  (`store`/`store_cmd`/`helper`) stays in `brish-builtin`, since the
  engine calls `store_cmd::dispatch` and the prompt calls
  `store::expand_template`. A third-party Rust plugin compiles against
  `brish-plugin-api` alone. No behaviour change; workspace test count
  unchanged.
- Removed `docs/bsh-technical-plan.md` and fixed the references to it.
  Its load-bearing decisions are recorded where they are enforced:
  deviations in `docs/ARCHITECTURE.md`, WASM deferral in the new
  `docs/PLUGIN-WASM.md`, ceilings in `docs/SECURITY.md`.

#### Fixed
- The unit-test suite was silently truncating.
  `exec_replaces_the_shell_or_redirects_it` ran `exec > f` in-process,
  and because a bare redirection is applied with `apply_bare` it
  permanently `dup2`'d a temp file onto fd 1 — the descriptor libtest
  shares. Every later `... ok` line vanished into a file the tempdir
  then deleted: the binary exited 0 with **no result line at all**, so
  the suite always under-reported and any failure after that point was
  invisible. `brish-builtin` now reports all 134 tests.
- `and_or_equal_precedence_skips_only_the_next_command` asserted on
  bare single letters in a file that parallel tests can write to during
  the dup2 window; a concurrent libtest line containing `a` or `c` made
  it fail roughly one run in three. Now uses distinctive markers.

#### Added
- `examples/plugins/words/` — a zero-compile plugin written as a POSIX
  shell script, supplying per-command completion wordlists. Companion
  docs in `PLUGINS.md` now lead with the zero-compile route.
- `docs/PLUGIN-WASM.md` — why WASM plugins are deferred, the seam
  capability split a WASM backend would face, and a build order if it
  is ever picked up.

Hardening close-out from the beta: stability proofs, missing POSIX
builtins, mid-command traps, quote-aware completion, docs.

#### Fixed
- `exec` is now a builtin (POSIX special builtin). It was missing
  entirely, so `exec 3>&1` — the standard fd-juggling idiom — died with
  "command not found". Bare `exec` applies redirections permanently;
  `exec cmd` replaces the shell with the child's status.
- A `case` pattern may now open with a group: POSIX 2.6.4 makes the
  parens delimiters, not literals, so `(ab)` matches the string `ab`.
  `/usr/bin/zgrep` and `which.debianutils` rely on this.
- `for i do … done` (omitted `in` list, i.e. `for i in "$@"`) parses.
  `/usr/bin/zforce` uses it.
- A function definition may put `{` on the next line after `name()`.
  Five Debian scripts declare functions that way.
- Differential QA harness in `qa/` (docker): parses 55 sh/bash scripts
  shipped by a real Debian and diffs 201 hermetic snippets against dash
  and bash, comparing stdout and exit status separately. It found all
  four bugs above. See `qa/report.md`.

#### Fixed
- `test -t fd` (POSIX): was rejected as an unknown operator, so every
  `[ -t 0 ]` tty guard errored — including this repo's own `install.sh`.
  A closed fd is now false; a non-numeric operand is an error.
- `test -r/-w/-x` now use `access(2)` instead of reading raw mode bits:
  a root-owned `0644` file no longer reports writable, and the answer
  respects uid/gid and read-only mounts.
- `test` with a single argument follows POSIX 2.6.1 (true iff the
  argument is non-empty). `test -z`, `test -t`, `test !` exited 2 where
  dash and bash exit 0.
- `&&` and `||` have equal precedence and associate left-to-right: a
  short-circuit now skips only the *next* command, so
  `false && a || b` still runs `b`. The engine used to abandon the rest
  of the list, silently dropping `b`.
- POSIX corpus 115 -> 137 cases: `-t`, `-r`/`-w`/`-x`, single-argument
  `test`, and mixed `&&`/`||` lists are all cross-checked against `dash`
  now. The gaps were why the four bugs above survived.

#### Fixed
- Trapped signals now interrupt a foreground wait even when the signal
  is delivered to a different thread: `wait_pid`/`wait_untraced` poll
  `WNOHANG` while another thread is alive and check the pending-trap
  word between ticks, because a process-directed signal lands on *any*
  unblocked thread — EINTR was never guaranteed to reach the thread in
  `waitpid`, so the flag could be set while the wait never woke
  (flaky mid-wait trap test, and a real hang in a threaded shell).

#### Fixed
- `brish-platform` umask call compiles on macOS (`mode_t` is `u16`
  there, `u32` on Linux).
- Function-nesting guard lowered 200 → 128 so the chunky engine frames
  stay inside the 2MiB test/REPL stack.

#### Added
- `command not found` aid (interactive only): a `did you mean '…'?`
  line for builtins, aliases and `$PATH` within edit distance 2, plus
  `[hooks] command_not_found` — a command line receiving the missing
  name as `$1`, its stdout being the advice.
- Right prompt (`[theme] prompt_right`) and transient prompt
  (`[theme] prompt_transient`, rendered after each command).
- `relconf -e`: open `config.toml` in `$VISUAL`/`$EDITOR`, then reload.
- `[completion] algorithm` (`prefix`/`substring`/`fuzzy`), `sort`, and
  `match_description`; providers match through `CompletionCtx::matches`.
- `HISTCONTROL=ignorespace`.
- Source-spanning errors: interactive parse/lexer failures print the
  offending line, a caret and `(line L, col C)`; `[errors] style`
  picks `fancy` (default on a tty), `short` (default in batch) or
  `plain`. Batch text is unchanged.
- `getopts` (POSIX: clusters, option-arguments, silent `:` mode, `--`).
- `local` (bash-style dynamic scoping in functions — save/restore
  per call frame on function return).
- `history -d N`; `HISTCONTROL=erasedups`/`ignoreboth` (consecutive
  dedup was always on) via a `History` wrapper snapshotted per prompt.
- Mid-command signal traps: handler sets a flag; EINTR-aware waits
  (spawn/pipeline/job/`wait`) drain + retry — `trap … TERM` now fires
  while a foreground command still runs. Forked subshells reset
  trappable dispositions (POSIX).
- Quote-aware completion: whitespace inside an unclosed quote stays in
  the word; the opening quote is matched around and restored on splice.
- Crash-resistance tests: Linux FD-leak smoke, `RawModeGuard`
  restore-on-drop, mid-wait trap delivery proof.
- Fuzz targets (`fuzz/`: lexer/parser/expand) — nightly smoke runs.
- POSIX corpus 49 → ~115 dash-verified cases. Found + fixed a real
  conformance bug: IFS whitespace at expansion edges is a field
  delimiter against adjacent literals (`[$x]` now splits like dash).
- Measured: coverage 83.65% lines, 13µs/keystroke highlight render,
  800k+ fuzz execs panic-free. Numbers in `docs/POSIX.md`.

#### Added (docs)
- `docs/ARCHITECTURE.md`, `docs/POSIX.md`, `docs/CONFIGURATION.md`,
  `docs/CONTRIBUTING.md`.

#### Stability promise (SemVer)
For 1.0+: `config.toml` schema, plugin trait/registry API, and builtin
semantics are stable (additive changes only). Behavior tweaks that
change script outcomes bump the minor version. Patch releases fix
bugs without behavior change.

### install.sh + shift-tab

#### Added
- `install.sh`: prebuilt GitHub Release install (sha256-verified) into
  `/usr/local/bin` (`PREFIX` override; `VERSION`/arg pins a tag;
  fallback hint to `cargo install --git` when no asset).
- `.github/workflows/release.yml`: on `v*` tags builds
  linux-{x86_64,aarch64} + macos-{aarch64,x86_64} tarballs + sha256,
  attaches to the release.
- README `## Install`: install.sh + `cargo install --path/--git` +
  `cargo binstall --git`. Workspace gains `description`/`repository`.
- crates.io deferred (name check + 7-crate publish chore).

#### Fixed
- Shift-Tab now rolls the completion menu back: bound to `MenuPrevious`
  (reedline ships no `BackTab` default; both SHIFT and bare BackTab
  variants covered).

### themes, prefixes, dynamic highlight

#### Added
- Dynamic syntax highlighting (zsh-patina reference): missing commands
  render **red**, resolvable callables (builtin/alias/`$PATH`) cyan,
  existing files/dirs in argument position **underlined**; precommands
  (`sudo`, `env`, …) keep command position. `[highlight] dynamic`
  (default `true`); skipped above 2000-byte lines; `$PATH` lookups
  cached per session; aliases snapshotted each prompt.

#### Changed
- **Themes**: builtin set is now exactly `briiish-minimal` (default),
  `briiish-plain`, `briiish-nerd-font`, `briiish-emoji`. Replaced
  `briiish`/`robbyrussell`/`minimal`/`plain`.
- **Plugin names** carry a `brish-` prefix: `brish-git`, `brish-venv`,
  `brish-aws`, `brish-docker`, `brish-kubectx`, `brish-themes`,
  `brish-announce-cd`, `brish-completion`, `brish-syntax-highlight`,
  `brish-autosuggest`, `brish-emacs`, `brish-vi`, `brish-menus`,
  `brish-history-search`, `brish-history`, `brish-validator`,
  `brish-pack-*`. Store (user-installed) plugin names unchanged.
  **Breaking**: update `[plugins] enabled/disabled` lists.

### NOTES.md customizability batch

Daily-driver customizability pass (`NOTES.md`).

#### Added
- `[cd] zoxide`: `cd <rel>` falls back to `zoxide query` when the
  operand is not a directory and zoxide is on PATH (link preferred).
- `ls` builtin: `[ls] backend = auto|builtin|eza` — auto/eza exec
  eza (with `--icons` when `[ls] icons`); builtin fallback covers
  `-a`/`-l`/operands with fixed Nerd-Font glyphs.
- `relconf`: reloads `config.toml`, rebuilds the registry, re-applies
  the theme — prompt/hooks/segments update without restart (reedline
  highlighter/menus/edit-mode still need one).
- `[output] table = auto|always|off`: `plugin`/`theme`/`jobs` lists
  render as aligned columns.
- `[prompt]`: `indicator`/`vi_normal`/`vi_visual`/`multiline`
  (empty = toggle off) + `completion_description` (named/`#rrggbb`/
  `off`); menu descriptions default muted (`DarkGray`).
- `[theme] prompt`: config-driven prompt template (`{arrow} {cwd}
  {segments} {fg:…}`, `\n` for multiline) — full oneline/multiline
  theming without Rust; `PS1` still wins.
- rcfile fallback: `--rcfile` > `~/.config/brish/.brishrc` >
  `~/.brishrc`.
- Syntax highlighting was already default-on (NOTES 8 satisfied by
  existing `syntax-highlight` plugin).

#### Changed
- Windows support fully removed (NOTES 11): `cfg(not(unix))` stubs
  dropped, CI target-gate removed, README scrubbed. Unix-only.
- Design docs archived: `docs/archive/PLUGIN-PLAN.md`,
  `docs/archive/BRISH-EXTENSIBILITY-PLAN.md` (NOTES 12);
  `bsh-technical-plan.md`/`PLUGINS.md`/`SECURITY.md` remain current.

### beta prep

Daily-driver close-out: missing POSIX builtins, plugin/theme crate
split, file-permission hardening.

#### Added
- `alias` / `unalias`: interactive-only expansion (POSIX batch safe),
  cycle guard, `command` suppression. Aliases ride `Env` (subshell
  inheritance) and are never exported.
- `trap`: `EXIT|INT|TERM|HUP|QUIT` (`SIG` prefix ok), list/`-p`/
  reset. Flag-only signal handlers (async-signal-safe); engine drains
  at command boundaries and before each prompt; `EXIT` runs once
  from `main` before exit. `KILL`/`STOP` refused. Ceiling: no
  mid-wait interrupt.
- `printf`: POSIX format cycling + defaults, escapes in the format
  (`\n` etc via the `$'…'` decoder), `%d i o u x X s c b f` with
  flags/width/precision. Builtins write fd1 via raw `write(2)` so
  redirects and cmd-subst pipes always receive bytes (libtest-proof).
- `umask`: print/set, octal + symbolic (`go-w`, `u=rwx,g=rx,o=`)
  applied to the running mask.
- `times`: shell + waited-child user/sys via `times(3)`,
  `MmSS.ss` format.
- `docs/SECURITY.md`: trust model, permission policy, ceilings.

#### Changed
- Crate split: `brish-plugin` is now API-only (traits, `Registry`,
  `CatalogEntry`, `announce`); new `brish-theme` crate holds the
  prompt catalog (briiish/robbyrussell/minimal/plain + git/venv/aws/
  kubectx/docker segments). Plugin names and config keys unchanged.
- `~/.config/brish/` created `0700`; history, `config.toml`, and the
  `z` database written `0600` (`paths::write_private`), pre-existing
  wide files tightened. REPL startup tightens the line-editor history
  file.
- `engine_plugins()` table: shared by startup + config validation;
  `vi-mode` default off (`[plugins] enabled = ["vi-mode"]`).

### first working core

First working core: POSIX execution engine (pipelines, redirections,
heredocs, subshells, functions, expansions), 40+ builtins, background
jobs with `wait`/`jobs`/`kill`, batch + reedline interactive REPL,
dash-checked POSIX subset tests, property tests, Windows build gate.

#### Renamed
- Project renamed **bsh → briSH (brily SHell)**; binary `brish`,
  workspace crates `brish-*`, env `BRISH_DEBUG`, history
  `~/.brish_history`. Error prefix `brish:`.

#### Added
- Added `briiish` theme: `❯` arrow (green/red by status) + cyan cwd
  basename + segments. New default interactive theme.
- robbyrussell theme kept in catalog; selectable via `theme robbyrussell`,
  `BRISH_THEME=robbyrussell`, `--theme robbyrussell`, or
  `[theme] name = "robbyrussell"`.
- robbyrussell-style default prompt: green/red `➜` + cyan cwd basename;
  `PS1`/`PS2` literal overrides, `NO_COLOR` support.
- Tab completion: builtins + `$PATH` commands, `$VARS`, file paths
  (columnar menu).
- `docs/PLUGIN-PLAN.md` — static plugin/extension design (plan 6.6).
- Plugin system: new `brish-plugin` crate — hook/segment/theme/
  completion/keymap traits + `Registry` with registration-order
  dispatch and catalog records.
- Engine hook wiring: pre/post/chdir hooks fire around command dispatch
  in the parent process only (forked pipelines/background/subshells/
  command substitutions are hook-silent); `Abort(n)` skips a command
  and sets `$?`. New `theme` (list/switch) and `plugin` (list catalog)
  engine builtins.
- Built-in plugin catalog: `default-themes` (robbyrussell/minimal/
  plain), `git-prompt` segment (`.git` ancestor pre-check + 1s cache,
  porcelain branch/ahead/behind/dirty parsing), `announce-cd` example
  hook (off by default).
- Config: `~/.config/brish/config.toml` (`[theme] name`,
  `[plugins] enabled/disabled`) with unknown-name warnings, bad-TOML
  fallback; default rc `~/.config/brish/.brishrc` (loaded when present,
  `--rcfile`/`--norc` unchanged); history moved to
  `~/.config/brish/.brish_history`; `--theme` flag + `BRISH_THEME` +
  config priority with validation against the registry.
- Theme system wired into the REPL: prompt renders the active theme
  from the registry with segments appended; `PS1`/`PS2` still win;
  core `$ ` fallback when no theme resolves.
- Completion is now a provider chain: the completer is a context
  router (command / `$var` / file positions) that merges, dedupes and
  caps suggestions from registry providers; builtin behaviour ships as
  the `default-completion` plugin (on by default).
- Keymap providers: plugins contribute stringly `(key, event)` pairs,
  parsed into reedline bindings at startup; unknown keys/events warn,
  conflicting keys warn and last provider wins (`keymap` module).
- `docs/PLUGINS.md` — plugin authoring guide (seams, config gate,
  worked examples); `docs/PLUGIN-PLAN.md` gains State + Deviations;
  technical plan 6.6 marked implemented; README refreshed (themes,
  config paths, extending).

- Engine plugin registration table `engine_plugins()` in `main.rs`,
  shared by startup and config validation. `vi-mode` is now wired as an
  opt-in engine plugin (off by default): `[plugins] enabled = ["vi-mode"]`
  activates vi editing, `plugin` lists it, no dead-code lint.

#### Fixed
- `[plugins] enabled` no longer warns `brish: unknown plugin: <name>` for
  engine plugin names (`syntax-highlight`, `autosuggest`, `emacs-mode`,
  `vi-mode`, `default-menus`, `history-search`, `history`, `validator`);
  the known-name list is derived from the same registration table.

### plugin store

- Store plugin manifests: `~/.config/brish/plugins/<name>/plugin.toml`
  declares any subset of seams (theme, keymap, completion wordlists,
  segment command, hooks, helper); discovery scans and validates
  (bad TOML/name mismatch warn and skip, never crash). Path family
  centralized in `brish-builtin::paths`.
- Declarative store seams: `[theme]` template themes (`{arrow}` `{cwd}`
  `{segments}` `{fg:…}`/`{bg:…}` named+hex, `NO_COLOR` aware),
  `[keymap]` pairs merged into the emacs map, `[completion]`
  wordlists with parent-command `args.<cmd>` matching —
  `CompletionCtx` gains `line_before`. Store plugins are config-gated
  like the catalog (installed = enabled by default); duplicate theme
  names warn (first registered wins).
- Helper subprocess bridge: `[segment]` command (first-line output,
  500ms deadline, (cwd,status) TTL cache), `[hooks]` pre/post/chdir
  external commands (1s deadline; pre honors `abort N` only on exit 0 —
  a broken guard can never wedge the shell), `[helper]` completion +
  keymap via line protocol (`value[\tdescription[\tdrop]]`), 250ms
  default deadline (`timeout_ms` override), kill-on-timeout,
  warn-once per adapter.
- Plugin store commands (engine `plugin` builtin): `list` (registry +
  disk), `add <name|url|path>` (index lookup via cached git index,
  pinned-commit verify fail-closed, local copy, auto-enable in
  `config.toml` comment-preserving via `toml_edit`), `rm [--purge]`
  (disable + optional file delete, traversal-safe names), `update`
  (re-clone/re-copy + atomic swap with rollback), `search`, `info`.
  Index URL: `$BRISH_INDEX` > `[store] index` > default. Added
  plugins activate on next shell start (registry immutable after
  startup). Segment manifests gain `timeout_ms` (default 500).
- Example store plugins + index manifests: `examples/plugins/starter`
  (theme/keymap/completion, zero deps), `examples/plugins/sentinel`
  (guard `pre_exec`, cached segment, helper `bin/sentinel` for
  completion/keymap protocol), `index/{starter,sentinel}.toml`.
- `docs/PLUGINS.md`: store install/distribute sections, index entry
  format, helper protocol table, trust model (no sandbox). README
  plugin store section; parse tests pin examples/index to the schema.

### job control

- Full job control in interactive (tty) shells: `fg [%n]`, `bg [%n]`,
  `jobs` states (`Running`/`Stopped`/`Done`), `kill %n` group-kills
  background jobs (own process group), stop detection via
  `WUNTRACED` with `128+SIGTSTP` statuses (platform-correct signal
  numbers — macOS and Linux differ). Stopped/finished jobs are
  notified (`[n] + Stopped …` / `[n]+ Done …`) before each prompt,
  each once.
- Terminal handoff: claim the controlling terminal at startup
  (`setpgid` + `tcsetpgrp`, TSTP/TTIN/TTOU ignored in the shell),
  `fg` gives the job group the terminal (`tcsetpgrp`), waits
  untraced, and returns the terminal on exit/stop. Ctrl-Z (raw-mode
  keybinding → `ExecuteHostCommand`) suspends the shell itself;
  background-launch parent calls `setpgid` to close the killpg race.
- New platform primitives in `brish-platform`: `poll_pid`,
  `wait_untraced`, `kill_group`, `claim_terminal`, `suspend_self`,
  `set_group_leader`, `signal_by_name` (POSIX names incl. `STOP`,
  numeric passthrough), `SIGCONT`/`SIGTSTP`/`SIGSTOP` consts from
  `nix::sys::signal`. Batch + unit tests cover bg/fg/stop/resume,
  notification-once, and group-safety; interactive TTY path (real
  Ctrl-Z, `tcsetpgrp` handoff) verified manually.

### bash-isms: (( )), $'…', <<<

- `((expr))` arithmetic command: lexed as its own token (never two
  subshells, matching bash), evaluated against shell variables with
  assignments; status 0 iff the value is non-zero, 1 on arithmetic
  errors (diagnostic on stderr, shell keeps going). Works standalone,
  in pipelines, negated (`! ((0))`), and with trailing redirections.
- `$'…'` ANSI-C quoting: `\\n` `\\t` `\\r` `\\a` `\\b` `\\f` `\\v`,
  `\\'` `\\\"` `\\\\` `\\?`, octal `\\0nnn`, hex `\\xHH`, `\\uHHHH`,
  `\\UHHHHHHHH`, and `\\cX` control escapes — decoded at lex time to a
  literal single-quoted part (no expansions inside, no field
  splitting). Unterminated `$'…'` continues the REPL line.
- `<<<` here-string redirection: expanded word plus trailing newline
  fed to stdin via the same unlinked-tempfile path as here-docs;
  works with IO numbers (`2<<< x`).
- Tests: lexer/parser units (tokens, spans, incomplete inputs) plus
  batch end-to-end cases; not added to the `dash`-checked POSIX
  corpus (all three are bashisms).

### brace expansion, globstar, ~user, echo -e, pipefail

- Brace expansion: `{a,b}` lists (nested, empty variants; quoted and
  escaped braces stay literal), `{1..3}` / `{01..03}` (zero-padded) /
  `{a..c}` ranges with optional step, 4096-item runaway cap. Applies in
  command words, `for` lists, here-strings, redirection targets, and
  assignment values (`v={a,b}` → `a b`). Ceiling: braces spanning
  quoted parts (`{a,"b,c"}`) stay literal — expansion only scans raw
  and escaped characters.
- `set -o globstar`: `**` matches zero or more directories, recursively
  when final; default off, where `**` degrades to a single `*` like
  bash. Globbing a literal segment that passes through a file
  (`*/*.txt` with top-level files) now yields no match instead of a
  `NotADirectory` error (pre-existing bug).
- Tilde: `~user` resolves through the platform user database
  (`nix::unistd::User::from_name`); assignment values expand a leading
  `~`/`~user` (`v=~`, `v=~/bin`) — the value-only form was previously
  left literal.
- `echo -e` / `-E` / `-n` (also `-ne`): bash escapes (`\n \t \r \a \b
  \f \v \e`, quoted backslashes, octal/hex/unicode, `\cX` line
  truncation); unknown escapes keep the backslash; default off like
  bash. Shares the ANSI-C decoder with `$'…'`.
- `set -o pipefail` / `set +o pipefail`: pipeline status is the
  rightmost non-zero exit when on (default off, last stage wins).
- Fixed: assignments preceding a command (`FOO=abc cmd`) now reach the
  child environment as POSIX requires (even for unexported shell vars),
  with value and export flag restored after the command.
- Tests: unit (brace variants, ranges, assignment tilde/braces,
  globstar tree) + batch (braces, globstar on/off, pipefail, echo
  escapes, prefix env, `~user`); the dash-checked POSIX corpus drops
  `echo a{b,c}d` (now a bashism).
