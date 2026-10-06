# briSH configuration

All settings live in `~/.config/brish/config.toml` (no config file =
defaults). Unknown keys/values warn and fall back to defaults; the
shell never refuses to start over config.

```toml
[theme]
name = "briiish-minimal"   # theme selector
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

## Theme names

`briiish-minimal` (default), `briiish-plain`, `briiish-nerd-font`,
`briiish-emoji`. Selection precedence: `--theme` > `$BRISH_THEME` >
`[theme] name` > default. `PS1`/`PS2` env override the theme.

## Prompt template tokens

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

## Runtime reload

`relconf` re-reads `config.toml`, rebuilds the plugin registry and
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
`erasedups`/`ignoreboth` remove older copies of a repeated line.