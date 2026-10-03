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
