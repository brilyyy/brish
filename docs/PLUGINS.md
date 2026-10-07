# Writing briSH plugins

Everything non-core is a plugin: themes, the git prompt segment, cd
announcements, completion providers, keymaps, edit modes, highlighting
and autosuggest. This guide is the authoritative reference; the crate
map is in [`ARCHITECTURE.md`](ARCHITECTURE.md).

## The two plugin crates

| Crate | Contains |
|---|---|
| `brish-plugin-api` | the traits + `Registry`, and nothing else. Engine-free leaf — **this is what a third-party Rust plugin compiles against**. |
| `brish-plugin` | every bundled implementation: `config`, `completion`, `highlight`, `hinter`, `edit_mode`, `packs`, `prompt`, `keymap`, `hist`, plus `builtin/` (announce-cd). |

`brish-plugin` sits above `brish-engine` because several plugins need
engine types (builtin names for completion, the lexer for highlighting).
`brish-theme` is a separate catalog so theme authors need not pull the
behavioral plugins. The binary (`brish`) owns `build_registry()` and
the REPL, and filters every catalog through `~/.config/brish/config.toml`.

Third-party plugins install into `~/.config/brish/plugins/<name>/` via
the `plugin` builtin (see [Installing](#installing-store-plugins)
below); each is a directory with a `plugin.toml` manifest.

## Config gate

```toml
# ~/.config/brish/config.toml
[theme]
name = "plain"

[plugins]
disabled = ["brish-git"]     # defaults minus these
# or
enabled = ["brish-themes"]  # exact list; replaces defaults
```

- `enabled` (exact list) wins over `disabled`.
- Unknown plugin names: warning, never a crash.
- Bad TOML: warning, defaults used.
- `plugin` builtin lists every catalog entry and its on/off state.
- Engine plugins (`brish_plugin::engine_plugins()`, installed at
  startup) use the same keys: `syntax-highlight`, `autosuggest`,
  `emacs-mode`, `vi-mode` (off by default), `default-menus`,
  `history-search`, `history`, `validator`. `plugin` lists them with
  their on/off state.

```toml
# ~/.config/brish/config.toml — optional store settings
[store]
index = "https://github.com/brilyyy/brish"   # git repo of index/*.toml
```

## The seams

| Trait | Fires when | Notes |
|---|---|---|
| `PreExecHook` | before a command runs | return `HookAction::Abort(n)` to skip it, `$? = n` |
| `PostExecHook` | after a command runs | sees the exit status |
| `ChdirHook` | after successful `cd` | old + new path |
| `PromptSegment` | every interactive prompt | `None` skips this render; themes choose to render segments |
| `Theme` | prompt render | whole left-prompt text; `PS1`/`PS2` env still wins |
| `CompletionProvider` | Tab | context: command / `$var` / file position |
| `KeymapProvider` | startup only | stringly `(key, event)` pairs, merged into emacs defaults |

A `Plugin` bundles any of the above:

```rust
use brish_plugin_api::{ChdirHook, HookAction, Plugin, Registry};
use std::path::Path;

struct AnnounceCd;
impl ChdirHook for AnnounceCd {
    fn on_cd(&self, old: &Path, new: &Path) {
        println!("cd: {} -> {}", old.display(), new.display());
    }
}
impl Plugin for AnnounceCd {
    fn name(&self) -> &str { "brish-announce-cd" }
    fn install(&self, reg: &mut Registry) {
        reg.on_chdir.push(Box::new(AnnounceCd));
    }
}
```

Add it to `builtin::catalog()` with `default_enabled: false`, rebuild,
then the user enables it in `config.toml`. That is the whole lifecycle.

Prefer the zero-compile route unless you genuinely need in-process
speed or a Rust type — see [Helper protocol](#helper-protocol).

## Hook semantics

- Hooks run **in the parent shell only**: pipelines, subshells,
  background jobs and command substitutions fork with hooks disabled.
  A hook never re-enters itself either (the `exec` builtin is exempt).
- Registration order is dispatch order; first `Abort` wins, later
  pre-hooks skip.
- `CmdCtx` is read-only; mutate the shell only through `HookAction`.

## Completion providers

```rust
impl CompletionProvider for MyProvider {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
        if !ctx.is_command { return vec![]; }   // wrong position: skip
        vec![Completion {
            value: "mycmd".into(),
            description: Some("does the thing".into()),
            keep_typing: false,                 // true = no trailing space
        }]
    }
}
```

The router merges all providers, dedupes by value, caps at 100
suggestions. The built-in command/`$var`/file behaviour ships as the
`default-completion` plugin — disable it in config if you want to
replace it wholesale.

## Keymaps

```rust
impl KeymapProvider for MyKeys {
    fn bindings(&self) -> Vec<(String, String)> {
        vec![("ctrl-g".into(), "menu-next".into())]
    }
}
```

Keys: `tab`, `shift-tab`, `enter`, `escape`, `up`/`down`/`left`/`right`,
`backspace`, `delete`, `home`, `end`, `pageup`, `pagedown`,
`ctrl-x`, `alt-1`, `shift-a`, single characters.
Events: `menu`, `menu-next`, `menu-previous`, `menu-accept`, `enter`,
`submit`, `escape`, `clear-screen`, `exit`, `interrupt`,
`history-next`, `history-previous`, `insert-newline`.

Unknown keys/events warn and skip; a key bound by several providers
warns and the last one wins.

## Themes

A theme owns the whole left-prompt layout; registered
[`PromptSegment`](crate::PromptSegment)s are appended wherever the
theme decides (arrow themes: after the cwd, minimal/plain:
ignored).

### The `Theme` trait

```rust
pub trait Theme: Send + Sync {
    fn name(&self) -> &str;
    /// Full left-prompt text; segments are ordered by registration.
    fn render(&self, status: i32, cwd: &Path, segments: &[&dyn PromptSegment]) -> String;
}
```

Register it from `Plugin::install`:

```rust
impl Plugin for MyPlugin {
    fn name(&self) -> &str { "my-plugin" }
    fn install(&self, reg: &mut Registry) {
        reg.themes.push(Box::new(MyTheme));
        // also reg.prompt_segments.push(...), etc.
    }
}
```

### Built-in themes

| Name | Visual | Source |
|---|---|---|
| **briiish-minimal** (default) | `{basename} ` — directory only, no color | `crates/brish-theme/src/themes.rs` (`BriiishMinimal`) |
| **briiish-plain** | `$ ` — POSIX-style | same file (`BriiishPlain`) |
| **briiish-nerd-font** | `❯ ` + Nerd-Font folder glyph + cyan cwd + segments | same file (`BriiishNerdFont`) |
| **briiish-emoji** | `✅`/`❌` status emoji + cyan cwd + segments | same file (`BriiishEmoji`) |

Helpers you can reuse: `basename(cwd)` (cwd basename with `.`
fallback) and `seg_text(status, cwd, segments)` (non-empty
segment texts, space-joined). Both are private to `themes.rs` —
copy them into your plugin if you need the same layout.

### Template themes (`[theme]` in `plugin.toml`)

Store plugins can declare a prompt declaratively — no Rust
needed:

```toml
[theme]
name = "mytheme"
prompt = "{fg:bright-magenta}{arrow} {fg:cyan}{cwd}{reset} {segments}"
```

Template tokens (expanded by `expand_template()` in
`crates/brish-builtin/src/store.rs`):

| Token | Expansion |
|---|---|
| `{arrow}` | `➜` — green on status 0, red otherwise (ANSI when colors on) |
| `{cwd}` | cwd basename |
| `{segments}` | space-joined rendered `PromptSegment`s |
| `{reset}` | `\x1b[0m` when colors on, empty otherwise |
| `{fg:SPEC}` / `{bg:SPEC}` | ANSI color: `black`/`red`/`green`/`yellow`/`blue`/`magenta`/`cyan`/`white`, optional `bright-` prefix, or `#rrggbb` |

Unknown tokens and unmatched `{` pass through literally.
`NO_COLOR` collapses every color token to empty.

### Selecting a theme

Precedence (highest first): `PS2` env while a continuation is
pending, `PS1` env (literal string), else the active theme looked
up by name in the registry. The name comes from `--theme NAME` >
`BRISH_THEME` env > `[theme] name` in `config.toml` > `theme`
builtin selection > `DEFAULT_THEME` (`"briiish-minimal"`). Unknown names
warn and fall back to `briiish-minimal`.

Registering a `Theme` makes it selectable: `theme mytheme`,
`BRISH_THEME=mytheme`, `brish --theme mytheme`, or
`[theme] name = "mytheme"` in config.

## Installing (store plugins)

```sh
brish -c 'plugin search example'       # query the index (name/description/tags)
brish -c 'plugin info starter'         # index entry + install state + seams
brish -c 'plugin add starter'          # install by index name
brish -c 'plugin add ~/src/my-plugin'  # install from a local directory
brish -c 'plugin add https://github.com/user/repo'  # git URL (plugin.toml at root)
brish -c 'plugin list'                 # catalog + installed, on/off
brish -c 'plugin update starter'       # re-fetch, verify, atomic swap
brish -c 'plugin rm --purge starter'   # disable + delete files (no flag: keep files)
```

- **Manifest**: one `plugin.toml` per plugin — any of `[theme]`,
  `[keymap]`, `[completion]`, `[segment]`, `[hooks]`, `[helper]`
  (all optional, merged into the registry alongside catalog plugins).
  See [The seams](#the-seams) and [Helper protocol](#helper-protocol).
- **Enable state**: `plugin add` adds the name to
  `[plugins] enabled` in `config.toml` (comment-preserving via
  `toml_edit`); `rm` moves it to `disabled`. Installed store plugins
  are enabled by default when no lists exist. The registry is
  immutable — **restart the shell to activate**.
- **Index**: a git repo containing `index/<name>.toml` entries
  (`name`, `description`, `source`, optional pinned `commit`,
  subdirectory `path`, `tags`). URL resolution: `$BRISH_INDEX` >
  `[store] index` > default. Index is cloned once into
  `~/.config/brish/index`, then fetched on demand.
- **Integrity**: when an entry pins `commit`, install/update run
  `git rev-parse` against it and **fail closed on mismatch**; the
  verified commit is recorded in `.brish-store-meta` next to the
  plugin for `info`/`update`. Installs copy files only — no `.git`
  inside the plugin dir; update = re-fetch + atomic swap (rollback on
  failure). `plugin add` on an existing name refuses; use `update`.
- **Trust model**: running a store plugin = running its helper scripts
  and hooks as your user — same as any tool on `PATH`. There is **no
  sandbox**; review third-party sources before `add`, prefer
  commit-pinned index entries. Helper/segment/hook subprocesses are
  deadline-killed (manifest `timeout_ms`, hooks 1000ms) but not
  isolated.
- Failures (missing `git`, bad TOML, unknown name, refused traversal)
  print `brish: ...` and return status 1 (usage errors: 2) — the shell
  never aborts.

## Distributing a plugin

1. Ship a directory with `plugin.toml` (schema above). Working
   examples: `examples/plugins/words/` (shell script, zero compile),
   `examples/plugins/starter/` (declarative only, no executable) and
   `examples/plugins/sentinel/` (helper binary + guard hook).
2. For the index: add `index/<name>.toml`:

   ```toml
   name = "starter"
   description = "Example declarative plugin: theme, keymap, completion wordlists"
   source = "https://github.com/you/repo"   # where the files live
   commit = "<pinned sha>"                  # optional but recommended
   path = "examples/plugins/starter"        # subdir inside source
   tags = ["theme"]
   ```

   Point the index at your fork: `[store] index` /
   `$BRISH_INDEX = "https://github.com/you/index"`.
3. Local dev loop: `plugin add ../my-plugin`, edit, `plugin rm
   --purge`, re-add — or `plugin update` after committing changes
   when installing from a git URL.

## Helper protocol

This is the **zero-compile path**. A plugin is a directory with a
`plugin.toml`, and every seam is answered by an ordinary executable —
a shell script is a perfectly good plugin. Nothing is built, nothing is
linked: `plugin add` copies the directory and restarts the shell.

Start with `examples/plugins/words/` — a ~40-line POSIX shell script
that supplies per-command completion wordlists. Then
`examples/plugins/sentinel/` for a hook + guard, and
`examples/plugins/starter/` for the fully declarative form (no
executable at all).

`[helper] path` is a `PATH`-adjacent executable (relative to the
plugin dir), invoked with `BRISH_PLUGIN_DIR` set to that dir and cwd
inherited. Line-oriented protocol; any seam may be omitted. Default
deadline 250ms (`[helper] timeout_ms`); `[segment] timeout_ms`
defaults to 500ms; hooks 1000ms. Killed on timeout, warn-once, never
wedge the shell.

| argv | stdout |
|---|---|
| `complete --word W --cwd D --before LINE [--command]` | `value<TAB>description<TAB>drop` lines (3rd field drops entry) |
| `segment --name S --cwd D --status N` | first line = segment text (exit 0) |
| `hook pre --cwd D --status N -- argv…` | `abort N` on exit 0 skips the command, `$? = N` |
| `hook post --cwd D --status N -- argv…` | ignored (run for side effects) |
| `hook chdir --cwd D --old O` | ignored |
| `keymap` | `key<TAB>event` lines (event vocab: `menu-next`, `history-next`, `enter`, …) |

## Rules

- **No panics**: the workspace denies `clippy::unwrap_used` /
  `expect_used` outside tests. Plugin output is data; the shell never
  unwraps it.
- **Registry is immutable after startup** — build it once in `main`.
- **Cheap prompt code**: the git segment pre-checks `.git` with an
  ancestor walk and caches per cwd for 1s. Prompt plugins run on every
  keystroke-line, keep them fast.
- **Startup warnings** (unknown config names, bad keymap entries) go
  to stderr, never abort the shell.
