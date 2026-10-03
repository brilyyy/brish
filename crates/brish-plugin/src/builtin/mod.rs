//! Built-in plugins shipped with briSH.
//!
//! Everything non-core is a plugin (docs/PLUGIN-PLAN.md): the default
//! themes, the git prompt segment, the cd announcer. The binary filters
//! this catalog through `config.toml [plugins]`.

pub mod announce;
pub mod git;
pub mod themes;

use crate::Plugin;

/// One catalog plugin: installed when enabled (catalog default or the
/// config file's `enabled`/`disabled` lists).
pub struct CatalogEntry {
    pub default_enabled: bool,
    pub plugin: Box<dyn Plugin>,
}

use announce::AnnounceCd;
use git::GitPrompt;
use themes::DefaultThemes;

/// In-tree plugins, in registration order. Theme/plugin names come
/// from `Plugin::name`, not duplicated here.
pub fn catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            default_enabled: true,
            plugin: Box::new(DefaultThemes),
        },
        CatalogEntry {
            default_enabled: true,
            plugin: Box::new(GitPrompt::default()),
        },
        CatalogEntry {
            default_enabled: false,
            plugin: Box::new(AnnounceCd),
        },
    ]
}
