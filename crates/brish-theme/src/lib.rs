//! briSH prompt catalog: themes + prompt segments.
//!
//! Split out of `brish-plugin` so theme authors depend on this crate
//! (+ the `brish-plugin` traits) without pulling the behavioral plugin
//! surface. `brish-plugin` stays the API: traits + `Registry` +
//! announce. Registration lives in the binary (`brish`), which filters
//! both catalogs through `config.toml`.
pub mod aws;
pub mod docker;
pub mod git;
pub mod kubectx;
pub mod segments;
pub mod themes;
pub mod venv;

use brish_plugin::CatalogEntry;

use aws::AwsPrompt;
use docker::DockerPrompt;
use git::GitPrompt;
use kubectx::KubeCtxPrompt;
use themes::BrishThemes;
use venv::VenvPrompt;

/// Prompt catalog, in registration order. Segment order = prompt
/// segment order (themes choose whether to render them).
pub fn catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            default_enabled: true,
            plugin: Box::new(BrishThemes),
        },
        CatalogEntry {
            default_enabled: true,
            plugin: Box::new(GitPrompt::default()),
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
