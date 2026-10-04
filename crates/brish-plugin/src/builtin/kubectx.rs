//! `kubectx-prompt` — [`PromptSegment`](crate::PromptSegment) showing
//! the active Kubernetes context.
//!
//! Pure: reads `$KUBECONFIG` (or default `$HOME/.kube/config`), parses
//! the current-context via INI-style top-level line, no subprocess. 1s TTL cache via [`segments::Ttl`].

use crate::{Plugin, PromptSegment};
use crate::builtin::segments::{paint, Ttl};
use std::path::{Path, PathBuf};

pub struct KubeCtxPrompt {
    ttl: Ttl,
}

impl Default for KubeCtxPrompt {
    fn default() -> Self {
        Self { ttl: Ttl::new() }
    }
}

fn kubeconfig_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KUBECONFIG") {
        let p = Path::new(&p);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = Path::new(&home).join(".kube").join("config");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn current_context(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("current-context:") {
            let rest = trimmed.trim_start_matches("current-context:").trim();
            let rest = rest.trim_matches(|c: char| c == '"' || c == '\'');
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

impl KubeCtxPrompt {
    fn render_now(&self) -> Option<String> {
        let path = kubeconfig_path()?;
        let text = std::fs::read_to_string(&path).ok()?;
        let ctx = current_context(&text)?;
        Some(paint("38;5;105", format!("-controller {ctx}")))
    }
}

impl PromptSegment for KubeCtxPrompt {
    fn render(&self, _status: i32, cwd: &Path) -> Option<String> {
        self.ttl.get(cwd, || self.render_now())
    }
}

impl Plugin for KubeCtxPrompt {
    fn name(&self) -> &str {
        "kubectx-prompt"
    }

    fn install(&self, reg: &mut crate::Registry) {
        reg.prompt_segments.push(Box::new(Self::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_context() {
        let yaml = r#"
apiVersion: v1
kind: Config
current-context: my-cluster
clusters: []
contexts: []
users: []
"#;
        assert_eq!(current_context(yaml), Some("my-cluster".into()));
    }

    #[test]
    fn quoted_current_context() {
        let yaml = "current-context: \"prod\"";
        assert_eq!(current_context(yaml), Some("prod".into()));
    }

    #[test]
    fn no_context_yields_none() {
        let yaml = "apiVersion: v1\nkind: Config";
        assert_eq!(current_context(yaml), None);
    }
}