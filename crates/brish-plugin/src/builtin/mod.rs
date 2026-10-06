//! Built-in plugins shipped with briSH.
//!
//! Everything non-core is a plugin (docs/PLUGIN-PLAN.md): the default
//! themes, the git prompt segment, the cd announcer. The binary filters
//! this catalog through `config.toml [plugins]`.

pub mod announce;
pub mod aws;
pub mod docker;
pub mod git;
pub mod kubectx;
pub mod segments;
pub mod themes;
pub mod venv;

use crate::Plugin;

/// One catalog plugin: installed when enabled (catalog default or the
/// config file's `enabled`/`disabled` lists).
pub struct CatalogEntry {
    pub default_enabled: bool,
    pub plugin: Box<dyn Plugin>,
}

use announce::AnnounceCd;
use aws::AwsPrompt;
use docker::DockerPrompt;
use git::GitPrompt;
use kubectx::KubeCtxPrompt;
use themes::DefaultThemes;
use venv::VenvPrompt;

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
        CatalogEntry {
            default_enabled: true,
            plugin: Box::new(VenvPrompt::default()),
        },
        CatalogEntry {
            default_enabled: true,
            plugin: Box::new(AwsPrompt::default()),
        },
        CatalogEntry {
            default_enabled: true,
            plugin: Box::new(KubeCtxPrompt::default()),
        },
        CatalogEntry {
            default_enabled: true,
            plugin: Box::new(DockerPrompt::default()),
        },
    ]
}
