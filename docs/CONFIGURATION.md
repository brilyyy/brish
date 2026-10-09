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
# Per-segment palette for powerline themes: segment_name = "bg_color" (256-color index)
# palette = { git = "11", venv = "5", aws = "208" }

[engine]
cmd_duration_mode = "wall"   # "wall" (default) or "cpu" — affects {cmd_duration} token

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

[update]
enabled = true               # check for updates on startup (default true)
interval_days = 13           # how often to check (default 13, oh-my-zsh default)
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
| `{path}` | full cwd with `~` substitution |
| `{path:short}` | shortened cwd (last 2 components full, earlier → first char) |
| `{home}` | `$HOME` |
| `{user}` | `$USER` or "user" |
| `{host}` | `$HOSTNAME` or "localhost" |
| `{context}` | `user@host` when SSH (`$SSH_CONNECTION`/`$SSH_TTY` set), else empty |
| `{time}` | local time HH:MM:SS (respects `$TZ`) |
| `{time:short}` | local time HH:MM |
| `{status}` | exit code if ≠0, else empty |
| `{status:all}` | exit code always |
| `{version}` | briSH version |
| `{segments}` | space-joined colored segment output |
| `{segments:plain}` | space-joined plain (SGR-stripped) segment output |
| `{segment:name}` | plain text of segment with `name()` |
| `{segmentc:name}` | colored text of segment with `name()` |
| `{cmd_duration}` | formatted command duration (wall/cpu per `[engine] cmd_duration_mode`) |
| `{reset}` | ANSI reset (empty under `NO_COLOR`); clears separator state |
| `{fg:SPEC}`/`{bg:SPEC}` | ANSI SGR: `black…white`, `bright-` prefix, a 256-color index (`11`), or `#rrggbb`; `{bg:}` updates separator state |
| `{sep}` | powerline separator `` (fg = last background set, bg = default) |
| `{sep:NEXT}` | separator into an explicit next color (`{sep:11}`) |
| `{sepp:NAME}` | separator into a `[theme] palette` entry (`{sepp:git}`) |
| `{bgp:NAME}` / `{fgp:NAME}` | background/foreground from the palette; `{bgp:}` also feeds `{sep}` |

Braces do not nest, so a palette key cannot be interpolated into `{bg:…}` —
`{bgp:NAME}` is the form a palette-driven theme uses.

**Conditionals:**
```
{if:TOKEN}BODY{endif}
{if:TOKEN}BODY{else}ELSE{endif}
```
`TOKEN` is true iff its expansion is non-empty. Nesting supported (max depth 8);
literal text in a skipped branch is dropped, not just tokens.

## Per-segment palette (powerline themes)

`[theme] palette` maps segment names to 256-color background indices, and the
template references them by key:
```toml
[theme]
name = "powerline"
palette = { git = "11", venv = "5", aws = "208" }
```
```
{bgp:git} {segment:git} {sepp:venv} …
```
See `examples/plugins/powerline/` for a complete theme built this way.

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

## Update check

`[update]` controls the oh-my-zsh-style version check on interactive startup.

| Key | Values | Default |
|---|---|---|
| `enabled` | bool — check for newer release on GitHub | `true` |
| `interval_days` | u64 — days between checks | `13` |

When enabled and the check interval has elapsed (tracked by
`~/.config/brish/.update_check` mtime), briSH performs a synchronous
check (max 3 s timeout) against GitHub Releases. If a newer version is
found, it prints a notice and prompts:

```console
briSH 0.3.0 is available (you have 0.2.0).
Would you like to update now? [Y/n]
```

- **Y / Enter** — downloads the prebuilt binary for your platform,
  verifies its sha256 (fail-closed), atomically replaces the current
  executable, and tells you to restart the shell.
- **n / any other key** — skips the update, reminds you to run
  `brish --self-update` later.

The stamp file is touched after a successful network check regardless
of outcome, so the prompt appears at most once per `interval_days`.
Network failures (offline, timeout, etc.) leave the stamp untouched,
so the check retries on the next startup.

To disable entirely:

```toml
[update]
enabled = false
```

Manual update anytime:

```sh
brish --self-update
```