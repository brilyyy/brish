# briSH configuration

All settings live in `~/.config/brish/config.toml` (no config file =
defaults). Unknown keys/values warn and fall back to defaults; the
shell never refuses to start over config.

```toml
[theme]
name = "briiish-minimal"   # theme selector
# prompt_right = "{segments}"      # optional right-hand prompt
# prompt_transient = "> "         # optional short prompt after a command
# Optional custom prompt template (overrides the named theme render;
# PS1 env still wins). \n makes a multiline prompt:
# prompt = "{arrow} {cwd} {segments}\n{fg:blue}❯{reset} "

[prompt]
indicator = "> "            # emacs prompt indicator ("" = no indicator)
vi_normal = ": "
vi_visual = "+ "
multiline = "::: "
completion_description = "darkgray"   # named / #rrggbb / off

[highlight]
dynamic = true             # missing cmds red, existing paths underlined

[hooks]
# command_not_found = "command-not-found"   # interactive; gets the
# name as $1, its stdout is the advice

[completion]
algorithm = "prefix"      # prefix | substring | fuzzy
sort = true
match_description = false

[errors]
style = "fancy"            # fancy (source excerpt + caret) | short | plain
                           # default: fancy on a tty, short in batch

[not_found]
style = "fancy"            # fancy (colored + `did you mean`) | short | plain
                           # default: fancy on a tty, short in batch; batch
                           # output never changes (POSIX line, status 127)
suggest = true             # `did you mean …` hint (+ edit-distance scan)

[plugins]
# enabled = ["brish-vi"]    # exact list (replaces defaults)
disabled = []              # defaults minus these

[cd]
zoxide = "auto"            # cd <rel> → `zoxide query` when installed

[ls]
backend = "auto"           # auto | builtin | eza
icons = true               # builtin backend glyphs / eza --icons

[output]
table = "auto"             # aligned plugin/theme/jobs lists

[store]
index = "https://github.com/brilyyy/brish"   # plugin index repo
```

## Right and transient prompts

`[theme] prompt_right` renders on the right edge (same tokens as
`prompt`); unset = empty, as before. `[theme] prompt_transient` is the
short prompt reedline repaints *after* a command runs, so a tall
multiline prompt scrolls away with its output instead of pushing the
whole session down; unset = no transient prompt.

## Command-not-found hook

`[hooks] command_not_found` is a command line that receives the
missing name as `$1`. Its stdout is the answer:

```toml
[hooks]
command_not_found = "command-not-found"   # Debian/Ubuntu, Arch (pkgfile)
```

Interactive only (batch keeps plain POSIX `name: command not found`,
status 127). A `did you mean '…'?` line (with the candidate's
origin: `builtin`, `alias` or `$PATH`) is printed first when a
builtin, alias or `$PATH` command is within edit distance 2 —
controlled by `[not_found] suggest`.

## Completion matching

| Key | Values | Default |
|---|---|---|
| `algorithm` | `prefix`, `substring`, `fuzzy` | `prefix` |
| `sort` | bool — sort candidates by value | `true` |
| `match_description` | bool — a candidate also matches via its description | `false` |

`fuzzy` = chars in order (`gt` → `git`); `substring` = anywhere in the
value (`it` → `git`).

## Theme names

`briiish-minimal` (default), `briiish-plain`, `briiish-nerd-font`,
`briiish-emoji`. Selection precedence: `--theme` > `$BRISH_THEME` >
`[theme] name` > default. `PS1`/`PS2` env override the theme.

## Prompt template tokens

`prompt`, `prompt_right` and `prompt_transient` all take these
tokens.

| Token | Meaning |
|---|---|
| `{arrow}` | `❯` green/red by last status |
| `{cwd}` | current directory basename |
| `{segments}` | registered prompt segments (git, venv, …) |
| `{reset}` | ANSI reset (empty under `NO_COLOR`) |
| `{fg:SPEC}`/`{bg:SPEC}` | `black…white`, `bright-` prefix, or `#rrggbb` |

## Plugin names (builtin, `brish-` prefix)

Themes `brish-themes`; segments `brish-git`, `brish-venv`, `brish-aws`,
`brish-kubectx`, `brish-docker`; behavior `brish-announce-cd`,
`brish-completion`; engine `brish-syntax-highlight`,
`brish-autosuggest`, `brish-emacs`, `brish-vi` (off), `brish-menus`,
`brish-history-search`, `brish-history`, `brish-validator`;
completion packs `brish-pack-git`, `brish-pack-docker`, …
Store-installed plugins keep their own names.

## Command-not-found report

`[not_found]` styles the interactive `command not found`
report. Batch, scripts and pipes always keep the plain POSIX
line (`brish: name: command not found`, status 127) so the
`dash` cross-check and anything parsing stderr see nothing new.

| Key | Values | Default |
|---|---|---|
| `style` | `fancy`, `short`, `plain` | `fancy` on a tty, `short` otherwise |
| `suggest` | bool — `did you mean …` hint | `true` |

`fancy` colors the report and hints where the candidate lives:

```console
brish ❯ nvm
brish: nvm: command not found
  ❯ did you mean 'nv' ($PATH)?
```

`short` is the classic two-line text (with the origin label),
`plain` drops the `brish:` prefix. ANSI is emitted only when
stderr is a terminal and `NO_COLOR` is unset/empty. `suggest =
false` also skips the edit-distance scan (a `read_dir` per
`$PATH` entry when no builtin/alias matched).

## Error reports

`[errors] style` picks how a fatal error is printed:

| Style | Output |
|---|---|
| `fancy` | `brish: parse error: …` + the offending source line + a caret + `(line L, col C)` |
| `short` | `brish: parse error: …` (one line) |
| `plain` | `parse error: …` (no `brish:` prefix) |

Default is `fancy` when stderr is a terminal, `short` otherwise, so
scripts and pipes keep the old single-line text. Plain ASCII, no ANSI:
`NO_COLOR` does not apply. Lines over 120 columns are cropped around
the caret; expansion errors carry no position yet and print their
message only.

## Runtime reload

`relconf -e` opens `config.toml` in `$VISUAL`/`$EDITOR` (creating an
empty file if missing) and then reloads it. `relconf` re-reads `config.toml`, rebuilds the plugin registry and
re-applies the theme. Prompt/hooks/segments/completions take effect
immediately; reedline-owned boxes (highlighter, menus, edit mode) need
a restart.

## Files

| Path | Purpose |
|---|---|
| `config.toml` | this file |
| `.brishrc` (or `~/.brishrc`) | startup commands, `--rcfile` overrides |
| `.brish_history` | interactive history (1000 entries; `HISTCONTROL`) |
| `z` | frecency database for the `z`/`cd` jump |
| `plugins/` | store plugins (one dir per plugin) |
| `index/` | cached plugin index clone |

`HISTCONTROL=ignoredups` is always on (consecutive dup suppression);
`erasedups`/`ignoreboth` remove older copies of a repeated line;
`ignorespace` keeps lines that start with a space out of history.