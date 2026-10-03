# Writing briSH plugins

Everything non-core is a plugin: themes, the git prompt segment, cd
announcements, completion providers, keymaps. This guide is the
authoritative API reference; `docs/PLUGIN-PLAN.md` is the design record.

## Where plugins live

In-tree plugins go in `crates/brish-plugin/src/builtin/` and are listed
in `builtin::catalog()` (one `CatalogEntry { default_enabled, plugin }`
per plugin). The binary installs each catalog entry at startup after
filtering it through `~/.config/brish/config.toml`.

Plugins that need engine types (like the built-in `default-completion`
provider) live in `crates/brish/src/` instead — `brish-plugin` itself
stays zero-dependency and engine-free.

## Config gate

```toml
# ~/.config/brish/config.toml
[theme]
name = "plain"

[plugins]
disabled = ["git-prompt"]     # defaults minus these
# or
enabled = ["default-themes"]  # exact list; replaces defaults
```

- `enabled` (exact list) wins over `disabled`.
- Unknown plugin names: warning, never a crash.
- Bad TOML: warning, defaults used.
- `plugin` builtin lists every catalog entry and its on/off state.

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
use brish_plugin::{ChdirHook, HookAction, Plugin, Registry};
use std::path::Path;

struct AnnounceCd;
impl ChdirHook for AnnounceCd {
    fn on_cd(&self, old: &Path, new: &Path) {
        println!("cd: {} -> {}", old.display(), new.display());
    }
}
impl Plugin for AnnounceCd {
    fn name(&self) -> &str { "announce-cd" }
    fn install(&self, reg: &mut Registry) {
        reg.on_chdir.push(Box::new(AnnounceCd));
    }
}
```

Add it to `catalog()` with `default_enabled: false`, rebuild, then the
user enables it in `config.toml`. That is the whole lifecycle.

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

```rust
impl Theme for Solarized {
    fn name(&self) -> &str { "solarized" }
    fn render(&self, status: i32, cwd: &Path, segments: &[&dyn PromptSegment]) -> String {
        let segs = segments.iter().filter_map(|s| s.render(status, cwd))
            .collect::<Vec<_>>().join(" ");
        // respect NO_COLOR via brish_plugin::color_enabled()
        format!("{} {}", /* ... */, cwd.display())
    }
}
```

Registering the `Theme` makes it selectable: `theme solarized`,
`BRISH_THEME=solarized`, `brish --theme solarized`, or
`[theme] name = "solarized"` in config. Unknown names warn and fall
back to `robbyrussell`.

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
