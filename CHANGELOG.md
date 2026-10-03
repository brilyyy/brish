# Changelog

## 0.1.0 — unreleased

First working core: POSIX execution engine (pipelines, redirections,
heredocs, subshells, functions, expansions), 40+ builtins, background
jobs with `wait`/`jobs`/`kill`, batch + reedline interactive REPL,
dash-checked POSIX subset tests, property tests, Windows build gate.

### Renamed
- Project renamed **bsh → briSH (brily SHell)**; binary `brish`,
  workspace crates `brish-*`, env `BRISH_DEBUG`, history
  `~/.brish_history`. Error prefix `brish:`.

### Added
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

## 0.1.0 — plugin store (unreleased)

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

## 0.1.0 — job control (unreleased)

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

## 0.1.0 — bash-isms: (( )), $'…', <<< (unreleased)

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

## 0.1.0 — brace expansion, globstar, ~user, echo -e, pipefail (unreleased)

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
