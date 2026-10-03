//! User database lookup (for `~user` tilde expansion).

/// Home directory of `name`, or `None` when the user is unknown (or
/// the platform has no user database).
#[cfg(unix)]
pub fn user_home(name: &str) -> Option<String> {
    nix::unistd::User::from_name(name)
        .ok()
        .flatten()
        .map(|u| u.dir.to_string_lossy().into_owned())
}

#[cfg(not(unix))]
pub fn user_home(_name: &str) -> Option<String> {
    None
}
