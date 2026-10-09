//! Shell environment: variables, exports, positional parameters, specials.
//!
//! Flat namespace (POSIX has no `local`); functions share the global map.
//! Exported flag decides which vars reach child processes.

use std::collections::HashMap;

use crate::error::Error;

/// A shell variable.
///
/// A variable holds either a scalar `value` or, when it was assigned an
/// array literal, `array`. `array` is additive on purpose: every existing
/// reader keeps using `value`, so the scalar path is unchanged.
///
/// `ponytail:` indices are dense — `a[5]=x` fills 0..4 with empty
/// elements rather than leaving a sparse array, and negative indices are
/// unsupported. Both are documented deviations in docs/POSIX.md; switch
/// to `BTreeMap<usize, String>` when a script needs sparse or negative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    pub value: String,
    /// Array elements, when this variable is an array.
    pub array: Option<Vec<String>>,
    pub exported: bool,
    pub readonly: bool,
}

impl Var {
    fn new(value: String) -> Self {
        Var {
            value,
            array: None,
            exported: false,
            readonly: false,
        }
    }

    /// Scalar view of a variable: element 0 for an array (bash `$a` is
    /// `${a[0]}`), else the plain value.
    pub fn scalar(&self) -> &str {
        match &self.array {
            Some(items) => items.first().map_or("", String::as_str),
            None => &self.value,
        }
    }
}

/// Default IFS when unset.
pub const DEFAULT_IFS: &str = " \t\n";

/// Shell option flags (`set -o` / `set -ex`).
///
/// `ponytail:` noclobber/monitor/ignoreeof/verbose stored now, honored when
/// the redirection/job/REPL layers that read them land (Phase 4/5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opts {
    pub allexport: bool,
    pub noclobber: bool,
    pub errexit: bool,
    pub noglob: bool,
    pub noexec: bool,
    pub nounset: bool,
    pub verbose: bool,
    pub xtrace: bool,
    pub monitor: bool,
    pub ignore_eof: bool,
    pub globstar: bool,
    pub pipefail: bool,
    /// `errtrace`: if set, `ERR` trap fires in subshells/functions/
    /// command substitution too (bash's `set -o errtrace`).
    pub errtrace: bool,
    /// `restricted`: POSIX restricted shell mode (`sh -r`). Once set,
    /// cannot be unset. Disables `cd`, absolute-path commands, output
    /// redirections, `exec` replacement, and setting `SHELL`/`PATH`/`ENV`/`BASH_ENV`.
    pub restricted: bool,
}

impl Opts {
    /// POSIX `-o` option names. Returns `false` for unknown names.
    pub fn set_opt(&mut self, name: &str, on: bool) -> bool {
        let slot = match name {
            "allexport" => &mut self.allexport,
            "noclobber" => &mut self.noclobber,
            "errexit" => &mut self.errexit,
            "noglob" => &mut self.noglob,
            "noexec" => &mut self.noexec,
            "nounset" => &mut self.nounset,
            "verbose" => &mut self.verbose,
            "xtrace" => &mut self.xtrace,
            "monitor" => &mut self.monitor,
            "ignoreeof" => &mut self.ignore_eof,
            "globstar" => &mut self.globstar,
            "pipefail" => &mut self.pipefail,
            "errtrace" => &mut self.errtrace,
            "restricted" => &mut self.restricted,
            _ => return false,
        };
        *slot = on;
        true
    }

    /// Single-letter options (`set -ex` / `set +ex`).
    pub fn set_letter(&mut self, letter: char, on: bool) -> bool {
        let name = match letter {
            'a' => "allexport",
            'C' => "noclobber",
            'e' => "errexit",
            'f' => "noglob",
            'm' => "monitor",
            'n' => "noexec",
            'u' => "nounset",
            'v' => "verbose",
            'x' => "xtrace",
            'E' => "errtrace",
            _ => return false,
        };
        self.set_opt(name, on)
    }

    /// Letters reflecting `set -o ...` state, for `$-`.
    pub fn letters(&self) -> String {
        let pairs = [
            ('a', self.allexport),
            ('C', self.noclobber),
            ('e', self.errexit),
            ('f', self.noglob),
            ('m', self.monitor),
            ('n', self.noexec),
            ('u', self.nounset),
            ('v', self.verbose),
            ('x', self.xtrace),
            ('E', self.errtrace),
        ];
        pairs
            .iter()
            .filter(|(_, on)| *on)
            .map(|(c, _)| *c)
            .collect()
    }
}

/// Shell state needed by expansion: variables, positional params, `$?`.
#[derive(Debug, Clone)]
pub struct Env {
    vars: HashMap<String, Var>,
    /// Interactive aliases. Rides `Env` so forks/subshells inherit for
    /// free; never exported (not a `Var`, absent from `child_env`).
    pub aliases: HashMap<String, String>,
    /// `trap` commands keyed by signal name (`INT`, `TERM`, `HUP`,
    /// `QUIT`, `EXIT`). Same inheritance/visibility rules as aliases.
    pub traps: HashMap<String, String>,
    positional: Vec<String>,
    /// `$0` — script/function name.
    pub name: String,
    /// `$?` — last exit status.
    pub status: i32,
    /// Wall-clock duration of last command in milliseconds.
    pub status_duration_ms: u128,
    /// `$!` — last background pid.
    pub last_bg: Option<u32>,
    /// `$$` — shell pid.
    pub pid: u32,
    /// `$-` — active option flags (e.g. `i` when interactive).
    pub flags: String,
    /// `set -o` option state.
    pub opts: Opts,
    /// Function-local frames (`local`): per call, name → snapshot of
    /// the global (or prior) value taken at first `local` mention.
    /// Popped/restored by the engine on function return.
    pub local_frames: Vec<HashMap<String, Option<Var>>>,
    /// `getopts` cursor: char offset inside the current `-abc` cluster.
    /// Reset when the script changes `OPTIND` (detected via the mirror).
    pub getopts_pos: usize,
    /// Mirror of the last `OPTIND` this shell acted on.
    pub getopts_ind: usize,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    /// Empty environment (tests, subshells before inheritance).
    pub fn new() -> Self {
        Env {
            vars: HashMap::new(),
            aliases: HashMap::new(),
            traps: HashMap::new(),
            positional: Vec::new(),
            name: "brish".to_string(),
            status: 0,
            status_duration_ms: 0,
            last_bg: None,
            pid: std::process::id(),
            flags: "h".to_string(),
            opts: Opts::default(),
            local_frames: Vec::new(),
            getopts_pos: 0,
            getopts_ind: 1,
        }
    }

    /// Seed from the process environment (all vars exported).
    pub fn from_std() -> Self {
        let mut env = Self::new();
        for (k, v) in std::env::vars() {
            if is_name(&k) {
                env.vars.insert(
                    k,
                    Var {
                        value: v,
                        array: None,
                        exported: true,
                        readonly: false,
                    },
                );
            }
        }
        env
    }

    /// Value of `name`, or `None` when unset (set-but-empty is `Some("")`).
    /// For an array this is element 0 (bash: `$a` is `${a[0]}`).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(Var::scalar)
    }

    pub fn get_var(&self, name: &str) -> Option<&Var> {
        self.vars.get(name)
    }

    /// All variables (unsorted).
    pub fn vars_iter(&self) -> impl Iterator<Item = (&String, &Var)> {
        self.vars.iter()
    }

    pub fn is_set(&self, name: &str) -> bool {
        self.vars.contains_key(name)
    }

    /// Set `name` to `value`; errors on readonly vars.
    /// Honors `set -a`: new/updated vars become exported.
    pub fn set(&mut self, name: &str, value: impl Into<String>) -> Result<(), Error> {
        if !is_name(name) {
            return Err(Error::expand(format!("bad variable name: {name}")));
        }
        if self.opts.restricted && matches!(name, "SHELL" | "PATH" | "ENV" | "BASH_ENV") {
            return Err(Error::expand(format!("{name}: restricted")));
        }
        match self.vars.get_mut(name) {
            Some(v) if v.readonly => Err(Error::expand(format!("{name}: readonly variable"))),
            Some(v) => {
                v.value = value.into();
                // A plain assignment replaces an array wholesale (bash:
                // `a=(x y); a=z` leaves `a` scalar with value `z`).
                v.array = None;
                if self.opts.allexport {
                    v.exported = true;
                }
                Ok(())
            }
            None => {
                let mut var = Var::new(value.into());
                if self.opts.allexport {
                    var.exported = true;
                }
                self.vars.insert(name.to_string(), var);
                Ok(())
            }
        }
    }

    // ---- arrays (bash extension; dense, never exported) ----

    /// Bind `name` to `items`, replacing any scalar or previous array.
    pub fn set_array<I, S>(&mut self, name: &str, items: I) -> Result<(), Error>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        if !is_name(name) {
            return Err(Error::expand(format!("bad variable name: {name}")));
        }
        let items: Vec<String> = items.into_iter().map(Into::into).collect();
        match self.vars.get_mut(name) {
            Some(v) if v.readonly => Err(Error::expand(format!("{name}: readonly variable"))),
            Some(v) => {
                v.array = Some(items.clone());
                // Keep `value` in step so scalar readers and `child_env`
                // still see something sensible.
                v.value = items.first().cloned().unwrap_or_default();
                Ok(())
            }
            None => {
                let value = items.first().cloned().unwrap_or_default();
                self.vars.insert(
                    name.to_string(),
                    Var {
                        value,
                        array: Some(items),
                        exported: false,
                        readonly: false,
                    },
                );
                Ok(())
            }
        }
    }

    /// Elements of `name`, or `None` if unset or not an array.
    pub fn array(&self, name: &str) -> Option<&[String]> {
        self.vars.get(name)?.array.as_deref()
    }

    /// True when `name` is bound to an array (even an empty one).
    pub fn is_array(&self, name: &str) -> bool {
        self.vars.get(name).is_some_and(|v| v.array.is_some())
    }

    /// Element `idx`, growing the array with empty strings if needed.
    pub fn set_index(
        &mut self,
        name: &str,
        idx: usize,
        value: impl Into<String>,
    ) -> Result<(), Error> {
        if !is_name(name) {
            return Err(Error::expand(format!("bad variable name: {name}")));
        }
        if self
            .vars
            .get(name)
            .is_some_and(|v| v.readonly && v.array.is_none())
        {
            return Err(Error::expand(format!("{name}: readonly variable")));
        }
        let v = self
            .vars
            .entry(name.to_string())
            .or_insert_with(|| Var::new(String::new()));
        let items = v.array.get_or_insert_with(Vec::new);
        if items.len() <= idx {
            items.resize(idx + 1, String::new());
        }
        items[idx] = value.into();
        // Keep element 0 mirrored into `value` for scalar readers.
        if idx == 0 {
            v.value = items[0].clone();
        }
        Ok(())
    }

    /// Drop element `idx`, closing the gap (bash compacts on `unset`).
    /// Returns whether the array changed.
    pub fn unset_index(&mut self, name: &str, idx: usize) -> bool {
        let Some(v) = self.vars.get_mut(name) else {
            return false;
        };
        let Some(items) = v.array.as_mut() else {
            return false;
        };
        if idx >= items.len() {
            return false;
        }
        items.remove(idx);
        v.value = items.first().cloned().unwrap_or_default();
        true
    }

    /// Set (creating if needed) without readonly check — shell bookkeeping
    /// (`PWD`/`OLDPWD`) and arith assignment where the check happens at the
    /// caller.
    pub fn set_unchecked(&mut self, name: &str, value: impl Into<String>) {
        if let Some(v) = self.vars.get_mut(name) {
            v.value = value.into();
        } else if is_name(name) {
            self.vars.insert(name.to_string(), Var::new(value.into()));
        }
    }

    /// Mark `name` exported (creating it empty when missing, like `export x`).
    pub fn export(&mut self, name: &str) -> Result<(), Error> {
        if !is_name(name) {
            return Err(Error::expand(format!("not a valid identifier: {name}")));
        }
        self.vars
            .entry(name.to_string())
            .or_insert_with(|| Var::new(String::new()));
        if let Some(v) = self.vars.get_mut(name) {
            v.exported = true;
        }
        Ok(())
    }

    pub fn set_readonly(&mut self, name: &str) -> Result<(), Error> {
        if !is_name(name) {
            return Err(Error::expand(format!("not a valid identifier: {name}")));
        }
        self.vars
            .entry(name.to_string())
            .or_insert_with(|| Var::new(String::new()));
        if let Some(v) = self.vars.get_mut(name) {
            v.readonly = true;
        }
        Ok(())
    }

    /// Remove a variable; errors on readonly.
    pub fn unset(&mut self, name: &str) -> Result<(), Error> {
        if self.opts.restricted && matches!(name, "SHELL" | "PATH" | "ENV" | "BASH_ENV") {
            return Err(Error::expand(format!("{name}: restricted")));
        }
        match self.vars.get(name) {
            Some(v) if v.readonly => Err(Error::expand(format!("{name}: readonly variable"))),
            _ => {
                self.vars.remove(name);
                Ok(())
            }
        }
    }

    /// `IFS` value (default whitespace when unset).
    pub fn ifs(&self) -> &str {
        self.get("IFS").unwrap_or(DEFAULT_IFS)
    }

    /// Positional params `$1..` (index 0 = `$1`).
    pub fn positional(&self) -> &[String] {
        &self.positional
    }

    pub fn set_positional(&mut self, params: Vec<String>) {
        self.positional = params;
    }

    /// Enter a function frame for `local` bookkeeping.
    pub fn push_fn_locals(&mut self) {
        self.local_frames.push(HashMap::new());
    }

    /// Leave the innermost function frame, restoring every variable
    /// that frame localized (unset when it did not exist before).
    pub fn pop_fn_locals(&mut self) {
        let Some(frame) = self.local_frames.pop() else {
            return;
        };
        for (name, prev) in frame {
            match prev {
                Some(v) => {
                    self.vars.insert(name, v);
                }
                None => {
                    self.vars.remove(&name);
                }
            }
        }
    }

    /// `local name[=value]` inside the innermost function frame.
    /// Snapshots the current value on first mention, then assigns.
    pub fn set_local(&mut self, name: &str, value: Option<&str>) -> Result<(), Error> {
        if !is_name(name) {
            return Err(Error::expand(format!("local: {name}: invalid name")));
        }
        let snapshot = self.vars.get(name).cloned();
        let frame = match self.local_frames.last_mut() {
            Some(f) => f,
            None => {
                return Err(Error::expand("local: can only be used in a function"));
            }
        };
        frame.entry(name.to_string()).or_insert(snapshot);
        match value {
            Some(v) => {
                self.set_unchecked(name, v);
            }
            None => {
                self.vars
                    .entry(name.to_string())
                    .or_insert_with(|| Var::new(String::new()));
            }
        }
        Ok(())
    }

    /// Exported variables as child-process environment pairs.
    /// Snapshot `(value, exported)` for a temp assignment window.
    pub fn snapshot(&self, name: &str) -> Option<(String, bool)> {
        self.vars
            .get(name)
            .map(|v| (v.scalar().to_string(), v.exported))
    }

    /// Restore a `snapshot`; `None` removes the variable.
    pub fn restore(&mut self, name: &str, prev: Option<(String, bool)>) {
        match prev {
            Some((val, exp)) => {
                if let Some(v) = self.vars.get_mut(name) {
                    v.value = val;
                    v.exported = exp;
                }
            }
            None => {
                self.vars.remove(name);
            }
        }
    }

    /// Restore a whole `Var` (array included); `None` removes it. Used to
    /// roll back a temporary array assignment after its command.
    pub fn restore_var(&mut self, name: &str, prev: Option<Var>) {
        match prev {
            Some(v) => {
                self.vars.insert(name.to_string(), v);
            }
            None => {
                self.vars.remove(name);
            }
        }
    }

    pub fn child_env(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = self
            .vars
            .iter()
            // Arrays are not exported: there is no portable way to pass
            // them through `environ`, and bash does not either. A child
            // sees element 0 under the plain name if it was exported.
            .filter(|(_, v)| v.exported && v.array.is_none())
            .map(|(k, v)| (k.clone(), v.scalar().to_string()))
            .collect();
        pairs.sort();
        pairs
    }
}

impl brish_words::ArithEnv for Env {
    fn get(&mut self, name: &str) -> Option<i64> {
        Env::get(self, name)?.parse::<i64>().ok()
    }

    fn set(&mut self, name: &str, value: i64) {
        let _ = self.set_unchecked_guarded(name, value.to_string());
    }
}

impl Env {
    fn set_unchecked_guarded(&mut self, name: &str, value: String) -> Result<(), Error> {
        if self.vars.get(name).is_some_and(|v| v.readonly) {
            return Err(Error::expand(format!("{name}: readonly variable")));
        }
        self.set_unchecked(name, value);
        Ok(())
    }
}

/// POSIX name: `[A-Za-z_][A-Za-z0-9_]*`.
pub fn is_name(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_unset() {
        let mut e = Env::new();
        e.set("FOO", "bar").unwrap();
        assert_eq!(e.get("FOO"), Some("bar"));
        e.unset("FOO").unwrap();
        assert_eq!(e.get("FOO"), None);
        e.set("EMPTY", "").unwrap();
        assert_eq!(e.get("EMPTY"), Some(""));
    }

    #[test]
    fn readonly_guard() {
        let mut e = Env::new();
        e.set("R", "1").unwrap();
        e.set_readonly("R").unwrap();
        assert!(e.set("R", "2").is_err());
        assert!(e.unset("R").is_err());
    }

    #[test]
    fn export_propagates() {
        let mut e = Env::new();
        e.set("A", "1").unwrap();
        e.set("B", "2").unwrap();
        e.export("B").unwrap();
        let child = e.child_env();
        assert_eq!(child, vec![("B".to_string(), "2".to_string())]);
    }

    #[test]
    fn invalid_names() {
        let mut e = Env::new();
        assert!(e.set("1BAD", "x").is_err());
        assert!(e.set("A-B", "x").is_err());
        assert!(!is_name(""));
        assert!(!is_name("1a"));
        assert!(!is_name("a-b"));
        assert!(is_name("_1x"));
    }

    #[test]
    fn positional() {
        let mut e = Env::new();
        e.set_positional(vec!["a".into(), "b".into()]);
        assert_eq!(e.positional().len(), 2);
        assert_eq!(e.ifs(), " \t\n");
        e.set("IFS", ":").unwrap();
        assert_eq!(e.ifs(), ":");
    }

    #[test]
    fn arith_env() {
        use brish_words::ArithEnv;
        let mut e = Env::new();
        e.set("n", "41").unwrap();
        assert_eq!(ArithEnv::get(&mut e, "n"), Some(41));
        ArithEnv::set(&mut e, "n", 42);
        assert_eq!(e.get("n"), Some("42"));
        assert_eq!(ArithEnv::get(&mut e, "unsetvar"), None);
        assert_eq!(ArithEnv::get(&mut e, "notanumber"), None);
    }
}
