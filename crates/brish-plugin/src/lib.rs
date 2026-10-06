//! Static plugin system for briSH (`docs/PLUGIN-PLAN.md`, plan 6.6).
//!
//! Traits + registry only, minimal dependencies (reedline for the
//! line-editing seams). Registration lives in the binary; the engine
//! walks the resulting registry read-only (no locks after startup —
//! dispatch is a slice walk over borrowed contexts).
pub mod builtin;

use std::path::Path;

/// Fallback theme name when nothing else resolves.
pub const DEFAULT_THEME: &str = "briiish";

/// True when ANSI colors should be emitted (`NO_COLOR` unset or empty).
pub fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

/// What a pre-exec hook tells the engine to do next.
pub enum HookAction {
    Continue,
    /// Skip the command; the shell sets this exit status.
    Abort(i32),
}

/// Everything a command hook may see. Read-only — hooks mutate the shell
/// only through documented channels (`HookAction`, `theme` builtin).
pub struct CmdCtx<'a> {
    /// Post-expansion words of the command about to run.
    pub argv: &'a [String],
    pub status_before: i32,
    pub cwd: &'a Path,
}

pub trait PreExecHook: Send + Sync {
    fn before(&self, ctx: &CmdCtx<'_>) -> HookAction;
}

pub trait PostExecHook: Send + Sync {
    fn after(&self, ctx: &CmdCtx<'_>, status: i32);
}

pub trait ChdirHook: Send + Sync {
    fn on_cd(&self, old: &Path, new: &Path);
}

pub trait PromptSegment: Send + Sync {
    /// Text appended after the base prompt, `None` to skip this render.
    fn render(&self, status: i32, cwd: &Path) -> Option<String>;
}

pub trait Theme: Send + Sync {
    fn name(&self) -> &str;
    /// Full left-prompt text; segments are ordered by registration.
    fn render(&self, status: i32, cwd: &Path, segments: &[&dyn PromptSegment]) -> String;
}

/// One completion candidate. `keep_typing` suppresses the trailing space
/// (directories stay open).
#[derive(Debug, Clone)]
pub struct Completion {
    pub value: String,
    pub description: Option<String>,
    pub keep_typing: bool,
}

pub struct CompletionCtx<'a> {
    /// Word under the cursor (without a leading `$` when `after_dollar`).
    pub word: &'a str,
    /// Cursor sits at a command position.
    pub is_command: bool,
    /// Word follows a `$` (variable completion).
    pub after_dollar: bool,
    pub cwd: &'a Path,
    /// Raw line text before the current word — lets providers match on
    /// the parent command (e.g. `args.git` wordlists).
    pub line_before: &'a str,
}

pub trait CompletionProvider: Send + Sync {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion>;
}

/// Stringly-typed bindings `(key description, event name)` — parsed by
/// the binary so this crate stays engine-free.
pub trait KeymapProvider: Send + Sync {
    fn bindings(&self) -> Vec<(String, String)>;
}

/// Creates a reedline edit mode from a merged keybinding set. The
/// factory pattern (rather than `Box<dyn EditMode>`) keeps the
/// registry cloneless: `Keybindings` is the only stateful input and
/// is cloned into each mode at `edit_repl` time.
pub trait EditModeFactory: Send + Sync {
    fn create(&self, keybindings: reedline::Keybindings) -> Box<dyn reedline::EditMode>;
}

/// Creates a reedline highlighter. Factories (not `Box<dyn Highlighter>`)
/// avoid the `Clone` requirement — reedline consumes the box once at
/// `Reedline::with_highlighter`.
pub trait HighlighterFactory: Send + Sync {
    fn create(&self) -> Box<dyn reedline::Highlighter>;
}

/// Creates a reedline hinter. Same factory rationale as
/// [`HighlighterFactory`]; hinters hold per-line state internally.
pub trait HinterFactory: Send + Sync {
    fn create(&self) -> Box<dyn reedline::Hinter>;
}

/// Creates a reedline menu. Menus are wrapped in [`ReedlineMenu`] by
/// the caller (EngineCompleter/HistoryMenu/WithCompleter), so the
/// factory returns the bare menu.
pub trait MenuFactory: Send + Sync {
    fn create(&self) -> Box<dyn reedline::Menu>;
}

/// Creates a reedline validator.
pub trait ValidatorFactory: Send + Sync {
    fn create(&self) -> Box<dyn reedline::Validator>;
}

/// Creates a reedline history backend.
pub trait HistoryFactory: Send + Sync {
    fn create(&self) -> Box<dyn reedline::History>;
}

/// A named bundle of objects contributed to one or more seams.
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn install(&self, registry: &mut Registry);
}

/// One catalog entry: installed when enabled (catalog default or the
/// config file's `enabled`/`disabled` lists).
pub struct CatalogEntry {
    pub default_enabled: bool,
    pub plugin: Box<dyn Plugin>,
}

/// Everything a user build can register into. Immutable after startup.
#[derive(Default)]
pub struct Registry {
    pub pre_exec: Vec<Box<dyn PreExecHook>>,
    pub post_exec: Vec<Box<dyn PostExecHook>>,
    pub on_chdir: Vec<Box<dyn ChdirHook>>,
    pub prompt_segments: Vec<Box<dyn PromptSegment>>,
    pub themes: Vec<Box<dyn Theme>>,
    pub completion_providers: Vec<Box<dyn CompletionProvider>>,
    pub keymaps: Vec<Box<dyn KeymapProvider>>,
    pub edit_mode_factories: Vec<Box<dyn EditModeFactory>>,
    pub highlighter_factories: Vec<Box<dyn HighlighterFactory>>,
    pub hinter_factories: Vec<Box<dyn HinterFactory>>,
    pub menu_factories: Vec<Box<dyn MenuFactory>>,
    pub validator_factories: Vec<Box<dyn ValidatorFactory>>,
    pub history_factories: Vec<Box<dyn HistoryFactory>>,
    /// Every catalog entry `(name, installed)` for the `plugin` builtin.
    installed: Vec<(String, bool)>,
}

impl Registry {
    /// Install a plugin: objects land in the seam vectors in call order,
    /// recorded as installed (`plugin` builtin lists it as on).
    pub fn install(&mut self, plugin: &dyn Plugin) {
        let name = plugin.name().to_string();
        plugin.install(self);
        self.installed.push((name, true));
    }

    /// Record a catalog entry. Disabled plugins are recorded but their
    /// `install` is skipped by the caller.
    pub fn record(&mut self, name: &str, installed: bool) {
        self.installed.push((name.to_string(), installed));
    }

    pub fn installed(&self) -> &[(String, bool)] {
        &self.installed
    }

    /// Fire pre hooks in registration order; first `Abort` wins.
    pub fn run_pre(&self, ctx: &CmdCtx<'_>) -> HookAction {
        for h in &self.pre_exec {
            if let HookAction::Abort(s) = h.before(ctx) {
                return HookAction::Abort(s);
            }
        }
        HookAction::Continue
    }

    pub fn run_post(&self, ctx: &CmdCtx<'_>, status: i32) {
        for h in &self.post_exec {
            h.after(ctx, status);
        }
    }
    pub fn run_chdir(&self, old: &Path, new: &Path) {
        for h in &self.on_chdir {
            h.on_cd(old, new);
        }
    }

    /// First-registered edit mode factory wins; `None` = the engine
    /// default (Emacs).
    pub fn edit_mode_factory(&self) -> Option<&dyn EditModeFactory> {
        self.edit_mode_factories.first().map(|b| b.as_ref())
    }

    pub fn highlighter_factory(&self) -> Option<&dyn HighlighterFactory> {
        self.highlighter_factories.first().map(|b| b.as_ref())
    }

    pub fn hinter_factory(&self) -> Option<&dyn HinterFactory> {
        self.hinter_factories.first().map(|b| b.as_ref())
    }
    pub fn menu_factory(&self) -> Option<&dyn MenuFactory> {
        self.menu_factories.first().map(|b| b.as_ref())
    }

    pub fn validator_factory(&self) -> Option<&dyn ValidatorFactory> {
        self.validator_factories.first().map(|b| b.as_ref())
    }

    pub fn history_factory(&self) -> Option<&dyn HistoryFactory> {
        self.history_factories.first().map(|b| b.as_ref())
    }

    pub fn menu_factories_len(&self) -> usize {
        self.menu_factories.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Log(Arc<Mutex<Vec<String>>>);

    impl PreExecHook for Log {
        fn before(&self, _ctx: &CmdCtx<'_>) -> HookAction {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push("pre".into());
            HookAction::Continue
        }
    }
    impl PostExecHook for Log {
        fn after(&self, _ctx: &CmdCtx<'_>, status: i32) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(format!("post:{status}"));
        }
    }
    impl ChdirHook for Log {
        fn on_cd(&self, _old: &Path, _new: &Path) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push("cd".into());
        }
    }

    struct Named(&'static str, Arc<Mutex<Vec<String>>>);
    impl Plugin for Named {
        fn name(&self) -> &str {
            self.0
        }
        fn install(&self, reg: &mut Registry) {
            reg.pre_exec.push(Box::new(Log(Arc::clone(&self.1))));
            reg.post_exec.push(Box::new(Log(Arc::clone(&self.1))));
        }
    }

    #[test]
    fn hooks_fire_in_registration_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut reg = Registry::default();
        reg.install(&Named("a", Arc::clone(&log)));
        reg.install(&Named("b", Arc::clone(&log)));

        let cwd = Path::new(".");
        let ctx = CmdCtx {
            argv: &["ls".to_string()],
            status_before: 0,
            cwd,
        };
        assert!(matches!(reg.run_pre(&ctx), HookAction::Continue));
        reg.run_post(&ctx, 3);
        assert_eq!(
            *log.lock().unwrap_or_else(|e| e.into_inner()),
            vec!["pre", "pre", "post:3", "post:3"]
        );
        assert_eq!(
            reg.installed(),
            vec![("a".to_string(), true), ("b".to_string(), true)]
        );
    }

    struct Abort(i32);
    impl PreExecHook for Abort {
        fn before(&self, _ctx: &CmdCtx<'_>) -> HookAction {
            HookAction::Abort(self.0)
        }
    }
    struct Never;
    impl PreExecHook for Never {
        fn before(&self, _ctx: &CmdCtx<'_>) -> HookAction {
            panic!("must not run after Abort");
        }
    }

    #[test]
    fn first_abort_wins_and_later_hooks_skip() {
        let mut reg = Registry::default();
        reg.pre_exec.push(Box::new(Abort(7)));
        reg.pre_exec.push(Box::new(Never));
        let cwd = Path::new(".");
        let ctx = CmdCtx {
            argv: &["touch".to_string()],
            status_before: 0,
            cwd,
        };
        assert!(matches!(reg.run_pre(&ctx), HookAction::Abort(7)));
    }

    #[test]
    fn chdir_hooks_fire_in_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut reg = Registry::default();
        reg.on_chdir.push(Box::new(Log(Arc::clone(&log))));
        reg.on_chdir.push(Box::new(Log(Arc::clone(&log))));
        reg.run_chdir(Path::new("/a"), Path::new("/b"));
        assert_eq!(log.lock().unwrap_or_else(|e| e.into_inner()).len(), 2);
    }
}
