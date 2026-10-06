use std::io::{BufRead, IsTerminal, Read, Write};

use brish_builtin::exec::{Engine, Outcome, Stop};
use brish_plugin::Plugin;
mod completion;
mod config;
mod edit_mode;
mod highlight;
mod hinter;
mod keymap;
mod packs;
mod prompt;

use clap::Parser;
use completion::BrishCompleter;
use prompt::BrishPrompt;
use reedline::{
    ColumnarMenu, Emacs, FileBackedHistory, KeyCode, KeyModifiers, MenuBuilder, Reedline,
    ReedlineEvent, ReedlineMenu, Signal, default_emacs_keybindings,
};
use std::sync::{Arc, Mutex};

/// briSH (brily SHell) — a memory-safe, crash-resistant POSIX shell.
#[derive(Parser, Debug)]
#[command(
    name = "brish",
    display_name = "briSH",
    about = "briSH (brily SHell) — a memory-safe POSIX shell",
    version
)]
struct Cli {
    /// Run a command string instead of a script (batch mode)
    #[arg(short = 'c')]
    command: Option<String>,

    /// Force interactive mode
    #[arg(short = 'i', conflicts_with = "command")]
    interactive: bool,

    /// Skip loading startup files
    #[arg(long)]
    norc: bool,

    /// Load the given rc file instead of the default
    #[arg(long)]
    rcfile: Option<std::path::PathBuf>,

    /// Prompt theme (overrides $BRISH_THEME and config.toml)
    #[arg(long)]
    theme: Option<String>,

    /// Script to run, followed by its arguments
    #[arg(trailing_var_arg = true)]
    script_args: Vec<String>,
}

/// Result of running one unit of source.
struct Run {
    /// Exit code for this unit (`$?`).
    code: i32,
    /// `exit` was called — terminate the REPL, not just this unit.
    exited: bool,
}

/// Engine plugins wired into the REPL: (name, plugin, default enabled).
/// `vi-mode` is opt-in — `enabled = ["vi-mode"]` (or `disabled`-minus
/// emacs) selects it; `Registry::edit_mode_factory` takes the first.
pub(crate) fn engine_plugins() -> [(&'static str, &'static dyn Plugin, bool); 8] {
    [
        ("syntax-highlight", &highlight::SyntaxHighlightPlugin, true),
        ("autosuggest", &hinter::AutosuggestPlugin, true),
        ("emacs-mode", &edit_mode::EmacsModePlugin, true),
        ("vi-mode", &edit_mode::ViModePlugin, false),
        ("default-menus", &edit_mode::DefaultMenusPlugin, true),
        ("history-search", &edit_mode::HistorySearchPlugin, true),
        ("history", &edit_mode::HistoryPlugin, true),
        ("validator", &edit_mode::ValidatorPlugin, true),
    ]
}

fn main() {
    brish_platform::reset_sigpipe();
    let cli = Cli::parse();
    let mut engine = Engine::new();

    // Startup plugins: catalog filtered through config.toml (plan P3).
    // The var-name snapshot is shared between the REPL (writer) and the
    // default completion provider (reader).
    let var_names: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // Store plugins are discovered before config loads so their names
    // count as known (disabling an installed plugin must not warn).
    let (stored, store_warnings) = brish_builtin::store::scan(&brish_builtin::paths::plugins_dir());
    for w in &store_warnings {
        eprintln!("brish: {w}");
    }
    let store_names: Vec<&str> = stored.iter().map(|p| p.manifest.name.as_str()).collect();
    let config = config::load_with_known(&store_names);
    let mut registry = brish_plugin::Registry::default();
    // Prompt catalog first (themes + segments; order = segment order),
    // then behavioral plugins (announce, ...).
    for entry in brish_theme::catalog()
        .into_iter()
        .chain(brish_plugin::builtin::catalog())
    {
        let plugin = entry.plugin;
        let name = plugin.name().to_string();
        if config.plugin_enabled(&name, entry.default_enabled) {
            registry.install(&*plugin);
        } else {
            registry.record(&name, false);
        }
    }
    let default_completion = completion::DefaultCompletion {
        vars: Arc::clone(&var_names),
    };
    if config.plugin_enabled(completion::DEFAULT_COMPLETION, true) {
        registry.install(&default_completion);
    } else {
        registry.record(completion::DEFAULT_COMPLETION, false);
    }
    for (name, plugin, default_enabled) in engine_plugins() {
        if config.plugin_enabled(name, default_enabled) {
            registry.install(plugin);
        } else {
            registry.record(name, false);
        }
    }
    // Warn on duplicate seam registrations (first wins in edit_repl),
    // same as the theme check below.
    if registry.highlighter_factories.len() > 1 {
        eprintln!("brish: duplicate highlighter");
    }
    if registry.hinter_factories.len() > 1 {
        eprintln!("brish: duplicate hinter");
    }
    // Completion packs: native Rust arrays, config-gated like everything
    // else. Disabled packs are recorded (so `plugin list` shows them)
    // but their provider is not installed.
    for pack in packs::catalog() {
        let name = pack.name().to_string();
        if config.plugin_enabled(&name, true) {
            registry.install(&pack);
        } else {
            registry.record(&name, false);
        }
    }
    // Store plugins, config-gated like the catalog (installed =
    // enabled by default).
    for p in stored {
        let name = p.manifest.name.clone();
        if config.plugin_enabled(&name, true) {
            registry.install(&p.into_plugin());
        } else {
            registry.record(&name, false);
        }
    }
    {
        // First registered wins on duplicate theme names (builtins go
        // first); warn so a shadowed store theme is visible.
        let mut seen = std::collections::HashSet::new();
        for t in &registry.themes {
            if !seen.insert(t.name()) {
                eprintln!("brish: duplicate theme name: {}", t.name());
            }
        }
    }
    // Theme: --theme > $BRISH_THEME > config > default, validated
    // against the registry (unknown → warn + default).
    if let Some(t) = cli.theme.clone() {
        engine.theme = t;
    } else if let Some(t) = std::env::var("BRISH_THEME").ok().filter(|v| !v.is_empty()) {
        engine.theme = t;
    } else if let Some(t) = config.theme.clone() {
        engine.theme = t;
    }
    if !registry.themes.is_empty() && !registry.themes.iter().any(|t| t.name() == engine.theme) {
        eprintln!("brish: unknown theme: {}", engine.theme);
        engine.theme = brish_plugin::DEFAULT_THEME.to_string();
    }
    engine.set_hooks(Arc::new(registry));

    let code = if let Some(cmd) = &cli.command {
        // bash -c: first trailing arg becomes $0, the rest positionals.
        let mut rest = cli.script_args.iter().cloned();
        engine.env.name = rest.next().unwrap_or_else(|| "brish".to_string());
        engine.env.set_positional(rest.collect());
        run_src(&mut engine, cmd).code
    } else if let Some(script) = cli.script_args.first().cloned() {
        engine.env.name = script.clone();
        engine.env.set_positional(cli.script_args[1..].to_vec());
        match std::fs::read_to_string(&script) {
            Ok(src) => run_src(&mut engine, &src).code,
            Err(e) => {
                eprintln!("brish: {script}: {e}");
                1
            }
        }
    } else if cli.interactive || std::io::stdin().is_terminal() {
        repl(&mut engine, &cli, var_names)
    } else {
        let mut src = String::new();
        match std::io::stdin().read_to_string(&mut src) {
            Ok(_) => run_src(&mut engine, &src).code,
            Err(e) => {
                eprintln!("brish: {e}");
                1
            }
        }
    };
    engine.run_exit_trap();
    std::process::exit(code);
}

/// Parse + run one unit of source.
fn run_src(engine: &mut Engine, src: &str) -> Run {
    let prog = match brish_core::parser::parse(src) {
        Ok(p) => p,
        Err(e) => return fatal(engine, e),
    };
    match engine.run(&prog) {
        Ok(Outcome::Exit(c)) => Run {
            code: c,
            exited: true,
        },
        Ok(Outcome::Status(s)) => Run {
            code: s,
            exited: false,
        },
        Err(e) => fatal(engine, e),
    }
}

/// Report a fatal error and map it to an exit status (2 for syntax,
/// 1 otherwise — bash convention).
fn fatal(engine: &mut Engine, e: brish_core::error::Error) -> Run {
    use brish_core::error::Error;
    let code = match e {
        Error::Parse(_) | Error::Incomplete => 2,
        _ => 1,
    };
    eprintln!("brish: {e}");
    engine.env.status = code;
    Run {
        code,
        exited: false,
    }
}

/// Startup files: default `~/.config/brish/.brishrc`, `--rcfile`
/// overrides, `--norc` skips. Missing default is fine; a missing
/// explicit `--rcfile` is an error (kept from before).
fn repl(engine: &mut Engine, cli: &Cli, var_names: Arc<Mutex<Vec<String>>>) -> i32 {
    // Set `$-`'s `i` before rc loads: aliases defined in `.brishrc`
    // expand for later lines in the same rc and the REPL.
    let interactive = cli.interactive || std::io::stdin().is_terminal();
    if interactive && !engine.env.flags.contains('i') {
        engine.env.flags.push('i');
    }
    if !cli.norc {
        let rc = cli.rcfile.clone().unwrap_or_else(config::rc_path);
        match std::fs::read_to_string(&rc) {
            Ok(src) => {
                let run = run_src(engine, &src);
                if run.exited {
                    return run.code;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && cli.rcfile.is_none() => {}
            Err(e) => {
                eprintln!("brish: {}: {e}", rc.display());
                return 1;
            }
        }
    }

    if interactive && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return edit_repl(engine, var_names);
    }
    plain_repl(engine, interactive)
}

/// Interactive reedline REPL (plan phase 5): line editing, history,
/// PS1/PS2 continuation, Ctrl-C clears the pending line, Ctrl-D exits.
fn edit_repl(engine: &mut Engine, var_names: Arc<Mutex<Vec<String>>>) -> i32 {
    let _ = brish_builtin::paths::ensure_config_dir();
    // reedline creates the history file with the process umask; tighten
    // it so commands (which may contain secrets) stay 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let h = config::history_path();
        if h.exists() {
            let _ = std::fs::set_permissions(&h, std::fs::Permissions::from_mode(0o600));
        }
    }
    // Job control: own process group + terminal, ignore TSTP/TTIN/TTOU
    // (children still stop normally; Ctrl-Z at the prompt stops us on
    // demand via `suspend_self`).
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        engine.set_job_control(true);
        if let Err(e) = brish_platform::claim_terminal() {
            eprintln!("brish: job control unavailable: {e}");
        }
    }
    let registry = Arc::clone(engine.hooks());
    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(keymap::MENU_NAME.to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    // Ctrl-Z suspends the shell (raw mode swallows the line-discipline
    // signal, so we stop ourselves from the read loop).
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('z'),
        ReedlineEvent::ExecuteHostCommand("brish-suspend".to_string()),
    );
    let installed =
        |name: &str| -> bool { registry.installed().iter().any(|(n, on)| n == name && *on) };
    // Ctrl-R opens the history menu; without the plugin reedline's
    // default inline SearchHistory binding stays.
    if installed("history-search") {
        keybindings.add_binding(
            KeyModifiers::CONTROL,
            KeyCode::Char('r'),
            ReedlineEvent::Menu(keymap::HISTORY_MENU.to_string()),
        );
    }
    // Provider keymaps merge last (they may override core bindings).
    for warning in keymap::merge(&mut keybindings, &registry.keymaps) {
        eprintln!("brish: {warning}");
    }
    // Edit mode: registry factory first, fall back to Emacs.
    let edit_mode: Box<dyn reedline::EditMode> = if let Some(f) = registry.edit_mode_factory() {
        f.create(keybindings.clone())
    } else {
        Box::new(Emacs::new(keybindings))
    };
    let mut rl = Reedline::create()
        .with_completer(Box::new(BrishCompleter::new(Arc::clone(&registry))))
        .with_edit_mode(edit_mode);
    // Menus: each registered menu factory becomes one ReedlineMenu
    // (`EngineCompleter` for completion menus, `HistoryMenu` for the
    // one bound to `HISTORY_MENU`). Empty → hard-coded default menu.
    for factory in &registry.menu_factories {
        let menu = factory.create();
        if menu.name() == crate::keymap::HISTORY_MENU {
            rl = rl.with_menu(ReedlineMenu::HistoryMenu(menu));
        } else {
            rl = rl.with_menu(ReedlineMenu::EngineCompleter(menu));
        }
    }
    if registry.menu_factories.is_empty() {
        rl = rl.with_menu(ReedlineMenu::EngineCompleter(Box::new(
            ColumnarMenu::default().with_name(keymap::MENU_NAME),
        )));
    }
    // History: registry factory first; fallback to FileBackedHistory.
    if let Some(hist) = registry.history_factory().map(|f| f.create()) {
        rl = rl.with_history(hist);
    } else if let Ok(hist) = FileBackedHistory::with_file(1000, config::history_path()) {
        rl = rl.with_history(Box::new(hist));
    }
    if let Some(h) = registry.highlighter_factory().map(|f| f.create()) {
        rl = rl.with_highlighter(h);
    }
    // Hinter: registry factory first.
    if let Some(h) = registry.hinter_factory().map(|f| f.create()) {
        rl = rl.with_hinter(h);
    }
    let mut buf = String::new();
    loop {
        {
            let mut names = var_names.lock().unwrap_or_else(|e| e.into_inner());
            names.clear();
            names.extend(engine.env.vars_iter().map(|(k, _)| k.clone()));
        }
        // bash prints completed/stopped job notices before each prompt.
        // job_notifications also drains pending signal traps (idle
        // shell still runs `trap ... TERM`).
        let notes = match engine.job_notifications() {
            Ok(n) => n,
            Err(Stop::Exit(c)) => return c,
            Err(Stop::Fail(e)) => {
                eprintln!("brish: {e}");
                return 1;
            }
            Err(_) => return engine.env.status,
        };
        for line in notes {
            println!("{line}");
        }
        let prompt = BrishPrompt::new(
            engine.env.status,
            !buf.is_empty(),
            engine.hooks(),
            &engine.theme,
        );
        match rl.read_line(&prompt) {
            Ok(Signal::Success(line)) => {
                buf.push_str(&line);
                buf.push('\n');
                if !brish_core::reader::is_complete(&buf) {
                    continue;
                }
                let src = std::mem::take(&mut buf);
                if !src.trim().is_empty() {
                    let run = run_src(engine, &src);
                    if run.exited {
                        return run.code;
                    }
                }
            }
            Ok(Signal::CtrlC) => buf.clear(),
            Ok(Signal::CtrlD) => return engine.env.status,
            Ok(Signal::HostCommand(cmd)) if cmd == "brish-suspend" => {
                brish_platform::suspend_self();
            }
            // Other host commands: nothing registered, ignore.
            Ok(_) => {}
            Err(e) => {
                eprintln!("brish: {e}");
                return 1;
            }
        }
    }
}

/// Non-tty REPL: plain line reads (pipes, `brish -i < script`).
fn plain_repl(engine: &mut Engine, interactive: bool) -> i32 {
    let ps1 = std::env::var("PS1").unwrap_or_else(|_| brish_core::reader::default_ps1());
    let ps2 = std::env::var("PS2").unwrap_or_else(|_| brish_core::reader::default_ps2());
    let stdin = std::io::stdin();
    let mut buf = String::new();
    loop {
        let prompt = if buf.is_empty() { &ps1 } else { &ps2 };
        if interactive && std::io::stdout().is_terminal() {
            print!("{prompt}");
            let _ = std::io::stdout().flush();
        }
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => {
                if interactive && std::io::stdout().is_terminal() {
                    println!();
                }
                return engine.env.status;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("brish: {e}");
                return 1;
            }
        }
        buf.push_str(&line);
        if !brish_core::reader::is_complete(&buf) {
            continue;
        }
        if !buf.trim().is_empty() {
            let run = run_src(engine, &buf);
            if run.exited {
                return run.code;
            }
        }
        buf.clear();
    }
}
