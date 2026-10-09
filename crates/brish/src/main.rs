use std::io::{BufRead, IsTerminal, Read, Write};

use brish_engine::{Engine, NotFoundStyle, Outcome, Stop};
use brish_plugin::engine_plugins;
use brish_plugin::{completion, config, edit_mode, highlight, hist, keymap, packs, report};
use brish_plugin_api::Plugin;

use brish_plugin::completion::BrishCompleter;
use brish_plugin::prompt::BrishPrompt;
use brish_plugin::report::Style;
use clap::Parser;
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

    /// Login shell: read /etc/profile, then the first of ~/.bash_profile,
    /// ~/.bash_login, ~/.profile (bash order)
    #[arg(short = 'l', long)]
    login: bool,

    /// Restricted shell mode (POSIX `sh -r`)
    #[arg(short = 'r')]
    restricted: bool,

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
    /// Wall-clock duration in milliseconds.
    duration_ms: u128,
}

/// Build the plugin registry from config + store + engine plugins.
/// Shared by startup and `relconf`.
fn build_registry(
    config: &config::Config,
    stored: Vec<brish_builtin::store::Stored>,
    var_names: &Arc<Mutex<Vec<String>>>,
    aliases: &Arc<Mutex<Vec<String>>>,
    hist_ctl: &Arc<hist::HistControl>,
) -> brish_plugin_api::Registry {
    let mut registry = brish_plugin_api::Registry::default();
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
        vars: Arc::clone(var_names),
    };
    if config.plugin_enabled(completion::DEFAULT_COMPLETION, true) {
        registry.install(&default_completion);
    } else {
        registry.record(completion::DEFAULT_COMPLETION, false);
    }
    // Syntax highlighter carries runtime state (aliases + dynamic flag).
    let syntax = highlight::SyntaxHighlightPlugin {
        aliases: Arc::clone(aliases),
        dynamic: config.dynamic_highlight,
    };
    if config.plugin_enabled(syntax.name(), true) {
        registry.install(&syntax);
    } else {
        registry.record(syntax.name(), false);
    }
    // History backend carries HISTCONTROL state.
    let history = edit_mode::HistoryPlugin {
        ctl: Arc::clone(hist_ctl),
    };
    if config.plugin_enabled(history.name(), true) {
        registry.install(&history);
    } else {
        registry.record(history.name(), false);
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
            // A `[wasm]` manifest is an interpreter-backed prompt
            // segment, not a seam StorePlugin knows about. Installed
            // before `p` moves into `into_plugin`; a bad module is a
            // warning, never a startup failure.
            #[cfg(feature = "wasm")]
            if let Some(decl) = p.manifest.wasm.as_ref()
                && let Some(warning) =
                    brish_plugin_wasm::install(&mut registry, &name, &p.dir, decl)
            {
                eprintln!("brish: {warning}");
            }
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
    registry
}

/// `--theme` > `$BRISH_THEME` > `[theme] name` > current default,
/// validated against the registry (unknown → warn + default).
fn resolve_theme(
    cli_theme: &Option<String>,
    config: &config::Config,
    registry: &brish_plugin_api::Registry,
) -> String {
    let mut theme = brish_plugin_api::DEFAULT_THEME.to_string();
    if let Some(t) = cli_theme {
        theme = t.clone();
    } else if let Some(t) = std::env::var("BRISH_THEME").ok().filter(|v| !v.is_empty()) {
        theme = t;
    } else if let Some(t) = config.theme.clone() {
        theme = t;
    }
    if !registry.themes.is_empty() && !registry.themes.iter().any(|t| t.name() == theme) {
        eprintln!("brish: unknown theme: {theme}");
        theme = brish_plugin_api::DEFAULT_THEME.to_string();
    }
    theme
}

fn apply_theme(
    engine: &mut Engine,
    cli: &Cli,
    config: &config::Config,
    registry: &brish_plugin_api::Registry,
) {
    engine.theme = resolve_theme(&cli.theme, config, registry);
}

fn main() {
    brish_platform::reset_sigpipe();
    let cli = Cli::parse();
    let mut engine = Engine::new();
    // `$-` reports `l` on every path, including `-lc` (profiles are only
    // read on the REPL path — see repl).
    if cli.login && !engine.env.flags.contains('l') {
        engine.env.flags.push('l');
    }
    // Restricted mode: set before any code runs (RC, command, etc.)
    if cli.restricted {
        engine.env.opts.restricted = true;
    }

    // Startup plugins: catalog filtered through config.toml (plan P3).
    // The var-name snapshot is shared between the REPL (writer) and the
    // default completion provider (reader).
    let var_names: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // Alias snapshot for dynamic highlighting (refreshed each prompt).
    let aliases: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let hist_ctl = Arc::new(hist::HistControl::default());
    // Store plugins are discovered before config loads so their names
    // count as known (disabling an installed plugin must not warn).
    let (stored, store_warnings) = brish_builtin::store::scan(&brish_builtin::paths::plugins_dir());
    for w in &store_warnings {
        eprintln!("brish: {w}");
    }
    let store_names: Vec<&str> = stored.iter().map(|p| p.manifest.name.as_str()).collect();
    let config = config::load_with_known(&store_names);
    // Fatal errors: source excerpt + caret on a terminal, one line in
    // batch (script/expect output must not move); `[errors] style`
    // overrides either way.
    let err_style = {
        let mode = if std::io::stderr().is_terminal() {
            Style::Fancy
        } else {
            Style::Short
        };
        config
            .error_style
            .as_deref()
            .map_or(mode, |s| Style::from_name(s, mode))
    };
    let registry = build_registry(&config, stored, &var_names, &aliases, &hist_ctl);
    // Theme: --theme > $BRISH_THEME > config > default, validated
    // against the registry (unknown → warn + default).
    apply_theme(&mut engine, &cli, &config, &registry);
    engine.set_hooks(Arc::new(registry));
    engine.set_command_not_found(config.command_not_found.clone());
    // Interactive command-not-found report: fancy on a terminal,
    // short otherwise; `[not_found] style` overrides either way.
    // Batch output never changes (POSIX line, status 127).
    let nf_mode =
        if (cli.interactive || std::io::stdin().is_terminal()) && std::io::stderr().is_terminal() {
            NotFoundStyle::Fancy
        } else {
            NotFoundStyle::Short
        };
    let not_found_style = config
        .not_found_style
        .as_deref()
        .map_or(nf_mode, |s| NotFoundStyle::from_name(s, nf_mode));
    engine.set_not_found_style(not_found_style, config.not_found_suggest);
    // `relconf` re-runs the same pipeline (config reload + registry
    // rebuild + theme re-apply). Reedline-owned pieces (highlighter,
    // menus, edit mode, completer box) still need a restart.
    {
        let var_names = Arc::clone(&var_names);
        let aliases = Arc::clone(&aliases);
        let hist_ctl = Arc::clone(&hist_ctl);
        let cli_theme = cli.theme.clone();
        engine.set_relconf(Arc::new(move || {
            let (stored, warns) = brish_builtin::store::scan(&brish_builtin::paths::plugins_dir());
            for w in &warns {
                eprintln!("brish: {w}");
            }
            let names: Vec<String> = stored.iter().map(|p| p.manifest.name.clone()).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let cfg = config::load_with_known(&refs);
            let reg = build_registry(&cfg, stored, &var_names, &aliases, &hist_ctl);
            let theme = resolve_theme(&cli_theme, &cfg, &reg);
            brish_engine::Reload {
                registry: reg,
                theme,
                command_not_found: cfg.command_not_found.clone(),
                not_found_style: cfg
                    .not_found_style
                    .as_deref()
                    .map_or(nf_mode, |s| NotFoundStyle::from_name(s, nf_mode)),
                not_found_suggest: cfg.not_found_suggest,
            }
        }));
    }

    let code = if let Some(cmd) = &cli.command {
        // bash -c: first trailing arg becomes $0, the rest positionals.
        let mut rest = cli.script_args.iter().cloned();
        engine.env.name = rest.next().unwrap_or_else(|| "brish".to_string());
        engine.env.set_positional(rest.collect());
        run_src(&mut engine, cmd, err_style, config.cmd_duration_mode).code
    } else if let Some(script) = cli.script_args.first().cloned() {
        engine.env.name = script.clone();
        engine.env.set_positional(cli.script_args[1..].to_vec());
        match std::fs::read_to_string(&script) {
            Ok(src) => run_src(&mut engine, &src, err_style, config.cmd_duration_mode).code,
            Err(e) => {
                eprintln!("brish: {script}: {e}");
                1
            }
        }
    } else if cli.interactive || std::io::stdin().is_terminal() {
        repl(
            &mut engine,
            &cli,
            var_names,
            aliases,
            hist_ctl,
            &config,
            err_style,
        )
    } else {
        let mut src = String::new();
        match std::io::stdin().read_to_string(&mut src) {
            Ok(_) => run_src(&mut engine, &src, err_style, config.cmd_duration_mode).code,
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
fn run_src(
    engine: &mut Engine,
    src: &str,
    style: Style,
    mode: brish_plugin_api::CmdDurationMode,
) -> Run {
    let start = match mode {
        brish_plugin_api::CmdDurationMode::Wall => std::time::Instant::now(),
        brish_plugin_api::CmdDurationMode::Cpu => {
            // For CPU time, we'd need getrusage; fall back to wall for now
            // TODO: implement CPU time via nix::sys::resource::getrusage
            std::time::Instant::now()
        }
    };
    let prog = match brish_core::parser::parse(src) {
        Ok(p) => p,
        Err(e) => return fatal(engine, e, src, style, start.elapsed().as_millis()),
    };
    let outcome = engine.run(&prog);
    let duration = start.elapsed().as_millis();
    match outcome {
        Ok(Outcome::Exit(c)) => Run {
            code: c,
            exited: true,
            duration_ms: duration,
        },
        Ok(Outcome::Status(s)) => Run {
            code: s,
            exited: false,
            duration_ms: duration,
        },
        Err(e) => fatal(engine, e, src, style, duration),
    }
}

/// Report a fatal error and map it to an exit status (2 for syntax,
/// 1 otherwise — bash convention).
fn fatal(
    engine: &mut Engine,
    e: brish_core::error::Error,
    src: &str,
    style: Style,
    duration_ms: u128,
) -> Run {
    use brish_core::error::Error;
    let code = match e {
        Error::Parse { .. } | Error::Incomplete => 2,
        _ => 1,
    };
    eprintln!("{}", report::render(src, &e, style));
    engine.env.status = code;
    Run {
        code,
        exited: false,
        duration_ms,
    }
}

/// Startup files: explicit `--rcfile` wins; else
/// `~/.config/brish/.brishrc`; else `~/.brishrc` (NOTES.md 5).
/// A missing default is fine; a missing explicit `--rcfile` is an
/// error (kept from before).
fn resolve_rc_from(
    explicit: Option<std::path::PathBuf>,
    cfg_rc: &std::path::Path,
    home_rc: &std::path::Path,
) -> std::path::PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    if cfg_rc.exists() {
        return cfg_rc.to_path_buf();
    }
    home_rc.to_path_buf()
}

fn resolve_rc(cli_rcfile: Option<std::path::PathBuf>) -> std::path::PathBuf {
    let cfg_rc = config::rc_path();
    let home_rc = dirs::home_dir()
        .map(|h| h.join(".brishrc"))
        .unwrap_or_else(|| cfg_rc.clone());
    resolve_rc_from(cli_rcfile, &cfg_rc, &home_rc)
}

/// Login startup files: `/etc/profile`, then the *first* of
/// `~/.brish_profile`, `~/.bash_profile`, `~/.bash_login`, `~/.profile`.
/// Native name first; the bash names are fallbacks so an existing setup
/// carries over. `.bashrc` is not implied (bash requires the profile to
/// source it) — chain the rc from `~/.brish_profile` if you want it.
fn resolve_login_rcs_from(home: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut rcs = vec![std::path::PathBuf::from("/etc/profile")];
    for name in [".brish_profile", ".bash_profile", ".bash_login", ".profile"] {
        let p = home.join(name);
        if p.exists() {
            rcs.push(p);
            break;
        }
    }
    rcs
}

fn resolve_login_rcs() -> Vec<std::path::PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    resolve_login_rcs_from(&home)
}

fn repl(
    engine: &mut Engine,
    cli: &Cli,
    var_names: Arc<Mutex<Vec<String>>>,
    aliases: Arc<Mutex<Vec<String>>>,
    hist_ctl: Arc<hist::HistControl>,
    cfg: &config::Config,
    err_style: Style,
) -> i32 {
    // Set `$-`'s `i` before rc loads: aliases defined in the rc expand
    // for later lines in the same rc and the REPL. `l` is set in main().
    let interactive = cli.interactive || std::io::stdin().is_terminal();
    if interactive && !engine.env.flags.contains('i') {
        engine.env.flags.push('i');
    }
    if !cli.norc {
        // ponytail: rc loads only on the REPL path, so `brish -lc 'cmd'`
        // skips profiles (matches the reported need). Hoist into main()
        // if ssh remote commands ever need them.
        let explicit = cli.rcfile.is_some();
        let rcs: Vec<std::path::PathBuf> = match cli.rcfile.clone() {
            Some(p) => vec![p],
            None if cli.login => resolve_login_rcs(),
            None => vec![resolve_rc(None)],
        };
        for rc in rcs {
            match std::fs::read_to_string(&rc) {
                Ok(src) => {
                    let run = run_src(engine, &src, err_style, cfg.cmd_duration_mode);
                    if run.exited {
                        return run.code;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => {}
                Err(e) => {
                    eprintln!("brish: {}: {e}", rc.display());
                    return 1;
                }
            }
        }
    }

    if interactive && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return edit_repl(engine, var_names, aliases, hist_ctl, cfg, err_style);
    }
    plain_repl(engine, interactive, err_style, cfg.cmd_duration_mode)
}

/// Interactive reedline REPL (plan phase 5): line editing, history,
/// PS1/PS2 continuation, Ctrl-C clears the pending line, Ctrl-D exits.
fn edit_repl(
    engine: &mut Engine,
    var_names: Arc<Mutex<Vec<String>>>,
    aliases: Arc<Mutex<Vec<String>>>,
    hist_ctl: Arc<hist::HistControl>,
    cfg: &config::Config,
    err_style: Style,
) -> i32 {
    let chrome = &cfg.prompt;
    let template = cfg.theme_prompt.as_deref();
    let right = cfg.theme_prompt_right.as_deref();
    let transient = cfg.theme_prompt_transient.as_deref();
    let palette = cfg.theme_palette.as_ref();
    let duration_mode = cfg.cmd_duration_mode;
    let _ = brish_builtin::paths::ensure_config_dir();
    // reedline creates the history file with the process umask; tighten
    // it so commands (which may contain secrets) stay 0600.
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
    // Shift-Tab rolls the open menu back (reedline ships no BackTab
    // default). Two variants: terminals that report SHIFT and those
    // that deliver bare BackTab.
    keybindings.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::BackTab,
        ReedlineEvent::MenuPrevious,
    );
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::BackTab,
        ReedlineEvent::MenuPrevious,
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
    if installed("brish-history-search") {
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
        .with_completer(Box::new(BrishCompleter::new(
            Arc::clone(&registry),
            completion::MatchOpts::from_config(cfg),
        )))
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
    // Transient prompt (`[theme] prompt_transient`): repaints a short
    // prompt after each command, so a tall prompt scrolls away with the
    // output instead of eating scrollback (nushell's transient prompt).
    // No template configured → behaviour unchanged.
    if let Some(tmpl) = transient.filter(|t| !t.is_empty()) {
        let transient_prompt = BrishPrompt::new(
            engine.env.status,
            false,
            engine.hooks(),
            &engine.theme,
            chrome,
            Some(tmpl),
            right,
            palette,
            0, // transient prompt doesn't show duration
            duration_mode,
        );
        rl = rl.with_transient_prompt(Box::new(transient_prompt));
    }
    let mut buf = String::new();
    loop {
        {
            let mut names = var_names.lock().unwrap_or_else(|e| e.into_inner());
            names.clear();
            names.extend(engine.env.vars_iter().map(|(k, _)| k.clone()));
            // Alias snapshot for the dynamic highlighter (patina-style
            // "known callable" check).
            let mut al = aliases.lock().unwrap_or_else(|e| e.into_inner());
            al.clear();
            al.extend(engine.env.aliases.keys().cloned());
            // HISTCONTROL snapshot for erasedups filtering on save.
            hist_ctl.set_from_value(engine.env.get("HISTCONTROL"));
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
            chrome,
            template,
            right,
            palette,
            engine.env.status_duration_ms, // Will be set after run_src
            duration_mode,
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
                    let run = run_src(engine, &src, err_style, duration_mode);
                    engine.env.status_duration_ms = run.duration_ms;
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
fn plain_repl(
    engine: &mut Engine,
    interactive: bool,
    err_style: Style,
    duration_mode: brish_plugin_api::CmdDurationMode,
) -> i32 {
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
            let run = run_src(engine, &buf, err_style, duration_mode);
            if run.exited {
                return run.code;
            }
        }
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_rc_prefers_explicit_then_config_then_home() {
        let d = tempfile::tempdir().expect("tmpdir");
        let explicit = d.path().join("explicit.rc");
        let cfg_rc = d.path().join("cfg.rc");
        let home_rc = d.path().join("home.rc");
        // explicit wins regardless of existence (caller errors on missing)
        assert_eq!(
            resolve_rc_from(Some(explicit.clone()), &cfg_rc, &home_rc),
            explicit
        );
        // config-dir exists → config wins
        std::fs::write(&cfg_rc, "").expect("w");
        assert_eq!(resolve_rc_from(None, &cfg_rc, &home_rc), cfg_rc);
        // config missing → home fallback
        std::fs::remove_file(&cfg_rc).expect("rm");
        assert_eq!(resolve_rc_from(None, &cfg_rc, &home_rc), home_rc);
    }

    #[test]
    fn login_rcs_are_etc_profile_plus_first_home_profile() {
        let d = tempfile::tempdir().expect("tmpdir");
        let home = d.path();
        let bash_login = home.join(".bash_login");
        let bash_profile = home.join(".bash_profile");

        // nothing in $HOME → /etc/profile only
        assert_eq!(
            resolve_login_rcs_from(home),
            vec![std::path::PathBuf::from("/etc/profile")]
        );
        // .bash_login only → it wins over .profile
        std::fs::write(&bash_login, "").expect("w");
        assert_eq!(
            resolve_login_rcs_from(home),
            vec![std::path::PathBuf::from("/etc/profile"), bash_login.clone()]
        );
        // .bash_profile beats .bash_login (bash order)
        std::fs::write(&bash_profile, "").expect("w");
        assert_eq!(
            resolve_login_rcs_from(home),
            vec![
                std::path::PathBuf::from("/etc/profile"),
                bash_profile.clone()
            ]
        );
        // native name beats every bash fallback
        let brish_profile = home.join(".brish_profile");
        std::fs::write(&brish_profile, "").expect("w");
        assert_eq!(
            resolve_login_rcs_from(home),
            vec![std::path::PathBuf::from("/etc/profile"), brish_profile]
        );
    }
}
