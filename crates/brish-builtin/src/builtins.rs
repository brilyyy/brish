//! Built-in shell commands.
//!
//! Dispatch table from word to built-in; individual built-ins land in
//! later plan phases (cd/export/exit first, job built-ins with the
//! engine).

/// POSIX built-ins recognized by the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltIn {
    /// `alias`
    Alias,
    /// `unalias`
    Unalias,
    /// `cd`
    Cd,
    /// `echo`
    Echo,
    /// `exit`
    Exit,
    /// `return`
    Return,
    /// `export`
    Export,
    /// `history`
    History,
    /// `read`
    Read,
    /// `source` / `.`
    Source,
    /// `test` / `[`
    Test,
    /// `trap`
    Trap,
    /// `unset`
    Unset,
    /// `pwd`
    Pwd,
    /// `printf`
    Printf,
    /// `umask`
    Umask,
    /// `times`
    Times,
    /// `readonly`
    Readonly,
    /// `set`
    Set,
    /// `shift`
    Shift,
    /// `command`
    Command,
    /// `type`
    Type,
    /// `:` (no-op)
    Colon,
    /// `break`
    Break,
    /// `continue`
    Continue,
    /// `z`
    Z,
    /// `ls`
    Ls,
    /// `local`
    Local,
    /// `getopts`
    Getopts,
}

impl BuiltIn {
    /// Look up a built-in by command name.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "alias" => BuiltIn::Alias,
            "unalias" => BuiltIn::Unalias,
            "cd" => BuiltIn::Cd,
            "echo" => BuiltIn::Echo,
            "exit" => BuiltIn::Exit,
            "return" => BuiltIn::Return,
            "export" => BuiltIn::Export,
            "history" => BuiltIn::History,
            "read" => BuiltIn::Read,
            "source" | "." => BuiltIn::Source,
            "test" | "[" => BuiltIn::Test,
            "trap" => BuiltIn::Trap,
            "unset" => BuiltIn::Unset,
            "pwd" => BuiltIn::Pwd,
            "printf" => BuiltIn::Printf,
            "umask" => BuiltIn::Umask,
            "times" => BuiltIn::Times,
            "readonly" => BuiltIn::Readonly,
            "set" => BuiltIn::Set,
            "shift" => BuiltIn::Shift,
            "command" => BuiltIn::Command,
            "type" => BuiltIn::Type,
            ":" => BuiltIn::Colon,
            "break" => BuiltIn::Break,
            "continue" => BuiltIn::Continue,
            "z" => BuiltIn::Z,
            "ls" => BuiltIn::Ls,
            "local" => BuiltIn::Local,
            "getopts" => BuiltIn::Getopts,
            _ => return None,
        })
    }

    /// Every built-in name, for completion and tooling. The
    /// engine-intercepted commands (`eval`, `wait`, `jobs`, `kill`,
    /// `true`, `false`) live in `exec.rs`, not in this enum.
    pub fn names() -> &'static [&'static str] {
        &[
            "alias", "break", "cd", "command", "continue", "echo", "exit", "export", "history",
            "getopts", "local", "ls", "printf", "pwd", "read", "readonly", "return", "set",
            "shift", "source", "test", "times", "trap", "type", "umask", "unalias", "unset", "z",
            ".", ":", "[",
        ]
    }

    /// Canonical command name.
    pub fn name(self) -> &'static str {
        match self {
            BuiltIn::Alias => "alias",
            BuiltIn::Unalias => "unalias",
            BuiltIn::Cd => "cd",
            BuiltIn::Echo => "echo",
            BuiltIn::Exit => "exit",
            BuiltIn::Return => "return",
            BuiltIn::Export => "export",
            BuiltIn::History => "history",
            BuiltIn::Read => "read",
            BuiltIn::Source => "source",
            BuiltIn::Test => "test",
            BuiltIn::Trap => "trap",
            BuiltIn::Unset => "unset",
            BuiltIn::Pwd => "pwd",
            BuiltIn::Printf => "printf",
            BuiltIn::Umask => "umask",
            BuiltIn::Times => "times",
            BuiltIn::Readonly => "readonly",
            BuiltIn::Set => "set",
            BuiltIn::Shift => "shift",
            BuiltIn::Command => "command",
            BuiltIn::Type => "type",
            BuiltIn::Colon => ":",
            BuiltIn::Break => "break",
            BuiltIn::Continue => "continue",
            BuiltIn::Z => "z",
            BuiltIn::Ls => "ls",
            BuiltIn::Local => "local",
            BuiltIn::Getopts => "getopts",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_all_resolve() {
        for n in BuiltIn::names() {
            assert!(BuiltIn::from_name(n).is_some(), "{n} not a builtin");
        }
        // Aliases resolve to a builtin whose canonical name is listed too.
        assert!(BuiltIn::names().contains(&BuiltIn::Test.name()));
        assert!(BuiltIn::names().contains(&BuiltIn::Source.name()));
    }

    #[test]
    fn round_trip() {
        for name in [
            "alias", "unalias", "cd", "getopts", "local", "ls", "echo", "exit", "export",
            "history", "read", "source", ".", "test", "[", ":", "return", "trap", "unset", "pwd",
            "printf", "umask", "times", "readonly", "set", "shift", "command", "type", "break",
            "continue",
        ] {
            let b = BuiltIn::from_name(name).unwrap();
            let _ = b.name();
        }
        assert_eq!(BuiltIn::from_name("ls"), Some(BuiltIn::Ls));
        assert_eq!(BuiltIn::from_name("."), Some(BuiltIn::Source));
        assert_eq!(BuiltIn::from_name("["), Some(BuiltIn::Test));
    }
}
