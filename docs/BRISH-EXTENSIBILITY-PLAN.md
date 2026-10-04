# Brish Full Customizability via Extension/Plugin API

Approved plan (durable copy; inline original at `local://brish-extensibility-plan.md`).

## Context

`brish-plugin::Registry` exposes 7 seams (themes, prompt_segments, completion_providers, keymaps, pre_exec, post_exec, on_chdir) and `crates/brish/src/main.rs::edit_repl` hardcodes 5 reedline integrations (edit mode Emacs, highlighter BrishHighlighter, hinter BrishHinter, menus ColumnarMenu/ListMenu, FileBackedHistory). Goal: every user-visible reedline capability replaceable through the same `Plugin::install(&mut Registry)` pattern, no second convention, no breakage.

## Approach

Build-tree order: plugin crate → binary wiring → built-in providers → docs. Each step keeps `cargo build --workspace` + `cargo test -p brish-plugin -p brish-builtin` green.

### Step 1 — Extend `brish-plugin::Registry` with new seams (minimal, additive)

Add reedline 0.52.0 to `crates/brish-plugin/Cargo.toml` (intentional exception to engine-free: reuse upstream traits instead of duplicating `StyledText`/`Style`).

Factory traits (all `Send + Sync`), because reedline builder methods consume `Box<dyn Trait>` (no `Clone`):

```rust
pub trait EditModeFactory: Send + Sync {
    fn create(&self, keybindings: reedline::Keybindings) -> Box<dyn reedline::EditMode>;
}
pub trait HighlighterFactory: Send + Sync { fn create(&self) -> Box<dyn reedline::Highlighter>; }
pub trait HinterFactory: Send + Sync     { fn create(&self) -> Box<dyn reedline::Hinter>; }
pub trait MenuFactory: Send + Sync       { fn create(&self) -> Box<dyn reedline::Menu>; }
pub trait ValidatorFactory: Send + Sync  { fn create(&self) -> Box<dyn reedline::Validator>; }
pub trait HistoryFactory: Send + Sync    { fn create(&self) -> Box<dyn reedline::History>; }
```

Registry fields: `edit_mode_factories`, `highlighter_factories`, `hinter_factories`, `menu_factories`, `validator_factories`, `history_factories` (all `Vec<Box<dyn …Factory>>`; `#[derive(Default)]` still works).

First-wins helpers: `edit_mode_factory()`, `highlighter_factory()`, `hinter_factory()`, `menu_factory()`, `validator_factory()`, `history_factory()` returning `Option<&dyn …Factory>`. Empty vec = feature disabled (matches `registry.record(name, false)` semantics). Duplicate registrations: warn in `main.rs`, first wins (same as themes).

### Step 2 — Hardcoded implementations become default catalog plugins

Thin wrapper plugins registered from `crates/brish/src/` (like `completion::DefaultCompletion`), each pushing one factory:

| Plugin | Name | Installs |
|---|---|---|
| `SyntaxHighlightPlugin` (highlight.rs) | `syntax-highlight` | `HighlighterFactory → BrishHighlighter` |
| `AutosuggestPlugin` (hinter.rs) | `autosuggest` | `HinterFactory → BrishHinter::default()` |
| `EmacsModePlugin` (edit_mode.rs) | `emacs-mode` | `EditModeFactory → Emacs::new(kb)` |
| `ViModePlugin` (edit_mode.rs) | `vi-mode` | `EditModeFactory → Vi::new(kb,kb,kb)` (opt-in via config) |
| `DefaultMenusPlugin` (edit_mode.rs) | `default-menus` | `MenuFactory → ColumnarMenu(MENU_NAME)` |
| `HistorySearchPlugin` (edit_mode.rs) | `history-search` | `MenuFactory → ListMenu(HISTORY_MENU)` |
| `HistoryPlugin` (edit_mode.rs) | `history` | `HistoryFactory → FileBackedHistory(1000, history_path())` |
| `ValidatorPlugin` (edit_mode.rs) | `validator` | `ValidatorFactory → DefaultValidator` |

Registration replaces `registry.record("syntax-highlight", …)` with real `registry.install(plugin)` gated by `config.plugin_enabled(name, true)`.

### Step 3 — `edit_repl` becomes registry-first

`crates/brish/src/main.rs` (was lines 290-308): edit mode from `registry.edit_mode_factory().map(create)` else `Emacs::new(kb)`; menus from `menu_factories` (name `HISTORY_MENU` → `ReedlineMenu::HistoryMenu`, else `EngineCompleter`; empty → ColumnarMenu default); history from `history_factory` else `FileBackedHistory::with_file(1000, …)`; highlighter/hinter likewise from factories; `history-search` plugin gates the Ctrl-R `Menu(HISTORY_MENU)` binding as before. Factory `create()` runs once per `edit_repl` invocation — fresh boxes, no shared mutable state (plugins needing shared state use `Arc<Mutex<_>>` like `git.rs` cache). File history `Err` → warn, run without history (existing behavior).

### Step 4 — Declarative knobs (deferred by design)

`Manifest` fields for declarative highlighter/hinter/edit-mode skipped in this cut — programmatic plugins already satisfy "fully customizable". Extension point: `[highlighter]`/`[hinter]`/`edit_mode = "vi"` tables in `plugin.toml` → `StorePlugin::install` pushes factories reading the decl, same as `ManifestKeymap`.

### Step 5 — Document and test

- `docs/PLUGINS.md`: new `## Reedline seams` table after Themes — each factory trait, signature, example `Plugin::install`.
- `docs/PLUGIN-PLAN.md`: state + deviations updated.
- Tests in `crates/brish-plugin/src/lib.rs`: two factories installed → `len()==2`, first wins (pure pick-helper). Existing 28 (brish-plugin) + 102 (brish-builtin) + highlight/hinter tests stay green.
- Parser (`brish_core::reader::is_complete`) and platform `Engine` (`crates/brish-builtin/src/exec.rs`) stay out of scope — separate RFC.

## Critical files

- `crates/brish-plugin/src/lib.rs` — Registry + factory traits (done: 6 traits, 6 Vec fields, 6 accessors)
- `crates/brish-plugin/Cargo.toml` — reedline 0.52.0 dep (done)
- `crates/brish/src/highlight.rs`, `hinter.rs`, `edit_mode.rs` — wrapper plugins (done; edit_mode.rs rewritten for MenuBuilder import + History fallback via `FileBackedHistory::default()`)
- `crates/brish/src/main.rs` — edit_repl registry-first wiring (wired; compile fixes in flight: double-boxing `Box::new(hist)`, `Menu::name()` check, dead-block cleanup)
- `crates/brish-builtin/src/store.rs` — Manifest/StorePlugin (untouched this cut)

## Verification

1. `cargo test -p brish-plugin -p brish-builtin --lib` — 28 + 102 green.
2. Default behavior unchanged: syntax colors (keywords blue bold, strings green), history-prefix hint light-gray, Ctrl-R history menu.
3. `BRISH_NO_HIGHLIGHT=1` or `[plugins] disabled = ["syntax-highlight"]` → colors + hint gone.
4. Custom highlighter plugin test: factory painting every word red → `pick_highlighter(&registry).create()` asserts `Color::Red`.
5. `ViModePlugin` installed → `Esc` then `k` navigates history; without it `Esc` does nothing.
6. Two highlighters → stderr warns duplicate, first wins.

## Assumptions

- Reedline 0.52.0 traits object-safe (`&self`/`&mut self` methods, no generics) — confirmed via source at `~/.cargo/registry/src/index.crates.io-…/reedline-0.52.0/src/`.
- `brish-plugin` now depends on reedline (engine-free relaxed) — fallback if rejected: brish-owned style types + adapter in `crates/brish/src/highlight.rs`.
- Vi::new signature `(insert, normal, visual)` keybinding sets — builtin passes the merged emacs set for all three in this cut (vi-specific bindings later).
