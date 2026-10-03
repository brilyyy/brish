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
