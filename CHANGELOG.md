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
